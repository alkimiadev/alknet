---
id: tls/server-extract
name: Extract server-side TLS code from alknet-core/endpoint.rs into alknet-tls
status: pending
depends_on: [tls/crate-init]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Phase 1, Task 2 of the crate extraction. Extract the server-side TLS setup code from
`crates/alknet-core/src/endpoint.rs` (lines 493-934) into `crates/alknet-tls/src/server.rs`
and `crates/alknet-tls/src/signing.rs` + `crates/alknet-tls/src/pem.rs` (shared helpers).

The old code **stays** in `endpoint.rs` (duplicated) — no breakage. The new crate is
self-contained and builds standalone.

### Types to extract

From `endpoint.rs` lines 493-934:

| Type/Function | Lines | Destination |
|---------------|-------|-------------|
| `TlsSetup` struct + `new()` + `new_acme()` | 493-611 | `server.rs` |
| `build_quinn_server_config_from_rustls()` | 614-624 | `server.rs` |
| `build_rustls_server_config()` | 626-674 | `server.rs` |
| `build_iroh_endpoint()` | 676-703 | `server.rs` (feature-gated on `iroh`) |
| `load_cert_chain()` | 705-714 | `pem.rs` (shared) |
| `load_private_key()` | 716-730 | `pem.rs` (shared) |
| `SelfSignedCert` struct | 732-736 | `server.rs` |
| `generate_self_signed_cert()` | 738-755 | `server.rs` |
| `AcceptAnyCertVerifier` struct + impls | 757-834 | `server.rs` |
| `RawKeyCertResolver` struct + impls | 836-873 | `server.rs` |
| `Ed25519SigningKey` struct + impls | 875-933 | `signing.rs` (shared) |

### New public API types

The extracted code currently uses free functions and private structs. Wrap them in
public API types for the crate:

```rust
// server.rs

/// Server-side TLS configuration, transport-agnostic.
/// Wraps a `rustls::ServerConfig` plus optional ACME state.
pub struct TlsServerConfig {
    pub(crate) rustls_config: rustls::ServerConfig,
    #[cfg(feature = "acme")]
    pub(crate) acme_state_handle: Option<tokio::task::JoinHandle<()>>,
}

impl TlsServerConfig {
    /// Build a server config from a `TlsIdentity` and ALPN list.
    /// ACME identities spawn a background cert-renewal task.
    pub async fn new(
        tls_identity: &alknet_core::config::TlsIdentity,
        alpns: &[Vec<u8>],
    ) -> Result<Self, TlsError> { ... }

    /// Convert to a `quinn::ServerConfig` for QUIC transport.
    #[cfg(feature = "quinn")]
    pub fn for_quinn(self) -> Result<quinn::ServerConfig, TlsError> { ... }
}
```

```rust
// signing.rs

/// Ed25519 signing key usable as both a rustls `SigningKey` and `Signer`.
/// Consolidated — one copy used by both server (`RawKeyCertResolver`) and
/// client (`RawKeyClientCertResolver`).
pub struct Ed25519SigningKey { ... }
```

```rust
// pem.rs

/// Load a PEM-encoded certificate chain from a file path.
pub fn load_cert_chain(path: &Path) -> Result<Vec<CertificateDer<'static>>, TlsError> { ... }

/// Load a PEM-encoded private key from a file path.
pub fn load_private_key(path: &Path) -> Result<PrivateKeyDer<'static>, TlsError> { ... }
```

### TlsError

Define a unified error type:

```rust
#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    #[error("TLS config error: {0}")]
    Config(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("certificate error: {0}")]
    Cert(String),
}
```

### Adaptations

1. **Error types**: Replace `EndpointError::TlsConfig(...)` with `TlsError::Config(...)` or `TlsError::Io(...)`. The `EndpointError` type stays in core — the extracted code uses `TlsError` instead.
2. **Imports**: Update all `crate::config::*` imports to `alknet_core::config::*`. Update `crate::fingerprint::*` to `alknet_core::fingerprint::*`.
3. **`Ed25519SigningKey`**: Move to `signing.rs` as a shared type. Both `server.rs` and (later) `client.rs` will use it from there.
4. **`load_cert_chain` / `load_private_key`**: Move to `pem.rs` as shared functions. Both `server.rs` and (later) `client.rs` will use them from there.
5. **`build_iroh_endpoint`**: This is an iroh-specific builder, not pure TLS. Gate on `#[cfg(feature = "iroh")]` and depend on `alknet-core/iroh`. If the iroh dep is too heavy for `alknet-tls`, leave it in core for now and note as a TODO.
6. **Feature gates**: `AcceptAnyCertVerifier`, `RawKeyCertResolver`, `SelfSignedCert`, `generate_self_signed_cert`, `build_rustls_server_config`, `build_quinn_server_config_from_rustls` are gated on `#[cfg(feature = "quinn")]`. `build_iroh_endpoint` is gated on `#[cfg(feature = "iroh")]`. `TlsSetup::new_acme` is gated on `#[cfg(feature = "acme")]`.

### What stays in core

The old code in `endpoint.rs` lines 493-934 is **not deleted** — it stays as a duplicate.
The prune happens in Phase 4. This task only adds code to `alknet-tls`.

## Acceptance Criteria

- [ ] `crates/alknet-tls/src/server.rs` contains `TlsServerConfig`, `TlsSetup`, `build_rustls_server_config`, `build_quinn_server_config_from_rustls`, `RawKeyCertResolver`, `AcceptAnyCertVerifier`, `SelfSignedCert`, `generate_self_signed_cert`
- [ ] `crates/alknet-tls/src/signing.rs` contains `Ed25519SigningKey` with `SigningKey` + `Signer` impls
- [ ] `crates/alknet-tls/src/pem.rs` contains `load_cert_chain` + `load_private_key`
- [ ] `TlsServerConfig::new()` accepts `&TlsIdentity` + `&[Vec<u8>]` and returns `Result<Self, TlsError>`
- [ ] `TlsServerConfig::for_quinn()` converts to `quinn::ServerConfig` (feature-gated)
- [ ] `TlsError` enum has `Config`, `Io`, `Cert` variants
- [ ] All extracted code uses `TlsError` (not `EndpointError`)
- [ ] All extracted code imports from `alknet_core` (not `crate::`)
- [ ] Feature gates correct: `quinn` for TLS types, `acme` for ACME, `iroh` for iroh builder
- [ ] `cargo check -p alknet-tls` succeeds (all feature combos)
- [ ] `cargo clippy -p alknet-tls` succeeds with no warnings
- [ ] `cargo test -p alknet-core` still passes (old code untouched)
- [ ] `cargo test -p alknet-call` still passes (old code untouched)

## References

- docs/research/alknet-crate-extraction/findings.md — Phase 1, server-side extraction
- crates/alknet-core/src/endpoint.rs — lines 493-934 (source code to extract)
- crates/alknet-core/src/config.rs — `TlsIdentity`, `Ed25519SecretKey`, `AcmeDirectory`
- crates/alknet-core/src/fingerprint.rs — `extract_ed25519_raw_key_from_spki`

## Notes

> This is the largest single extraction in Phase 1 (~440 lines). The code is
> well-understood and tested — the main work is adapting error types and imports.
> `Ed25519SigningKey` and `load_cert_chain`/`load_private_key` are extracted to
> shared modules because the client-side extraction (next task) also needs them.
> The `build_iroh_endpoint` function may be deferred if the iroh dep is too heavy
> for `alknet-tls` — note as TODO if so. The old code in `endpoint.rs` is NOT
> deleted — that's Phase 4.

## Summary

> To be filled on completion
