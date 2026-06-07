---
id: architecture/spec-flowgraph
name: Create flowgraph.md architecture spec (or stub referencing crate docs)
status: pending
depends_on:
  - architecture/adr-027-crate-decomposition
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Create `docs/architecture/flowgraph.md` — an architecture spec for the `alknet-flowgraph` crate, covering FlowGraph<N,E>, operation graph construction, call graph population, and type compatibility checking.

Like storage.md, this can be a contract-level document if the crate will have its own docs later. The key contract point: `OperationSpec` and `CallNodeAttrs` types must match alknet-core's definitions, with serialization as the bridge.

**Source**: `docs/research/flow.md` (472 lines, straightforward port of TypeScript design)

This is the lowest-risk new spec — pure computation crate, no I/O, no external state, straightforward petgraph port.

## Acceptance Criteria

- [ ] `docs/architecture/flowgraph.md` exists with YAML frontmatter (`status: draft`)
- [ ] Follows spec format: What, Why, Architecture, Constraints, Open Questions, Design Decisions
- [ ] Documents `FlowGraph<N, E>` generic graph over `petgraph::DiGraph`
- [ ] Documents `NodeAttributes` / `EdgeAttributes` traits
- [ ] Documents operation graph construction from `OperationSpec`s
- [ ] Documents call graph population from `EventEnvelope` events
- [ ] Documents type compatibility checking (jsonschema)
- [ ] Documents cycle detection, topological sort, reachability queries
- [ ] Documents serde serialization/deserialization
- [ ] States crate dependencies: petgraph, serde, serde_json, jsonschema, thiserror
- [ ] States crate does NOT depend on alknet-core, alknet-storage, or alknet-secret
- [ ] States interface back to core: OperationSpec and CallNodeAttrs types match alknet-core's definitions; bridge is serialization
- [ ] References ADR-027
- [ ] `docs/architecture/README.md` updated to include flowgraph.md

## References

- docs/research/flow.md — full FlowGraph, operation graph, call graph design
- docs/research/integration-plan.md — Phase 2.3 (alknet-flowgraph)
- docs/architecture/decisions/027-crate-decomposition.md

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion