---
id: core/auth-service-irpc
name: Implement AuthProtocol irpc service enum behind feature flag
status: pending
depends_on:
  - core/identity-type-provider
scope: narrow
risk: medium
impact: component
level: implementation
---

## Description

Define `AuthProtocol` irpc service enum behind the `irpc` feature flag in alknet-core, per ADR-028 and identity.md.

The `AuthProtocol` provides an async boundary for auth verification. `ConfigIdentityProvider` wraps `ArcSwap<DynamicConfig>` directly in Phase 1 (the trait-based path). When the service layer is enabled, `AuthServiceImpl` delegates to `ConfigIdentityProvider` via irpc. The trait-based path and the irpc path produce identical `Identity` results.

**Key additions** (behind `irpc` feature flag):
- `AuthProtocol` enum: `VerifyPubkey`, `VerifyToken`, `ReloadKeys`, `CheckAccess`
- `AuthResult` enum: `Ok(Identity)`, `Denied(String)`
- `AuthServiceImpl` backed by `ConfigIdentityProvider` (ArcSwap path)

**What stays the same**: The `IdentityProvider` trait is the contract. Without the `irpc` feature, auth goes through `ConfigIdentityProvider` directly. With the feature, `AuthServiceImpl` provides an irpc entry point.

## Acceptance Criteria

- [ ] `AuthProtocol` enum defined in `crates/alknet-core/src/auth/auth_protocol.rs` (behind `irpc` feature flag)
- [ ] `AuthResult` type defined (matching identity.md spec)
- [ ] `AuthServiceImpl` implemented, wrapping `ConfigIdentityProvider` (ArcSwap path)
- [ ] `irpc` feature flag added to alknet-core's `Cargo.toml`
- [ ] Without `irpc` feature, the code compiles and all existing tests pass unchanged
- [ ] With `irpc` feature, `AuthProtocol` and `AuthServiceImpl` are available
- [ ] `AuthServiceImpl::verify_pubkey()` produces the same `Identity` as `ConfigIdentityProvider::resolve_from_fingerprint()`

## References

- docs/architecture/decisions/028-auth-irpc-service.md — ADR-028
- docs/architecture/identity.md — AuthProtocol enum, AuthResult, AuthServiceImpl
- docs/architecture/services.md — Service definition pattern

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion