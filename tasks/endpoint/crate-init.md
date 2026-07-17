---
id: endpoint/crate-init
name: Initialize alknet-endpoint crate with Cargo.toml, dependencies, and module skeleton
status: pending
depends_on: [tls/review-tls]
scope: moderate
risk: low
impact: project
level: implementation
---

## Description

Phase 2, Task 1 of the crate extraction (per `docs/research/alknet-crate-extraction/findings.md`).
Initialize the `alknet-endpoint` crate from scratch. This crate provides the server-side
multi-transport accept-loop runner — `AlknetEndpoint`, `HandlerRegistry`, and the
transport-specific accept loops (quinn, iroh, TCP+TLS).

The endpoint takes **pre-built transports** via builder methods (`with_quinn`, `with_iroh`,
`with_tcp_tls`) — it does not build transports and does not depend on `alknet-tls`. The
assembly layer builds transports from `alknet-tls`'s `TlsServerConfig` and hands them to
the endpoint. See `docs/architecture/crates/endpoint/README.md` for the full spec.

### Crate setup

Create `crates/alknet-endpoint/` with:

- `Cargo.toml` — package metadata, dependencies, feature flags
- `src/lib.rs` — crate root with module declarations and re-exports
- Module skeleton files for:
  - `src/registry.rs` — `HandlerRegistry` (extracted from `endpoint.rs` lines 66-116)
  - `src/endpoint.rs` — `AlknetEndpoint` struct, `new`, builder methods, `run`, `shutdown` (built fresh against ADR-083 shape)
  - `src/dispatch.rs` — `dispatch` (public), `build_auth_context`, ACME guard (extracted from `endpoint.rs` lines 330-490)
  - `src/accept/mod.rs` — accept module declarations
  - `src/accept/quinn.rs` — `dispatch_quinn`, `run_quinn_accept_loop`, `extract_quinn_alpn`, `extract_quinn_client_fingerprint` (extracted from `endpoint.rs` lines 287-388)
  - `src/accept/iroh.rs` — `dispatch_iroh`, `run_iroh_accept_loop`, `extract_iroh_client_fingerprint` (extracted from `endpoint.rs` lines 390-472)
  - `src/accept/tcp_tls.rs` — `dispatch_tcp_tls`, `run_tcp_tls_accept_loop`, `extract_tcp_tls_alpn`, `extract_tcp_tls_client_fingerprint` (new code, not in current `endpoint.rs`)

### Dependencies

Per the findings (Phase 2) and the architecture spec:

| Crate | Purpose |
|-------|---------|
| `alknet-core` | `Connection`, `ProtocolHandler`, `AuthContext`, `IdentityProvider`, `DynamicConfig` (workspace path) |
| `quinn` 0.11 | QUIC transport (optional, feature-gated) |
| `iroh` 0.28 | Iroh transport (optional, feature-gated) |
| `tokio-rustls` 0.26 | TCP+TLS transport (optional, feature-gated) |
| `tokio` 1 (full) | Async runtime, spawn, watch, TcpListener |
| `arc-swap` 1 | `DynamicConfig` |
| `tracing` 0.1 | Structured logging |

The endpoint does **not** depend on `alknet-tls` — it takes pre-built transports. TLS config
construction stays at the assembly layer.

### Feature flags

```toml
[features]
default = []
quinn = ["dep:quinn", "alknet-core/quinn"]      # with_quinn — quinn accept loop
iroh = ["dep:iroh", "alknet-core/iroh"]          # with_iroh — iroh accept loop
tcp = ["dep:tokio-rustls"]                       # with_tcp_tls — TCP+TLS accept loop
```

The `quinn`/`iroh` features pull the corresponding features on `alknet-core` (for
`Connection::from_quinn` / `from_iroh` — the constructors stay in core). A deployment
enables the features for the transports it runs.

### Workspace Cargo.toml

Add `crates/alknet-endpoint` to the workspace `members` list in the root `Cargo.toml`.

### Module skeleton

```rust
// src/lib.rs
//! alknet-endpoint: Server-side multi-transport accept-loop runner.
//!
//! `AlknetEndpoint` takes pre-built transports (quinn, iroh, TCP+TLS) via
//! builder methods, runs their accept loops inside `run()`, and dispatches
//! each accepted connection to the registered `ProtocolHandler` by ALPN.
//!
//! The endpoint does not build transports and does not depend on
//! `alknet-tls` — transport construction is the assembly layer's concern.

pub mod accept;
pub mod dispatch;
pub mod endpoint;
pub mod registry;

// Re-exports (filled in by subsequent tasks)
```

Each module file gets a doc comment and `// TODO: implement` marker.

## Acceptance Criteria

- [ ] `crates/alknet-endpoint/Cargo.toml` exists with all dependencies and feature flags
- [ ] `crates/alknet-endpoint/src/lib.rs` exists with module declarations
- [ ] Module skeleton files exist: `registry.rs`, `endpoint.rs`, `dispatch.rs`, `accept/mod.rs`, `accept/quinn.rs`, `accept/iroh.rs`, `accept/tcp_tls.rs`
- [ ] Root `Cargo.toml` `members` list includes `crates/alknet-endpoint`
- [ ] `cargo check -p alknet-endpoint` succeeds
- [ ] `cargo clippy -p alknet-endpoint` succeeds with no warnings
- [ ] Dual licensing: `MIT OR Apache-2.0` (workspace-inherited)
- [ ] `alknet-core` dependency uses workspace path (`path = "../alknet-core"`)
- [ ] No dependency on `alknet-tls` (endpoint takes pre-built transports)
- [ ] Feature flags: `quinn`, `iroh`, `tcp` (all optional, default off)

## References

- docs/research/alknet-crate-extraction/findings.md — Phase 2
- docs/architecture/crates/endpoint/README.md — full architecture spec
- docs/architecture/decisions/083-endpoint-as-accept-loop-runner.md — ADR-083
- crates/alknet-core/Cargo.toml — reference for dep versions
- crates/alknet-tls/Cargo.toml — reference for feature flag pattern

## Notes

> This is the foundational setup task for alknet-endpoint. All subsequent endpoint/*
> tasks depend on this one. The crate has no alknet dependencies beyond core.
> The endpoint does NOT depend on alknet-tls — it takes pre-built transports via
> builder methods. The `quinn`/`iroh` features pull the corresponding features on
> `alknet-core` for `Connection::from_quinn` / `from_iroh` constructors.

## Summary

> To be filled on completion
