---
id: review-alknet-secret-spec-conformance
name: Review alknet-secret crate for spec conformance and prepare for Phase A integration
status: pending
depends_on: [spec-update-secret-service, derivedkey-zeroize-security, key-caching-ttl, irpc-secret-protocol-integration, derive-password-implementation, secp256k1-ethereum-derivation, encryption-salt-kdf, crypto-test-vectors]
scope: moderate
risk: low
impact: phase
level: review
---

## Description

Review the complete alknet-secret crate for spec conformance, security properties, and readiness for Phase A integration (connecting to alknet-storage via EncryptedData wire format and alknet-core via OperationEnv/irpc).

This review covers all tasks in the secret-service task group. It verifies that the implementation matches the updated spec and that the crate is safe and correct for production use.

### Review Checklist

1. **Spec conformance**: Does every module, type, and method in the implementation match what the updated `secret-service.md` specifies?
   - Mnemonic module matches spec (BIP39, Language, Seed, Zeroize)
   - Derivation module matches spec (SLIP-0010, path constants, ExtendedPrivKey)
   - Encryption module matches spec (AES-256-GCM, EncryptedData, salt purpose)
   - Protocol module matches spec (SecretProtocol, SecretMessage, DerivedKey with Zeroize)
   - Service module matches spec (SecretServiceHandle, SecretServiceActor, lifecycle)
   - Cache module matches spec (KeyCache, TTL, LRU, cleared on Lock)

2. **Security properties (ADR-038)**:
   - All sensitive types derive `Zeroize` with `#[zeroize(drop)]`
   - `DerivedKey.private_key` is zeroized on drop (not Clone)
   - `Seed` is zeroized on drop
   - `Mnemonic` is zeroized on drop
   - `CachedKey.private_key` is zeroized on drop
   - `EncryptionKey` is zeroized on drop
   - `Lock` zeroizes all cached keys and the seed
   - No private key material leaks through `Debug`, `Display`, or `Serialize`

3. **irpc integration**:
   - `SecretMessage` is properly defined with oneshot channels
   - `SecretServiceActor` processes all message variants
   - Local use via `SecretServiceHandle` still works without irpc overhead
   - The actor model doesn't introduce deadlocks or race conditions

4. **Test vectors**:
   - BIP39 test vectors pass
   - SLIP-0010 test vectors pass
   - AES-256-GCM test vectors pass
   - Cross-consistency (mnemonic → seed → key) pass

5. **Feature flag**:
   - `secp256k1` feature flag works correctly
   - Without the flag, `derive_ethereum_key()` returns `UnsupportedKeyType`
   - With the flag, BIP-0032 derivation produces correct secp256k1 keys

6. **No circular dependencies**:
   - `alknet-secret` does not depend on `alknet-core` or `alknet-storage`
   - Check `Cargo.toml` for any accidental dependencies

7. **Wire format compatibility**:
   - `EncryptedData` serialization matches the spec (key_version, salt, iv, data — all Base64)
   - `DerivedKey` serialization redacts `private_key`
   - `SecretProtocol` variants serialize correctly for irpc (postcard format)

8. **Public API completeness**:
   - All spec'd re-exports are present in `lib.rs`
   - `SecretService`, `SecretServiceHandle`, `SecretServiceActor`, `SecretMessage` all re-exported
   - `CacheConfig` is available for configuration
   - Feature-gated types (`Secp256k1` items) are behind the `secp256k1` flag

## Acceptance Criteria

- [ ] All tests pass: `cargo test -p alknet-secret --all-features`
- [ ] No compiler warnings: `cargo clippy -p alknet-secret --all-features`
- [ ] Formatting is clean: `cargo fmt -p alknet-secret -- --check`
- [ ] Every module, type, and method in the implementation matches the updated spec
- [ ] All sensitive types implement `Zeroize` with `#[zeroize(drop)]`
- [ ] `DerivedKey` is not `Clone` and private_key is zeroized
- [ ] `DerivedKey` serialization does not expose `private_key`
- [ ] Key cache works: TTL eviction, LRU eviction, cleared on Lock
- [ ] `SecretServiceActor` processes all `SecretMessage` variants correctly
- [ ] BIP39, SLIP-0010, and AES-256-GCM test vectors pass
- [ ] `secp256k1` feature flag gates Ethereum derivation correctly
- [ ] `alknet-secret` has no dependency on `alknet-core` or `alknet-storage`
- [ ] `EncryptedData` JSON serialization matches the spec format
- [ ] `SecretProtocol` / `SecretMessage` types are correctly structured for irpc
- [ ] Public API (`lib.rs` re-exports) matches the spec's crate interface section
- [ ] Documentation comments on all public items

## References

- docs/architecture/secret-service.md — Updated spec
- docs/architecture/decisions/038-seed-lifecycle-memory-security.md — ADR-038
- docs/architecture/decisions/027-crate-decomposition.md — ADR-027
- All implementation tasks in this task group

## Notes

> This review should be thorough but should not block on minor documentation phrasing. Focus on: security properties (zeroize), spec conformance (does the code match what the spec says), and integration readiness (can Phase A wire this crate to alknet-core and alknet-storage?).

> If any deviations between spec and implementation are found, document them and create follow-up tasks rather than fixing them during review.

## Summary

> To be filled on completion