---
status: planned
last_updated: 2026-06-15
---

# alknet-napi

> **Status: Planned** — This spec has not been written yet.

## Purpose

Node.js native addon providing a call protocol client. Uses napi-rs for FFI. Depends only on alknet-call (not alknet-core) to keep the dependency tree minimal. Exposes connect/disconnect, call operations, and event subscriptions to JavaScript/TypeScript.

## References

- [overview.md](../../overview.md)
- ADR-003: Crate decomposition
- ADR-005: irpc as call protocol foundation