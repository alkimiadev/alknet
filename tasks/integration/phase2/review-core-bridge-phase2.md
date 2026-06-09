---
id: review-core-bridge-phase2
name: Review all Phase 2 changes for spec conformance and prepare for Phase 3
status: completed
depends_on: [stream-interface-message-interface-split, ssh-session-call-protocol-bridge, raw-framing-interface-implementation, credential-provider-trait, api-keys-dynamic-config, listenconfig-http-dns-stubs, axum-http-router-scaffold]
scope: narrow
risk: low
impact: phase
level: review
---

## Description

Review all Phase 2 implementation for spec conformance, architectural consistency, and completeness before Phase 3 crate development begins. Per integration plan section 4.5, a second doc sync should capture any deviations between spec and implementation.

This review covers:
1. **Spec conformance**: Do implementations match the architecture docs and ADRs (035, 036, 037)?
2. **Layer boundary discipline**: Does every component belong to exactly one layer? No call protocol logic in the interface layer, no interface logic in the transport layer.
3. **Terminology consistency**: head/worker everywhere (no hub/spoke), StreamInterface/MessageInterface (no bare "Interface" trait), consistent naming.
4. **Test coverage**: Do all Phase 2 tasks have tests that verify acceptance criteria?
5. **No circular dependencies**: alknet-core doesn't depend on alknet-secret, alknet-storage, or alknet-flowgraph.
6. **Doc sync**: Update architecture docs to reflect Phase 2 implementation state. Specifically:
   - `interface.md` — StreamInterface/MessageInterface split, InterfaceRequest/InterfaceResponse
   - `auth.md` — API keys, resolve_from_token() changes
   - `configuration.md` — DynamicConfig additions (api_keys, credentials)
   - `call-protocol.md` — SshSession bridge, RawFraming auth flow
   - Any deviations between spec and implementation should be documented

## Acceptance Criteria

- [ ] All Phase 2 tasks have acceptance criteria verified (each task's AC checklist is complete)
- [ ] Layer boundaries are clean: interface layer produces/consumes `InterfaceEvent`; protocol layer handles `EventEnvelope`; transport layer provides byte streams
- [ ] No `Interface` trait references remain (all renamed to `StreamInterface`)
- [ ] No `TransportKind::Dns` in the enum (DNS is a `MessageInterface`)
- [ ] `Cargo.toml` dependency check: alknet-core has no circular deps on external crates
- [ ] `http` feature flag correctly gates axum dependency
- [ ] Architecture docs updated for Phase 2 state:
  - [ ] `interface.md` reflects StreamInterface/MessageInterface split
  - [ ] `auth.md` reflects API keys in DynamicConfig
  - [ ] `configuration.md` reflects new DynamicConfig sections
  - [ ] `call-protocol.md` reflects functional SshSession bridge
- [ ] All tests pass: `cargo test --all-features`
- [ ] No compiler warnings on Phase 2 code
- [ ] `taskgraph parallel --path tasks/integration/phase2` shows all tasks completed

## References

- docs/research/integration-plan.md — Phase 4.5 doc sync
- All Phase 2 ADRs: 035, 036, 037
- All Phase 2 implementation tasks (2.1–2.7)
- docs/architecture/ — architecture docs to update

## Notes

> This is a quality gate before Phase 3. The review should be thorough but shouldn't block on minor documentation phrasing. Focus on structural conformance: are layers respected, are traits correct, are dependencies acyclic?

> If any deviations between spec and implementation are found, document them in the relevant architecture doc with a "Deviation from spec" note explaining why.

## Summary

> Review complete. All 7 implementation tasks verified. 487 tests pass (all-features), clippy clean, fmt clean. No bare `Interface` trait references remain. No `TransportKind::Dns` in enum. No circular deps. `http` feature flag gates axum correctly. Layer boundaries clean. Architecture docs (interface.md, auth.md, configuration.md, call-protocol.md) updated with Phase 2 implementation notes.