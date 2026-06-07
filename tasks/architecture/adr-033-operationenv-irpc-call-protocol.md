---
id: architecture/adr-033-operationenv-irpc-call-protocol
name: Write ADR-033 — OperationEnv, irpc, and call protocol relationship
status: pending
depends_on:
  - architecture/adr-028-auth-irpc-service
  - architecture/adr-027-crate-decomposition
scope: moderate
risk: high
impact: project
level: implementation
---

## Description

Write ADR-033 establishing OperationEnv as the universal composition mechanism that unifies irpc services and call protocol operations from the handler's perspective.

This is the most conceptually complex ADR. It must clearly establish:

1. **OperationEnv is not an implementation detail from @alkdev/operations** — it's the universal composition mechanism. Handlers compose through `context.env[namespace][op](input)` regardless of dispatch path.

2. **Three dispatch paths**, all producing the same result:
   - **Local dispatch**: Direct function call through the operation registry. Zero serialization.
   - **Service dispatch (irpc)**: In-cluster, Rust-to-Rust. Postcard serialization over tokio channels (local) or QUIC streams (remote). Domain-level.
   - **Remote dispatch (call protocol)**: Cross-node, cross-language. JSON EventEnvelope over any (Transport, Interface) pair. Integration-level.

3. **irpc is one dispatch backend for OperationEnv**, not a replacement for it. The call protocol is another dispatch backend. Both are Layer 3, at different scope boundaries.

4. **The behavioral contract**: namespace + operation name → invoke with input, return output. The Rust implementation can use typed method dispatch or a registry internally, but the handler-facing API must preserve this contract.

5. **An irpc service CAN be exposed as a call protocol operation** — the registry maps the path to a handler that internally calls the irpc service.

This ADR resolves the potential confusion where "service" could mean irpc service protocol, call protocol operation, or external service. The consistent naming is: irpc service (in-cluster, Rust-to-Rust, postcard), operation (path-based, call protocol, cross-node, JSON), external service (any reachable endpoint). OperationEnv unifies them.

## Acceptance Criteria

- [ ] `docs/architecture/decisions/033-operationenv-irpc-call-protocol.md` exists
- [ ] ADR follows established format
- [ ] Context explains the confusion: irpc service vs call protocol operation vs external service are different dispatch mechanisms that need unification
- [ ] Decision states: OperationEnv is the universal composition mechanism; three dispatch paths (local, irpc, remote) all produce ResponseEnvelope; handlers don't know the dispatch path; irpc is one backend, not a replacement; an irpc service can back a call protocol operation
- [ ] Shows the dispatch diagram: OperationEnv → local | irpc service | remote call protocol
- [ ] Shows the composition layers: Call Protocol (Layer 3, external, JSON) → irpc Service (Layer 3, internal, postcard) → Honker Streams (domain events)
- [ ] Shows OperationEnv wiring example for minimal and production deployments
- [ ] Defines the consistent naming: irpc service / operation / external service
- [ ] Consequences: handlers compose through a single interface regardless of deployment; adapters (MCP, HTTP, DNS) map to operations through this interface; irpc gives type-safe efficient in-cluster calls; call protocol gives universal cross-language cross-node calls
- [ ] Hard constraint explicitly stated: the OperationEnv composition model must match the behavioral contract from @alkdev/operations
- [ ] References: research/services.md, @alkdev/operations, integration-plan.md

## References

- docs/research/services.md — OperationContext, OperationEnv, irpc service layer
- docs/research/integration-plan.md — ADR 033 entry, the three-layer clarification, OperationEnv section
- docs/architecture/call-protocol.md — OperationSpec, OperationRegistry, call protocol events
- @alkdev/operations — TypeScript OperationEnv implementation

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion