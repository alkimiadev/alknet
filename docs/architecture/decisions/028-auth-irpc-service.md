# ADR-028: Auth as irpc Service

## Status

Accepted

## Context

For head nodes serving many users, in-memory key lookup via `ArcSwap<DynamicConfig>`
doesn't scale. Loading all authorized keys into RAM and atomic-swapping the
entire set on each reload works for small deployments but requires holding every
key in memory. For production deployments with hundreds or thousands of users,
auth verification should query a database on demand rather than holding all keys
in memory.

The current `ArcSwap<DynamicConfig>` approach works for CLI and single-node
setups. What's needed is an async boundary that allows auth verification to go
through a service — locally via channels for minimal deployments, or via irpc
for production deployments where auth runs on a separate process or node.

The critical design point: callers go through the `IdentityProvider` trait
(ADR-029). The irpc service is one way to satisfy the trait. Both paths produce
the same result — an `Identity` or rejection. The trait is the contract; the
service is an implementation path.

## Decision

**Auth verification is provided via an irpc service protocol, with
`IdentityProvider` as the interface contract and `ConfigIdentityProvider`
(ArcSwap-backed) as the default implementation.**

### IdentityProvider Trait (ADR-029) — The Contract

Callers depend on `IdentityProvider`, not on any concrete implementation:

```rust
pub trait IdentityProvider: Send + Sync + 'static {
    fn resolve_from_fingerprint(&self, fingerprint: &str) -> Option<Identity>;
    fn resolve_from_token(&self, token: &AuthToken) -> Option<Identity>;
}
```

### ConfigIdentityProvider — Default Implementation

Reads from `ArcSwap<DynamicConfig.auth>`. No database needed. Every authorized
key gets a default scope set. This is the default for CLI and single-node
deployments.

### AuthProtocol irpc Service — Behind Feature Flag

```rust
#[rpc_requests(message = AuthMessage)]
#[derive(Debug, Serialize, Deserialize)]
enum AuthProtocol {
    #[rpc(tx=oneshot::Sender<AuthResult>)]
    #[wrap(VerifyPubkey)]
    VerifyPubkey { fingerprint: String, key_data: Vec<u8> },

    #[rpc(tx=oneshot::Sender<AuthResult>)]
    #[wrap(VerifyToken)]
    VerifyToken { token_bytes: Vec<u8>, timestamp: u64 },

    #[rpc(tx=oneshot::Sender<()>)]
    #[wrap(ReloadKeys)]
    ReloadKeys,

    #[rpc(tx=oneshot::Sender<bool>)]
    #[wrap(CheckAccess)]
    CheckAccess { identity: Identity, operation: String },
}

enum AuthResult {
    Ok(Identity),
    Denied(String),
}
```

The `AuthProtocol` is behind the `irpc` feature flag in alknet-core. Nodes
that only do SSH tunneling don't need the service layer overhead. When the
feature is disabled, auth goes through `IdentityProvider` directly.

### AuthServiceImpl

Two implementations exist (the second is a future phase):

- **ConfigAuthService** — backed by `ConfigIdentityProvider` (ArcSwap path).
  Wraps the trait in an irpc service for deployments that use the service layer
  but don't have SQLite. This is the Phase 1 path: it ships with alknet-core.
- **StorageAuthService** — backed by SQLite `peer_credentials` and `api_keys`
  tables (in alknet-storage, not yet built). Queries on demand. Can maintain an
  LRU cache for hot fingerprints. This is a Phase 2+ implementation — the
  contract is defined here so alknet-storage can implement it later.

Both produce the same `AuthResult` — an `Identity` or a denial. Callers don't
know or care which backend is running.

### Integration with IdentityProvider

The irpc service and the trait compose. A caller goes through `IdentityProvider`,
which may internally delegate to the irpc service, or may satisfy the request
locally via `ConfigIdentityProvider`. The deployment topology determines the
path:

- **Minimal (CLI, single-node)**: `ConfigIdentityProvider` reads from
  `ArcSwap<DynamicConfig>`. No irpc overhead.
- **Production with local auth**: `AuthServiceImpl` wraps
  `StorageIdentityProvider` locally. The handler calls `IdentityProvider` which
  routes to the local irpc service.
- **Distributed auth**: Handler on a worker node calls `IdentityProvider` which
  routes to a remote auth irpc service over QUIC.

### ConfigService Integration

`AuthProtocol::ReloadKeys` triggers reload of the dynamic config's auth section.
For the `ConfigIdentityProvider` path, this is equivalent to
`ConfigReloadHandle::reload()`. For the `StorageIdentityProvider` path, this
refreshes the LRU cache. Both update atomically — ongoing connections are
unaffected, new connections pick up changes.

## Consequences

- **Positive**: Minimal deployments use `ArcSwap` without irpc overhead. No
  database dependency for CLI users.
- **Positive**: Production deployments wire `StorageIdentityProvider` behind the
  irpc service. Auth scales to thousands of users without loading all keys into
  memory.
- **Positive**: The `IdentityProvider` trait is the only contract callers depend
  on. This keeps alknet-core lean and testable.
- **Positive**: Feature flag (`irpc`) keeps core lean for deployments that don't
  need the service layer.
- **Positive**: Both paths produce identical `Identity` results. Behavioral
  parity is enforced by the shared `Identity` type.
- **Negative**: Two implementations must be kept in sync. `ConfigIdentityProvider`
  and `StorageIdentityProvider` must produce the same `Identity` for the same
  input. Integration tests should verify this.
- **Negative**: The `irpc` feature flag adds conditional compilation complexity.
  The core must compile and work without it, and the service layer must work
  with it enabled.

## References

- [research/services.md](../../research/services.md) — AuthService, AuthProtocol definition
- [auth.md](../auth.md) — IdentityProvider trait, Identity struct
- [research/configuration.md](../../research/configuration.md) — Auth service approach
- [research/integration-plan.md](../../research/integration-plan.md) — Phase 1.4
- [ADR-029](029-identity-core-type.md) — Identity as core type
- [ADR-027](027-crate-decomposition.md) — Crate decomposition