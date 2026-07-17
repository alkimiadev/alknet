---
id: tls/tests
name: Move and adapt TLS tests from endpoint.rs and call_client.rs into alknet-tls
status: pending
depends_on: [tls/client-extract]
scope: moderate
risk: low
impact: component
level: implementation
---

## Description

Phase 1, Task 4 of the crate extraction. Move the TLS-related tests from
`crates/alknet-core/src/endpoint.rs` and `crates/alknet-call/src/client/call_client.rs`
into `crates/alknet-tls/`. Adapt them to test the new public API types
(`TlsServerConfig`, `TlsClientConfig`) instead of the old free functions.

The old tests **stay** in their original files (duplicated) — no breakage. The new
crate's tests are self-contained and pass standalone.

### Server-side tests to move (from `endpoint.rs`)

22 tests total. Move to `crates/alknet-tls/src/server.rs` `#[cfg(test)] mod tests`:

| Test | Line | What it tests |
|------|------|---------------|
| `raw_key_cert_resolver_only_raw_public_keys` | 1099 | `RawKeyCertResolver` trait impl |
| `self_signed_cert_generation_produces_cert_and_key` | 1204 | `generate_self_signed_cert` |
| `acme_directory_production_url` | 1255 | `AcmeDirectory::Production` URL |
| `acme_directory_staging_url` | 1262 | `AcmeDirectory::Staging` URL |
| `acme_directory_custom_url` | 1272 | `AcmeDirectory::Custom` URL |
| `tls_setup_x509_returns_no_acme_state` | 1280 | `TlsSetup::new` with X509 |
| `build_rustls_server_config_raw_key_succeeds` | 1368 | `build_rustls_server_config` RawKey |
| `build_rustls_server_config_self_signed_succeeds` | 1379 | `build_rustls_server_config` SelfSigned |
| `build_rustls_server_config_acme_is_unreachable` | 1391 | ACME guard in `build_rustls_server_config` |
| `build_quinn_server_config_from_rustls_succeeds` | 1403 | `build_quinn_server_config_from_rustls` |
| `load_private_key_returns_error_when_no_key_present` | 1415 | `load_private_key` error path |
| `load_private_key_returns_error_when_file_missing` | 1428 | `load_private_key` missing file |
| `load_cert_chain_returns_error_when_file_missing` | 1440 | `load_cert_chain` missing file |
| `accept_any_cert_verifier_offers_and_does_not_require_client_auth` | 1454 | `AcceptAnyCertVerifier` trait |
| `accept_any_cert_verifier_verifies_any_client_cert` | 1464 | `AcceptAnyCertVerifier` verify |
| `accept_any_cert_verifier_supported_schemes_are_non_empty` | 1478 | `AcceptAnyCertVerifier` schemes |
| `accept_any_cert_verifier_debug_is_implemented` | 1489 | `AcceptAnyCertVerifier` Debug |
| `ed25519_signing_key_choose_scheme_returns_some_for_ed25519` | 1499 | `Ed25519SigningKey` choose_scheme |
| `ed25519_signing_key_choose_scheme_returns_none_without_ed25519` | 1512 | `Ed25519SigningKey` no ED25519 |
| `ed25519_signing_key_algorithm_is_ed25519` | 1525 | `Ed25519SigningKey` algorithm |
| `ed25519_signing_key_public_key_returns_spki` | 1534 | `Ed25519SigningKey` public_key |
| `ed25519_signing_key_signer_signs_message` | 1545 | `Ed25519SigningKey` sign |
| `ed25519_signing_key_debug_does_not_leak_material` | 1560 | `Ed25519SigningKey` Debug |
| `raw_key_cert_resolver_debug_is_implemented` | 1569 | `RawKeyCertResolver` Debug |

### Client-side tests to move (from `call_client.rs`)

10 tests total. Move to `crates/alknet-tls/src/client.rs` `#[cfg(test)] mod tests`:

