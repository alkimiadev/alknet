---
id: derive-password-implementation
name: Implement deterministic password derivation for DerivePassword
status: pending
depends_on: [spec-update-secret-service, derivedkey-zeroize-security]
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

The `SecretProtocol::DerivePassword` variant exists in the protocol enum but has no corresponding service method. The spec (after update) defines deterministic password derivation as:

- **Algorithm**: HMAC-SHA512 at the derivation path `m/74'/1'/0'/{hash}'` where `{hash}'` is a site-specific hardened index
- **Output**: Truncate the derived key material to `length` bytes, encode as Base64url (URL-safe Base64 without padding)
- **Path format**: `m/74'/1'/0'/{hash}'` — SLIP-0010 hardened-only derivation

The current `SecretServiceHandle` has methods for `DeriveEd25519`, `DeriveEncryptionKey`, `DeriveEthereumKey`, `Encrypt`, and `Decrypt`, but no `derive_password`.

**Implementation:**

1. Add `SecretServiceHandle::derive_password(&self, path: &str, length: usize) -> Result<Vec<u8>, SecretServiceError>`

2. The implementation:
   - Derive a key at the given path using SLIP-0010 (same as Ed25519 derivation)
   - Take the first `length` bytes of the private key material
   - Encode as Base64url (no padding) using `base64::engine::general_purpose::URL_SAFE_NO_PAD`
   - The output is a deterministic password string that can be regenerated from the same seed + path

   **Wait** — there's a design choice here. Should `DerivePassword` return raw bytes (`Vec<u8>`) or an encoded string (`String`)? The spec says the protocol variant returns `Vec<u8>`, but a "password" is typically a string. Let me check the protocol definition more carefully.

   The protocol says:
   ```rust
   #[rpc(tx=oneshot::Sender<Vec<u8>>)]
   #[wrap(DerivePassword)]
   DerivePassword { path: String, length: usize },
   ```

   So the return type is `Vec<u8>`. The encoding to a usable password string should happen at the call site or be a separate method. For the protocol, return raw derived bytes.

   **Resolution**: `derive_password()` returns `Vec<u8>` (raw derived bytes). A convenience method `derive_password_string(path: &str, length: usize) -> Result<String, SecretServiceError>` returns the Base64url-encoded string for use as an actual password. The protocol variant returns `Vec<u8>`.

3. Add `SecretProtocol::DerivePassword` dispatch to the service actor (irpc task depends on `irpc-secret-protocol-integration`).

4. The `derive_password` method in `SecretServiceHandle`:
   ```rust
   pub fn derive_password(&self, path: &str, length: usize) -> Result<Vec<u8>, SecretServiceError> {
       let inner = self.inner.read().unwrap();
       if !inner.unlocked {
           return Err(SecretServiceError::ServiceLocked);
       }
       let seed = inner.seed.as_ref().ok_or(SecretServiceError::ServiceLocked)?;
       let key = derivation::derive_path_from_seed(seed.as_bytes(), path)?;
       // Return first `length` bytes of private key
       let result = key.private_key()[..length.min(key.private_key().len())].to_vec();
       Ok(result)
   }
   ```

## Acceptance Criteria

- [ ] `SecretServiceHandle::derive_password(&self, path: &str, length: usize) -> Result<Vec<u8>, SecretServiceError>` method added
- [ ] `derive_password` returns the first `length` bytes of the derived private key at the given path
- [ ] `derive_password` requires unlocked state (returns `ServiceLocked` if locked)
- [ ] `SecretServiceHandle::derive_password_string(&self, path: &str, length: usize) -> Result<String, SecretServiceError>` convenience method returns Base64url-encoded string
- [ ] `derive_password` uses the key cache (if `key-caching-ttl` task is complete)
- [ ] Unit test: `derive_password` at a known path returns deterministic bytes
- [ ] Unit test: `derive_password` at the same path returns the same bytes (deterministic)
- [ ] Unit test: `derive_password` at a different path returns different bytes
- [ ] Unit test: `derive_password` length parameter truncates correctly
- [ ] Unit test: `derive_password_string` returns valid Base64url (no padding)
- [ ] Unit test: `derive_password` returns `ServiceLocked` error when service is locked

## References

- docs/architecture/secret-service.md — Key derivation, password derivation path
- crates/alknet-secret/src/service.rs — SecretServiceHandle (add derive_password)
- crates/alknet-secret/src/protocol.rs — SecretProtocol::DerivePassword variant
- crates/alknet-secret/src/derivation.rs — derive_path_from_seed, PATHS

## Notes

> The `length` parameter in `DerivePassword` specifies bytes, not characters. Since Ed25519 derived keys are 32 bytes, the maximum useful length is 32. For longer passwords, the spec says to use Base64url encoding of the full 32 bytes, which gives a ~43-character string. Password managers typically want 16-32 byte keys encoded as ~22-43 character strings.

> The `PATHS::DEVICE_PREFIX` pattern (`m/74'/0'/0'`) allows parameterized device identity. Similarly, `site_password_path(hash)` (`m/74'/1'/0'/{hash}'`) allows site-specific passwords. Both work with the same derivation function — the path is just different.

## Summary

> To be filled on completion