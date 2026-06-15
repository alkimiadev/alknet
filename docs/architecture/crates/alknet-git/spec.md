---
status: planned
last_updated: 2026-06-15
---

# alknet-git

> **Status: Planned** — This spec has not been written yet.

## Purpose

Git smart protocol handler implementing `ProtocolHandler` on ALPN `alknet/git`. Uses gix (Apache-2.0/MIT) for pack generation, ref resolution, and object store. Custom pkt-line protocol adapter for QUIC streams. No HTTP layer — git protocol directly over QUIC.

## Key Questions

- **OQ-10**: Git adapter scope — smart protocol only or full server?

## References

- [overview.md](../../overview.md)
- ADR-002: ProtocolHandler trait