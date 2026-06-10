---
id: secp256k1-ethereum-derivation
name: Add BIP-0032 secp256k1 derivation for Ethereum keys behind feature flag
status: completed
depends_on: [spec-update-secret-service, derivedkey-zeroize-security]
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

The `SecretProtocol::DeriveEthereumKey` variant exists in the protocol and `SecretServiceHandle::derive_ethereum_key()` exists in the service, but the current implementation uses the same SLIP-0010 Ed25519 derivation for all key types. This is incorrect.

BIP-0032 secp256k1 derivation (used for Ethereum at path `m/44'/60'/0'/0/0`) is fundamentally different from SLIP-0010 Ed25519:

- **SLIP-0010** uses hardened-only derivation with HMAC-SHA512, producing Ed25519 keys
- **BIP-0032** supports both hardened and unhardened derivation for secp256k1 (the Ethereum path has unhardened indices: `/0/0` at the end)
- The master key derivation algorithm is different (HMAC-SHA512 with "Bitcoin seed" vs "ed25519 seed")
- The key format is different (secp256k1 private key + compressed public key)

The Ethereum path `m/44'/60'/0'/0/0` has **unhardened** indices at positions 4 and 5 (the last two `0`s), which SLIP-0010 does not support (SLIP-0010 requires all indices to be hardened).

**Implementation:**

1. Add the `secp256k1` crate (Rust bindings to libsecp256k1) as an optional dependency behind a `secp256k1` feature flag:

   ```toml
   [features]
   secp256k1 = ["dep:secp256k1"]

   [dependencies]
   secp256k1 = { version = "0.29", optional = true }
   ```

   **Note**: The Rust crate is named `secp256k1` on crates.io (it wraps the C library `libsecp256k1`). Do not use `libsecp256k1` — that is the C library name, not the Rust crate name.

2. Add a `ethereum.rs` module (behind `secp256k1` feature flag) that implements BIP-0032 secp256k1 derivation:
   - `derive_secp256k1_master_key(seed: &[u8]) -> Result<Secp256k1ExtendedKey, DerivationError>`
   - `derive_secp256k1_path(seed: &[u8], path: &str]) -> Result<ExtendedPrivKey, DerivationError>`
   - This uses HMAC-SHA512 with key "Bitcoin seed" (different from SLIP-0010's "ed25519 seed")
   - Supports both hardened (≥ 0x80000000) and unhardened indices

3. Update `SecretServiceHandle::derive_ethereum_key()` (behind `secp256k1` feature flag):
   - When feature is enabled, use BIP-0032 derivation for paths starting with `m/44'`
   - Return `DerivedKey { key_type: KeyType::Secp256k1, private_key, public_key }` where public_key is compressed (33 bytes)
   - When feature is NOT enabled, return `SecretServiceError::UnsupportedKeyType`

4. Update `DerivationError` to include `Secp256k1(String)` and `UnsupportedKeyType` variants.

5. The `parse_derivation_path` function in `derivation.rs` already supports unhardened indices (it correctly parses `/0/0` at the end). The dispatch to SLIP-0010 vs BIP-0032 should be based on the path's coin type or an explicit parameter, not automatic path detection. Use an explicit method: `derive_ethereum_key()` always uses BIP-0032.

**Current bug**: The existing `derive_ethereum_key` method calls `derive_path_from_seed` which uses SLIP-0010 (Ed25519 only). The path `m/44'/60'/0'/0/0` has unhardened indices that SLIP-0010 doesn't handle correctly. This must be fixed.

## Acceptance Criteria

- [ ] `secp256k1` crate (Rust bindings to libsecp256k1) added behind `secp256k1` feature flag in `Cargo.toml`
- [ ] `ethereum.rs` module added (behind `secp256k1` feature flag) with BIP-0032 secp256k1 derivation
- [ ] `derive_secp256k1_master_key()` uses HMAC-SHA512 with "Bitcoin seed" key
- [ ] `derive_secp256k1_path()` supports both hardened and unhardened indices
- [ ] `SecretServiceHandle::derive_ethereum_key()` uses BIP-0032 derivation when `secp256k1` feature is enabled
- [ ] `SecretServiceHandle::derive_ethereum_key()` returns `UnsupportedKeyType` error when `secp256k1` feature is disabled
- [ ] `SecretServiceHandle::derive_ed25519()` still uses SLIP-0010 (unchanged behavior)
- [ ] `SecretServiceHandle::derive_encryption_key()` still uses SLIP-0010 (unchanged behavior)
- [ ] `derive_ethereum_key()` returns `DerivedKey { key_type: Secp256k1, private_key: 32-byte-secp256k1-key, public_key: 33-byte-compressed-point }`
- [ ] `DerivationError` gains `Secp256k1(String)` and `UnsupportedKeyType` variants
- [ ] `lib.rs` conditionally exports `ethereum` module and `Secp256k1`-related types
- [ ] Unit test: Ethereum derivation at path `m/44'/60'/0'/0/0` produces a valid secp256k1 keypair (behind `secp256k1` feature)
- [ ] Unit test: Ethereum derivation produces different keys from Ed25519 derivation at the same seed
- [ ] Unit test: Calling `derive_ethereum_key()` without `secp256k1` feature returns `UnsupportedKeyType`
- [ ] Existing Ed25519 and encryption key derivation tests still pass
- [ ] BIP-0032 known test vector: known seed → known secp256k1 master key (if available)

## References

- docs/architecture/secret-service.md — secp256k1 derivation note (after spec update)
- crates/alknet-secret/src/derivation.rs — Current SLIP-0010 only derivation
- crates/alknet-secret/src/service.rs — derive_ethereum_key (currently uses SLIP-0010, needs fix)
- BIP-0032: https://github.com/bitcoin/bips/blob/master/bip-0032.mediawiki
- Ethereum path EIP-84: `m/44'/60'/0'/0/0`

## Notes

> This task should be done after `derivedkey-zeroize-security` since `derive_ethereum_key` returns `DerivedKey` which will have the zeroize changes applied.

> The `secp256k1` crate on crates.io (version 0.29+) provides Rust bindings to the C library `libsecp256k1`. The `ed25519-bip32` crate handles SLIP-0010 Ed25519. These are different algorithms and must not be mixed.

> For the Ethereum public key: BIP-0032 secp256k1 produces 33-byte compressed public keys. The `public_key` field in `DerivedKey` is `Vec<u8>`, so 33 bytes is fine. Document this size difference from Ed25519 (32-byte public keys).

> Consider whether `derive_path_from_seed` should be renamed to `derive_ed25519_path_from_seed` for clarity, since it's specifically SLIP-0010 Ed25519. The public API (`derive_ed25519`, `derive_encryption_key`, `derive_ethereum_key`) already makes this distinction, but the internal function name could be clearer.

## Summary

> To be filled on completion