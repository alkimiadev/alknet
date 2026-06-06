---
status: draft
last_updated: 2026-06-04
phase: exploration
---

# Configuration Architecture

## Terminology Change: Head/Worker

This document previously used **hub/spoke** terminology. It has been updated to **head/worker**:

- **Head node**: The coordinating node (formerly "hub"). A head can also be a worker.
- **Worker node**: A node that connects to a head and registers services (formerly "spoke").
- **Node**: Any participant in the network. Every node has an identity.

This better reflects that a head is also a worker, enabling mesh topologies.

## Problem

## Problem

Alknet's configuration is loaded once at startup and never changes. This has
three specific failures:

1. **No hot reload of authentication credentials.** Adding or removing an
   authorized key requires restarting the server process. In a head/worker
   deployment where keys are managed via a database (see
   `@alkdev/storage`'s `peer_credentials` table), the alknet process must be
   restarted every time a key is added, revoked, or rotated. This is
   operationally unacceptable for a production service.

2. **No port forwarding access control.** Any authenticated client can open a
   `direct-tcpip` channel to any destination. There is no policy governing
   which hosts, ports, or `alknet-*` control channels a client may access. This
   is a security gap — a compromised key grants unrestricted network access
   through the tunnel.

3. **No structured configuration beyond CLI flags.** ADR-011 chose
   programmatic-first configuration for the alpha. This was correct — it
   avoided cross-platform path issues and kept the API surface small. But as
   alknet moves toward publishable releases, operators need config files for
   reproducible deployments, and the NAPI layer needs programmatic reload
   capability that the current `ServeOptions` builder pattern doesn't support.

### What's Not The Problem

- This does not propose depending on Honker, SQLite, or any specific data
  source at the `alknet-core` level. The core provides a reload mechanism;
  data sources plug in from outside.
- This does not propose file-watching (potential attack vector, unnecessary
  complexity). CLI usage loads config once at startup. Programmatic usage
  (NAPI, head node) calls reload explicitly.
- This does not replace the existing `ServeOptions` builder pattern. It
  generalizes it.

## Analysis

### Static vs Dynamic Configuration

Not all configuration should be reloadable. Transport-level settings (listen
address, TLS certificates, host key) require socket/TLS renegotiation to change
at runtime — effectively a restart. Auth and forwarding policy can change
atomically without disrupting existing connections.

| Category | Examples | Reloadable? |
|---|---|---|
| Transport | listen addr, TLS cert/key, iroh relay, stealth mode | No — requires bind change |
| Identity | host key, host key algorithm | No — requires SSH re-negotiation |
| Auth | authorized keys, cert authorities | **Yes** — next auth check picks up changes |
| Forwarding | allowed destinations, per-principal rules | **Yes** — next channel open picks up changes |
| Rate limits | max connections per IP, max auth attempts | **Yes** — next check picks up changes |

The split is clean: anything that affects the SSH handshake or socket binding
is static. Anything that's checked per-connection or per-channel is dynamic.

### Auth Reload: Service Approach

The original design held all authorized keys in memory via `ArcSwap<DynamicConfig>`. For small deployments this works, but for nodes serving many users it requires loading every key into RAM and atomic-swapping the entire set on each reload.

The improved approach is to make auth an **irpc service** (see [core.md](core.md) and [services.md](services.md)). Auth verification becomes a service call: `VerifyPubkey { fingerprint, key_data }` → `oneshot::Sender<AuthResult>`. The service can:

- Query SQLite on demand (no need to hold all keys in memory)
- Maintain an LRU cache for hot keys
- Subscribe to honker streams for key invalidation
- Run locally (in-process mpsc) or remotely (QUIC stream)

`ArcSwap<DynamicConfig>` remains as a fallback for minimal deployments (CLI usage, single-node setups) where SQLite overhead isn't warranted. The service approach is the primary path for production deployments.

### Current Architecture

```
ServeOptions (builder) → Server::new()
  ├─ Arc<server::Config>          (russh config, immutable)
  ├─ Arc<ServerAuthConfig>        (keys + CAs, immutable after load)
  ├─ Arc<ConnectionRateLimiter>   (mutable but not reloadable)
  └─ ServerHandler::new(auth_config, ...)

ServerHandler
  ├─ auth_config: Arc<ServerAuthConfig>  ← shared, immutable
  ├─ connection_limiter: Arc<ConnectionRateLimiter>
  ├─ outbound_proxy: Option<ProxyConfig>
  └─ (no forwarding policy field)
```

`auth_publickey()` reads from `self.auth_config` via `Arc` dereference. No
path to update it.

### Proposed Architecture

Replace `Arc<ServerAuthConfig>` with a service-based approach:

```
StaticConfig (Arc, loaded once)
  ├─ transport mode, listen addr, TLS config, iroh config
  ├─ stealth, proxy
  ├─ host key
  └─ max_auth_attempts, max_connections_per_ip

AuthService (irpc service, local or remote)
  ├─ VerifyPubkey(fingerprint, key_data) → AuthResult
  ├─ VerifyToken(token_bytes) → AuthResult
  └─ ReloadKeys() → ()
     Backed by: SQLite (peer_credentials, api_keys)
     Optional: ArcSwap<DynamicConfig> for minimal deployments

ConfigService (irpc service, always local)
  ├─ ReloadDynamicConfig(DynamicConfig)
  └─ GetForwardingPolicy() → ForwardingPolicy

DynamicConfig (Arc<ArcSwap<DynamicConfig>>, reloadable)
  ├─ forwarding: ForwardingPolicy
  └─ rate_limits: RateLimitConfig
```

For production: auth verification goes through the auth service, which queries SQLite. The `DynamicConfig` only holds forwarding policy and rate limits — not the full key set. For minimal deployments: auth falls back to `ArcSwap<DynamicConfig>` with all keys in memory, wrapped by the same service interface.

`ArcSwap` provides lock-free reads on the hot path. Every `auth_publickey()`
and `channel_open_direct_tcpip()` call does an `Arc` dereference — zero cost
compared to the current approach. Writes are atomic: `store()` swaps the
pointer. Existing connections finish with their current config, new connections
get the new config.

### Forwarding Policy

Currently, `channel_open_direct_tcpip` in `handler.rs` spawns a proxy task for
any destination. The only gate is authentication. A forwarding policy adds a
check before the proxy spawn:

```rust
pub struct ForwardingPolicy {
    default: ForwardingAction,
    rules: Vec<ForwardingRule>,
}

pub struct ForwardingRule {
    target: TargetPattern,
    action: ForwardingAction,
    principals: Vec<String>,
}

pub enum ForwardingAction { Allow, Deny }
pub enum TargetPattern {
    Any,
    Host(String),
    Cidr(IpNetwork),
    PortRange(String, Range<u16>),
    AlknetPrefix,
}
```

Rule evaluation: first match wins, default applies if no rule matches. This
model maps to OpenSSH's `AllowTcpForwarding` + `PermitOpen` but is more
expressive. It also maps to `peer_credentials.metadata.scopes` in `@alkdev/storage`
— the head node can generate forwarding rules from stored scopes.

Rule ordering matters. A deny-then-allow pattern gives blocklist semantics. An
allow-then-deny pattern gives allowlist semantics. Both are useful. The
default determines the fallback.

### Configuration File Format

ADR-011 chose "programmatic-first, no config file." This was correct for alpha.
For publishable releases, a config file enables:

- Reproducible deployments (version-controlled config)
- Less verbose CLI invocations
- Separate files for static and dynamic config (only static needs to be in the
  config file; dynamic comes from the reload mechanism)

TOML is the idiomatic Rust choice. The config file covers static config only —
the same fields as `ServeOptions`. Dynamic config (auth, forwarding) comes from
the reload mechanism, not from the file. This preserves ADR-011's intent: the
core doesn't know about the data source for auth keys, it just provides a place
to put them.

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

[server.iroh]
relay = "https://relay.alk.dev"

[auth]
host_key = "/etc/alknet/ssh/host_key"

[forwarding]
default = "deny"

[[forwarding.rules]]
target = "localhost:*"
action = "allow"

[[forwarding.rules]]
target = "alknet-*"
action = "allow"

[[forwarding.rules]]
target = "*:22"
action = "deny"
```

The `[[forwarding.rules]]` array syntax is TOML's array-of-tables pattern.
Rules are evaluated in order; first match wins.

### NAPI Reload API

The NAPI layer exposes the reload handle:

```typescript
interface AlknetServer {
  reloadAuth(auth: { authorizedKeys?: Buffer, certAuthority?: Buffer }): void;
  reloadForwarding(policy: ForwardingPolicyConfig): void;
  reloadAll(config: DynamicConfig): void;
}

interface ForwardingPolicyConfig {
  default: 'allow' | 'deny';
  rules: ForwardingRuleConfig[];
}

interface ForwardingRuleConfig {
  target: string;      // "localhost:*", "10.0.0.0/8:80", "alknet-*"
  action: 'allow' | 'deny';
  principals?: string[];  // default ["*"]
}
```

The head node calls `server.reloadAuth(...)` after writing to `peer_credentials`.
The NAPI layer parses the key data and constructs a new `DynamicConfig`, then
calls the `ConfigReloadHandle`.

### Client Configuration

Client configuration is almost entirely static (which server to connect to,
which key to use). The only potential dynamic config is key rotation, which is
less urgent because clients don't serve. For now, client configuration stays
as `ConnectOptions` — no `ArcSwap` needed.

A config file for client connections could define named profiles:

```toml
[profiles.production]
server = "head.alk.dev:443"
transport = "tls"
identity = "/home/user/.ssh/id_ed25519"

[profiles.staging]
server = "staging.alk.dev:22"
transport = "tcp"
identity = "/home/user/.ssh/staging_key"
```

This is a convenience layer on top of `ConnectOptions`, not a replacement.

### CLI vs Programmatic Behavior

| Interface | Static config | Dynamic config | Reload mechanism |
|---|---|---|---|
| CLI | Flags + optional `--config` file | Loaded at startup from `--authorized-keys` | None (restart to change) |
| Core Rust | `StaticConfig` struct | `AuthService` (irpc) or `ArcSwap<DynamicConfig>` (minimal) | `ConfigService::reload()` or `ConfigReloadHandle::reload()` |
| NAPI | `serve()` options | Same | `server.reloadAuth()`, `server.reloadForwarding()` |

The CLI doesn't need a reload mechanism. When you're running alknet from the
command line, restarting is fine. The reload mechanism exists for programmatic
consumers and for the auth service pattern where keys are queried on demand from
a database.

### Multi-Transport Listeners

A head node may want to accept connections on multiple transports simultaneously:

- TCP on port 22 (simple, direct SSH)
- TLS on port 443 (stealth mode, corporate firewalls)
- iroh QUIC (P2P, no port forwarding needed)
- WebTransport on port 443 (browser clients, shares the HTTP/3 listener)

Currently `ServeTransportMode` is a single enum and `Server::run()` takes one
acceptor. To serve multiple transports, the architecture needs to change.

**Option A: `Server` manages multiple listeners internally.**

```rust
pub struct Server {
    // Shared state (one copy, shared across all listeners)
    config: Arc<server::Config>,
    dynamic_config: Arc<ArcSwap<DynamicConfig>>,
    connection_limiter: Arc<ConnectionRateLimiter>,
    outbound_proxy: Option<ProxyConfig>,
    sessions: Arc<tokio::sync::Mutex<Vec<ActiveSession>>>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,

    // Per-listener state
    listeners: Vec<ListenerConfig>,
}

pub struct ListenerConfig {
    transport: ServeTransportMode,
    listen_addr: SocketAddr,
    stealth: bool,
    // Transport-specific config (TLS cert, iroh relay, etc.)
    tls: Option<TlsConfig>,
    iroh: Option<IrohConfig>,
}
```

`Server::run()` spawns one accept loop per `ListenerConfig`. Each loop
constructs its own acceptor and `ServerHandler` (with the appropriate
`TransportKind` tag), but shares the auth config, connection limiter, and
session list. Shutdown signal goes to all loops.

**Option B: Caller manages multiple `Server` instances.**

The caller creates N `Server` objects, each with its own transport. They share
`Arc<ArcSwap<DynamicConfig>>` and `Arc<ConnectionRateLimiter>` explicitly.

Option A is better because: shared shutdown, shared session tracking, single
point for config reload. Option B puts coordination burden on the caller and
makes graceful shutdown harder (N independent shutdown channels).

**The TLS + WebTransport coexistence question.** Both TLS and WebTransport
use port 443. WebTransport is HTTP/3 (QUIC), TLS on port 443 is typically
TCP+TLS. They can share the port because they're different protocols — QUIC
is UDP, TLS-over-TCP is TCP. The kernel routes by protocol. But if both are
on 443, the stealth mode protocol detector needs to handle HTTP/3 as well:

```
Port 443:
  TCP connection → TLS handshake → SSH (existing)
  UDP "connection" → QUIC handshake → WebTransport → stream proxy
```

This is similar to how iroh-live-relay works: HTTP/3 listener accepts
WebTransport sessions, each session opens bidirectional streams that map to
internal services.

**Config file for multi-transport:**

```toml
[[listeners]]
transport = "tls"
listen = "0.0.0.0:443"
stealth = true

[listeners.tls]
cert = "/etc/alknet/tls/cert.pem"
key = "/etc/alknet/tls/key.pem"

[[listeners]]
transport = "tcp"
listen = "0.0.0.0:22"

[[listeners]]
transport = "iroh"
iroh_relay = "https://relay.alk.dev"

[[listeners]]
transport = "webtransport"
listen = "0.0.0.0:443"
# WebTransport shares port 443 with TLS because QUIC is UDP, TLS is TCP

[listeners.webtransport]
cert = "/etc/alknet/tls/cert.pem"
key = "/etc/alknet/tls/key.pem"
```

The `[[listeners]]` array-of-tables pattern means each listener is an
independent config block. The `[auth]`, `[forwarding]`, and `[server]`
sections at the top level are shared — they apply to all listeners.

**NAPI multi-transport:**

```typescript
const server = await serve({
  listeners: [
    { transport: 'tls', listen: '0.0.0.0:443', stealth: true, tlsCert: '...', tlsKey: '...' },
    { transport: 'tcp', listen: '0.0.0.0:22' },
    { transport: 'iroh', irohRelay: 'https://relay.alk.dev' },
  ],
  hostKey: hostKeyBuffer,
  authorizedKeys: keysBuffer,
});
```

Single `AlknetServer` object, single `reloadAuth()` call affects all
listeners.

### Transport Kind and WebTransport

The `TransportKind` enum (currently `Tcp | Tls | Iroh`) tags each connection
so the handler can behave differently per transport. Adding `WebTransport` to
this enum is straightforward — WebTransport connections are identifiable at
accept time. The handler behavior is the same (port forwarding only), but
the tag enables transport-specific logging and future policy differences
(e.g., WebTransport clients can only access `alknet-*` control channels).

## Proposed Solution

### Phase 1: Static/Dynamic Split

1. Introduce `StaticConfig` and `DynamicConfig` structs
2. Replace `Arc<ServerAuthConfig>` in `ServerHandler` with
   `Arc<ArcSwap<DynamicConfig>>`
3. Add `ConfigReloadHandle` with `reload(DynamicConfig)` method
4. Expose `reloadAuth()` on the NAPI `AlknetServer` object

**Scope**: `alknet-core` auth module + `alknet-napi` serve module

**Risk**: Low — internal refactor, no protocol changes

### Phase 2: Forwarding Policy

1. Add `ForwardingPolicy` to `DynamicConfig`
2. Add policy check to `channel_open_direct_tcpip` before proxy spawn
3. Expose `reloadForwarding()` on NAPI `AlknetServer`

**Scope**: `alknet-core` handler + `alknet-napi`

**Risk**: Low — new check, default-allow preserves current behavior

### Phase 3: Config File

1. Add `--config <path>` CLI flag parsing TOML
2. CLI flags override config file values (same precedence as cargo)
3. Config file only covers static config + initial auth config path
4. Add `serde` derive to `StaticConfig`

**Scope**: `alknet-cli` (new binary crate) + `alknet-core` config module

**Risk**: Medium — new dependency (`toml` crate), new CLI surface to validate

### Phase 4: Client Profiles

1. Add `[profiles]` section to client config file
2. `--profile production` loads named profile
3. CLI flags override profile values

**Scope**: `alknet-cli`

**Risk**: Low — convenience layer only

### Phase 5: Multi-Transport Listeners

1. Change `ServeTransportMode` from single enum to `Vec<ListenerConfig>`
2. `Server::run()` spawns one accept loop per listener, sharing `DynamicConfig`
3. Single shutdown signal drains all listeners
4. Add `[[listeners]]` to config file format
5. NAPI `serve()` accepts `listeners` array instead of single `transport`
6. Add `WebTransport` to `TransportKind` enum (initially as a tag only;
   actual WebTransport acceptor is a separate R&D phase)

**Scope**: `alknet-core` serve.rs + `alknet-napi` + `alknet-cli`

**Risk**: Medium — changes the primary API surface of `serve()`. Backwards
compat via accepting both `transport: string` (single) and
`listeners: array` (multi) in NAPI.

## Open Questions

- **OQ-CFG-01**: Should forwarding rules support per-user scope derived from
  the authenticated key's metadata (e.g., `peer_credentials.metadata.scopes`)?
  Or is a global rules table with principal matching sufficient?

  Global rules with principal matching is simpler and covers most cases. Per-user
  scope derived from certificates is more granular but requires the server to
  maintain a mapping from key fingerprint to scope. This mapping comes from the
  head node's database, not from the SSH protocol. Phase 2 starts with global rules;
  per-user scope can be added as an extension.

- **OQ-CFG-02**: Should the config file watch for changes and auto-reload?

  No. File watching is a potential attack vector (symlink races, inotify
  limitations on network filesystems). The CLI loads once at startup. The NAPI
  layer reloads explicitly. This is the right model for a security-sensitive
  tool.

- **OQ-CFG-03**: Should `ArcSwap` be the reload primitive, or is `RwLock`
  sufficient?

  `ArcSwap` is the standard pattern for this in Rust network services
  (`arc-swap` crate). It provides lock-free reads (the hot path) and atomic
  writes. `RwLock` would also work but adds lock contention on reads. The
  `arc-swap` dependency is small (~500 lines) and well-maintained. Prefer it.

- **OQ-CFG-04**: Should TLS and WebTransport on the same port share a single
  QUIC listener (like iroh Router's ALPN dispatch), or run as separate
  listeners on the same port?

  They can't conflict because QUIC is UDP and TLS-over-TCP is TCP — the
  kernel routes by protocol, not by port number. They're naturally separate
  listeners even on the same port. However, if iroh is also running on the
  same host, the iroh endpoint already owns a QUIC listener. The WebTransport
  listener needs its own. Options: (a) share the iroh endpoint's QUIC listener
  with ALPN dispatch (reuses `from_endpoint` pattern), (b) separate QUIC
  listeners on different ports, (c) bind both to 443/UDP — possible if
  `SO_REUSEPORT` is used. Needs R&D; defer to WebTransport transport design
  session.

  ~~**Update**: WebTransport is out of scope for the current configuration
  work. It requires a fundamentally different authentication model (HTTP-level
  API keys/session tokens vs SSH key-based auth). The `ServerHandler` only
  knows SSH `auth_publickey`. WebTransport auth would need its own handler
  path. This connects to the broader question of whether `DynamicConfig.auth`
  should be transport-aware (see OQ-CFG-06). WebTransport transport design
  is a separate R&D session.~~

  **Update 2**: Auth concern is resolved by ADR-023. The same authorized_keys
  set verifies both SSH pubkey auth and token auth (Ed25519-signed timestamp
  for WebTransport). One key material, two presentations. The remaining
  question is purely about QUIC listener coexistence — which is a transport
  implementation detail, not an auth question. See [auth.md](../architecture/auth.md)
  and [ADR-023](../architecture/decisions/023-unified-auth-shared-key-material.md).

- **OQ-CFG-05**: Does `TransportKind::WebTransport` need any handler behavior
  different from other transports?

  Initially no — all transports get the same port-forwarding-only handler.
  But WebTransport connections come from browsers, which have different trust
  assumptions. A future forwarding policy might restrict WebTransport clients
  to `alknet-*` control channels only (no arbitrary host:port forwarding).
  This is a policy question, not a transport question. The `TransportKind` tag
  on the handler enables transport-aware policy rules in `ForwardingPolicy`
  without changing the handler. Defer to Phase 2 (forwarding policy design).

- **OQ-CFG-06**: Should the auth layer be transport-aware?

  Currently `DynamicConfig.auth` is `ServerAuthConfig` — SSH keys and CAs
  only. This works for SSH over any transport (TCP, TLS, iroh) because SSH
  carries its own auth protocol. But non-SSH transports (WebTransport,
  WebSocket) use HTTP-level authentication (API keys, session tokens in
  headers/query params). The auth question is: does the same `DynamicConfig`
  serve both models, or does each transport carry its own auth config?

  ~~Option A: `AuthPolicy` contains both SSH auth and API key auth:
  ```rust
  pub struct AuthPolicy {
      ssh: SshAuthConfig,           // for SSH-over-any-transport
      api_keys: Option<ApiKeysConfig>,  // for non-SSH transports
  }
  ```

  Option B: Auth is per-listener. Each `ListenerConfig` carries its own auth
  config appropriate to its transport.

  Option A is simpler for the initial implementation — the SSH auth path is
  unchanged, and API key auth is additive. Option B is more flexible but
  duplicates the shared auth state (keys should be reloadable once, not per
  listener).

  For now, the config architecture should accommodate Option A as a future
  extension. Phase 1 implements `DynamicConfig` with SSH auth only. API key
  auth is added when a non-SSH transport is implemented.~~

  **Resolved by ADR-023**: The auth layer is transport-aware in its
  *presentation*, not its *material*. `AuthPolicy` holds `SshAuthConfig` and
  `TokenAuthConfig`, where `TokenAuthConfig.key_source` defaults to
  `Shared` (same `authorized_keys` set as SSH auth). The same Ed25519 keys
  serve both paths: SSH presents the public key in the handshake; WebTransport
  presents an Ed25519-signed timestamp token. Verification produces the same
  `Identity` type via the `IdentityProvider` trait. One `reloadAuth()` call
  updates both. See [auth.md](../architecture/auth.md) and
   [ADR-023](../architecture/decisions/023-unified-auth-shared-key-material.md).

- **OQ-CFG-07**: Should auth and secret services share a single irpc endpoint
  or be separate services?

  Separate services are better. Auth (verify credentials) and Secret (derive/store
  keys) have different security boundaries. The secret service holds the master
  seed; the auth service only needs public key fingerprints. They may run on
  different machines. See [services.md](services.md) for protocol definitions.

- **OQ-CFG-08**: How do external credentials (API keys, OAuth tokens) relate
  to the secret service's HD key derivation?

  HD-derived keys (from SLIP-0010/BIP39) cover self-generated secrets (identity
  keys, encryption keys, SSH keys). External credentials (third-party API keys,
  OAuth tokens) can't be derived — they must be stored encrypted. The secret
  service handles both: derived keys are regenerated on demand; stored secrets
  are encrypted with a key that is itself derived from the seed. See
  [services.md](services.md) for the `SecretProtocol` definition.

## Decisions Required

These decisions will be extracted into ADRs when the architecture is finalized:

1. **ADR-020**: Static/dynamic config split. Auth delegated to `AuthService` (irpc)
  for production; `ArcSwap<DynamicConfig>` for minimal deployments. Supersedes
  ADR-011's "no config file" — adds optional config file while preserving
  programmatic-first API.

2. **ADR-021**: Forwarding policy with rule-based allow/deny. Default-allow
  preserves current behavior during migration; default-deny for production
  deployments.

3. **ADR-022**: Multi-transport listeners. `Server` spawns multiple accept
  loops sharing auth config, session state, and shutdown. Replaces single
  `ServeTransportMode` with `Vec<ListenerConfig>`.

4. **ADR-026**: Head/worker terminology. Replace hub/spoke with head/worker
  throughout all documentation and APIs. A head is also a worker.

5. **ADR-028**: Auth as service. Auth verification via irpc `AuthProtocol`
  service, not in-memory key set. Enables SQLite-backed auth for production,
  `ArcSwap` fallback for minimal deployments.

## References

- [ADR-011](../architecture/decisions/011-no-ssh-config-programmatic-api.md) — Programmatic-first API (superseded by ADR-020)
- [ADR-012](../architecture/decisions/012-auth-ed25519-and-cert-authority.md) — Auth key format
- [ADR-018](../architecture/decisions/018-control-channel-for-pubsub.md) — Control channel routing
- `server/handler.rs` — Current `Arc<ServerAuthConfig>` usage
- `server/serve.rs` — Current single-transport `Server::run()` accept loop
- `auth/server_auth.rs` — `ServerAuthConfig` struct
- `auth/keys.rs` — `KeySource` and key loading
- `@alkdev/storage/docs/architecture/sqlite-host.md` — `peer_credentials` table schema
- [wtransport](https://github.com/BiagioFesta/wtransport) — Rust WebTransport library (in `/workspace/wtransport`)
- [arc-swap crate](https://docs.rs/arc-swap) — Lock-free read, atomic write for shared state
- [ADR-023](../architecture/decisions/023-unified-auth-shared-key-material.md) — Unified auth with shared key material
- [auth.md](../architecture/auth.md) — Unified auth architecture spec
- [call-protocol.md](../architecture/call-protocol.md) — Bidirectional call protocol spec
- [services.md](services.md) — Service layer architecture (irpc services)
- [core.md](core.md) — Core overview, head/worker terminology, service layer