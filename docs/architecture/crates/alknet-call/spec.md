---
status: planned
last_updated: 2026-06-15
---

# alknet-call

> **Status: Planned** — This spec has not been written yet.

## Purpose

Call protocol handler implementing `ProtocolHandler` on ALPN `alknet/call`. Provides JSON-RPC via irpc with operation registry, streaming subscriptions, pub/sub, and access control.

## Key Questions

- **OQ-07**: Call protocol scope within a connection — one stream per operation vs multiplexed

## References

- [overview.md](../../overview.md)
- ADR-005: irpc as call protocol foundation