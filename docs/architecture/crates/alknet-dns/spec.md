---
status: planned
last_updated: 2026-06-15
---

# alknet-dns

> **Status: Planned** — This spec has not been written yet.

## Purpose

DNS handler implementing `ProtocolHandler` on ALPN `alknet/dns`. Uses hickory-proto (`#![no_std]`, WASM-compatible) for DNS wire format and pkarr for self-sovereign DNS. Provides service discovery, control channel via AuthToken in query labels, and encrypted DNS transports (DoT, DoQ, DoH3).

## References

- [overview.md](../../overview.md)
- ADR-002: ProtocolHandler trait