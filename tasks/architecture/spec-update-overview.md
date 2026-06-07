---
id: architecture/spec-update-overview
name: Update overview.md — add crate structure, Layer 3, services, identity
status: pending
depends_on:
  - architecture/adr-027-crate-decomposition
  - architecture/adr-026-transport-interface-separation
  - architecture/adr-033-operationenv-irpc-call-protocol
  - architecture/adr-029-identity-core-type
scope: moderate
risk: low
impact: project
level: implementation
---

## Description

Update `docs/architecture/overview.md` to reflect the expanded scope from the integration plan. The current overview documents the alpha scope (SSH tunneling). It needs additions for:

1. **Crate structure** — alknet-core, alknet-secret, alknet-storage, alknet-flowgraph, alknet-napi, alknet (CLI). Per ADR-027. Note which crates exist now (core, napi) vs which are contracted for later phases (secret, storage, flowgraph).
2. **Three-layer model** — Transport (Layer 1), Interface (Layer 2), Protocol (Layer 3). SSH is an interface, not a transport. Per ADR-026.
3. **Service layer concept** — irpc services for in-cluster communication, OperationEnv for composition. Per ADR-033. Note this is Phase 2+ for the service implementations; Phase 1 uses ConfigIdentityProvider and ConfigServiceImpl directly.
4. **Identity as core type** — Identity struct and IdentityProvider trait in alknet-core. Per ADR-029. Phase 1 ships ConfigIdentityProvider only; StorageIdentityProvider is contracted for when alknet-storage is built.
5. **Updated dependency table** — new dependencies (irpc feature-gated, bip39, rusqlite, honker, petgraph, jsonschema)
6. **Updated ADR table** — add ADRs 026-034
7. **Updated architecture constraints** — add: Interface as Layer 2, OperationEnv as universal composition, event boundary discipline, static/dynamic config split

The existing content (purpose, SSH tunneling, transport pluggability, etc.) stays. We're adding, not replacing.

## Acceptance Criteria

- [ ] Crate structure section added with phase notes: core and napi exist now; secret, storage, flowgraph are Phase 2+ contracts
- [ ] Phase boundary noted: service implementations (irpc) are Phase 2+; Phase 1 uses ConfigIdentityProvider and ArcSwap directly
- [ ] Three-layer model mentioned in architecture constraints per ADR-026
- [ ] Service layer concept mentioned: irpc + OperationEnv per ADR-033
- [ ] Identity and IdentityProvider mentioned as core types per ADR-029
- [ ] Updated dependency table with new crate dependencies
- [ ] ADR table updated: ADRs 026-034 added with correct titles and status
- [ ] Architecture constraints updated: add Layer 2 interface concept, OperationEnv, event boundary, static/dynamic config
- [ ] All new references to architecture specs link correctly (identity.md, services.md, interface.md, configuration.md, etc. — even if those specs are still being written)
- [ ] `last_updated` in YAML frontmatter updated
- [ ] No hub/spoke terminology remains

## References

- docs/research/integration-plan.md — expanded scope, dependency graph
- docs/architecture/overview.md — current content to update

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion