# OQ-61: Multi-Owner Shutdown Coordination

- **Origin**: `docs/architecture/decisions/083-endpoint-as-accept-loop-runner.md`
  (the endpoint owns dispatched handlers; the assembly layer owns
  spawned accept loops — coordination between them on shutdown is
  unspecified).
- **Status**: open
- **Door type**: two-way (the coordination mechanism is an
  implementation detail; the ownership boundary — endpoint owns
  dispatched handlers, assembly layer owns spawned accept loops — is
  the one-way part, committed in ADR-083)
- **Priority**: medium (matters for clean hub shutdown; doesn't block
  the endpoint-shape decision or the TLS extraction)
- **Blocked on**: nothing structural. The question is which
  coordination primitive (shared `shutdown_sender`, separate channel,
  drain semantics) — a design choice, not a missing capability.
- **Resolution**: Not yet decided. The boundary is clear:

  - The endpoint owns shutdown of **dispatched handlers** (it spawned
    them via `tokio::spawn` in `dispatch`).
  - The assembly layer owns shutdown of the **accept loops it spawned**
    (the TCP+TLS listeners).

  The open question is the **coordination mechanism**:

  - Does the assembly layer use the endpoint's `shutdown_sender()` to
    signal the TCP+TLS loops, or a separate channel?
  - Does `endpoint.shutdown()` drain in-flight dispatched handlers
    (including those from TCP+TLS loops), or only the quinn/iroh ones
    it spawned via `run()`?
  - What happens to in-flight `dispatch` calls after the endpoint is
    shut down — are they rejected, or do they complete?

  A likely shape: the assembly layer uses the endpoint's
  `shutdown_sender()` for all its accept loops (one signal, all loops
  stop accepting); the endpoint's `shutdown()` drains all dispatched
  handlers regardless of which transport spawned them (the endpoint
  owns them all once `dispatch` is called). But this needs to be
  written down and verified against the drain semantics.
- **Cross-references**: ADR-083 (endpoint refactor — ownership
  boundary), ADR-010 (original shutdown design — single-owner, the
  simpler case being generalized)