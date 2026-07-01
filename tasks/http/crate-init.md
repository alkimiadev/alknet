---
id: http/crate-init
name: Initialize alknet-http crate with Cargo.toml, dependencies, and module skeleton
status: pending
depends_on: []
scope: moderate
risk: low
impact: project
level: implementation
---

## Description

Initialize the `alknet-http` crate from scratch. This crate is the HTTP
interface for alknet: it serves inbound HTTP on standard ALPNs (`h2`,
`http/1.1`, with WebSocket upgrade for browser bidirectional access to the
call protocol) and hosts the HTTP-backed call-protocol adapters
(`from_openapi`, `to_openapi`, `from_mcp`, `to_mcp`). HTTP/3 + WebTransport
(`h3`) is deferred per ADR-044 and is **not** part of this crate's initial
release.

The crate has two roles in one (ADR-039): HTTP server (`HttpAdapter`, a
`ProtocolHandler` for `h2`/`http/1.1` + WS upgrade) and HTTP client host
(`from_openapi`/`from_mcp` forwarding via `reqwest`, `to_openapi`/`to_mcp`
projections via `axum`). Both directions share the same HTTP dependencies,
which is why they live in one crate.

### Crate setup

Create `crates/alknet-http/` with:

- `Cargo.toml` — package metadata, dependencies, feature gates
- `src/lib.rs` — crate root with module declarations and re-exports
- Module skeleton files for:
  - `src/server/mod.rs` — `HttpAdapter`, axum-over-QUIC, gateway routes,
    `/healthz`, decoy, custom routes
  - `src/gateway/mod.rs` — shared dispatch spine (`GatewayDispatch`),
    error mapping
  - `src/client/mod.rs` — shared HTTP client (`ClientWithMiddleware`)
  - `src/adapters/mod.rs` — `from_openapi`, `to_openapi`, `from_mcp`,
    `to_mcp`
  - `src/websocket/mod.rs` — WS upgrade handler, framing, dispatch handoff

### Dependencies

| Crate | Purpose | Feature gate |
|-------|---------|--------------|
| `alknet-core` | `ProtocolHandler`, `Connection`, `AuthContext`, `Capabilities`, `IdentityProvider`, `AuthToken`, `Identity`, `HandlerError` (workspace path) | default |
| `alknet-call` | `OperationAdapter`, `AdapterError`, `OperationSpec`, `Handler`, `HandlerRegistration`, `OperationRegistry`, `OperationProvenance`, `OperationContext`, `ResponseEnvelope`, `CallError`, `EventEnvelope`, `Dispatcher`, `CallConnection` (workspace path) | default |
| `axum` | HTTP server — `Router`, extractors, middleware, WebSocket upgrade | default |
| `hyper` | HTTP/1.1 + HTTP/2 framing (axum is built on hyper) | default |
| `reqwest` | HTTP client — `from_openapi`/`from_mcp` forwarding | default |
| `reqwest-middleware` | `ClientWithMiddleware` wrapper for middleware stack | default |
| `reqwest-retry` | `RetryTransientMiddleware` / `ExponentialBackoff` | default |
| `tokio` 1 (full) | Async runtime | default |
| `serde` 1 | Serialization | default |
| `serde_json` 1 | JSON wire format, JSON Schema values | default |
| `async-trait` 0.1 | Async traits | default |
| `tracing` 0.1 | Structured logging | default |
| `thiserror` 2 | Error enums | default |
| `uuid` 1 | Request ID generation (UUID v4) | default |
| `futures` | Stream trait for SSE / subscriptions | default |
| `rmcp` | MCP streamable HTTP — `from_mcp`/`to_mcp` | `mcp` feature |
| `openapiv3` | OpenAPI document types (or a local type — two-way door) | default |

### Feature gates

```toml
[features]
default = ["h2", "http1"]
mcp = ["dep:rmcp"]
# h3 (HTTP/3 + WebTransport) is deferred per ADR-044 — not in the v1
# feature set. The browser bidirectional path uses WebSocket (native to
# axum, no feature gate). When WebTransport revives, the `h3` feature
# gate returns; see ADR-044 and webtransport.md.
```

- `h2` + `http1` (default): the `axum` + `hyper` HTTP/1.1 + HTTP/2 server,
  including WebSocket upgrade for browser bidirectional access (ADR-044).
- `mcp`: the `rmcp` dependency with streamable HTTP transport features
  only (ADR-037 — stdio is excluded). Adds `from_mcp`/`to_mcp`.

### Workspace Cargo.toml

Add `crates/alknet-http` to the workspace `members` list in the root
`Cargo.toml`.

### Module skeleton

```rust
// src/lib.rs
//! alknet-http: HTTP interface for alknet — serves HTTP/1.1 + HTTP/2 on
//! standard ALPNs (with WebSocket upgrade for browser bidirectional access)
//! and hosts the HTTP-backed call-protocol adapters.
//!
//! Two roles in one crate (ADR-039): HTTP server (HttpAdapter, a
//! ProtocolHandler for h2/http1.1 + WS upgrade) and HTTP client host
//! (from_openapi/from_mcp forwarding, to_openapi/to_mcp projections).

pub mod server;
pub mod gateway;
pub mod client;
pub mod adapters;
pub mod websocket;
```

Each module file gets a doc comment and `// TODO: implement` marker.

## Acceptance Criteria

- [ ] `crates/alknet-http/Cargo.toml` exists with all dependencies and feature gates
- [ ] `crates/alknet-http/src/lib.rs` exists with module declarations
- [ ] All module skeleton files exist (server/, gateway/, client/, adapters/, websocket/)
- [ ] Root `Cargo.toml` `members` list includes `crates/alknet-http`
- [ ] `cargo check -p alknet-http` succeeds
- [ ] `cargo clippy -p alknet-http` succeeds with no warnings
- [ ] Dual licensing: `MIT OR Apache-2.0` (workspace-inherited)
- [ ] `alknet-core` dependency uses workspace path (`path = "../alknet-core"`)
- [ ] `alknet-call` dependency uses workspace path (`path = "../alknet-call"`)
- [ ] `mcp` feature gate pulls in `rmcp` with streamable HTTP transport features only (no stdio)
- [ ] `h3`/WebTransport dependency is absent (deferred per ADR-044)

## References

- docs/architecture/crates/http/README.md — crate index
- docs/architecture/crates/http/overview.md — crate overview, dependencies, feature gates
- docs/architecture/decisions/003-crate-decomposition.md — ADR-003 (Amendment 1: alknet-call is protocol-foundation)
- docs/architecture/decisions/039-http-server-and-client-host-colocated.md — ADR-039 (one crate for server + client host)
- docs/architecture/decisions/044-defer-webtransport-browsers-use-websocket.md — ADR-044 (h3 deferred)
- docs/architecture/decisions/037-mcp-stdio-transport-exclusion.md — ADR-037 (streamable HTTP only)

## Notes

> alknet-http depends on alknet-core (ProtocolHandler, Connection,
> IdentityProvider, Capabilities) and alknet-call (OperationAdapter,
> OperationRegistry, HandlerRegistration, EventEnvelope, Dispatcher). The
> crate has five subsystems: server (HttpAdapter, axum over QUIC), gateway
> (shared dispatch spine, error mapping), client (shared HTTP client),
> adapters (from_openapi/to_openapi/from_mcp/to_mcp), and websocket (WS
> upgrade, native session). The module structure reflects this split. The
> `mcp` feature gate is optional; the default feature set is `h2` + `http1`.
> The `h3`/WebTransport dependency is absent (deferred per ADR-044).

## Summary

> To be filled on completion