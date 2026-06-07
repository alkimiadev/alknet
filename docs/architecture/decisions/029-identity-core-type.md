# ADR-029: Identity as Core Type

## Status

Accepted

## Context

The `Identity` struct and `IdentityProvider` trait are needed by auth,
forwarding policy, and call protocol — three different subsystems in
alknet-core. Without placing them in core, these subsystems would each define
their own identity type, leading to duplication and conversion boilerplate.

The constraint: alknet-core must not depend on alknet-storage or any database.
The `IdentityProvider` trait must be in core so that the handler can resolve
identities without knowing whether the backing store is a config file or a
SQLite database. External crates provide implementations.

Earlier research defined `Identity` inconsistently: `{node_id, fingerprint,
scopes}` in services.md and `{id, scopes, resources}` in auth.md. The unified
model uses `{id, scopes, resources}` where `id` serves as both fingerprint (for
key-based auth from config) and account UUID (for database-backed auth).

## Decision

**`Identity` struct and `IdentityProvider` trait live in `alknet_core::auth`.**

### Identity Struct

```rust
pub struct Identity {
    pub id: String,                               // Fingerprint (config auth) or account UUID (database auth)
    pub scopes: Vec<String>,                      // e.g., ["relay:connect", "service:gitea:read"]
    pub resources: HashMap<String, Vec<String>>,   // e.g., {"service": ["gitea", "registry"]}
}
```

The `id` field serves dual purpose: when using config-based authentication
(`ConfigIdentityProvider`), it holds the Ed25519 key fingerprint. When using
database-backed authentication (`StorageIdentityProvider`), it holds the account
UUID from the `accounts` table. This keeps the type simple while accommodating
both auth paths.

The `scopes` field provides authorization scope strings used by
`ForwardingPolicy` and `AccessControl` in `OperationSpec`. The `resources`
field provides resource-level authorization beyond what scopes offer (e.g., which
services this identity can access).

### IdentityProvider Trait

```rust
pub trait IdentityProvider: Send + Sync + 'static {
    fn resolve_from_fingerprint(&self, fingerprint: &str) -> Option<Identity>;
    fn resolve_from_token(&self, token: &AuthToken) -> Option<Identity>;
}
```

The trait is the contract. Callers (auth handler, forwarding policy, call
protocol) depend on `IdentityProvider` — not on any concrete implementation.

### Default and Production Implementations

- **`ConfigIdentityProvider`** (in alknet-core) — reads from
  `ArcSwap<DynamicConfig.auth>`. Every authorized key gets a default scope set.
  No database needed. This is the default for minimal deployments.
- **`StorageIdentityProvider`** (in alknet-storage) — backed by SQLite
  `peer_credentials` and `api_keys` tables plus the ACL graph. Resolves
  fingerprint → account → organization membership → effective scopes. This is
  the production implementation for head nodes.

alknet-core never depends on alknet-storage. The trait relationship is:
alknet-core *defines* the trait, alknet-storage *implements* it. The CLI or
NAPI assembly layer wires the concrete implementation.

### Why Not in alknet-storage?

If `Identity` lived in alknet-storage, alknet-core would need to depend on
alknet-storage to use the type — creating a circular dependency (since
alknet-storage implements alknet-core's `IdentityProvider` trait). Placing the
type and trait in core breaks the cycle.

## Consequences

- **Positive**: alknet-core has no database dependency. Auth, forwarding, and
  call protocol all use the same `Identity` type.
- **Positive**: alknet-storage implements the core trait. The CLI/NAPI layer
  wires the concrete implementation. Deployment topology determines which impl
  to use.
- **Positive**: The `id` field serves dual purpose (fingerprint or UUID),
  avoiding separate types for config-based and database-based auth.
- **Positive**: `ForwardingPolicy` and `AccessControl` can reference scopes from
  `Identity` without knowing where they came from.
- **Negative**: Two implementations of `IdentityProvider` exist — `Config` and
  `Storage`. Both must produce identical `Identity` results for the same input.
  Tests should verify behavioral parity.
- **Negative**: The trait abstraction adds a level of indirection for the
  minimal (config-only) deployment path. The cost is negligible — the
  `ConfigIdentityProvider` is a simple `ArcSwap` dereference.

## References

- [auth.md](../auth.md) — IdentityProvider trait, Identity struct, unified auth
- [research/services.md](../../research/services.md) — AuthService, Identity section
- [research/integration-plan.md](../../research/integration-plan.md) — Phase 1.2
- [ADR-023](023-unified-auth-shared-key-material.md) — Unified auth with shared key material
- [ADR-028](028-auth-irpc-service.md) — Auth as irpc service
- [OQ-18](../open-questions.md) — IdentityProvider owns scopes