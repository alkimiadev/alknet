---
status: planned
last_updated: 2026-06-15
---

# alknet-msg

> **Status: Planned** — This spec has not been written yet.

## Purpose

Messaging handler implementing `ProtocolHandler` on ALPN `alknet/msg`. Provides E2E encrypted direct messages (encrypt with recipient's public key) and mixnet support (Chaum 1981: nested encryption, batch-and-reorder, return addresses as digital pseudonyms).

## References

- [overview.md](../../overview.md)
- ADR-002: ProtocolHandler trait