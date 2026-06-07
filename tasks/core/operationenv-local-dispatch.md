---
id: core/operationenv-local-dispatch
name: Implement OperationEnv local dispatch and event envelope framing
status: completed
depends_on:
  - core/operation-context-registry
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Implement Phase 1 OperationEnv functionality: local dispatch through the operation registry, and the `EventEnvelope` wire format for the call protocol.

OperationEnv is the universal composition mechanism (ADR-033). Phase 1 ships with local dispatch only — `OperationEnv::local(registry)` creates an environment where `env.invoke(namespace, op, input)` directly calls the registered handler function.

**Key additions**:
- `OperationEnv` struct with `local()` constructor and `invoke()` method
- `EventEnvelope` struct: type (event type string), id (correlation key), payload ( serde_json::Value)
- Frame encoding: 4-byte big-endian length prefix + UTF-8 JSON body
- `PendingRequestMap` for call/subscribe correlation
- Call protocol event types: `call.requested`, `call.responded`, `call.completed`, `call.aborted`, `call.error`
- Service discovery operations: `/services/list`, `/services/schema` registered by default

**Local dispatch only**: In Phase 1, `OperationEnv::local()` creates an environment where all operations resolve to local function calls. The `service()` and `remote()` dispatch paths are stubbed out with `unimplemented!()` or returning an error, to be filled in Phase 2+.

## Acceptance Criteria

- [ ] `OperationEnv::local(registry)` creates an environment with local dispatch
- [ ] `OperationEnv::invoke(namespace, op, input)` resolves to the local handler and returns `ResponseEnvelope`
- [ ] `EventEnvelope` struct defined with `type`, `id`, `payload` fields per call-protocol.md
- [ ] Frame encoding/decoding: 4-byte BE length prefix + JSON body
- [ ] `PendingRequestMap` with call/subscribe entry types
- [ ] Call protocol event type constants defined
- [ ] Default operations registered: `/services/list`, `/services/schema`
- [ ] Unit tests: local invoke → correct ResponseEnvelope
- [ ] Unit tests: frame encoding round-trip
- [ ] Unit tests: EventEnvelope serialization/deserialization

## References

- docs/architecture/call-protocol.md — EventEnvelope, PendingRequestMap, service discovery
- docs/architecture/decisions/033-operationenv-irpc-call-protocol.md — OperationEnv composition model, local dispatch first
- docs/architecture/services.md — OperationEnv deployment topologies

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion