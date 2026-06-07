---
id: architecture/adr-029-identity-core-type
name: Write ADR-029 — Identity as core type
status: pending
depends_on: []
scope: single
risk: low
impact: project
level: implementation
---

## Description

Write ADR-029 establishing `Identity` struct and `IdentityProvider` trait as core types in `alknet-core`.

The `Identity` struct and `IdentityProvider` trait are already defined in `auth.md` (the draft architecture spec). This ADR formalizes the decision that they live in `alknet-core` — not in alknet-storage, not in alknet-services — so that core auth, forwarding policy, and call protocol all reference the same type without circular dependencies.

The key constraint: alknet-core defines the trait, external crates provide implementations. `ConfigIdentityProvider` (ArcSwap-backed, in core) is the default. `StorageIdentityProvider` (SQLite-backed, in alknet-storage) is the production impl. Core never depends on storage.

## Acceptance Criteria

- [ ] `docs/architecture/decisions/029-identity-core-type.md` exists
- [ ] ADR follows established format
- [ ] Context explains why Identity must be in core: auth, forwarding, call protocol all need it; can't have circular deps
- [ ] Decision states: `Identity { id, scopes, resources }` and `IdentityProvider` trait live in `alknet_core::auth`; `id` is a fingerprint (config-based auth) or account UUID (database-backed auth); derivation and storage are external concerns; default `ConfigIdentityProvider` reads from `DynamicConfig.auth`; production `StorageIdentityProvider` is in alknet-storage
- [ ] Consequences: alknet-core has no database dependency; alknet-storage implements the core trait; the `id` field serves dual purpose (fingerprint or UUID)
- [ ] Resolves OQ-18: IdentityProvider owns scopes, ForwardingPolicy uses scopes from Identity
- [ ] References: auth.md, research/services.md Identity section, research/integration-plan.md

## References

- docs/architecture/auth.md — Identity and IdentityProvider trait definitions
- docs/research/services.md — Identity section
- docs/research/integration-plan.md — ADR 029 entry, Phase 1.2
- docs/architecture/open-questions.md — OQ-18

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion