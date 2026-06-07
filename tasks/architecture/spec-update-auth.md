---
id: architecture/spec-update-auth
name: Update auth.md — add IdentityProvider vs AuthService relationship
status: pending
depends_on:
  - architecture/spec-identity
  - architecture/adr-028-auth-irpc-service
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Update `docs/architecture/auth.md` to add the IdentityProvider vs AuthService relationship and update for the `AuthProtocol` irpc service per ADR-028.

The current auth.md already defines `IdentityProvider` trait and `Identity` struct — which is good. After identity.md exists as the canonical home for those definitions, auth.md should reference identity.md instead of defining them inline.

**Changes needed**:
1. Replace inline `Identity` and `IdentityProvider` definitions with references to identity.md
2. Add section on `AuthProtocol` irpc service (VerifyPubkey, VerifyToken, ReloadKeys, CheckAccess) behind `irpc` feature flag
3. Add section on `ConfigIdentityProvider` as the default impl (ArcSwap-backed)
4. Clarify the relationship: `IdentityProvider` is the contract, irpc `AuthProtocol` is one way to implement it, `ConfigIdentityProvider` is another
5. Remove inline decision rationale about IdentityProvider placement — reference ADR-029
6. Reference ADR-028 for the irpc service decision

**What stays the same**: Token authentication design, AuthPolicy structure, browser-side token construction, WebTransport session request inspection, security considerations, all existing constraints.

## Acceptance Criteria

- [ ] `Identity` and `IdentityProvider` definitions reference identity.md (canonical) rather than defining inline
- [ ] `AuthProtocol` irpc service documented with variants (VerifyPubkey, VerifyToken, ReloadKeys, CheckAccess) per ADR-028
- [ ] `ConfigIdentityProvider` documented as default implementation (ArcSwap path)
- [ ] Relationship between trait-based path and irpc path clearly stated
- [ ] `irpc` feature flag mentioned for AuthProtocol
- [ ] Inline decision rationale replaced with ADR references (028, 029)
- [ ] `last_updated` in YAML frontmatter updated
- [ ] No hub/spoke terminology
- [ ] References section updated to include identity.md, ADR-028, ADR-029

## References

- docs/architecture/auth.md — current content to update
- docs/research/integration-plan.md — auth.md update entry
- docs/architecture/decisions/028-auth-irpc-service.md
- docs/architecture/decisions/029-identity-core-type.md

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion