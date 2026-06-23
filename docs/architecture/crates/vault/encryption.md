---
status: draft
last_updated: 2026-06-20
---

# Encryption

AES-256-GCM encryption and decryption for external credentials that cannot
be derived from the seed.

## What

External credentials (API keys, OAuth tokens, signing keys obtained from
third parties) cannot be derived from the BIP39 seed — they're arbitrary
bytes, not deterministic functions of the seed. The vault encrypts these
with a key *derived from* the seed, producing an `EncryptedData` blob that
can be stored outside the vault (in a config file, a database, or external
storage) and decrypted later with the same seed.

This is the second axis of the vault's secret model:

| Axis | Source | Mechanism | Example |
|------|--------|-----------|---------|
| Derived keys | Seed → HD derivation | Deterministic | Node identity, SSH host key |
| Encrypted credentials | External → AES-256-GCM | Seed-derived key | Google API key, OAuth token |

## Why AES-256-GCM

AES-256-GCM is an authenticated encryption scheme — it provides both
confidentiality (encryption) and integrity (authentication tag). A
tampered ciphertext fails decryption. This is the correct mode for
credential storage: if an attacker modifies an encrypted API key in
storage, decryption fails rather than producing a different (potentially
dangerous) plaintext.

GCM is also hardware-accelerated on modern CPUs (AES-NI), making it fast
enough that encryption is never a bottleneck.

## Key Derivation: HD, Not PBKDF2

The encryption key is derived from the BIP39 seed via SLIP-0010 HD
derivation at path `m/74'/2'/0'/0'` (`PATHS::ENCRYPTION`). This is a
deliberate choice over the PBKDF2 approach used by the TypeScript
predecessor (`@alkdev/storage/src/graphs/crypto.ts`). See ADR-020 for the
full rationale.

| Aspect | TS predecessor (PBKDF2) | Vault (HD derivation) |
|--------|--------------------------|----------------------|
| Secret input | Password (user-provided) | BIP39 seed (64 bytes) |
| Salt role | Load-bearing — part of key derivation | Unused — stored for wire-format compat |
| Derivation | PBKDF2 (100k iterations) | SLIP-0010 (a few HMACs) |
| Speed | Intentionally slow | Instant |
| Reproducible | Only with exact password | Deterministic from mnemonic |
| key_version | 1 | 2 |

Data encrypted by the TS implementation (PBKDF2, key_version=1) **cannot be
decrypted by the vault** — the keys are different even if the password
equals the mnemonic. Migration is a one-time re-encryption (see ADR-020).

## Encryption Key

The encryption key is derived from the seed at path `m/74'/2'/0'/0'`
(`PATHS::ENCRYPTION`):

```rust
pub struct EncryptionKey {
    key_bytes: [u8; 32],   // 32-byte AES-256 key
    key_version: u32,      // for rotation tracking
}
```

- `new(key_bytes, key_version)`: Construct from raw bytes.
- `from_derived_bytes(bytes, key_version)`: Take the first 32 bytes of
  derived key material (the private key bytes from SLIP-0010 derivation).
- `version()`: Return the key version (for rotation).

`EncryptionKey` implements `Zeroize` and `ZeroizeOnDrop` — the key bytes
are zeroized before deallocation.

The key is derived once (at unlock time or on first encrypt/decrypt) and
cached in the `KeyCache` (see [service.md](service.md)). Subsequent
encrypt/decrypt operations use the cached key.

## EncryptedData

The encrypted blob format. This is the **stable wire format** shared with
`alknet-storage` (a future crate) by type-level agreement, not by a crate
dependency. Both crates must agree on the serialization format.

