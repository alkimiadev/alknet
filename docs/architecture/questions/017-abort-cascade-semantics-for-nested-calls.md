# OQ-17: Abort Cascade Semantics for Nested Calls

- **Origin**: [call-protocol.md](crates/call/call-protocol.md), [operation-registry.md](crates/call/operation-registry.md)
- **Status**: resolved
- **Door type**: One-way (protocol schema), two-way (mechanism)
- **Priority**: high
- **Resolution**: `call.aborted` cascades to all non-terminal descendants in the call tree. The CallAdapter walks the tree (indexed by `parent_request_id` in `PendingRequestMap`) and sends `call.aborted` for each descendant. Default policy is `abort-dependents` (abort everything downstream); `continue-running` is an opt-in for long-running work that should survive a parent's abort. Handlers clean up via Rust's async drop semantics (future dropped → `Drop` guards release resources). The cascade is protocol-level (server discovers descendants and propagates); the mechanism (parent-indexed map, cancellation tokens, or a separate graph) is a two-way door. See ADR-016.
- **Cross-references**: ADR-012, ADR-015, ADR-016, [call-protocol.md](crates/call/call-protocol.md), [operation-registry.md](crates/call/operation-registry.md)
