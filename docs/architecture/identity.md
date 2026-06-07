---
status: draft
last_updated: 2026-06-07
---

# Identity

## What

The `Identity` type and `IdentityProvider` trait are the core abstractions for
authentication and authorization in alknet. `Identity` is the unified result of
auth verification — whether via SSH public key, signed timestamp token, or
database lookup. `IdentityProvider` is the trait that resolves credentials to an
`Identity`, decoupling alknet-core from any specific identity storage.

## Why

Auth, forwarding policy, and call protocol all need to know who is making a
request and what they are authorized to do. Without `Identity` in core, each
subsystem would define its own identity type, leading to duplication and
conversion boilerplate. Without `IdentityProvider` as a trait, alknet-core
would either hardcode config-file-based auth or take a database dependency —
neither acceptable for a library crate.

The `IdentityProvider` trait exists because the same auth verification concept
needs two implementations: `ConfigIdentityProvider` for minimal deployments (all
keys in memory via ArcSwap) and `StorageIdentityProvider` for production (SQLite
lookup via `peer_credentials` and ACL graph). The trait is the contract; the
backing store is pluggable.

## Architecture

### Identity Struct

```rust
pub struct Identity {
    pub id: String,                               // Fingerprint or account UUID
    pub scopes: Vec<String>,                      // e.g., ["relay:connect", "service:gitea:read"]
    pub resources: HashMap<String, Vec<String>>,   // e.g., {"service": ["gitea", "registry"]}
}
```

The `id` field serves dual purpose:
- **Config-based auth** (`ConfigIdentityProvider`): holds the Ed25519 key
  fingerprint (e.g., `SHA256:abc123...`)
- **Database-backed auth** (`StorageIdentityProvider`): holds the account UUID
  from the `accounts` table

This keeps the type simple while accommodating both auth paths. Downstream
consumers (forwarding policy, call protocol ACL checks) use `scopes` and
`resources` without knowing whether the identity came from a config file or a
database.

### IdentityProvider Trait

```rust
pub trait IdentityProvider: Send + Sync + 'static {
    /// Resolve an SSH public key fingerprint to an identity.
    fn resolve_from_fingerprint(&self, fingerprint: &str) -> Option<Identity>;

    /// Resolve an auth token to an identity.
    /// Returns None if the token is invalid, expired, or the key is not authorized.
    fn resolve_from_token(&self, token: &AuthToken) -> Option<Identity>;
}
```

Both SSH key auth and token auth resolve to the same `Identity` type. The trait
lives in `alknet_core::auth`.

### ConfigIdentityProvider (Default)

Reads from `ArcSwap<DynamicConfig.auth>` per ADR-030. Every authorized key gets
a default scope set. No database dependency. This is the default for CLI and
single-node deployments.

```rust
pub struct ConfigIdentityProvider {
    auth_config: Arc<ArcSwap<DynamicConfig>>,
}

impl IdentityProvider for ConfigIdentityProvider {
    fn resolve_from_fingerprint(&self, fingerprint: &str) -> Option<Identity> {
        let config = self.auth_config.load();
        config.auth.ssh.authorized_keys.get(fingerprint)
            .map(|key_entry| Identity {
                id: fingerprint.to_string(),
                scopes: key_entry.scopes.clone(),
                resources: key_entry.resources.clone(),
            })
    }

    fn resolve_from_token(&self, token: &AuthToken) -> Option<Identity> {
        // Verify Ed25519 signature against the same authorized_keys set
        // Resolve to the same Identity as SSH auth would produce
    }
}
```

### StorageIdentityProvider (Future — Phase 2+)

Implemented in `alknet-storage` (a crate that doesn't exist yet). Backed by
SQLite `peer_credentials` and `api_keys` tables plus the ACL graph. Resolves
fingerprint → account → organization membership → effective scopes.

This implementation is defined here so the contract is clear, but alknet-storage
hasn't been built yet. Phase 1 uses `ConfigIdentityProvider` exclusively. When
alknet-storage is built, it implements alknet-core's `IdentityProvider` trait,
and the CLI/NAPI assembly layer wires the concrete implementation.

### AuthProtocol irpc Service

The `AuthProtocol` irpc service (behind the `irpc` feature flag per ADR-028)
provides an async boundary for auth verification. It is one way to satisfy the
`IdentityProvider` trait, not a replacement for it:

```rust
enum AuthProtocol {
    VerifyPubkey { fingerprint: String, key_data: Vec<u8> },
    VerifyToken { token_bytes: Vec<u8>, timestamp: u64 },
    ReloadKeys,
    CheckAccess { identity: Identity, operation: String },
}

enum AuthResult {
    Ok(Identity),
    Denied(String),
}
```

The relationship:
- **Trait-based path**: Handler calls `identity_provider.resolve_from_fingerprint()`
  directly. Zero overhead. Used when irpc is disabled or when the
  implementation is local.
- **irpc path**: Handler calls `identity_provider.resolve_from_fingerprint()`,
  which internally delegates to `AuthProtocol::VerifyPubkey` via an irpc client.
  Used in production deployments with SQLite-backed auth.

Both paths produce the same `Identity` result. Note: the irpc path requires the
service layer to be built (Phase 2+). Phase 1 uses the trait path exclusively.

### Auth Flows

**SSH key auth** (existing, unchanged):
```
Client connects → SSH handshake → auth_publickey() callback
  → IdentityProvider::resolve_from_fingerprint(fingerprint)
  → Some(Identity) or None
```

**Token auth** (new, for non-SSH transports):
```
Browser connects → WebTransport CONNECT request
  → Extract token from URL path or Authorization header
  → IdentityProvider::resolve_from_token(token)
  → Some(Identity) or None
```

Both paths produce an `Identity`. The `Identity` is attached to the connection
and used by `ForwardingPolicy` and call protocol for authorization decisions.

## Constraints

- `Identity` and `IdentityProvider` live in `alknet_core::auth`. No database
  dependency at the core level (ADR-029).
- alknet-storage implements the core trait — the dependency goes from storage
  to core, not the other way.
- The `id` field in `Identity` serves dual purpose (fingerprint or UUID). This
  is a deliberate simplification — downstream consumers don't need to know the
  source.
- Certificate authority tokens are not supported for token auth in v1 (ADR-023).
- The irpc feature flag means nodes that only do SSH tunneling don't need the
  service layer overhead.

## Open Questions

- None specific to this spec. See [open-questions.md](open-questions.md) for
  general auth questions (OQ-15, OQ-19).

## Design Decisions

| ADR | Decision | Summary |
|-----|----------|---------|
| [029](decisions/029-identity-core-type.md) | Identity as core type | `Identity` and `IdentityProvider` live in alknet-core, not storage |
| [028](decisions/028-auth-irpc-service.md) | Auth as irpc service | `AuthProtocol` behind feature flag; `IdentityProvider` is the contract |
| [023](decisions/023-unified-auth-shared-key-material.md) | Unified auth | Same key material for SSH and token auth; same `Identity` result |

## References

- [auth.md](auth.md) — Token authentication, AuthPolicy, WebTransport session handling
- [research/services.md](../research/services.md) — AuthService, AuthProtocol definition
- [research/integration-plan.md](../research/integration-plan.md) — Phase 1.2
- [ADR-030](decisions/030-static-dynamic-config-split.md) — DynamicConfig (ConfigIdentityProvider reads from it)
- [ADR-031](decisions/031-forwarding-policy.md) — ForwardingPolicy consumes Identity.scopes