---
id: encryption-salt-kdf
name: Document EncryptedData salt as reserved for future KDF-based key derivation
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

The salt is stored but serves no purpose. The spec update (spec-update-secret-service) resolves this by documenting the salt as reserved for future KDF-based key rotation. In v1, the encryption key is derived directly from the seed at path `m/74'/2'/0'/0'` without a salt-based KDF. HKDF-based key derivation is deferred to Phase B.

**Decision: Option B — Document salt as reserved.** The spec update has already made this decision. This task implements Option B only.

## Implementation (Option B only)

1. Add documentation to `encryption.rs` explaining that the `salt` field in `EncryptedData` is reserved for future KDF-based key derivation (Phase B). In v1, the encryption key is derived directly from the seed at path `m/74'/2'/0'/0'` without using the salt.
2. Add a doc comment on the `EncryptedData.salt` field explaining its reserved purpose and that it is not used in v1 key derivation.
3. Add a `// TODO(Phase B): Use salt in HKDF-based key derivation` comment on the salt generation in `encrypt()`.
4. No code behavior changes — existing tests must pass unchanged.

## Acceptance Criteria

- [ ] `encryption.rs` module-level documentation explains that the salt field is reserved for future KDF-based key derivation
- [ ] `EncryptedData` struct has doc comment on `salt` field explaining reserved purpose and that it is not used in v1 key derivation
- [ ] `// TODO(Phase B)` comment on salt generation in `encrypt()`
- [ ] No behavior changes — all existing tests pass unchanged

## References

- docs/architecture/secret-service.md — Encryption section (after spec update, which specifies "salt is reserved for future KDF-based key rotation")
- crates/alknet-secret/src/encryption.rs — Current encrypt/decrypt implementation

## Notes

> The spec update task already decided on Option B. HKDF-based key derivation is deferred to Phase B. This task only documents the salt as reserved and adds TODO comments.

> The architect's message specifically called out: "The EncryptedData struct has a salt field but the encryption function generates a random salt per encryption without using it for key derivation. Either the salt should be used in a KDF, or the field should be documented as reserved." The spec update chose "document as reserved" for v1.

## Summary

> To be filled on completion