---
id: core/crate-init
name: Initialize alknet-core crate with Cargo.toml, dependencies, and module skeleton
status: pending
depends_on: []
scope: moderate
risk: low
impact: project
level: implementation
---

## Description

Initialize the `alknet-core` crate from scratch. The workspace currently has
only `alknet-vault`. This task creates the crate directory, `Cargo.toml`,
`lib.rs`, and the module skeleton that subsequent core tasks will fill in.

### Crate setup

Create `crates/alknet-core/` with:

- `Cargo.toml` — package metadata, dependencies, feature flags
- `src/lib.rs` — crate root with module declarations and re-exports
- Module skeleton files (empty or with `// TODO` markers) for:
  - `src/types.rs` — ProtocolHandler, HandlerError, Connection, BiStream, SendStream, RecvStream, StreamError, Capabilities
  - `src/auth.rs` — AuthContext, Identity, IdentityProvider, AuthToken, ConfigIdentityProvider
  - `src/config.rs` — StaticConfig, DynamicConfig, AuthPolicy, ApiKeyEntry, RateLimitConfig, ConfigReloadHandle, ConfigError, TlsIdentity
  - `src/endpoint.rs` — AlknetEndpoint, HandlerRegistry, EndpointError

### Dependencies

Per the architecture specs (overview.md, core/README.md, endpoint.md):

| Crate | Purpose |
|-------|---------|
| `tokio` 1 (full) | Async runtime, watch channel for shutdown |
| `quinn` | QUIC endpoint (feature-gated) |
| `iroh` | P2P relay-assisted endpoint (feature-gated) |
| `rustls` | TLS implementation |
| `rustls-pki-types` | TLS types (CertificateDer, PrivateKeyDer) |
| `serde` 1 | Serialization for config types |
| `serde_json` 1 | JSON for config, JSON Schema values |
| `toml` 0.8 | Config file format |
| `arc-swap` 1 | Atomic config swap for DynamicConfig |
| `async-trait` 0.1 | ProtocolHandler trait (async fn in trait) |
| `tracing` 0.1 | Structured logging |
| `thiserror` 2 | Error enums |
| `zeroize` 1 | Capabilities zeroization |
| `bytes` 1 | Byte buffer types for streams |
| `futures` | AsyncRead/AsyncWrite for BiStream trait |

### Feature flags

```toml
[features]
default = ["quinn"]
quinn = ["dep:quinn"]
iroh = ["dep:iroh"]
```

Both quinn and iroh are optional, both can be active simultaneously (ADR-010).
`quinn` is default-on for the common case; `iroh` is opt-in.

### Workspace Cargo.toml

Add `crates/alknet-core` to the workspace `members` list in the root
`Cargo.toml`.

### Module skeleton

```rust
// src/lib.rs
//! alknet-core: Core library for ALPN-based protocol dispatch.

pub mod types;
pub mod auth;
pub mod config;
pub mod endpoint;

// Re-exports (filled in by subsequent tasks)
```

Each module file gets a doc comment and `// TODO: implement` marker. The
subsequent tasks (core-types, config, auth, endpoint) fill these in.

## Acceptance Criteria

- [ ] `crates/alknet-core/Cargo.toml` exists with all dependencies and feature flags
- [ ] `crates/alknet-core/src/lib.rs` exists with module declarations
- [ ] Module skeleton files exist: `types.rs`, `auth.rs`, `config.rs`, `endpoint.rs`
- [ ] Root `Cargo.toml` `members` list includes `crates/alknet-core`
- [ ] `cargo check -p alknet-core` succeeds
- [ ] `cargo clippy -p alknet-core` succeeds with no warnings
- [ ] Dual licensing: `MIT OR Apache-2.0` (workspace-inherited)

## References

- docs/architecture/overview.md — crate graph, shared types
- docs/architecture/crates/core/README.md — crate index
- docs/architecture/crates/core/core-types.md — types to implement
- docs/architecture/crates/core/endpoint.md — endpoint, features (quinn + iroh)
- docs/architecture/crates/core/config.md — config types
- docs/architecture/crates/core/auth.md — auth types
- docs/architecture/decisions/003-crate-decomposition.md — ADR-003
- docs/architecture/decisions/010-alpn-router-and-endpoint.md — ADR-010 (feature-gating)

## Notes

> This is the foundational setup task for alknet-core. All subsequent core
> tasks depend on this one. The crate has no alknet dependencies (vault is
> standalone; core doesn't depend on vault). The feature flags for quinn/iroh
> are important — both are optional and can be active simultaneously.

## Summary

> To be filled on completion