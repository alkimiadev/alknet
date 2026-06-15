---
status: planned
last_updated: 2026-06-15
---

# alknet-sftp

> **Status: Planned** — This spec has not been written yet.

## Purpose

SFTP handler implementing `ProtocolHandler` on ALPN `alknet/sftp`. Provides russh-sftp protocol core with 26 packet types, custom serde codec, and pure data transformation. WASM-ready: only `read_packet()` couples to I/O.

## References

- [overview.md](../../overview.md)
- ADR-002: ProtocolHandler trait
- russh-sftp reference: `docs/research/references/ssh/russh-sftp/`