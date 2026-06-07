---
id: architecture/spec-storage
name: Create storage.md architecture spec (or stub referencing crate docs)
status: pending
depends_on:
  - architecture/adr-027-crate-decomposition
  - architecture/adr-029-identity-core-type
  - architecture/adr-032-event-boundary-discipline
scope: moderate
risk: low
impact: component
level: implementation
---

## Description

Create `docs/architecture/storage.md` — an architecture spec for the `alknet-storage` crate, covering the metagraph data model, identity tables, ACL graph, honker integration, and StorageProtocol irpc service.

The integration plan notes this could be "a new spec or reference alknet-storage's own docs." Since alknet-storage doesn't exist yet as a crate, we need an architecture spec here to define its contract — especially the interface back to core (`StorageIdentityProvider` implementing alknet-core's `IdentityProvider` trait).

If the crate will have its own detailed docs later, this spec can be a contract-level document: what storage provides, what it depends on, and how it connects to core.

**Source**: `docs/research/storage.md` (460 lines, comprehensive)

## Acceptance Criteria

- [ ] `docs/architecture/storage.md` exists with YAML frontmatter (`status: draft`)
- [ ] Follows spec format: What, Why, Architecture, Constraints, Open Questions, Design Decisions
- [ ] Documents metagraph data model: GraphType, NodeType, EdgeType, Graph, Node, Edge
- [ ] Documents identity tables: accounts, organizations, peer_credentials, api_keys, audit_logs
- [ ] Documents ACL as metagraph: PrincipalNode, DelegatesEdge, access control graph
- [ ] Documents encrypted node type: bridges to alknet-secret's EncryptedData format
- [ ] Documents honker integration: stream_publish/subscribe, notify/listen, queue/claim
- [ ] Documents System DB vs Tenant DB separation
- [ ] Documents `StorageIdentityProvider`: implements alknet-core's `IdentityProvider` trait (queries peer_credentials + ACL graph) per ADR-029
- [ ] Documents `StorageProtocol` irpc service with key variants
- [ ] States crate dependencies: rusqlite, honker, petgraph, jsonschema, irpc
- [ ] States crate does NOT depend on alknet-core (implements core's trait by depending on alknet-core types, not the full crate — or via the trait definition only)
- [ ] Event boundary per ADR-032: honker streams stay within storage service, StorageProtocol serves as internal boundary, call protocol events are projections
- [ ] References ADR-027, ADR-029, ADR-032
- [ ] `docs/architecture/README.md` updated to include storage.md

## References

- docs/research/storage.md — full metagraph, identity, ACL, honker definitions
- docs/research/integration-plan.md — Phase 2.2 (alknet-storage)
- docs/architecture/decisions/027-crate-decomposition.md
- docs/architecture/decisions/029-identity-core-type.md

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion