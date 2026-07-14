# OQ-61: Multi-Owner Shutdown Coordination

- **Origin**: `docs/architecture/decisions/083-endpoint-as-accept-loop-runner.md`
  (the endpoint owns dispatched handlers; the assembly layer owns
  spawned accept loops — coordination between them on shutdown was
  unspecified).
- **Status**: dissolved
- **Door type**: two-way (the coordination mechanism was an
  implementation detail)
- **Priority**: medium
- **Resolution**: Dissolved by ADR-083 revision (2026-07-14). The
  problem this OQ tracked — coordinating shutdown between the endpoint's
  owned loops and external TCP+TLS accept loops — does not arise. The
  TCP+TLS accept loop is now an owned transport (via `with_tcp_tls`),
  not an external sibling. The endpoint owns all its accept loops
  (quinn, iroh, TCP+TLS); `shutdown()` stops them all and drains all
  dispatched handlers. One owner, one shutdown. The `dispatch` method
  stays public for SSH channels and future WebTransport streams, but
  those are connection-internal multiplexing callers, not listener
  loops with independent shutdown needs — they call `dispatch` on
  connections the endpoint already accepted and dispatched a handler
  for.

  The premise of this OQ (external TCP+TLS loop + endpoint-owned
  dispatch = multi-owner shutdown) was a consequence of TCP+TLS being
  external. ADR-083's revision made TCP+TLS internal, retiring the
  premise.
- **Cross-references**: ADR-083 (endpoint refactor — TCP+TLS is now
  owned), OQ-60 (resolved — the TCP+TLS loop lives in core)