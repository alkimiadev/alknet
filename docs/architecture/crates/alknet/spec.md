---
status: planned
last_updated: 2026-06-15
---

# alknet (CLI)

> **Status: Planned** — This spec has not been written yet.

## Purpose

CLI binary that assembles all handler crates and starts the alknet endpoint. Registers ProtocolHandler implementations with the ALPN router based on configuration. The only crate that depends on all handler crates.

## References

- [overview.md](../../overview.md)
- ADR-003: Crate decomposition