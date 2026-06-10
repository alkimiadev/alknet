---
id: encryption-salt-kdf
name: Clarify and fix EncryptedData salt usage — use HKDF for key derivation or document as reserved
status: pending
depends_on: [spec-update-secret-service]
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

The `EncryptedData` struct has a `salt` field that is generated randomly during encryption but not used in the key derivation process. The current encryption flow is:

1. Derive key from seed at path `m/74'/2'/0'/0'`
2. Use first 32 bytes of derived private key as AES-256-GCM key
3. Generate random 12-byte IV
4. Generate random 32-byte salt (stored but NOT used in key derivation)
5. Encrypt with AES-256-GCM using the derived key + random IV
6. Store `{key_version, salt, iv, data}` as `EncryptedData`

The salt is stored but serves no purpose. This is a gap because:

- Without a KDF, the same derived key is used for every encryption operation (different IVs provide per-message randomness, but the key itself is static)
- Salt-based key derivation would add an additional security layer: even if the derivation path is known, the salt provides per-encryption diversity
- The `key_version` field exists for rotation but without KDF-based key derivation, there's no mechanism to rotate to a stronger key

**The spec update (spec-update-secret-service task) decides one of two paths:**

### Option A: Use HKDF for key derivation (recommended for v1)

Replace the direct "first 32 bytes of derived key" approach with:
1. Derive master key from seed at path `m/74'/2'/0'/0'`
2. Use HKDF-SHA256 with `salt` and `info = "alknet-encryption-v{key_version}"` to derive the actual AES-256-GCM key
3. This means: same seed + same path + different salt = different AES key

Benefits: Each encryption uses a unique derived key (even with the same master key), providing forward security and key diversity. The salt is now purposeful.

### Option B: Document salt as reserved (Phase B)

Keep the current approach (direct key from derivation path) and document the salt field as "reserved for future KDF-based key derivation." Add a comment explaining that v1 doesn't use the salt.

This is simpler in v1 but defers the security improvement.

**This task implements whichever option the spec update chooses.** If the spec says "use HKDF now," implement Option A. If it says "document as reserved," implement Option B.

**If Option A (HKDF):**

1. Add `hkdf` dependency to `Cargo.toml`
2. Modify `encryption::encrypt()`:
   - Generate random salt (32 bytes)
   - Use HKDF-SHA256 to derive AES key from: `master_key + salt + info`
   - The `info` string includes the key version for forward compatibility
3. Modify `encryption::decrypt()`:
   - Use HKDF-SHA256 with the stored salt to re-derive the AES key
   - Decrypt ciphertext with the derived key + stored IV
4. **Backward compatibility**: Add an `EncryptedData::version` or check if salt is empty/all-zeros to detect v1 (direct key) vs v2 (HKDF) format. Or, since key_version=1 is already in use, bump key_version to 2 for HKDF-derived keys and support both in decrypt.

**If Option B (reserved):**

1. Add documentation/comments to `encryption.rs` and `EncryptedData` explaining that the salt is reserved for future KDF
2. Add a `// TODO(Phase B)` comment on the salt generation
3. No code behavior changes

## Acceptance Criteria

**If Option A (HKDF — recommended):**

- [ ] `hkdf` dependency added to `Cargo.toml`
- [ ] `encrypt()` uses HKDF-SHA256 with `salt + info = "alknet-encryption-v{key_version}"` to derive AES key
- [ ] `decrypt()` uses HKDF-SHA256 with stored `salt` to re-derive AES key
- [ ] `EncryptedData` with `key_version >= 2` uses HKDF
- [ ] `EncryptedData` with `key_version == 1` uses direct key (backward compat)
- [ ] Backward compatibility: data encrypted with v1 format can still be decrypted
- [ ] `CURRENT_KEY_VERSION` bumped to 2
- [ ] Unit test: encrypt/decrypt round-trip with HKDF (key_version 2)
- [ ] Unit test: decrypt v1-encrypted data (direct key) still works
- [ ] Unit test: different salts produce different ciphertext keys (even with same master key)
- [ ] `EncryptionKey` struct updated to carry HKDF info if needed

**If Option B (reserved):**

- [ ] `encryption.rs` has documentation explaining salt is reserved for future KDF
- [ ] `EncryptedData` struct has doc comment on `salt` field explaining reserved purpose
- [ ] `// TODO(Phase B)` comment on salt generation in `encrypt()`
- [ ] No behavior changes — existing tests pass unchanged

## References

- docs/architecture/secret-service.md — Encryption section (after spec update)
- crates/alknet-secret/src/encryption.rs — Current encrypt/decrypt implementation
- HKDF (RFC 5869): https://tools.ietf.org/html/rfc5869

## Notes

> My recommendation is Option A (HKDF). It's a small amount of additional code (the `hkdf` crate is tiny and well-tested), it makes the `salt` field purposeful, and it provides per-encryption key diversity. The backward compatibility concern is manageable: decrypt based on `key_version` (v1 = direct, v2 = HKDF).

> The architect's message specifically called out: "The EncryptedData struct has a salt field but the encryption function generates a random salt per encryption without using it for key derivation. Either the salt should be used in a KDF, or the field should be documented as reserved." This task resolves that ambiguity.

## Summary

> To be filled on completion