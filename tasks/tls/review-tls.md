---
id: tls/review-tls
name: Review alknet-tls implementation for spec conformance, deduplication, and test coverage
status: completed
depends_on: [tls/tests]
scope: moderate
risk: low
impact: phase
level: review
---

## Description

Phase 1 review checkpoint. Verify the `alknet-tls` crate is spec-conformant,
self-contained, and ready for downstream consumption by `alknet-endpoint` (Phase 2)
and `alknet-client` (Phase 3).

### Review Checklist

1. **Crate structure**:
   - Module layout matches spec: `server.rs`, `client.rs`, `signing.rs`, `pem.rs`
   - Public API types: `TlsServerConfig`, `TlsClientConfig`, `TlsError`, `Ed25519SigningKey`
   - Re-exports in `lib.rs` are correct and minimal

2. **Server-side conformance**:
   - `TlsServerConfig::new()` accepts `&TlsIdentity` + `&[Vec<u8>]` and returns `Result<Self, TlsError>`
   - `TlsServerConfig::for_quinn()` converts to `quinn::ServerConfig` (feature-gated)
   - `RawKeyCertResolver` implements `ResolvesServerCert` with `only_raw_public_keys() == true`
   - `AcceptAnyCertVerifier` implements `ClientCertVerifier` in "request-but-don't-require" mode
   - `SelfSignedCert` generation uses `rcgen`
   - ACME path (`TlsSetup::new_acme`) is feature-gated on `acme`
   - `build_iroh_endpoint` is either extracted (feature-gated on `iroh`) or deferred with a TODO

3. **Client-side conformance**:
   - `TlsClientConfig::new()` accepts `&ConnectionCredentials` + `&[u8]` and returns `Result<Self, TlsError>`
   - `TlsClientConfig::for_quinn()` converts to `quinn::ClientConfig` (feature-gated)
   - `FingerprintPinVerifier` implements `ServerCertVerifier` with fingerprint matching
   - `select_server_verifier` logic: `Some` → fingerprint pin, `None` → CA verification (ADR-034 §3)
   - `RawKeyClientCertResolver` implements `ResolvesClientCert` with `only_raw_public_keys()` detection
   - `NoClientCertResolver` implements `ResolvesClientCert` with `has_certs() == false`
   - `load_platform_root_cert_store` includes `webpki-roots` fallback (ADR-088 §5)

4. **Shared code deduplication**:
   - `Ed25519SigningKey` is defined once in `signing.rs`, used by both server and client
   - `load_cert_chain` / `load_private_key` are defined once in `pem.rs`, used by both
   - No duplicate `Ed25519SigningKey` or PEM loaders between server and client modules

5. **Dependency hygiene**:
   - `rustls-native-certs` and `webpki-roots` are always-present (not feature-gated) per ADR-088 §5
   - `quinn` is optional, gated behind `quinn` feature
   - `tokio-rustls` is optional, gated behind `tcp` feature
   - `rustls-acme` is optional, gated behind `acme` feature
   - No unexpected heavy deps

6. **Error handling**:
   - `TlsError` has `Config`, `Io`, `Cert` variants
   - All public fallible functions return `Result<_, TlsError>` (no raw `String` errors)
   - Error messages are descriptive

7. **Test coverage**:
   - All 22 server-side tests pass
   - All 10 client-side tests pass
   - `Ed25519SigningKey` tests (6) pass
   - PEM loader tests (3) pass
   - Tests exercise error paths (missing files, wrong fingerprints, etc.)
   - Feature-gated tests are correctly annotated

8. **Cross-cutting checks**:
   - `cargo build -p alknet-tls` succeeds (all feature combos)
   - `cargo test -p alknet-tls` succeeds (all feature combos)
   - `cargo clippy -p alknet-tls --all-targets` succeeds with no warnings
   - `cargo fmt --check -p alknet-tls` passes
   - `cargo build --workspace` still succeeds (old code untouched)
   - `cargo test --workspace` still succeeds (old tests untouched)

## Acceptance Criteria

- [ ] Crate structure matches spec (4 modules, public API types)
- [ ] `TlsServerConfig` API correct and feature-gated
- [ ] `TlsClientConfig` API correct and feature-gated
- [ ] `Ed25519SigningKey` deduplicated (one copy in `signing.rs`)
- [ ] `load_cert_chain` / `load_private_key` deduplicated (one copy in `pem.rs`)
- [ ] `webpki-roots` fallback present in `load_platform_root_cert_store`
- [ ] `rustls-native-certs` + `webpki-roots` always-present (not feature-gated)
- [ ] All 41 tests pass (22 server + 10 client + 6 signing + 3 pem)
- [ ] `cargo build -p alknet-tls` succeeds (all feature combos)
- [ ] `cargo test -p alknet-tls` succeeds (all feature combos)
- [ ] `cargo clippy -p alknet-tls --all-targets` succeeds with no warnings
- [ ] `cargo fmt --check -p alknet-tls` passes
- [ ] Workspace still green: `cargo build --workspace` + `cargo test --workspace` pass

## References

- docs/research/alknet-crate-extraction/findings.md — Phase 1
- docs/architecture/decisions/088-webpki-roots-fallback.md — ADR-088 §5
- docs/architecture/decisions/034-outgoing-only-x509-and-three-peer-roles.md — ADR-034 §3
- tasks/tls/crate-init.md
- tasks/tls/server-extract.md
- tasks/tls/client-extract.md
- tasks/tls/tests.md

## Notes

> This review gates Phase 1 completion. The crate must be self-contained and
> spec-conformant before Phase 2 (`alknet-endpoint`) and Phase 3 (`alknet-client`)
> begin, since both depend on `alknet-tls`. The old code in core and call is
> intentionally still present (duplicated) — the prunes happen in Phases 4-5.
> If deviations are found, document and fix before proceeding to Phase 2.

## Summary

> To be filled on completion
