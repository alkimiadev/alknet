# OQ-18: Privilege Model and Authority Context

- **Origin**: [operation-registry.md](crates/call/operation-registry.md)
- **Status**: resolved
- **Door type**: One-way (ACL model), two-way (specific APIs)
- **Priority**: high
- **Resolution**: The `internal` flag on `OperationContext` marks calls that originated from composition (a handler calling another operation via `OperationEnv`), as opposed to external calls that arrived as `call.requested` from a wire client. The `internal` flag switches the authority context: the ACL check runs against the composing handler's identity (set at registration), not the caller's identity and not as a blanket skip. This replaces the previous `trusted` flag, which skipped ACL entirely — a privilege escalation vector. Operations have External/Internal visibility. Internal operations return `NOT_FOUND` when called from the wire and are excluded from `services/list`. The composition env is scoped — a handler can only invoke a declared set of operations. Handler identity is carried on `OperationContext` alongside caller identity (the principal/agent pair). See ADR-015.
- **Cross-references**: ADR-014, ADR-015, [call-protocol.md](crates/call/call-protocol.md), [operation-registry.md](crates/call/operation-registry.md)
