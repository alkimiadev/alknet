---
id: vault/key-versioning-rotation
name: Implement version-indexed encryption key paths, bump CURRENT_KEY_VERSION to 2, and add rotate method
status: completed
depends_on: [vault/irpc-removal]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Fix drift items #3, #9, and #10 as one coherent feature: the version-indexed
key rotation mechanism from ADR-021. These three drifts are tightly coupled —
`CURRENT_KEY_VERSION = 2` (drift #3), version-aware `encrypt`/`decrypt` via
`encryption_path_for_version` (drift #9), and the `rotate` method (drift #10)
form the complete key rotation feature. Splitting them would produce tasks that
don't compile independently.

### Drift #3: Bump CURRENT_KEY_VERSION

Current: `CURRENT_KEY_VERSION = 1` (but the key is HD-derived, and v1 is
reserved for the TypeScript PBKDF2 legacy per ADR-020).

Target: `CURRENT_KEY_VERSION = 2` (HD-derived, per ADR-020).

Version semantics:
- v1: TypeScript predecessor's PBKDF2-encrypted data — the vault **cannot**
  decrypt it (different key derivation). Migration is a one-time re-encryption.
- v2: HD-derived at `m/74'/2'/0'/0'` (PATHS::ENCRYPTION) — current.
- v3+: `m/74'/2'/0'/1'`, `m/74'/2'/0'/2'`, etc. — future rotation versions.

### Drift #9: Version-aware encrypt/decrypt

Current: `encrypt`/`decrypt` always derive at `PATHS::ENCRYPTION` regardless of
the `key_version` parameter.

Target:
- `encrypt(plaintext, key_version)`: derive the encryption key at
  `encryption_path_for_version(key_version)`, stamp the same `key_version` on
  the resulting `EncryptedData`.
- `decrypt(encrypted)`: derive the key at
  `encryption_path_for_version(encrypted.key_version)` — the blob carries its
  own version, and each version maps to a distinct derivation path.

This requires:
1. `encryption_path_for_version(version: u32) -> Result<String, DerivationError>`
   already exists in `derivation.rs` — verify it returns `InvalidPath` for
   `version < 2` (v1 is TS legacy, v0 is meaningless).
2. `derive_encryption_key_for_version(version: u32) -> Result<DerivedKey, VaultServiceError>`
   — a new method on `VaultServiceHandle` that maps version → path → derive.
   Cached by path (same cache as `derive_encryption_key`).
3. `encrypt` and `decrypt` use `derive_encryption_key_for_version` instead of
   deriving at the fixed `PATHS::ENCRYPTION` path.

### Drift #10: Implement rotate

Current: no `rotate` method exists.

Target:
```rust
pub fn rotate(&self, encrypted: &EncryptedData, to_version: u32) -> Result<EncryptedData, VaultServiceError>;
```

Decrypts with the old version's key (from `encrypted.key_version`), re-encrypts
with the new version's key (`to_version`). Returns the new `EncryptedData` —
the caller replaces the blob in storage. No new mnemonic needed; the same seed
produces all version keys via different derivation paths (ADR-021).

### Implementation notes

- `derive_encryption_key(path)` (the path-based API) remains as-is for deriving
  at arbitrary paths. `derive_encryption_key_for_version(version)` is the
  version-aware API used by `encrypt`/`decrypt`. Both share the same cache
  (keyed by derivation path).
- `encrypt` and `decrypt` extract the `EncryptionKey` from the `DerivedKey` via
  `EncryptionKey::from_derived_bytes` (see encryption.md).
- `encryption_path_for_version` returns `InvalidPath` for `version < 2`.
  `derive_encryption_key_for_version` propagates this as
  `VaultServiceError::InvalidPath`.

### Scope

This task touches `encryption.rs` (CURRENT_KEY_VERSION), `service.rs` (encrypt,
decrypt, rotate, derive_encryption_key_for_version), and possibly `derivation.rs`
(verify `encryption_path_for_version`). It depends on the irpc removal task
(drift #4) because both modify `service.rs`.

## Acceptance Criteria

- [ ] `CURRENT_KEY_VERSION` is `2` in `encryption.rs`
- [ ] `derive_encryption_key_for_version(version)` method added to `VaultServiceHandle`
- [ ] `derive_encryption_key_for_version` returns `InvalidPath` for `version < 2`
- [ ] `encrypt(plaintext, key_version)` derives at `encryption_path_for_version(key_version)`
- [ ] `encrypt` stamps the passed `key_version` on the resulting `EncryptedData`
- [ ] `decrypt(encrypted)` derives at `encryption_path_for_version(encrypted.key_version)`
- [ ] `rotate(encrypted, to_version)` method implemented: decrypt old, re-encrypt new
- [ ] `rotate` returns `EncryptedData` with `key_version = to_version`
- [ ] Unit test: encrypt at v2, decrypt at v2 — round-trip succeeds
- [ ] Unit test: encrypt at v2, rotate to v3, decrypt at v3 — round-trip succeeds
- [ ] Unit test: decrypt v2 blob after rotation — old key still derivable (partial rotation safe)
- [ ] Unit test: `derive_encryption_key_for_version(1)` returns `InvalidPath`
- [ ] Unit test: `derive_encryption_key_for_version(0)` returns `InvalidPath`
- [ ] `cargo test` succeeds
- [ ] `cargo clippy` succeeds with no warnings

## References

- docs/architecture/crates/vault/README.md — Known Source Drift table items #3, #9, #10
- docs/architecture/crates/vault/encryption.md — Key Versioning, Rotation, EncryptionKey
- docs/architecture/crates/vault/service.md — encrypt, decrypt, rotate, derive_encryption_key_for_version
- docs/architecture/crates/vault/mnemonic-derivation.md — encryption_path_for_version, PATHS
- docs/architecture/decisions/020-hd-derivation-for-encryption-keys.md — ADR-020
- docs/architecture/decisions/021-key-rotation-via-version-indexed-paths.md — ADR-021

## Notes

> These three drifts are one feature: version-indexed key rotation (ADR-021).
> Splitting them would produce tasks that don't compile independently —
> bumping the version without version-aware encrypt/decrypt would make v2
> blobs undecryptable, and rotate without version-aware encrypt/decrypt has no
> keys to work with. Depends on irpc removal because both modify `service.rs`.

## Summary

Bumped `CURRENT_KEY_VERSION` to 2 (HD-derived per ADR-020). Added
`encryption_path_for_version` in derivation.rs (v2 → `m/74'/2'/0'/0'`, v3 →
`m/74'/2'/0'/1'`, rejects version < 2). Added `derive_encryption_key_for_version`
+ version-aware `encrypt`/`decrypt` + `rotate` method on `VaultServiceHandle`
(ADR-021). Each version maps to a distinct derivation path; the blob carries
its own version. 68 lib + 14 integration tests pass; clippy clean. Merged to
develop (resolved conflicts with remove-password-derivation and
poisoned-lock-recovery).