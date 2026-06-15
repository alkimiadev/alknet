---
status: planned
last_updated: 2026-06-15
---

# alknet-http

> **Status: Planned** — This spec has not been written yet.

## Purpose

HTTP handler implementing `ProtocolHandler` on ALPN `alknet/http`. Provides axum router with auth middleware, REST API, dashboard, and MCP endpoint. Also handles standard HTTP ALPNs (`h2`, `http/1.1`) and WebTransport upgrade on `h3`.

## Key Questions

- How does HttpAdapter handle both `alknet/http` and standard ALPNs (`h2`, `http/1.1`, `h3`)?
- WebTransport upgrade on `h3` — is this a separate handler or integrated into HttpAdapter?

## References

- [overview.md](../../overview.md)
- ADR-002: ProtocolHandler trait
- ADR-006: ALPN string convention and connection model