| Test | Line | What it tests | Adaptation |
|------|------|---------------|------------|
| `fingerprint_pin_verifier_matches_correct_ed25519_fingerprint` | 750 | verifier accept | Test via `FingerprintPinVerifier` directly (no change needed) |
| `fingerprint_pin_verifier_rejects_wrong_ed25519_fingerprint` | 769 | verifier reject | Same |
| `fingerprint_pin_verifier_matches_correct_sha256_fingerprint` | 789 | verifier X.509 accept | Same |
| `fingerprint_pin_verifier_rejects_wrong_sha256_fingerprint` | 806 | verifier X.509 reject | Same |
| `select_server_verifier_returns_ca_verifier_for_none` | 822 | CA path | Test via `TlsClientConfig::new` or keep as unit test of internal fn |
| `select_server_verifier_returns_fingerprint_pin_for_some` | 839 | pin path | Same |
| `build_client_auth_presents_ed25519_raw_key_without_error` | 857 | client cert resolver | Test via `TlsClientConfig::new` or keep as unit test |
| `build_client_auth_none_resolves_to_no_client_cert` | 879 | no-cert resolver | Same |
| `build_quinn_client_config_with_raw_key_identity_builds_without_error` | 893 | full config build | Adapt to test `TlsClientConfig::new` + `for_quinn()` |
| `build_quinn_client_config_with_no_remote_identity_builds_without_error` | 909 | CA-verify config | Adapt to test `TlsClientConfig::new` + `for_quinn()` |

### Test adaptations

1. **Imports**: Update to use `alknet_tls::*` types, `alknet_core::config::*`, etc.
2. **Server tests**: Most server-side tests test free functions directly — they can stay
   as unit tests of the internal functions, or be adapted to test through `TlsServerConfig::new()`.
   The `tls_setup_x509_returns_no_acme_state` test should go through `TlsServerConfig::new()`.
3. **Client tests**: The `build_quinn_client_config_*` tests should be adapted to test
   `TlsClientConfig::new(credentials, alpn)?.for_quinn()` instead of the free function.
   The verifier and client-auth tests can stay as unit tests of the internal functions.
4. **`Ed25519SigningKey` tests**: Move to `crates/alknet-tls/src/signing.rs` `#[cfg(test)] mod tests`.
5. **`load_cert_chain` / `load_private_key` tests**: Move to `crates/alknet-tls/src/pem.rs` `#[cfg(test)] mod tests`.
6. **Test helpers**: The `build_ed25519_spki_der`, `build_x509_cert_der`, `aws_lc_rs_provider`,
   `verify_pin` helpers from `call_client.rs` tests should move with the tests that use them.
7. **Feature gates**: All quinn-dependent tests need `#[cfg(feature = "quinn")]`. The
   `acme_directory_*` tests don't need quinn. The `ed25519_signing_key_*` tests need quinn
   (they use `rustls::sign::SigningKey`).

### What stays in the original files

The old tests in `endpoint.rs` and `call_client.rs` are **not deleted** — they stay as
duplicates. The prune happens in Phase 4 (core) and Phase 5 (call). This task only adds
tests to `alknet-tls`.

## Acceptance Criteria

- [ ] All 22 server-side TLS tests moved to `alknet-tls/src/server.rs` and pass
- [ ] All 10 client-side TLS tests moved to `alknet-tls/src/client.rs` and pass
- [ ] `Ed25519SigningKey` tests (6) moved to `alknet-tls/src/signing.rs` and pass
- [ ] `load_cert_chain` / `load_private_key` tests (3) moved to `alknet-tls/src/pem.rs` and pass
- [ ] `build_quinn_client_config_*` tests adapted to use `TlsClientConfig::new().for_quinn()`
- [ ] `tls_setup_x509_returns_no_acme_state` adapted to use `TlsServerConfig::new()`
- [ ] All test helpers (`build_ed25519_spki_der`, `build_x509_cert_der`, `aws_lc_rs_provider`, `verify_pin`) moved with their tests
- [ ] Feature gates correct on all moved tests
- [ ] `cargo test -p alknet-tls` passes (all feature combos)
- [ ] `cargo test -p alknet-core` still passes (old tests untouched)
- [ ] `cargo test -p alknet-call` still passes (old tests untouched)
- [ ] `cargo clippy -p alknet-tls --all-targets` succeeds with no warnings

## References

- docs/research/alknet-crate-extraction/findings.md — Phase 1, test lists
- crates/alknet-core/src/endpoint.rs — lines 935-1606 (server-side tests)
- crates/alknet-call/src/client/call_client.rs — lines 569-930 (client-side tests)

## Notes

> This is the test migration task — 32 tests total (22 server + 10 client).
> The tests are well-understood and mostly need import updates. The
> `build_quinn_client_config_*` tests need the most adaptation (testing through
> `TlsClientConfig` instead of the free function). The old tests stay in their
> original files — the prune happens in Phases 4-5. Test helpers that are shared
> between multiple test functions should move to a `#[cfg(test)]` module in the
> same file.

## Summary

> To be filled on completion
