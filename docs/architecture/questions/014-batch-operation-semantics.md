# OQ-14: Batch Operation Semantics

- **Origin**: [call-protocol.md](crates/call/call-protocol.md)
- **Status**: resolved
- **Door type**: Two-way
- **Priority**: low
- **Resolution**: Batch is a client-side pattern — multiple `call.requested` events with correlated IDs, responses arrive independently. QUIC's stream multiplexing provides the concurrency and ordering guarantees that batch would need. Batch-specific event types (e.g., `batch.requested`, `batch.responded`) would add protocol complexity without clear benefit over sending multiple `call.requested` events. See ADR-012.
- **Cross-references**: ADR-012
