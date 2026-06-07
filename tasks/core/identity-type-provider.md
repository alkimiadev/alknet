---
id: core/identity-type-provider
name: Implement Identity struct and IdentityProvider trait
status: pending
depends_on:
  - core/config-static-dynamic-split
scope: moderate
risk: low
impact: component
level: implementation
---

## Description

Define `Identity` struct and `IdentityProvider` trait in `alknet_core::auth`, per ADR-029 and identity.md. This is the contract that decouples auth verification from any specific storage.

The `Identity` type is the unified result of auth verification — whether via SSH public key, signed timestamp token, or database lookup. The `IdentityProvider` trait resolves credentials to an `Identity`, decoupling alknet-core from any specific identity storage.

**Key additions**:
- `Identity` struct: `{ id: String, scopes: Vec<String>, resources: HashMap<String, Vec<String>> }`
- `IdentityProvider` trait: `resolve_from_fingerprint(&str) -> Option<Identity>` and `resolve_from_token(&AuthToken) -> Option<Identity>`
- `ConfigIdentityProvider`: reads from `ArcSwap<DynamicConfig.auth>`, the default implementation for minimal deployments
- `AuthToken` type for future token-based auth (WebTransport, etc.)

**Key changes**:
- `ServerHandler::auth_publickey()` currently reads from `Arc<ServerAuthConfig>` directly. After this task, it goes through `IdentityProvider::resolve_from_fingerprint()`.
- The `Identity` (specifically `id` and `scopes`) will be attached to the SSH session for use by `ForwardingPolicy` (task 1.3).

**Depends on config-static-dynamic-split** because `ConfigIdentityProvider` reads from `ArcSwap<DynamicConfig>`, which must exist first.

## Acceptance Criteria

- [ ] `Identity` struct defined in `crates/alknet-core/src/auth/identity.rs` with `id`, `scopes`, `resources` fields
- [ ] `IdentityProvider` trait defined in `crates/alknet-core/src/auth/identity.rs` with `resolve_from_fingerprint` and `resolve_from_token` methods
- [ ] `ConfigIdentityProvider` implemented, reading from `ArcSwap<DynamicConfig.auth>` for key lookups and producing `Identity` with scopes/resources from key entries
- [ ] `AuthToken` struct defined (placeholder for future token auth — just the type, no verification logic needed yet)
- [ ] `ServerHandler::auth_publickey()` delegated through `IdentityProvider` instead of reading directly from `ServerAuthConfig`
- [ ] Authenticated `Identity` stored in the session/handler for later use by `ForwardingPolicy`
- [ ] All existing auth tests pass (behavior is identical — `ConfigIdentityProvider` wraps what `ServerAuthConfig.authenticate_publickey()` already does)
- [ ] New unit tests: `ConfigIdentityProvider::resolve_from_fingerprint()` returns `Some(Identity)` for valid keys, `None` for invalid
- [ ] New unit tests: `Identity` struct has correct `id`, `scopes`, `resources`

## References

- docs/architecture/identity.md — Identity struct, IdentityProvider trait, ConfigIdentityProvider
- docs/architecture/decisions/029-identity-core-type.md — ADR-029
- docs/architecture/decisions/028-auth-irpc-service.md — AuthProtocol behind feature flag, IdentityProvider is the contract
- crates/alknet-core/src/auth/server_auth.rs — current ServerAuthConfig to be wrapped by ConfigIdentityProvider
- crates/alknet-core/src/server/handler.rs — auth_publickey() to be delegated to IdentityProvider

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion