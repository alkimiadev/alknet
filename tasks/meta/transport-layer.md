---
id: meta/transport-layer
name: Complete transport layer — trait, TCP, TLS, iroh, ACME
status: completed
depends_on:
  - transport/trait-and-types
  - transport/tcp-transport
  - transport/tls-transport
  - transport/iroh-transport
  - transport/acme-cert-provisioning
scope: system
risk: high
impact: phase
level: planning
---

## Description

Meta task that clusters all transport module tasks. Once complete, the transport layer provides a clean `Transport`/`TransportAcceptor` abstraction with TCP, TLS (feature-gated), iroh (feature-gated), and ACME (feature-gated) implementations. All transports produce the `AsyncRead + AsyncWrite + Unpin + Send` streams that SSH consumes.

## Acceptance Criteria

- [ ] All transport tasks completed
- [ ] `Transport` trait produces duplex streams consumed by `russh::connect_stream()` / `russh::run_stream()`
- [ ] TCP, TLS, iroh transports all work end-to-end
- [ ] ACME cert provisioning integrates with TLS acceptor
- [ ] Feature flags correctly gate optional transports

## References

- docs/architecture/transport.md

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion