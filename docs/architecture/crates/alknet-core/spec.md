---
status: planned
last_updated: 2026-06-15
---

# alknet-core

> **Status: Planned** — This spec has not been written yet. It will be produced as part of Phase 1 architecture work.

## Purpose

Core crate providing the `ProtocolHandler` trait, ALPN router, endpoint, `BiStream`, `AuthContext`, `IdentityProvider`, configuration types, and shared infrastructure used by all handler crates.

## Key Questions

- **OQ-01**: BiStream type definition — trait vs concrete type vs newtype
- **OQ-05**: Multi-transport endpoint — TCP, TLS, iroh support scope

## References

- [overview.md](../../overview.md)
- ADR-001: ALPN-based protocol dispatch
- ADR-002: ProtocolHandler trait
- ADR-003: Crate decomposition
- ADR-004: Auth as shared core
- ADR-006: ALPN string convention and connection model