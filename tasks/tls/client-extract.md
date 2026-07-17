---
id: tls/client-extract
name: Extract client-side TLS code from alknet-call/call_client.rs into alknet-tls
status: pending
depends_on: [tls/server-extract]
scope: narrow
risk: medium
impact: component
level: implementation
---

## Description

Phase 1, Task 3 of the crate extraction. Extract the client-side TLS setup code from
`crates/alknet-call/src/client/call_client.rs` (lines 189-320) into
`crates/alknet-tls/src/client.rs`. Reuse the shared `Ed25519SigningKey` from
`signing.rs` and `load_cert_chain`/`load_private_key` from `pem.rs` (already extracted
in the previous task).

The old code **stays** in `call_client.rs` (duplicated) — no breakage. The new crate is
self-contained and builds standalone.

### Types to extract

From `call_client.rs` lines 189-320:

| Type/Function | Lines | Destination |
|---------------|-------|-------------|
| `build_quinn_client_config()` | 189-211 | `client.rs` |
| `build_client_auth()` | 213-246 | `client.rs` |
| `select_server_verifier()` | 248-278 | `client.rs` |
| `load_platform_root_cert_store()` | 280-297 | `client.rs` |
| `load_cert_chain()` | 299-308 | **skip** — already in `pem.rs` from server-extract |
| `load_private_key()` | 310-321 | **skip** — already in `pem.rs` from server-extract |

Also extract from `call_client.rs` lines 323-567 (the struct impls):

| Type/Function | Lines | Destination |
|---------------|-------|-------------|
| `RawKeyClientCertResolver` struct + impls | 323-374 | `client.rs` |
| `NoClientCertResolver` struct + impls | 376-403 | `client.rs` |
| `FingerprintPinVerifier` struct + impls | 405-507 | `client.rs` |
| `Ed25519SigningKey` struct + impls | 509-567 | **skip** — already in `signing.rs` from server-extract |

### New public API type

Wrap the extracted client-side code in a public API type:

```rust
// client.rs

/// Client-side TLS configuration, transport-agnostic.
/// Wraps a `rustls::ClientConfig` built from `ConnectionCredentials`.
pub struct TlsClientConfig {
    pub(crate) rustls_config: rustls::ClientConfig,
}

impl TlsClientConfig {
    /// Build a client config from `ConnectionCredentials` and an ALPN.
    /// Selects the server cert verifier by `remote_identity` presence
    /// (ADR-034 §3): `Some` → fingerprint pin, `None` → CA verification.
    pub fn new(
        credentials: &alknet_core::credentials::ConnectionCredentials,
        alpn: &[u8],
    ) -> Result<Self, TlsError> { ... }

    /// Convert to a `quinn::ClientConfig` for QUIC transport.
    #[cfg(feature = "quinn")]
    pub fn for_quinn(self) -> Result<quinn::ClientConfig, TlsError> { ... }
}
```

### Adaptations

1. **Error types**: Replace `String` error returns with `TlsError`. The current code returns
   `Result<_, String>` from most functions — convert to `Result<_, TlsError>`.
2. **Imports**: Update `alknet_core::config::*` and `alknet_core::fingerprint::*` imports.
   Use `crate::signing::Ed25519SigningKey` (not a local copy).
   Use `crate::pem::load_cert_chain` / `crate::pem::load_private_key` (not local copies).
3. **`load_platform_root_cert_store`**: Add the `webpki-roots` fallback (ADR-088 §5) —
   when the platform store is empty, merge built-in `webpki-roots` so `NoRootAnchors` is
   unreachable in practice. This is new code, not extracted.
4. **`FingerprintPinVerifier`**: The `verify_tls12_signature` and `verify_tls13_signature`
   methods use `alknet_core::fingerprint::extract_ed25519_raw_key_from_spki` — keep that
   import.
5. **Feature gates**: All client-side TLS code is gated on `#[cfg(feature = "quinn")]`.
   The `TlsClientConfig::new()` constructor itself is **not** feature-gated (it builds a
   `rustls::ClientConfig`, which is transport-agnostic). Only `for_quinn()` is gated.

### What stays in call

The old code in `call_client.rs` lines 189-567 is **not deleted** — it stays as a duplicate.
The prune happens in Phase 5. This task only adds code to `alknet-tls`.

## Acceptance Criteria

- [ ] `crates/alknet-tls/src/client.rs` contains `TlsClientConfig`, `build_quinn_client_config` (as `TlsClientConfig::new`), `build_client_auth`, `select_server_verifier`, `load_platform_root_cert_store`, `FingerprintPinVerifier`, `RawKeyClientCertResolver`, `NoClientCertResolver`
- [ ] `TlsClientConfig::new()` accepts `&ConnectionCredentials` + `&[u8]` and returns `Result<Self, TlsError>`
- [ ] `TlsClientConfig::for_quinn()` converts to `quinn::ClientConfig` (feature-gated)
- [ ] `load_platform_root_cert_store` includes `webpki-roots` fallback (ADR-088 §5)
- [ ] Client code uses `crate::signing::Ed25519SigningKey` (not a local copy)
- [ ] Client code uses `crate::pem::load_cert_chain` / `crate::pem::load_private_key` (not local copies)
- [ ] All error returns use `TlsError` (not `String`)
- [ ] Feature gates correct: `quinn` for `for_quinn()` and quinn-specific helpers
- [ ] `cargo check -p alknet-tls` succeeds (all feature combos)
- [ ] `cargo clippy -p alknet-tls` succeeds with no warnings
- [ ] `cargo test -p alknet-core` still passes (old code untouched)
- [ ] `cargo test -p alknet-call` still passes (old code untouched)

## References

- docs/research/alknet-crate-extraction/findings.md — Phase 1, client-side extraction
- docs/architecture/decisions/088-webpki-roots-fallback.md — ADR-088 §5
- docs/architecture/decisions/034-outgoing-only-x509-and-three-peer-roles.md — ADR-034 §3
- crates/alknet-call/src/client/call_client.rs — lines 189-567 (source code to extract)
- crates/alknet-core/src/fingerprint.rs — `extract_ed25519_raw_key_from_spki`, `fingerprint_from_cert_der`

## Notes

> This is the smaller extraction (~130 lines of implementation). The main work
> is adapting error types (String → TlsError) and reusing the shared
> `Ed25519SigningKey` and `load_cert_chain`/`load_private_key` from the
> server-extract task. The `webpki-roots` fallback in `load_platform_root_cert_store`
> is new code (not extracted) per ADR-088 §5. The old code in `call_client.rs` is
> NOT deleted — that's Phase 5.

## Summary

> To be filled on completion
