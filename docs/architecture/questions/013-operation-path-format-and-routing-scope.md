# OQ-13: Operation Path Format and Routing Scope

- **Origin**: [operation-registry.md](crates/call/operation-registry.md)
- **Status**: resolved
- **Door type**: Two-way
- **Priority**: medium
- **Resolution**: alknet-call uses `/{service}/{op}` (e.g., `/fs/readFile`, `/agent/chat`, `/services/list`). The `/{node}/{service}/{op}` pattern from the reference implementation served a head/worker routing model that is a separate architectural concern. Remote dispatch (federation / node-level routing) would be a different mechanism at a different layer, not a prefix added to alknet-call's operation paths. See ADR-005, ADR-012.
- **Cross-references**: ADR-005, ADR-012
