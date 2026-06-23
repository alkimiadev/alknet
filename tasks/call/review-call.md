---
id: call/review-call
name: Review alknet-call implementation for spec conformance and pattern consistency
status: pending
depends_on: [call/protocol/abort-cascade]
scope: broad
risk: low
impact: phase
level: review
---

## Description

Review the alknet-call implementation for spec conformance, pattern
consistency, and correctness. This is the quality checkpoint at the end of the
call phase — the most complex crate in this batch.

### Review Checklist

1. **Registry conformance** (operation-registry.md):
   - `OperationSpec` has all 8 fields, `path()` adds leading slash
   - `OperationType` (Query/Mutation/Subscription), `Visibility` (External/Internal)
   - `ErrorDefinition` with code, description, schema, http_status (ADR-023)
   - `AccessControl` with required_scopes (AND), required_scopes_any (OR), resource checks
   - `AccessControl::check` returns Allowed/Forbidden, None identity with restrictions → Forbidden
   - `OperationContext` has all 10 fields, `internal` is pub(crate), `is_internal()` reads
   - `AbortPolicy` (AbortDependents default, ContinueRunning opt-in)
   - `CompositionAuthority` with label, scopes, resources, `as_identity()`
   - `ScopedOperationEnv` with `allows()` reachability check
   - `Handler` type (async closure → ResponseEnvelope)
   - `HandlerRegistration` with all 6 fields (spec, handler, provenance, authority, scoped_env, caps)
   - `OperationProvenance` with all 6 variants
   - `OperationRegistry` with register, registration, invoke, list_operations
   - `OperationRegistryBuilder` with with_local, with_leaf, with, build
   - `OperationEnv` trait with invoke, invoke_with_policy, contains
   - `LocalOperationEnv` reachability check, authority switch, fresh metadata, inherited deadline
   - `CompositeOperationEnv` overlay dispatch (session → connection → base), contains probe
   - `services/list` returns External only, `services/schema` returns full spec

2. **Protocol conformance** (call-protocol.md):
   - `EventEnvelope` with type, id, payload (JSON, length-prefixed framing)
   - `ResponseEnvelope` with request_id, result
   - `CallError` with code, message, retryable, details
   - 5 event types: call.requested, call.responded, call.completed, call.aborted, call.error
   - Wire payload schemas match spec table
   - `call.requested` has operationId (leading slash), input, optional auth_token
   - `call.error` has protocol-level codes (NOT_FOUND, FORBIDDEN, INVALID_INPUT, INTERNAL, TIMEOUT)
   - `PendingRequestMap` correlates by ID (not stream), handles all event types
   - `CallConnection` with Layer 2 overlay, register_imported, overlay_env, call/subscribe/abort
   - `CallAdapter` implements ProtocolHandler for alknet/call
   - CallAdapter stream handling (accept_bi loop, FrameFramedReader/Writer)
   - Per-request identity resolution (auth_token overrides connection-level)
   - `build_root_context` sets internal: false, deadline, capabilities from registration
   - `compose_root_env` builds CompositeOperationEnv (base + session + connection)
   - operationId leading slash stripped before lookup
   - ResponseEnvelope → EventEnvelope conversion
   - Timeout: 30s default, composed calls inherit parent deadline
   - Abort cascade: walks tree by parent_request_id, AbortDependents/ContinueRunning

3. **ADR conformance**:
   - ADR-005: irpc framing used
   - ADR-012: bidirectional streams, ID-based correlation
   - ADR-014: no secret material on wire, Capabilities non-serializable
   - ADR-015: internal flag switches authority (handler_identity vs identity), Visibility
   - ADR-016: abort cascade, AbortPolicy, default AbortDependents
   - ADR-017: connection direction independent of call direction
   - ADR-022: registration bundle (provenance, authority, scoped_env, capabilities)
   - ADR-023: ErrorDefinition, typed details in call.error
   - ADR-024: registry layering (curated + session + connection), OperationEnv as trait

4. **Security constraints**:
   - Capabilities non-serializable (no Serialize derive)
   - Capabilities zeroized, immutable after construction
   - Metadata does not propagate through composition (fresh HashMap::new())
   - Call protocol carries no secret material
   - Internal ops return NOT_FOUND from wire (don't leak existence)
   - Reachability check (scoped_env.allows) bounds composition attack surface
   - Request IDs are UUID v4 (non-deterministic, no collisions)

5. **Pattern consistency**:
   - OperationEnv is a trait (not concrete) — enables layering
   - CompositeOperationEnv uses contains() probe before dispatch
   - Authority switch in invoke_with_policy (child identity = parent handler_identity)
   - Deadline inheritance (children don't get fresh 30s)
   - ArcSwap not used in call (that's core's pattern)

6. **Test coverage**:
   - Unit tests for AccessControl::check (all combinations)
   - Unit tests for OperationContext construction
   - Unit tests for OperationEnv (LocalOperationEnv, CompositeOperationEnv)
   - Unit tests for PendingRequestMap (all event types, timeouts, fail_all)
   - Unit tests for framing (round-trip, truncation)
   - Unit tests for abort cascade (both policies, tree walking)
   - Integration test: call.requested → dispatch → call.responded
   - Integration test: auth_token overrides identity
   - Integration test: Internal op → NOT_FOUND from wire
   - Integration test: ACL denied → FORBIDDEN
   - Integration test: subscription streaming (multiple responded, completed)

## Acceptance Criteria

- [ ] All registry types match operation-registry.md
- [ ] All protocol types match call-protocol.md
- [ ] All ADRs conformed to (005, 012, 014, 015, 016, 017, 022, 023, 024)
- [ ] Capabilities non-serializable, zeroized, immutable
- [ ] Metadata does not propagate through composition
- [ ] Internal ops return NOT_FOUND from wire
- [ ] Reachability check bounds composition
- [ ] Request IDs are UUID v4
- [ ] OperationEnv is a trait (not concrete)
- [ ] CompositeOperationEnv uses contains() probe
- [ ] Authority switch correct (internal: true → handler_identity)
- [ ] Deadline inheritance correct (children inherit parent deadline)
- [ ] Test coverage adequate for all functionality
- [ ] `cargo fmt --check -p alknet-call` passes
- [ ] `cargo clippy -p alknet-call` passes with no warnings
- [ ] All tests pass

## References

- docs/architecture/crates/call/README.md
- docs/architecture/crates/call/call-protocol.md
- docs/architecture/crates/call/operation-registry.md
- docs/architecture/decisions/ (relevant ADRs: 005, 012, 014-017, 022-024)

## Notes

> This is the most complex crate in this batch. The review should verify that
> the registry layering (ADR-024), authority switch (ADR-015), abort cascade
> (ADR-016), and composition model (ADR-022) all work correctly together. The
> OperationEnv trait-object design is load-bearing — verify it's a trait, not
> concrete. If deviations are found, document and fix before considering the
> call crate complete.

## Summary

> To be filled on completion