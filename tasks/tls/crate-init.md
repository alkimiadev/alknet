---
id: tls/crate-init
name: Initialize alknet-tls crate with Cargo.toml, dependencies, and module skeleton
status: completed
depends_on: [core/connection-credentials]
scope: moderate
risk: low
impact: project
level: implementation
---

## Description

Phase 1, Task 1 of the crate extraction (per `docs/research/alknet-crate-extraction/findings.md`).
Initialize the `alknet-tls` crate from scratch. This crate provides TLS setup types for both
server-side and client-side — `TlsServerConfig`, `TlsClientConfig`, `TlsError`, and the shared
TLS helpers (`Ed25519SigningKey`, cert/key loaders, verifiers, cert resolvers).

### Crate setup

Create `crates/alknet-tls/` with:

- `Cargo.toml` — package metadata, dependencies, feature flags
- `src/lib.rs` — crate root with module declarations and re-exports
- Module skeleton files for:
  - `src/server.rs` — `TlsServerConfig`, `build_rustls_server_config`, `RawKeyCertResolver`, `AcceptAnyCertVerifier`, `SelfSignedCert`, `generate_self_signed_cert`, `TlsSetup` (extracted from `endpoint.rs` lines 493-934)
  - `src/client.rs` — `TlsClientConfig`, `build_quinn_client_config`, `build_client_auth`, `select_server_verifier`, `FingerprintPinVerifier`, `RawKeyClientCertResolver`, `NoClientCertResolver`, `load_platform_root_cert_store` (extracted from `call_client.rs` lines 189-320)
  - `src/signing.rs` — `Ed25519SigningKey` (consolidated — one copy, used by both server + client)
  - `src/pem.rs` — `load_cert_chain`, `load_private_key` (consolidated — one copy, used by both server + client)

### Dependencies

Per the findings (Phase 1):

| Crate | Purpose |
|-------|---------|
| `alknet-core` | `TlsIdentity`, `Ed25519SecretKey`, `fingerprint` (workspace path) |
| `rustls` 0.23 | TLS implementation (aws-lc-rs) |
| `rustls-pemfile` 2 | PEM cert/key loading |
| `rustls-native-certs` 0.8 | Platform root CA store |
| `webpki-roots` | Built-in root CA fallback (new — not extracted) |
| `rcgen` 0.13 | Self-signed cert generation |
| `tokio` 1 (full) | Async runtime |
| `quinn` 0.11 | QUIC transport (optional, feature-gated) |
| `tokio-rustls` 0.26 | TCP+TLS transport (optional, feature-gated) |
| `rustls-acme` 0.12 | ACME cert provisioning (optional, feature-gated) |
| `tracing` 0.1 | Structured logging |
| `thiserror` 2 | Error enums |

`rustls-native-certs` and `webpki-roots` are **always-present (not feature-gated)** — the
unknown-X.509-remote CA-verification path in `TlsClientConfig::new` is transport-agnostic;
the `webpki-roots` fallback merges built-in roots when the platform store is empty so
`NoRootAnchors` is unreachable in practice (ADR-088 §5).

### Feature flags

```toml
[features]
default = ["quinn"]
quinn = ["dep:quinn"]
tcp = ["dep:tokio-rustls"]
acme = ["dep:rustls-acme"]
```

### Workspace Cargo.toml

Add `crates/alknet-tls` to the workspace `members` list in the root `Cargo.toml`.

### Module skeleton

```rust
// src/lib.rs
//! alknet-tls: TLS setup types for alknet — server config, client config,
//! verifiers, cert resolvers, and shared signing helpers.
//!
//! Provides `TlsServerConfig` (server-side TLS setup) and `TlsClientConfig`
//! (client-side TLS setup), both transport-agnostic. Transport-specific
//! conversion (e.g. `for_quinn()`) is feature-gated.

pub mod client;
pub mod pem;
pub mod server;
pub mod signing;

// Re-exports (filled in by subsequent tasks)
```

Each module file gets a doc comment and `// TODO: implement` marker.

## Acceptance Criteria

- [ ] `crates/alknet-tls/Cargo.toml` exists with all dependencies and feature flags
- [ ] `crates/alknet-tls/src/lib.rs` exists with module declarations
- [ ] Module skeleton files exist: `server.rs`, `client.rs`, `signing.rs`, `pem.rs`
- [ ] Root `Cargo.toml` `members` list includes `crates/alknet-tls`
- [ ] `cargo check -p alknet-tls` succeeds
- [ ] `cargo clippy -p alknet-tls` succeeds with no warnings
- [ ] Dual licensing: `MIT OR Apache-2.0` (workspace-inherited)
- [ ] `alknet-core` dependency uses workspace path (`path = "../alknet-core"`)

## References

- docs/research/alknet-crate-extraction/findings.md — Phase 1
- docs/architecture/decisions/088-webpki-roots-fallback.md — ADR-088 §5
- crates/alknet-core/Cargo.toml — reference for dep versions
- crates/alknet-call/Cargo.toml — reference for dep versions

## Notes

> This is the foundational setup task for alknet-tls. All subsequent tls/*
> tasks depend on this one. The crate has no alknet dependencies beyond core.
> `rustls-native-certs` and `webpki-roots` are always-present (not feature-gated)
> per ADR-088 §5. The `quinn`/`tcp`/`acme` features gate transport-specific
> conversion methods, not the core TLS types.

## Summary

> To be filled on completion