A TypeScript `EncryptedDataSchema` from the `@alkdev/storage` library
predates the Rust implementation. The Rust `EncryptedData` is a superset
of the TypeScript schema. The migration path is: re-encrypt
TypeScript-encrypted data using the Rust vault with a new key version.
This cross-language compatibility is why the wire format must stay stable —
changing it breaks both `alknet-storage` and the TypeScript consumer.

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EncryptedData {
    pub key_version: u32,  // rotation tracking
    pub salt: String,      // base64, 32 bytes — unused in v2 (wire-format compat, see ADR-020)
    pub iv: String,        // base64, 12 bytes — AES-GCM nonce
    pub data: String,      // base64 — ciphertext + auth tag
}
```

All binary fields are base64-encoded as strings for JSON serialization
compatibility. The `iv` is 12 bytes (the standard GCM nonce size). The
`data` field includes the GCM authentication tag appended to the ciphertext
(the `aes-gcm` crate handles this).

### Salt field (unused in v2 — reserved for future KDF)

The `salt` field is **unused for key derivation in v2** (HD derivation
doesn't need a salt — the derivation path provides domain separation). The
salt is generated randomly (32 bytes) and stored for wire-format
compatibility with the TypeScript `EncryptedDataSchema`, but it plays no
cryptographic role.

In the TypeScript predecessor, the salt was load-bearing — it was part of
the PBKDF2 key derivation. The vault's HD derivation doesn't use it, but the
field is kept in the wire format so the struct doesn't need to change if a
future KDF-based derivation is added.

If KDF-based key derivation is ever implemented (using HKDF or PBKDF2 with
the salt as input), it would be a new `key_version` and would not affect
existing v2 data. This is additive — see OQ-22 (key rotation) and ADR-020
(HD derivation decision).

## Encrypt and Decrypt

```rust
pub fn encrypt(plaintext: &str, key: &EncryptionKey) -> Result<EncryptedData, EncryptionError>;
pub fn decrypt(encrypted: &EncryptedData, key: &EncryptionKey) -> Result<String, EncryptionError>;
```

`encrypt`:
1. Generates a random 12-byte IV (must use `OsRng` — see Security Constraints)
2. Generates a random 32-byte salt (stored for wire-format compat, unused in key derivation)
3. Encrypts the plaintext with AES-256-GCM
4. Returns `EncryptedData { key_version, salt, iv, data }`

`decrypt`:
1. Decodes the base64 IV and ciphertext
2. Decrypts with AES-256-GCM (verifies the auth tag)
3. Returns the plaintext string

The IV is generated fresh for each encryption call. **IV reuse under the
same key is catastrophic for GCM** (authenticity breaks, two-time-pad on
plaintext). The use of `OsRng` for IV generation is a security-critical
constraint — see below.

## Key Versioning

`CURRENT_KEY_VERSION` is `2`. Version `1` is reserved for the TypeScript
predecessor's PBKDF2-encrypted data (see ADR-020). Each version maps to a
unique derivation path — the last hardened index is the version offset
(see ADR-021):

```
v2: m/74'/2'/0'/0'    ← PATHS::ENCRYPTION (current)
v3: m/74'/2'/0'/1'
v4: m/74'/2'/0'/2'
```

`encrypt` stamps the version onto new blobs. `decrypt` derives the key at
the path indicated by `encrypted.key_version` — each version has its own
cryptographically independent key. Old version keys remain derivable (the
seed doesn't change), so partial rotation is safe.

### Rotation

Key rotation re-encrypts a blob from one version to another. The vault
provides a `rotate` method; the caller (assembly layer or migration tool)
handles replacing the blob in storage:

```rust
pub fn rotate(&self, encrypted: &EncryptedData, to_version: u32) -> Result<EncryptedData, VaultServiceError>;
```

Rotation decrypts with the old version's key and re-encrypts with the new
version's key. No new mnemonic needed — the same seed produces all version
keys via different paths. See ADR-021 for the full mechanism.

**The current source uses `CURRENT_KEY_VERSION = 1` with HD derivation and
does not implement version-indexed paths or `rotate`.** These are drift
items to be corrected during implementation sync. See ADR-020 (version
bump to 2) and ADR-021 (rotation mechanism).

## Errors

```rust
pub enum EncryptionError {
    Encryption(String),       // encryption failed
    Decryption(String),       // decryption failed (wrong key, tampered data, bad UTF-8)
    Decoding(String),         // base64 decoding failed
    KeyVersionMismatch { expected: u32, actual: u32 },  // unused — see note below
}
```

Decryption failures are intentionally generic — they don't distinguish
"wrong key" from "tampered data" from "corrupted storage" to avoid
leaking information to an attacker.

`KeyVersionMismatch` is **defined but unused.** ADR-021 implements key
rotation via version-indexed derivation paths — `decrypt` derives the key
at the path indicated by `encrypted.key_version`, so there is no
version-mismatch to detect at the error level (every blob carries its own
version, and every version has a derivable key). This variant predates
ADR-021's rotation mechanism and is retained in the enum for source
compatibility but is not emitted by any code path in v2. An implementer
should not wire it up or expect it to fire. If a future use case requires
enforcing version constraints (e.g., "refuse to decrypt blobs older than
v3"), this variant could be repurposed — but that would be a new decision,
not part of ADR-021's rotation scheme.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| AES-256-GCM for credential encryption | [ADR-026](../../decisions/026-vault-key-model-hd-derivation.md) | Authenticated encryption, hardware-accelerated |
| HD derivation, not PBKDF2 | [ADR-020](../../decisions/020-hd-derivation-for-encryption-keys.md) | Seed-derived key; no password; deterministic |
| Salt unused in v2 (wire-format compat) | [ADR-020](../../decisions/020-hd-derivation-for-encryption-keys.md) | Kept for TS compat; not used in key derivation |
| Key derived at `m/74'/2'/0'/0'` | [ADR-026](../../decisions/026-vault-key-model-hd-derivation.md) | Dedicated account for encryption keys |
| Version-indexed paths for rotation | [ADR-021](../../decisions/021-key-rotation-via-version-indexed-paths.md) | `m/74'/2'/0'/{version-2}'` |
| Key versioning (v1=TS PBKDF2, v2=vault HD) | [ADR-020](../../decisions/020-hd-derivation-for-encryption-keys.md) | Distinguishes derivation methods |
| All fields base64-encoded | — | JSON serialization compatibility |
| `EncryptedData` wire format frozen | [ADR-018](../../decisions/018-vault-standalone-crate.md) | Fields, encoding, semantics locked; no removal without migration |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-20** (resolved by ADR-020): Salt/KDF — HD derivation is the method;
  the salt field is unused in v2 (wire-format compatibility only).
- **OQ-22** (resolved by ADR-021): Key rotation — version-indexed paths;
  `rotate` method decrypts old, re-encrypts new.

## Security Constraints

These are security-critical implementation requirements.

- **OsRng for IVs**: The IV must be generated with `OsRng` (or an
  equivalent CSPRNG), never `rand::random()`. IV reuse under the same key
  is catastrophic for GCM — it breaks authenticity and creates a
  two-time-pad on the plaintext. **The current source uses
  `rand::random()` for IV generation (`encryption.rs` line 133) — this is a
  known drift from the spec and must be corrected during implementation
  sync.** `rand::random()` uses the thread-local RNG which may not be a
  CSPRNG on all platforms; `OsRng` reads from the operating system's
  entropy source and is the correct choice for cryptographic nonces.
- **Zeroized drop**: `EncryptionKey` derives `Zeroize` and
  `ZeroizeOnDrop`. The key bytes are zeroized before deallocation. Do not
  store key material in types that don't zeroize.
- **No plaintext in logs**: `EncryptedData` is safe to log (it's
  ciphertext). The plaintext and the `EncryptionKey` are not. Do not add
  `Debug` or `Display` implementations that print key bytes or plaintext.

## References

- [NIST SP 800-38D](https://nvlpubs.nist.gov/nistpubs/Legacy/SP/nistspecialpublication800-38d.pdf) —
  AES-GCM specification
- Implementation: `crates/alknet-vault/src/encryption.rs`
- Tests: `crates/alknet-vault/tests/test_vectors.rs`,
  `crates/alknet-vault/src/encryption.rs` (unit tests)
- [service.md](service.md) — how the vault caches the encryption key