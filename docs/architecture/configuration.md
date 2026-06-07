---
status: draft
last_updated: 2026-06-07
---

# Configuration

## What

Alknet's configuration is split into `StaticConfig` (immutable after startup) and
`DynamicConfig` (hot-reloadable at runtime), with `ArcSwap` providing lock-free
reads on the hot path. `ConfigService` wraps reloads behind an irpc protocol
for production deployments.

## Why

Three specific failures motivated the split (ADR-030):

1. No hot reload of authentication credentials — adding a key requires a restart.
2. No port forwarding access control — any authenticated client has unrestricted
   access (ADR-031).
3. No structured configuration beyond CLI flags — operators need config files
   and the NAPI layer needs programmatic reload.

The split is clean: anything that affects SSH handshake or socket binding is
static; anything checked per-connection or per-channel is dynamic.

## Architecture

### StaticConfig

Immutable after startup. Constructed from `ServeOptions` (the builder pattern
is preserved per ADR-011). Contains:

- Transport mode, listen address
- TLS config (cert, key)
- iroh config (relay URL)
- Stealth mode flag
- Host key, host key algorithm
- Max auth attempts, max connections per IP
- Proxy config

Changing any of these requires a restart.

### DynamicConfig

Hot-reloadable at runtime via `ArcSwap<DynamicConfig>`. Contains:

- `AuthPolicy` — authorized keys, certificate authorities, token config
- `ForwardingPolicy` — allow/deny rules for channel targets (ADR-031)
- `RateLimitConfig` — rate limiting parameters

`ArcSwap` provides lock-free reads. Every `auth_publickey()` and
`channel_open_direct_tcpip()` call does a single `Arc` dereference — zero cost
compared to the current approach. Writes are atomic: `store()` swaps the
pointer.

### ConfigReloadHandle

```rust
pub struct ConfigReloadHandle {
    dynamic: Arc<ArcSwap<DynamicConfig>>,
}

impl ConfigReloadHandle {
    pub fn reload(&self, new_config: DynamicConfig) { ... }
}
```

Obtained from `Server::run()`. Passed to NAPI or CLI for explicit reload.

### ConfigServiceImpl

The Phase 1 implementation of config service logic, backed by
`ArcSwap<DynamicConfig>`. Where `ConfigIdentityProvider` wraps the auth section
of `DynamicConfig`, `ConfigServiceImpl` wraps the forwarding and rate-limit
sections. Both are ArcSwap-backed and share the same `DynamicConfig` instance.

```rust
pub struct ConfigServiceImpl {
    dynamic: Arc<ArcSwap<DynamicConfig>>,
}

impl ConfigServiceImpl {
    pub fn forwarding_policy(&self) -> Arc<ForwardingPolicy> {
        self.dynamic.load().forwarding.clone()
    }

    pub fn rate_limits(&self) -> Arc<RateLimitConfig> {
        self.dynamic.load().rate_limits.clone()
    }

    pub fn reload(&self, new_config: DynamicConfig) {
        self.dynamic.store(Arc::new(new_config));
    }
}
```

Phase 1 deploys `ConfigServiceImpl` directly — no irpc service boundary. The
`ConfigProtocol` irpc service (behind feature flag) wraps `ConfigServiceImpl`
for production deployments that use the service layer. This mirrors the
`ConfigIdentityProvider` / `AuthProtocol` pattern from [identity.md](identity.md)
and ADR-028.

### ConfigService irpc Service

```rust
enum ConfigProtocol {
    GetForwardingPolicy,
    GetRateLimits,
    ReloadForwarding { policy: ForwardingPolicy },
    ReloadRateLimits { limits: RateLimitConfig },
}
```

Behind the `irpc` feature flag. For production deployments that use the service
layer. For minimal deployments, direct `ConfigReloadHandle::reload()` is
sufficient.

### ForwardingPolicy

