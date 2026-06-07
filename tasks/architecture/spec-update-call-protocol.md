---
id: architecture/spec-update-call-protocol
name: Update call-protocol.md — add OperationEnv dispatch paths, irpc as backend
status: pending
depends_on:
  - architecture/adr-033-operationenv-irpc-call-protocol
  - architecture/spec-services
  - architecture/spec-interface
scope: moderate
risk: medium
impact: phase
level: implementation
---

## Description

Update `docs/architecture/call-protocol.md` to add OperationEnv as the universal composition mechanism with three dispatch paths, and show how irpc is one backend for OperationEnv (not a replacement).

The current call-protocol.md already covers: operation paths, EventEnvelope wire format, call protocol events, bidirectional routing, head/worker architecture, operation registry, ACL, service discovery, PendingRequestMap, and protocol adapter layer. It was already updated for head/worker terminology.

**What's missing** (per integration plan):
1. **OperationEnv as universal composition mechanism** — the handler-facing API that unifies local calls, irpc service calls, and remote call protocol calls
2. **Three dispatch paths** — local dispatch, irpc service dispatch, remote dispatch, all producing the same ResponseEnvelope
3. **OperationContext** — request_id, parent_request_id, identity, metadata, env, trusted
4. **How irpc is one backend** — not a replacement for the call protocol or for OperationEnv
5. **How a call protocol handler can call an irpc service internally** (e.g., /head/auth/verify calls AuthProtocol::VerifyPubkey) — note: this is Phase 2+ composition; Phase 1 uses IdentityProvider directly

**Phase boundary note**: Phase 1 ships with local dispatch only (direct function calls through the operation registry). The irpc service dispatch and remote dispatch paths are contracted here but not built yet. OperationEnv should be documented with all three paths, but the spec should make it clear that Phase 1 is local-only. The agent service pattern example (`/head/agent/chat`) is a downstream application concern, not a core requirement.

**What stays the same**: Operation paths, EventEnvelope, call protocol events, bidirectional routing, head/worker architecture, PendingRequestMap, protocol adapter layer.

**Note**: Hub/spoke was already updated to head/worker. ADR references are partial (currently references 018, 024, 025).

## Acceptance Criteria

- [ ] OperationEnv section added, documenting it as universal composition mechanism per ADR-033
- [ ] Three dispatch paths documented with examples: local (zero serialization), irpc service (postcard), remote (JSON EventEnvelope)
- [ ] OperationContext struct documented (request_id, parent_request_id, identity, metadata, env, trusted)
- [ ] ResponseEnvelope as universal return type documented
- [ ] Operation handlers receive `(input: Value, context: OperationContext) -> ResponseEnvelope`
- [ ] irpc explicitly positioned as one dispatch backend for OperationEnv per ADR-033
- [ ] Phase boundary clear: Phase 1 is local dispatch only; irpc and remote dispatch paths are contracted but not built yet
- [ ] Agent service pattern is noted as a downstream application concern, not a core requirement
- [ ] Hard constraint stated: OperationEnv composition model matches @alkdev/operations behavioral contract
- [ ] ADR table updated with references to 028, 033
- [ ] Reference to services.md for full OperationEnv and irpc service details
- [ ] Reference to interface.md for how EventEngrams flow from interfaces
- [ ] `last_updated` in YAML frontmatter updated
- [ ] No hub/spoke terminology in any new content

## References

- docs/architecture/call-protocol.md — current content to update
- docs/research/integration-plan.md — call-protocol.md update entry
- docs/architecture/decisions/033-operationenv-irpc-call-protocol.md
- @alkdev/operations — TypeScript OperationEnv

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion