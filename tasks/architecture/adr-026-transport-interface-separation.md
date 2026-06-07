---
id: architecture/adr-026-transport-interface-separation
name: Write ADR-026 — Transport/interface separation (three-layer model)
status: pending
depends_on: []
scope: moderate
risk: high
impact: project
level: implementation
---

## Description

Write ADR-026 establishing the three-layer model: Transport (Layer 1), Interface (Layer 2), Protocol (Layer 3). This is the most architecturally significant new ADR — it redefines SSH as an interface (not a transport) and enables the DNS control channel, raw framing, and future WebTransport as (Transport, Interface) pairs.

The three layers:
- **Layer 1: Transport** — produces byte streams. TCP, TLS, iroh, DNS (as byte carrier), WebTransport. A `Transport` still produces `AsyncRead + AsyncWrite + Unpin + Send`.
- **Layer 2: Interface** — consumes a `Transport::Stream` and produces call protocol events (sessions). SSH is an interface. Raw framing (4-byte length prefix + JSON EventEnvelope) is an interface. DNS control channel is a (DNS transport, raw framing interface) pair.
- **Layer 3: Protocol** — carries semantics. Call protocol events, operation registry, service calls. Protocol is agnostic to both Transport and Interface below it.

A **connection** is always a (Transport, Interface) pair. The valid combinations are enumerated:
- (TLS, SSH) — standard alknet tunnel
- (TCP, SSH) — plain SSH tunnel
- (iroh, SSH) — P2P SSH tunnel
- (DNS, raw framing) — DNS control channel
- (WebTransport, SSH) — browser SSH tunnel (future)
- (WebTransport, raw framing) — browser call protocol (future)
- (TCP, raw framing) — direct call protocol, local mesh

Key changes from current architecture:
- SSH is an interface, not a transport. Currently deeply embedded in ServerHandler.
- The `TransportKind` enum gains `Dns` and `WebTransport` variants (initially tags only).
- Raw framing (4-byte BE length prefix + JSON) is an interface without SSH wrapping.
- DNS control channel carries call protocol frames directly — it does NOT wrap SSH inside DNS.

This ADR requires careful review because it's the foundation for Phase 1.8 (Interface Abstraction), which is the most invasive code change.

## Acceptance Criteria

- [ ] `docs/architecture/decisions/026-transport-interface-separation.md` exists
- [ ] ADR follows established format
- [ ] Context explains why SSH is currently tangled with transport and why separating them matters (enables DNS, raw framing, WebTransport without SSH)
- [ ] Decision states: three layers; SSH is Layer 2 not Layer 1; Transport trait produces byte streams unchanged; Interface trait consumes Transport::Stream and produces call protocol sessions; connection = (Transport, Interface) pair; valid pairs enumerated
- [ ] Shows the Interface trait signature (consume stream, produce sessions)
- [ ] Lists the valid (Transport, Interface) combinations
- [ ] Consequences: enables DNS control channel without SSH wrapping; enables raw framing for service mesh; SSH becomes pluggable; ServerHandler is refactored into SshInterface
- [ ] DNS control channel carries call protocol directly (NOT SSH inside DNS) — explicitly stated
- [ ] References: research/core.md DNS section, integration-plan.md Phase 1.8

## References

- docs/research/core.md — transport layer, DNS transport section
- docs/research/integration-plan.md — Phase 1.8, three-layer model, DNS as (DNS transport, raw framing interface)
- docs/architecture/transport.md — current Transport trait (unchanged at Layer 1)
- docs/architecture/server.md — current ServerHandler (will become SshInterface)

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion