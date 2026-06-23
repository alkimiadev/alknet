---
id: vault/review-vault-sync
name: Review vault implementation against specs after all drift fixes
status: pending
depends_on: [vault/irpc-removal, vault/osrng-iv-generation, vault/poisoned-lock-recovery, vault/remove-password-derivation, vault/unlock-new-zeroizing-return, vault/key-versioning-rotation, vault/derivedkey-serialization, vault/cache-zeroization-test]
scope: moderate
risk: low
impact: phase
level: review
---

## Description

Review the vault crate implementation against the architecture specs after all
drift fixes are complete. This is the quality checkpoint before the spec-sync
task — verify that the implementation matches the specs and that no drift
items were missed or incompletely fixed.

### Review Checklist

1. **Drift table verification** — every item in the vault README's Known Source
   Drift table is resolved:
   - #1: OsRng for IVs (encryption.rs)
   - #2: No unwrap() on RwLock (service.rs)
   - #3: CURRENT_KEY_VERSION = 2 (encryption.rs)
   - #4: irpc removed, direct method calls (protocol.rs, service.rs, Cargo.toml)
   - #5: DerivedKey always-redact serialization (protocol.rs)
   - #6: Cache zeroization tested (cache.rs)
   - #7: derive_password removed (service.rs, derivation)
   - #8: unlock_new returns Zeroizing<String> (service.rs)
   - #9: encrypt/decrypt version-aware (service.rs)
   - #10: rotate implemented (service.rs)

2. **Spec conformance** — implementation matches the spec docs:
   - `VaultServiceHandle` API matches service.md (all methods, signatures, semantics)
   - `DerivedKey` / `KeyType` match protocol.md (serialization, redaction, move-only)
   - `EncryptedData` / `EncryptionKey` match encryption.md (fields, key versioning)
   - `Mnemonic` / `Seed` / `ExtendedPrivKey` match mnemonic-derivation.md
   - `KeyCache` / `CachedKey` / `CacheConfig` match service.md Cache section
   - PATHS constants match mnemonic-derivation.md (IDENTITY, DEVICE_PREFIX, SSH_HOST, ENCRYPTION, ETHEREUM)
   - `encryption_path_for_version` matches (returns InvalidPath for version < 2)

3. **Security constraints** (from service.md, encryption.md, README.md):
   - OsRng for IVs and salt (no `rand::random()`)
   - Zeroized drop on Seed, Mnemonic, ExtendedPrivKey, EncryptionKey, CachedKey, DerivedKey
   - No `unwrap()` or `expect()` outside tests
   - DerivedKey is move-only (no Clone)
   - DerivedKey Debug impl redacts private key
   - Cache eviction zeroizes (tested)
   - No tokio dependency (local-only, std::sync::RwLock)

4. **Public API** — `lib.rs` re-exports match the vault README's Public API section:
   - `Mnemonic`, `Seed`, `Language` from mnemonic
   - `DerivationError`, `ExtendedPrivKey`, `PATHS` from derivation
   - `EncryptedData`, `EncryptionError`, `EncryptionKey`, `CURRENT_KEY_VERSION` from encryption
   - `DerivedKey`, `KeyType` from protocol
   - `VaultServiceError`, `VaultServiceHandle` from service
   - `CacheConfig` from cache
   - No `VaultMessage`, `VaultProtocol`, `VaultServiceActor` (removed)

5. **Test coverage**:
   - Derivation test vectors (BIP39 "abandon...about" vector)
   - Encryption round-trip tests
   - Service lifecycle tests (unlock, lock, derive, encrypt, decrypt, rotate)
   - Cache tests (LRU, TTL, clear, zeroization)
   - Serialization redaction tests (JSON redact, reject redacted deserialize)

6. **Code quality**:
   - `cargo fmt --check` passes
   - `cargo clippy` passes with no warnings
   - No dead code (removed irpc/actor/password paths fully gone)

## Acceptance Criteria

- [ ] All 10 drift items verified resolved
- [ ] VaultServiceHandle API matches service.md
- [ ] DerivedKey / KeyType match protocol.md
- [ ] EncryptedData / EncryptionKey match encryption.md
- [ ] Mnemonic / Seed / ExtendedPrivKey match mnemonic-derivation.md
- [ ] KeyCache / CachedKey / CacheConfig match service.md
- [ ] PATHS constants match mnemonic-derivation.md
- [ ] All security constraints satisfied (OsRng, zeroize, no unwrap, move-only, redaction)
- [ ] Public API (lib.rs re-exports) matches vault README
- [ ] Test coverage adequate for all functionality
- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy` passes with no warnings
- [ ] All tests pass
- [ ] No dead code from removed features (irpc, actor, password derivation)

## References

- docs/architecture/crates/vault/README.md — drift table, public API, security constraints
- docs/architecture/crates/vault/service.md
- docs/architecture/crates/vault/encryption.md
- docs/architecture/crates/vault/protocol.md
- docs/architecture/crates/vault/mnemonic-derivation.md
- docs/architecture/decisions/018-vault-standalone-crate.md
- docs/architecture/decisions/020-hd-derivation-for-encryption-keys.md
- docs/architecture/decisions/021-key-rotation-via-version-indexed-paths.md
- docs/architecture/decisions/025-vault-local-only-dispatch.md
- docs/architecture/decisions/026-vault-key-model-hd-derivation.md

## Notes

> This review verifies the vault is spec-conformant after all drift fixes. If
> deviations are found, document them and create fix tasks before proceeding
> to the spec-sync task. This is the last checkpoint before the vault docs are
> updated to remove the drift table and bump status.

## Summary

> To be filled on completion