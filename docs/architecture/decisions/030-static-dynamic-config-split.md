# ADR-030: Static/Dynamic Configuration Split

## Status

Accepted

## Context

Alknet's configuration is loaded once at startup and never changes. This causes
three specific failures:

1. **No hot reload of authentication credentials.** Adding or removing an
   authorized key requires restarting the server process. In head/worker
   deployments where keys are managed via a database, the process must be
   restarted every time a key is added, revoked, or rotated. This is
   operationally unacceptable.

2. **No port forwarding access control.** Any authenticated client can open a
   `direct-tcpip` channel to any destination. There is no policy governing
   which hosts, ports, or alknet control channels a client may access. A
   compromised key grants unrestricted network access through the tunnel.

3. **No structured configuration beyond CLI flags.** ADR-011 chose
   programmatic-first configuration for the alpha — correct at the time. But as
   alknet moves toward publishable releases, operators need config files for
   reproducible deployments, and the NAPI layer needs programmatic reload
   capability that `ServeOptions` doesn't currently support.

Not all configuration should be reloadable. Transport-level settings (listen
address, TLS certificates, host key) require socket/TLS renegotiation to change
at runtime — effectively a restart. Auth and forwarding policy can change
atomically without disrupting existing connections.

## Decision

**Split configuration into `StaticConfig` and `DynamicConfig`.**

### StaticConfig

Immutable after startup. Constructed from `ServeOptions` (the builder pattern is
preserved). Contains everything that affects socket binding, TLS handshakes, or
SSH session negotiation:

- Transport mode, listen address
- TLS config (cert, key)
- iroh config (relay URL)
- Stealth mode flag
- Host key, host key algorithm
- Max auth attempts, max connections per IP
- Proxy config

Changing any of these requires a restart.

### DynamicConfig

Hot-reloadable at runtime via `ArcSwap<DynamicConfig>`. Contains everything
checked per-connection or per-channel:

- `AuthPolicy` — authorized keys, certificate authorities, token config
- `ForwardingPolicy` — allow/deny rules for channel targets (ADR-031)
- `RateLimitConfig` — rate limiting parameters

`ArcSwap` provides lock-free reads on the hot path (every `auth_publickey()` and
every `channel_open_direct_tcpip()` call does an `Arc` dereference — zero cost
compared to the current approach). Writes are atomic: `store()` swaps the
pointer. Existing connections finish with their current config; new connections
get the new config.

### ConfigReloadHandle

```rust
pub struct ConfigReloadHandle {
    dynamic: Arc<ArcSwap<DynamicConfig>>,
}

impl ConfigReloadHandle {
    pub fn reload(&self, new_config: DynamicConfig) { ... }
}
```

The handle is obtained from `Server::run()` and passed to NAPI or the CLI.

### ConfigService

The `ConfigService` wraps `ArcSwap<DynamicConfig>` reloads behind an irpc
protocol (behind the `irpc` feature flag) for production deployments that use
the service layer. For minimal deployments (CLI, single-node), direct
`ConfigReloadHandle::reload()` is sufficient.

### TOML Config File

An optional TOML config file covers static config plus initial auth/forwarding
paths. This **amends** ADR-011 (does not supersede it) — the programmatic-first
API remains primary. The config file is a convenience input format:

```toml
[server]
transport = "tls"
listen = "0.0.0.0:443"
stealth = false
max_connections_per_ip = 5
max_auth_attempts = 3

[server.tls]
cert = "/etc/alknet/tls/cert.pem"
key = "/etc/alknet/tls/key.pem"

[auth]
host_key = "/etc/alknet/ssh/host_key"

[forwarding]
default = "deny"
```

### NAPI Reload API

```typescript
interface AlknetServer {
  reloadAuth(auth: { authorizedKeys?: Buffer, certAuthority?: Buffer }): void;
  reloadForwarding(policy: ForwardingPolicyConfig): void;
  reloadAll(config: DynamicConfig): void;
}
```

The NAPI layer parses key data and constructs a new `DynamicConfig`, then calls
`ConfigReloadHandle::reload()`.

### Client Configuration

Client configuration stays as `ConnectOptions` — no `ArcSwap` needed. Client
config is almost entirely static (which server to connect to, which key to use).

## Consequences

- **Positive**: Auth credentials and forwarding policy can be reloaded without
  restarting the server. Adding a key via `reloadAuth()` takes effect on the
  next connection attempt.
- **Positive**: ADR-011's programmatic-first intent is preserved. The TOML
  config file is an optional convenience layer, not a replacement for
  `ServeOptions`.
- **Positive**: `ArcSwap` provides zero-cost reads on the hot path. Every auth
  check and every channel open is a single `Arc` dereference.
- **Positive**: The `ConfigService` irpc protocol (behind feature flag) allows
  production deployments to integrate config reload into their service mesh
  without taking a direct dependency on `DynamicConfig` internals.
- **Positive**: Forwarding policy is now part of `DynamicConfig` — operators can
  restrict access per identity, per destination, per transport (ADR-031).
- **Negative**: Two config structs where there was one. The split is clean
  (transport vs. policy) but adds surface area.
- **Negative**: Config file introduces `toml` as a dependency in the CLI crate.
  This is acceptable for a CLI binary.

## References

- [research/configuration.md](../../research/configuration.md) — Full analysis
- [ADR-011](011-no-ssh-config-programmatic-api.md) — Programmatic-first API (amended, not superseded)
- [ADR-031](031-forwarding-policy.md) — Forwarding policy (part of DynamicConfig)
- [ADR-029](029-identity-core-type.md) — Identity as core type (DynamicConfig.auth uses IdentityProvider)
- [integration-plan.md](../../research/integration-plan.md) — Phase 1.1