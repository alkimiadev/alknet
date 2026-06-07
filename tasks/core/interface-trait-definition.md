---
id: core/interface-trait-definition
name: Define Interface trait and InterfaceConfig types
status: completed
depends_on:
  - core/multi-transport-listeners
  - core/operationenv-local-dispatch
scope: narrow
risk: high
impact: project
level: implementation
---

## Description

Define the `Interface` trait and `InterfaceConfig` types that form Layer 2 of the three-layer model (ADR-026, interface.md). This is the type definition and trait design task — NOT the `SshInterface` extraction (that's the next task).

The `Interface` trait is the most architecturally significant new abstraction. It consumes a `Transport::Stream` and produces call protocol sessions. Currently, SSH is deeply embedded in `ServerHandler`. This task defines the trait and config types; the next task (ssh-interface-extraction) does the invasive refactoring.

**Key additions**:
- `Interface` trait: `accept(stream: TransportStream, config: &InterfaceConfig) -> Result<Self::Session>`
- `InterfaceConfig` enum: `Ssh(SshInterfaceConfig)`, `RawFraming(RawFramingConfig)`
- `SshInterfaceConfig`: `auth: Arc<dyn IdentityProvider>`, `forwarding: Arc<ArcSwap<DynamicConfig>>`, `host_key: Arc<PrivateKey>`
- `RawFramingConfig`: minimal (no SSH-specific config; auth via transport or call protocol)
- `InterfaceSession` trait or enum: what the produced session looks like — this is the key design question (OQ-IF-01)
- Valid `(Transport, Interface)` pair enumeration

**The key design decision**: How does the Interface session type relate to the call protocol's `EventEnvelope` stream? Per interface.md OQ-IF-01, every session should produce `EventEnvelope` frames, but SSH sessions have channels and auth, while raw framing sessions are just a byte stream with framing. This task must resolve this question concretely.

## Acceptance Criteria

- [ ] `Interface` trait defined in `crates/alknet-core/src/interface/mod.rs` with `accept()` and associated `Session` type
- [ ] `InterfaceConfig` enum defined with `Ssh` and `RawFraming` variants
- [ ] `SshInterfaceConfig` defined with `auth`, `forwarding`, `host_key` fields
- [ ] `RawFramingConfig` defined (minimal)
- [ ] Valid `(Transport, Interface)` pair enumeration defined (e.g., as a const or validation function)
- [ ] The session type question (OQ-IF-01) is resolved: documented decision on how sessions produce EventEnvelope frames
- [ ] Module re-exported from `crates/alknet-core/src/lib.rs`
- [ ] Decision documented in task notes for the next task (ssh-interface-extraction)

## References

- docs/architecture/interface.md — Interface trait, SshInterface, RawFramingInterface, valid pairs
- docs/architecture/decisions/026-transport-interface-separation.md — ADR-026
- docs/architecture/call-protocol.md — EventEnvelope, call protocol events
- docs/architecture/decisions/033-operationenv-irpc-call-protocol.md — Protocol is interface-agnostic

## Notes

> OQ-IF-01 MUST be resolved before or during this task: how does the Interface session type relate to the call protocol's EventEnvelope stream?

## Summary

> To be filled on completion