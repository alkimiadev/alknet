---
id: architecture/spec-identity
name: Create identity.md architecture spec
status: completed
depends_on:
  - architecture/adr-029-identity-core-type
  - architecture/adr-028-auth-irpc-service
scope: narrow
risk: low
impact: phase
level: implementation
---

## Description

Create `docs/architecture/identity.md` — a new architecture spec defining the `Identity` type, `IdentityProvider` trait, and the auth flows for SSH key-based and token-based authentication.

This is mostly a carry-forward from `auth.md` (which already defines `IdentityProvider` trait and `Identity` struct) plus the Identity section from `research/services.md`. The key addition is making the IdentityProvider vs AuthService relationship explicit per ADR-028: `IdentityProvider` is the contract, `ConfigIdentityProvider` is the default ArcSwap-backed impl, and `AuthProtocol` irpc service is one way to satisfy the trait (behind feature flag).

**Source material**: 
- `auth.md` sections: IdentityProvider Trait, AuthPolicy Structure, Auth Flow in the Server, Token Authentication
- `research/services.md` AuthService section (AuthProtocol enum, AuthResult type)
- ADR-029 (identity as core type), ADR-028 (auth as irpc service), ADR-023 (unified auth)

**Relationship to auth.md**: After identity.md exists, auth.md should be updated to reference identity.md for the `Identity` and `IdentityProvider` definitions rather than defining them inline. This is handled in the `auth.md` update task.

## Acceptance Criteria

- [ ] `docs/architecture/identity.md` exists with YAML frontmatter (`status: draft`)
- [ ] Follows spec format: What, Why, Architecture, Constraints, Open Questions, Design Decisions
- [ ] Defines `Identity` struct: `{ id, scopes, resources }` — canonical definition per ADR-029
- [ ] Defines `IdentityProvider` trait: `resolve_from_fingerprint()`, `resolve_from_token()`
- [ ] Documents default implementation: `ConfigIdentityProvider` reading from `ArcSwap<DynamicConfig.auth>`
- [ ] Documents head implementation: `StorageIdentityProvider` backed by SQLite `peer_credentials` + ACL graph (in alknet-storage, not core)
- [ ] Documents irpc service path: `AuthProtocol` enum (VerifyPubkey, VerifyToken, ReloadKeys, CheckAccess) behind `irpc` feature flag per ADR-028
- [ ] Shows both auth flows: SSH key path and token auth path, both resolving to same `Identity`
- [ ] Consistent head/worker terminology
- [ ] References ADR-029, ADR-028, ADR-023
- [ ] `docs/architecture/README.md` updated to include identity.md

## References

- docs/architecture/auth.md — existing IdentityProvider and Identity definitions
- docs/research/services.md — AuthService, AuthProtocol enum
- docs/architecture/decisions/029-identity-core-type.md — identity placement decision
- docs/architecture/decisions/028-auth-irpc-service.md — auth as irpc service
- docs/architecture/decisions/023-unified-auth-shared-key-material.md — unified auth

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion