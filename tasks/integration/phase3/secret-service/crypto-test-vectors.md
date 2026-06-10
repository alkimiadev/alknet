---
id: crypto-test-vectors
name: Add BIP39 and SLIP-0010 known-answer test vectors for derivation correctness
status: completed
depends_on: [spec-update-secret-service]
scope: moderate
risk: low
impact: component
level: implementation
---

## Description

A crypto crate needs known-answer tests against published test vectors. The current test suite tests functional behavior (round-trip, determinism, error conditions) but not against published reference vectors. This means we don't know for certain that our BIP39 seed derivation or SLIP-0010 key derivation matches the standard.

**Required test vectors:**

### 1. BIP39 Test Vectors

From https://github.com/trezor/python-mnemonic/blob/master/vectors.json or the BIP39 reference:

- **Mnemonic → Seed**: Given a known mnemonic phrase and optional passphrase, the derived seed must match the published hex value byte-for-byte.
- Test with both 12-word and 24-word mnemonics
- Test with and without passphrase
- The `bip39` crate may already have these internally; verify with external reference

### 2. SLIP-0010 Test Vectors

From https://github.com/satoshilabs/slips/blob/master/slip-0010.md:

- **Seed → Master Key**: Given a known seed, the HMAC-SHA512 master key must match the published test vector.
- **Master Key → Child Key (Ed25519)**: Given the master key and known derivation indices, the derived child key must match the published test vector.
- **Path Derivation**: Given a known seed and path `m/74'/0'/0'/0'`, the final key must be deterministic and verified against the chain of individual derivation steps.
- SLIP-0010 test vector 1 uses the path `m/0h/1h/2h` with known seed, producing known keys at each step.

### 3. AES-256-GCM Test Vectors

From NIST SP 800-38D or equivalent:

- **Known key + known IV + known plaintext → known ciphertext**: Verify our AES-256-GCM implementation produces the expected ciphertext and tag.
- This is more of a sanity check since we use the `aes-gcm` crate which is well-tested, but it ensures our key handling (derived key → AES key) is correct.

### 4. Cross-consistency Tests

- **Mnemonic → Seed → Master Key → Derived Key at known path**: End-to-end test that starts with a known mnemonic, derives the seed, then derives keys at known paths, and verifies the result at each step.
- **Different mnemonics produce different keys**: Verify that two different mnemonics produce different keys at the same path (no accidental collisions).

**Implementation:**

Add a `tests/test_vectors.rs` integration test file. Test vectors are hardcoded hex strings verified against the published references.

For SLIP-0010 specifically, use the official test vectors from the SLIP-0010 specification. The SLIP-0010 spec provides:
- Test vector 1: seed → master key, then child derivations at `m/0h`, `m/0h/1h`, `m/0h/1h/2h`
- These use the "ed25519 seed" HMAC key and hardened-only derivation

For alknet-specific paths (`m/74'/0'/0'/0'`, etc.), generate test vectors once with a known mnemonic and commit the expected results. These serve as regression tests.

## Acceptance Criteria

- [ ] `tests/test_vectors.rs` file added with BIP39 known-answer test vectors
- [ ] BIP39 test: known mnemonic + known passphrase → known seed (hex comparison)
- [ ] BIP39 test: known mnemonic + no passphrase → known seed (hex comparison)
- [ ] BIP39 test: different mnemonics produce different seeds
- [ ] SLIP-0010 test: known seed → known master key (hex comparison against SLIP-0010 spec vector)
- [ ] SLIP-0010 test: master key → derived child key at `m/0h` (matches SLIP-0010 test vector)
- [ ] SLIP-0010 test: master key → derived child key at `m/0h/1h/2h` (matches SLIP-0010 test vector)
- [ ] AES-256-GCM test: known key + known IV + known plaintext → known ciphertext (NIST vector or equivalent)
- [ ] Cross-consistency test: end-to-end mnemonic → seed → derived key at `m/74'/0'/0'/0'`
- [ ] All test vectors pass: `cargo test -p alknet-secret --test test_vectors`
- [ ] Test vectors reference their source (URL or specification section) in comments

## References

- docs/architecture/secret-service.md — Test vectors section (after spec update)
- SLIP-0010: https://github.com/satoshilabs/slips/blob/master/slip-0010.md
- BIP39: https://github.com/bitcoin/bips/blob/master/bip-0039.mediawiki
- BIP39 test vectors: https://github.com/trezor/python-mnemonic/blob/master/vectors.json
- AES-256-GCM: NIST SP 800-38D
- crates/alknet-secret/src/mnemonic.rs — BIP39 implementation
- crates/alknet-secret/src/derivation.rs — SLIP-0010 implementation
- crates/alknet-secret/src/encryption.rs — AES-256-GCM implementation

## Notes

> The `bip39` and `ed25519-bip32` crates may have their own internal test vectors. Our tests verify our *wrapper* code (Mnemonic, Seed, ExtendedPrivKey) produces the same results. If the underlying crates have known-answer tests, we can reference them but should still have our own integration tests that exercise the full stack.

> For alknet-specific paths (`m/74'/...`), there are no published test vectors (74' is reserved for alknet). We generate our own "known-answer" vectors from a fixed mnemonic and commit the expected hex values as regression tests.

> Use `hex` crate (already in dev-dependencies) for encoding/decoding test vector bytes.

## Summary

> To be filled on completion