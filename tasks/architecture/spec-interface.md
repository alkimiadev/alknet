---
id: architecture/spec-interface
name: Create interface.md architecture spec (Layer 2)
status: pending
depends_on:
  - architecture/adr-026-transport-interface-separation
  - architecture/adr-033-operationenv-irpc-call-protocol
scope: moderate
risk: high
impact: project
level: implementation
---

## Description

Create `docs/architecture/interface.md` — the new Layer 2 spec defining the Interface abstraction. This is the most architecturally significant new spec document.

Currently, SSH is deeply embedded in the server handler. The Interface trait extracts it into a pluggable Layer 2 component:

```rust
#[async_trait]
pub trait Interface: Send + Sync + 'static {
    type Session;
    async fn accept(stream: TransportStream, config: &InterfaceConfig) -> Result<Self::Session>;
    // The session produces call protocol events and handles responses
}
```

The spec must define:
1. **The `Interface` trait** — consumes a `Transport::Stream` and produces call protocol sessions
2. **`SshInterface`** — wraps existing russh handler, produces SSH channels + control channel
3. **`RawFramingInterface`** — reads length-prefixed JSON EventEnvelope frames directly, no SSH wrapping
4. **Valid (Transport, Interface) pairs** — enumerated per ADR-026
5. **How the call protocol is interface-agnostic** — it receives EventEnvelope frames from any interface

**The hard part**: The existing `ServerHandler` owns auth, channel management, and proxy logic. The spec must cleanly define what moves to Layer 3 (call protocol), what stays in the interface, and what's shared. The split needs to be clean — this is explicitly flagged as needing careful design review in the integration plan.

**Depends on ADR-033 because**: The Interface trait must produce call protocol events, and OperationEnv defines how those events are dispatched. The Interface needs to produce the right shape of events for the OperationEnv to consume.

## Acceptance Criteria

- [ ] `docs/architecture/interface.md` exists with YAML frontmatter (`status: draft`)
- [ ] Follows spec format: What, Why, Architecture, Constraints, Open Questions, Design Decisions
- [ ] Defines the three-layer model (reference ADR-026): Transport (Layer 1), Interface (Layer 2), Protocol (Layer 3)
- [ ] Defines `Interface` trait with signature, bounds, and lifecycle
- [ ] Defines `SshInterface` — what it wraps from existing ServerHandler (auth delegation, channel open, proxy)
- [ ] Defines `RawFramingInterface` — 4-byte BE length prefix + JSON EventEnvelope, no SSH
- [ ] Enumerates valid (Transport, Interface) pairs per ADR-026 table
- [ ] Defines what `InterfaceConfig` contains (different per interface type)
- [ ] Clearly separates what moves to call protocol (Layer 3) vs what stays in the interface (Layer 2)
- [ ] Shows how `auth_publickey()` maps to SshInterface (not RawFramingInterface which uses token auth)
- [ ] Shows how `channel_open_direct_tcpip()` proxy logic relates to Layer 3 forwarding policy
- [ ] DNS control channel explicitly defined as (DNS transport, raw framing interface) — NOT SSH inside DNS
- [ ] Call protocol is interface-agnostic: receives EventEnvelope from any interface
- [ ] References ADR-026, ADR-033
- [ ] `docs/architecture/README.md` updated to include interface.md
- [ ] Flags any open design questions (e.g., exactly how auth maps across interfaces)

## References

- docs/research/integration-plan.md — Phase 1.8, three-layer model, valid (Transport, Interface) pairs
- docs/research/core.md — DNS transport, interface concept
- docs/architecture/decisions/026-transport-interface-separation.md
- docs/architecture/decisions/033-operationenv-irpc-call-protocol.md
- docs/architecture/server.md — current ServerHandler (source of SshInterface logic)

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion