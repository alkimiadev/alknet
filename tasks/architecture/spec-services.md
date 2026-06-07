---
id: architecture/spec-services
name: Create services.md architecture spec (irpc service layer + OperationEnv)
status: completed
depends_on:
  - architecture/adr-033-operationenv-irpc-call-protocol
  - architecture/adr-027-crate-decomposition
  - architecture/adr-028-auth-irpc-service
  - architecture/adr-032-event-boundary-discipline
scope: broad
risk: high
impact: project
level: implementation
---

## Description

Create `docs/architecture/services.md` — the irpc service layer spec. This integrates three things that the research treated separately:

1. **irpc service protocols** — AuthProtocol, SecretProtocol, ConfigProtocol, StorageProtocol — their enum definitions, wire formats, and backends
2. **OperationEnv** — the universal composition mechanism with three dispatch paths (local, irpc, remote)
3. **OperationContext** — the request context that handlers receive

This is the second most complex new spec (after interface.md). The integration plan spends the most words on this topic because it's where the most confusion existed between irpc services, call protocol operations, and external services.

The spec must make it crystal clear:
- irpc services are in-cluster, Rust-to-Rust, postcard serialization
- Call protocol operations are cross-node, cross-language, JSON EventEnvelope
- OperationEnv unifies them from the handler's perspective
- An irpc service can back a call protocol operation via OperationEnv
- Both are Layer 3 but at different scope boundaries

**Source**: `docs/research/services.md` (808 lines) + integration plan's OperationEnv and dispatch path sections + ADR-033

## Acceptance Criteria

- [ ] `docs/architecture/services.md` exists with YAML frontmatter (`status: draft`)
- [ ] Follows spec format: What, Why, Architecture, Constraints, Open Questions, Design Decisions
- [ ] Documents irpc service pattern: `#[rpc_requests]` enum, Serializable vs WithChannels, `Client<S>`
- [ ] Documents all four service protocols: AuthProtocol, SecretProtocol, ConfigProtocol, StorageProtocol (type signatures, not full implementations — those go in per-crate specs)
- [ ] Documents OperationContext struct: request_id, parent_request_id, identity, metadata, env, trusted
- [ ] Documents OperationEnv as universal composition mechanism per ADR-033
- [ ] Shows three dispatch paths with examples: local (direct call), irpc service (postcard over mpsc/QUIC), remote (call protocol EventEnvelope)
- [ ] Shows OperationEnv wiring for minimal and production deployments
- [ ] Shows how adapters (MCP, OpenAPI, HTTP, DNS) map to OperationEnv
- [ ] Consistent naming: irpc service / operation / external service (per ADR-033)
- [ ] Composition diagram: Call Protocol → irpc Service → Honker Streams (per ADR-032)
- [ ] Hard constraint stated: handler-facing OperationEnv API matches @alkdev/operations behavioral contract
- [ ] Event boundary per ADR-032: domain events never cross boundaries without projection
- [ ] References ADR-027, ADR-028, ADR-032, ADR-033
- [ ] `docs/architecture/README.md` updated to include services.md

## References

- docs/research/services.md — full service protocol definitions, OperationContext, OperationEnv
- docs/research/integration-plan.md — OperationEnv section, three dispatch paths, adapter patterns
- docs/architecture/decisions/033-operationenv-irpc-call-protocol.md
- docs/architecture/decisions/027-crate-decomposition.md
- docs/architecture/decisions/028-auth-irpc-service.md
- docs/architecture/decisions/032-event-boundary-discipline.md
- @alkdev/operations — TypeScript OperationEnv implementation

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion