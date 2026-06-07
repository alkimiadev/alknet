---
id: core/operation-context-registry
name: Implement OperationContext, OperationRegistry, and OperationSpec
status: pending
depends_on:
  - core/identity-type-provider
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Implement the call protocol's core types: `OperationContext`, `OperationSpec`, `OperationRegistry`, `ResponseEnvelope`, and `CallError` in `alknet_core::call`, per call-protocol.md and ADR-033.

This is the foundation for the OperationEnv composition mechanism. Phase 1 ships with local dispatch only — the registry maps operation paths to handler functions. irpc and remote dispatch are contracted in the spec but not built yet.

**Key additions**:
- `OperationSpec` struct: name, namespace, op_type (Query/Mutation/Subscription), input_schema, output_schema, access_control
- `OperationType` enum: Query, Mutation, Subscription
- `AccessControl` struct: required_scopes, required_scopes_any, resource_type, resource_action
- `OperationContext` struct: request_id, parent_request_id, identity, metadata, env (OperationEnv), trusted
- `OperationRegistry` struct: maps paths to `(OperationSpec, handler)` pairs, supports registration and lookup
- `ResponseEnvelope` struct: request_id, result (Result<Value, CallError>)
- `CallError` struct: code, message, retryable
- Handler signature: `fn(input: Value, context: OperationContext) -> ResponseEnvelope` (or async)

**Important**: `OperationEnv` is defined here but only with local dispatch in Phase 1. The `env` field on `OperationContext` allows handlers to compose operations by calling `context.env.invoke(namespace, op, input)`. In Phase 1, this always resolves to a direct function call through the registry.

## Acceptance Criteria

- [ ] `OperationSpec`, `OperationType`, `AccessControl` defined in `crates/alknet-core/src/call/spec.rs`
- [ ] `OperationContext` defined in `crates/alknet-core/src/call/context.rs` with all fields per call-protocol.md
- [ ] `ResponseEnvelope` and `CallError` defined in `crates/alknet-core/src/call/response.rs`
- [ ] `OperationRegistry` defined in `crates/alknet-core/src/call/registry.rs` with `register()`, `lookup()`, `invoke()` methods
- [ ] `OperationEnv` defined with `local()` constructor and `invoke(namespace, op, input)` method for local dispatch only
- [ ] Handler signature is clear (sync or async, Value in, ResponseEnvelope out)
- [ ] Namespace-based routing: `/head/auth/verify` → namespace "auth", op "verify"
- [ ] Unit tests: register and invoke an operation, verify ResponseEnvelope result
- [ ] Unit tests: AccessControl checks against Identity.scopes and Identity.resources
- [ ] Module re-exported from `crates/alknet-core/src/lib.rs`

## References

- docs/architecture/call-protocol.md — OperationSpec, OperationRegistry, OperationContext, ResponseEnvelope
- docs/architecture/decisions/033-operationenv-irpc-call-protocol.md — ADR-033, OperationEnv composition model
- docs/architecture/services.md — OperationContext fields

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion