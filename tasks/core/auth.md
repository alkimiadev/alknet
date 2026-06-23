---
id: core/auth
name: Implement AuthContext, Identity, AuthToken, IdentityProvider trait, and ConfigIdentityProvider
status: completed
depends_on: [core/core-types]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Implement the authentication types in `src/auth.rs`. Auth is hybrid: the
endpoint resolves what it can (TLS-level), handlers resolve what they need
(protocol-level). AuthContext may be partial — handlers complete auth inside
`handle()`.

### AuthContext

```rust
#[derive(Clone)]
pub struct AuthContext {
    pub identity: Option<Identity>,
    pub alpn: Vec<u8>,
    pub remote_addr: Option<SocketAddr>,
    pub tls_client_fingerprint: Option<String>,
}
```

Created by the endpoint for each incoming connection. Passed to
`ProtocolHandler::handle()` as an immutable reference.

- `identity`: peer's authenticated identity, if resolved by the endpoint. None
  means the endpoint has no identity info for this connection.
- `alpn`: negotiated ALPN — always present after TLS handshake.
- `remote_addr`: peer's address, if available (may be None for iroh).
- `tls_client_fingerprint`: SHA-256 fingerprint of TLS client cert, if presented.

`AuthContext` is `Clone` (handlers clone for per-stream contexts) and immutable
in `handle()` (handlers create local variables for resolved identity, they
don't mutate the shared context).

### Identity

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Identity {
    pub id: String,
    pub scopes: Vec<String>,
    pub resources: HashMap<String, Vec<String>>,
}
```

The authenticated peer identity. `id` is ALPN-agnostic:
- SSH key auth: `"SHA256:abc123..."` (key fingerprint)
- API key auth: `"alk_test"` (key prefix)
- Certificate auth: `"username"` (principal name)

### AuthToken

```rust
#[derive(Debug, Clone)]
pub struct AuthToken {
    pub raw: Vec<u8>,
}
```

Opaque authentication token carried in protocol frames. The handler that
extracted it knows its encoding.

### IdentityProvider trait

```rust
pub trait IdentityProvider: Send + Sync + 'static {
    fn resolve_from_fingerprint(&self, fingerprint: &str) -> Option<Identity>;
    fn resolve_from_token(&self, token: &AuthToken) -> Option<Identity>;
}
```

- `resolve_from_fingerprint()`: used by endpoint (TLS client cert) and SSH (key fingerprint)
- `resolve_from_token()`: used by call protocol (AuthToken in first frame) and HTTP (Bearer header)
- Both return `Option<Identity>` — None means credential not recognized

### ConfigIdentityProvider

```rust
pub struct ConfigIdentityProvider {
    dynamic: Arc<ArcSwap<DynamicConfig>>,
}
```

The default implementation. Resolves identities from `DynamicConfig` (reads
from ArcSwap on every call — hot-reloadable).

Resolution logic:
- **Fingerprint**: look up in `DynamicConfig::auth::authorized_fingerprints`.
  If found, return `Identity { id: fingerprint, scopes: ["relay:connect"], resources: {} }`.
- **Token**: parse as UTF-8. If starts with `alk_`, look up in
  `DynamicConfig::auth::api_keys` by prefix match + SHA-256 hash. If found and
  not expired, return `Identity { id: prefix, scopes: entry.scopes, resources: entry.resources }`.

Changes to DynamicConfig via ConfigReloadHandle are reflected immediately.

### Two Identity Scopes

There are two distinct identity scopes that must not be conflated:

| Scope | Where set | Where stored | Represents | Used for |
|-------|-----------|--------------|------------|----------|
| Connection-level | Handler in `handle()` | `Connection` (via `set_identity`) | Who opened the QUIC connection | Observability, logging |
| Per-request | CallAdapter per `call.requested` | `OperationContext.identity` | Who makes this specific call | ACL (ADR-015) |

The connection-level identity is stable (set once). The per-request identity
is dynamic (resolved per call, potentially different across requests). The
per-request identity takes precedence for ACL.

### Security constraints

- **Token entropy**: generated `alk_` tokens must have ≥128 bits of entropy.
  The prefix (first 8 chars) is for O(1) lookup and is not secret — it appears
  in logs by design. SHA-256 of the full token allows offline verification; this
  is safe only if the full token is high-entropy.
- **Config reload must be authenticated**: a reload that adds an authorized
  fingerprint or API key grants access immediately. The reload trigger must be
  local-only or admin-scoped.
- **Connection-level identity is for observability only**: per-request identity
  takes precedence for ACL.

## Acceptance Criteria

- [ ] `AuthContext` struct with all 4 fields, derives `Clone`
- [ ] `Identity` struct with `id`, `scopes`, `resources`, derives `Clone`, `PartialEq`
- [ ] `AuthToken` struct with `raw` field, derives `Clone`
- [ ] `IdentityProvider` trait with both methods
- [ ] `ConfigIdentityProvider` struct holding `Arc<ArcSwap<DynamicConfig>>`
- [ ] `ConfigIdentityProvider::resolve_from_fingerprint` looks up in authorized_fingerprints
- [ ] `ConfigIdentityProvider::resolve_from_token` parses `alk_` prefix, matches by hash, checks expiry
- [ ] ConfigIdentityProvider reads from ArcSwap on every call (hot-reloadable)
- [ ] Unit test: fingerprint resolution (known fingerprint → Some, unknown → None)
- [ ] Unit test: token resolution (valid non-expired → Some, expired → None, unknown → None)
- [ ] Unit test: config reload changes resolution results immediately
- [ ] `cargo test -p alknet-core` succeeds
- [ ] `cargo clippy -p alknet-core` succeeds with no warnings

## References

- docs/architecture/crates/core/auth.md — all type definitions, resolution flow
- docs/architecture/decisions/004-auth-as-shared-core.md — ADR-004
- docs/architecture/decisions/011-authcontext-structure.md — ADR-011

## Notes

> Auth is hybrid: endpoint resolves TLS-level, handler resolves protocol-level.
> AuthContext may be partial (identity = None). The two identity scopes
> (connection-level for observability, per-request for ACL) must not be
> conflated. ConfigIdentityProvider reads from ArcSwap on every call so config
> reloads take effect immediately.

## Summary

Implemented `AuthContext`, `Identity`, `AuthToken`, `IdentityProvider` trait,
and `ConfigIdentityProvider` in `auth.rs`. ConfigIdentityProvider reads from
`ArcSwap<DynamicConfig>` on every call (hot-reloadable): fingerprint resolution
via `authorized_fingerprints` HashSet, token resolution via `alk_` prefix +
SHA-256 hash + expiry check. Also implemented minimal `config.rs` types
(`DynamicConfig`, `AuthPolicy`, `ApiKeyEntry`, `RateLimitConfig`,
`ConfigReloadHandle`) needed by auth — aligned with architecture docs for the
parallel `core/config` task to extend. 27 unit tests pass; clippy clean.
Merged to develop.