Part of DynamicConfig (ADR-031). Evaluated per-channel-open, matched against
the authenticated `Identity`. Rules are evaluated in order; first match wins.
Default determines fallback.

```rust
pub struct ForwardingPolicy {
    pub default: ForwardingAction,
    pub rules: Vec<ForwardingRule>,
}
```

### TOML Config File

Optional convenience input format (amends ADR-011, does not replace
programmatic API). Covers static config plus initial auth/forwarding paths.

```toml
[server]
transport = "tls"
listen = "0.0.0.0:443"

[auth]
host_key = "/etc/alknet/ssh/host_key"

[forwarding]
default = "deny"

[[forwarding.rules]]
target = "localhost:*"
action = "allow"
```

### NAPI Reload API

```typescript
interface AlknetServer {
  reloadAuth(auth: { authorizedKeys?: Buffer, certAuthority?: Buffer }): void;
  reloadForwarding(policy: ForwardingPolicyConfig): void;
  reloadAll(config: DynamicConfig): void;
}
```

### Multi-Transport Listeners

A head node may accept connections on multiple transports simultaneously. The
architecture supports `Vec<ListenerConfig>` instead of a single
`ServeTransportMode`. `Server::run()` spawns one accept loop per listener,
sharing `DynamicConfig`, `ConnectionRateLimiter`, sessions, and shutdown signal.

```toml
[[listeners]]
transport = "tls"
listen = "0.0.0.0:443"
stealth = true

[[listeners]]
transport = "tcp"
listen = "0.0.0.0:22"

[[listeners]]
transport = "iroh"
iroh_relay = "https://relay.alk.dev"
```

### CLI vs Programmatic Behavior

| Interface | Static config | Dynamic config | Reload mechanism |
|-----------|--------------|----------------|------------------|
| CLI | Flags + optional `--config` file | Loaded at startup from `--authorized-keys` | None (restart to change) |
| Core Rust | `StaticConfig` struct | `AuthProtocol` (irpc) or `ConfigIdentityProvider` (ArcSwap) | `ConfigProtocol::ReloadDynamicConfig` or `ConfigReloadHandle::reload()` |
| NAPI | `serve()` options | Same | `server.reloadAuth()`, `server.reloadForwarding()` |

## Constraints

- `StaticConfig` cannot be changed after startup. Changing transport mode,
  listen address, TLS config, or host key requires a restart.
- `DynamicConfig` is reloaded atomically via `ArcSwap`. Existing connections
  continue with their current config; new connections get the new config.
- Config file is optional. `ServeOptions` builder pattern remains the primary
  API (amends ADR-011, does not supersede it).
- No file watching (OQ-13 resolved: potential attack vector, unnecessary
  complexity).
- Client configuration stays as `ConnectOptions` — no `ArcSwap` needed.

## Open Questions

- None. All configuration-related questions are resolved per ADR-030, ADR-031,
  and the resolved OQs in [open-questions.md](open-questions.md).

## Design Decisions

| ADR | Decision | Summary |
|-----|----------|---------|
| [030](decisions/030-static-dynamic-config-split.md) | Static/dynamic config split | Immutable transport vs. reloadable auth/forwarding |
| [011](decisions/011-no-ssh-config-programmatic-api.md) | Programmatic-first API | Amended, not superseded — TOML is convenience layer |
| [031](decisions/031-forwarding-policy.md) | Forwarding policy | Rule-based allow/deny, TransportKind-aware |
| [029](decisions/029-identity-core-type.md) | Identity as core type | DynamicConfig.auth consumed by IdentityProvider |
| [028](decisions/028-auth-irpc-service.md) | Auth as irpc service | ConfigService wraps DynamicConfig reloads |

## References

- [research/configuration.md](../research/configuration.md) — Full analysis and proposed solution
- [identity.md](identity.md) — IdentityProvider trait, DynamicConfig.auth
- [ADR-013](decisions/013-fail2ban-friendly-logging.md) — Rate limiting parameters