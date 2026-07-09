# OQ-27: from_call Re-Import Trigger

- **Origin**: [client-and-adapters.md](crates/call/client-and-adapters.md), ADR-017 Assumption 4
- **Status**: **resolved** (2026-07-09 — amended from 2026-06-27 resolution)
- **Door type**: Two-way
- **Priority**: low
- **Resolution**: `from_call` is a **manual free function**; the assembly layer
  calls it after `connect()`. The overlay is per-connection (Layer 2, ADR-024),
  so a stale overlay dies with the connection; re-import on reconnect is
  naturally scoped to the new connection. A `CallConnection::refresh()` method
  for mid-connection re-discovery is a genuine feature addition —
  non-breaking, additive — if a deployment needs manual re-discovery without
  drop-and-reconnect. See [ADR-069](../decisions/069-from-call-manual-free-function.md)
  for the full rationale.

  The original 2026-06-27 resolution ("auto-re-import on connection
  establishment") was aspirational — written before the implementation
  existed. The implementation made the right call (manual free function);
  this amendment aligns the spec with the implementation.
- **Cross-references**: ADR-017, ADR-024, [client-and-adapters.md](crates/call/client-and-adapters.md)
