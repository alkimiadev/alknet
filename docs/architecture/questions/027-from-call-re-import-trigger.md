# OQ-27: from_call Re-Import Trigger

- **Origin**: [client-and-adapters.md](crates/call/client-and-adapters.md), ADR-017 Assumption 4
- **Status**: **resolved** (2026-06-27)
- **Door type**: Two-way
- **Priority**: low
- **Resolution**: The decision is **auto-re-import on connection
  establishment**. The overlay is per-connection (Layer 2, ADR-024), so a
  stale overlay dies with the connection; re-import on reconnect is
  naturally scoped to the new connection. This is the right default for the
  runner pattern (a worker reconnects → the hub re-discovers the worker's
  ops automatically). An explicit `CallConnection::refresh()` method is a
  genuine feature addition — non-breaking, additive — if a deployment
  needs manual control.
- **Cross-references**: ADR-017, ADR-024, [client-and-adapters.md](crates/call/client-and-adapters.md)
