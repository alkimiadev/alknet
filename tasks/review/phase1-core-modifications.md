---
id: review/phase1-core-modifications
name: Review Phase 1 core modifications — config split, identity, forwarding, OperationEnv, interface abstraction
status: pending
depends_on:
  - core/ssh-interface-extraction
  - core/operationenv-local-dispatch
  - core/auth-service-irpc
  - core/config-service-irpc
  - core/napi-reload-api
scope: broad
risk: medium
impact: project
level: review
---

## Description

Review the Phase 1 core modifications after all implementation tasks are complete. This is a quality checkpoint before Phase 2 (external crates) and Phase 3 (integration/wiring).

**Review checklist**:
1. All ADRs (026-034) are correctly reflected in implementation
2. Crate dependencies are acyclic — core doesn't depend on secret, storage, or flowgraph
3. Terminology is consistent — head/worker everywhere, no hub/spoke remaining
4. Layer boundaries are clean — Interface produces call protocol events, Protocol is agnostic
5. `IdentityProvider` trait is the sole contract for auth — no direct `ServerAuthConfig` usage remains
6. `DynamicConfig` + ArcSwap provides hot-reload for auth and forwarding
7. `ForwardingPolicy` default-allow preserves current behavior
8. OperationEnv local dispatch works correctly through the registry
9. Feature flags (`irpc`) compile correctly — core without feature flag has no irpc dependency
10. All existing tests pass
11. New test coverage for config reload, identity resolution, forwarding policy
12. NAPI reload API functions correctly
13. Interface trait and SshInterface extraction don't break SSH tunnel functionality

## Acceptance Criteria

- [ ] Code adheres to architecture specs (configuration.md, identity.md, interface.md, call-protocol.md, services.md)
- [ ] Patterns are consistent (IdentityProvider, DynamicConfig/ArcSwap, OperationEnv, Interface trait)
- [ ] Tests cover core functionality: config hot-reload, identity resolution, forwarding policy evaluation, local dispatch
- [ ] No cargo build errors or warnings
- [ ] All feature flag combinations compile: default, irpc, tls, iroh, acme
- [ ] Documentation comments reference ADR numbers
- [ ] Phase 1 implementation notes are filled in on all task files

## References

- docs/architecture/overview.md
- docs/architecture/configuration.md
- docs/architecture/identity.md
- docs/architecture/interface.md
- docs/architecture/call-protocol.md
- docs/architecture/services.md
- docs/architecture/decisions/ (ADR-026 through ADR-034)

## Notes

> To be filled by review agent

## Summary

> To be filled on completion