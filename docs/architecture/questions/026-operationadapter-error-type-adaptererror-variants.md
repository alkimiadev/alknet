# OQ-26: OperationAdapter Error Type (AdapterError Variants)

- **Origin**: [client-and-adapters.md](crates/call/client-and-adapters.md), ADR-017 §5, [ADR-029](decisions/029-peer-graph-routing-model.md) §5
- **Status**: **resolved** (2026-06-27)
- **Door type**: Two-way
- **Priority**: medium
- **Resolution**: The `AdapterError` enum is `#[non_exhaustive]` +
  `thiserror::Error`, with these v1 variants:
  - `DiscoveryFailed { message: String }` — `from_call` remote unreachable / `services/list` failed
  - `SchemaParse { message: String }` — `from_openapi` / `from_jsonschema` couldn't parse the spec
  - `Transport { message: String }` — underlying transport error (QUIC for `from_call`, HTTP for `from_openapi`/`from_mcp`)
  - `Unauthorized { message: String }` — HTTP 401 for `from_openapi`/`from_mcp`, auth rejected for `from_call`
  - `SamePeerCollision { message: String }` — namespace collision *within a single peer* (ADR-029 §5: cross-peer collision dissolves; same-peer collision stays an error). Replaces the flat `Conflict` variant from the pre-ADR-029 implementation.

  `#[non_exhaustive]` lets `alknet-http`'s adapters extend without breaking
  match arms. The variant payloads are `String` messages — kept simple and
  `Send + Sync` by construction. This matches the shipped implementation
  (`crates/alknet-call/src/client/mod.rs`) except `Conflict` →
  `SamePeerCollision` (the ADR-029 migration renames it). Two-way door:
  adding variants later is non-breaking; renaming a variant is a match-arm
  update but not an architectural change.
- **Cross-references**: ADR-017, ADR-029, [client-and-adapters.md](crates/call/client-and-adapters.md)
