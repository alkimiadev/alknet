---
status: draft
last_updated: 2026-06-19
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
    pub salt: String,      // base64, 32 bytes — reserved for Phase B (see OQ-20)
    pub iv: String,        // base64, 12 bytes — AES-GCM nonce
    pub data: String,      // base64 — ciphertext + auth tag
}
```

All binary fields are base64-encoded as strings for JSON serialization
compatibility. The `iv` is 12 bytes (the standard GCM nonce size). The
`data` field includes the GCM authentication tag appended to the ciphertext
(the `aes-gcm` crate handles this).

### Salt field (reserved for Phase B)

The `salt` field is **reserved for future KDF-based key derivation** (Phase
B, OQ-20). In v1, the encryption key is derived directly from the seed at
path `m/74'/2'/0'/0'` **without using the salt**. The salt is generated
randomly (32 bytes) and stored in `EncryptedData.salt` for forward
compatibility, but it plays no role in the v1 key derivation process.

When key rotation is implemented in Phase B, the salt will be used as
input to HKDF or PBKDF2 for stretch-based key derivation, allowing the
same seed to produce different encryption keys without changing the
derivation path. The wire format does not need to change — the `salt`
field is already present and populated.

This is a deliberate forward-compatibility decision: the field exists in
v1 so that v2 can use it without a format migration. The cost is 32 extra
bytes per `EncryptedData`; the benefit is no future format break.

## Encrypt and Decrypt

```rust
pub fn encrypt(plaintext: &str, key: &EncryptionKey) -> Result<EncryptedData, EncryptionError>;
pub fn decrypt(encrypted: &EncryptedData, key: &EncryptionKey) -> Result<String, EncryptionError>;
```

`encrypt`:
1. Generates a random 12-byte IV (must use `OsRng` — see Security Constraints)
2. Generates a random 32-byte salt (stored, not used in v1)
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

`CURRENT_KEY_VERSION` is `1`. Key versioning allows re-encryption when the
encryption key is rotated:

1. Derive a new key from a new derivation path or new seed
2. Decrypt all existing `EncryptedData` with key version 1
3. Re-encrypt with key version 2
4. Update storage

The key version is stored in `EncryptedData.key_version` so decryption can
select the right key. The rotation workflow itself is not specced — see
OQ-22.

## Errors

```rust
pub enum EncryptionError {
    Encryption(String),       // encryption failed
    Decryption(String),       // decryption failed (wrong key, tampered data, bad UTF-8)
    Decoding(String),         // base64 decoding failed
    KeyVersionMismatch { expected: u32, actual: u32 },  // reserved for Phase B
}
```

Decryption failures are intentionally generic — they don't distinguish
"wrong key" from "tampered data" from "corrupted storage" to avoid
leaking information to an attacker.

`KeyVersionMismatch` is **defined but unused in v1** — neither `encrypt()`
nor `decrypt()` returns it. It is reserved for Phase B key rotation (OQ-22),
where the vault may enforce version matching before decrypting. In v1, the
`key_version` is stamped onto `EncryptedData` and `EncryptionKey` for
forward compatibility but does not gate decryption. An implementer should
not expect this variant to fire in v1.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| AES-256-GCM for credential encryption | — | Authenticated encryption, hardware-accelerated |
| Salt reserved for Phase B (OQ-20) | — | Forward-compatible wire format; v1 doesn't use salt |
| Key derived at `m/74'/2'/0'/0'` | — | Dedicated account for encryption keys |
| Key versioning | — | Rotation support without format break |
| All fields base64-encoded | — | JSON serialization compatibility |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-20** (open): Salt/KDF Phase B — when and how to use the reserved
  `salt` field for KDF-based key derivation.
- **OQ-22** (open): Key rotation mechanism — the key versioning is in place,
  but the rotation workflow (re-encrypt all data, update storage) is not
  specced.

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