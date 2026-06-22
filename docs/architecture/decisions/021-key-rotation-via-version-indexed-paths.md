# ADR-021: Key Rotation via Version-Indexed Derivation Paths

## Status

Accepted

## Context

ADR-020 established that the vault derives the AES-256-GCM encryption key
from the BIP39 seed via SLIP-0010 HD derivation at path `m/74'/2'/0'/0'`.
The `EncryptedData.key_version` field exists for rotation tracking, but
the current implementation always derives at the same path regardless of
version — `key_version` is metadata, not a functional selector.

OQ-22 asked: how does key rotation work? The key versioning is in place,
but the rotation mechanism — how a new key is derived, how existing data
is re-encrypted, and how the vault selects the right key for decryption —
is not specified.

### Why rotation matters

Key rotation is a fundamental security hygiene practice. The scenarios
that require it:

1. **Suspected key compromise**: the encryption key may have leaked
   (memory dump, process compromise, log accident). All data encrypted
   with that key must be re-encrypted with a new key.
2. **Periodic rotation**: security policy mandates key rotation every N
   months. The vault must support this without re-deriving from a new
   mnemonic (which would require re-deploying all nodes).
3. **Version transition**: moving from TS PBKDF2 data (v1) to vault HD
   data (v2, per ADR-020) is itself a rotation. The mechanism should
   generalize — it's the same operation.

### What "rotation" means concretely

Rotating from key version N to N+1:

1. Derive a new encryption key at a new derivation path
2. For each existing `EncryptedData` blob with `key_version: N`:
   - Decrypt with the v-N key
   - Re-encrypt the plaintext with the v-(N+1) key
   - Replace the blob in storage with `key_version: N+1`
3. New encryptions use `key_version: N+1`
4. Old keys remain available for decrypting any data that hasn't been
   rotated yet (partial rotation is safe)

The question is: **how is the new key derived?** The options:

- **Option A: New derivation path per version.** `m/74'/2'/0'/0'` for v2,
  `m/74'/2'/0'/1'` for v3, etc. Each version gets its own HD key. No
  new seed needed.
- **Option B: New mnemonic (new seed).** Generate a new mnemonic, unlock
  with it, re-encrypt everything. This is heavy — it changes *all* derived
  keys (identity, SSH host, etc.), not just the encryption key.
- **Option C: KDF from the existing key.** Use HKDF or PBKDF2 with the
  existing derived key + the salt as input. This is the salt field's
  potential use (OQ-20 mentioned this), but it adds KDF complexity and
  the salt becomes load-bearing.

## Decision

### 1. Version-indexed derivation paths

Each key version maps to a unique derivation path. The last hardened index
in the encryption path is the key version:

```
v2: m/74'/2'/0'/0'    ← PATHS::ENCRYPTION (current)
v3: m/74'/2'/0'/1'
v4: m/74'/2'/0'/2'
...
```

The `encryption_path_for_version(version)` function constructs the path:

```rust
pub fn encryption_path_for_version(version: u32) -> String {
    // v1 is the TS PBKDF2 legacy — not an HD path. The vault starts at v2.
    // v2 → m/74'/2'/0'/0', v3 → m/74'/2'/0'/1', etc.
    let index = version.saturating_sub(2);
    format!("m/74'/2'/0'/{}'", index)
}
```

`PATHS::ENCRYPTION` remains `m/74'/2'/0'/0'` — it's the v2 path, and v2
is the current version. When the vault is rotated to v3,
`encryption_path_for_version(3)` produces `m/74'/2'/0'/1'`.

This means:
- No new mnemonic needed — rotation uses the same seed, different path
- Each version's key is cryptographically independent (HD derivation
  ensures this)
- The derivation path is self-documenting (`m/74'/2'/0'/1'` is clearly
  "encryption key, version 3")
- Old keys are always derivable (the seed doesn't change), so partial
  rotation is safe — the vault can decrypt any version

### 2. `encrypt_key(version)` and `decrypt_key(version)` methods

The `VaultServiceHandle` gains version-aware key derivation:

```rust
impl VaultServiceHandle {
    /// Derive the encryption key for the given version. Cached.
    fn derive_encryption_key_for_version(
        &self,
        version: u32,
    ) -> Result<EncryptionKey, VaultServiceError> {
        let path = encryption_path_for_version(version);
        // ... derive at path, cache by path ...
    }

    /// Encrypt with the current key version.
    pub fn encrypt(&self, plaintext: &str, key_version: u32) -> Result<EncryptedData, VaultServiceError>;

    /// Decrypt by deriving the key at the version indicated by the blob.
    pub fn decrypt(&self, encrypted: &EncryptedData) -> Result<String, VaultServiceError> {
        let key = self.derive_encryption_key_for_version(encrypted.key_version)?;
        encryption::decrypt(encrypted, &key)
    }
}
```

`decrypt` now derives the key at the path **indicated by
`encrypted.key_version`** — not always at `PATHS::ENCRYPTION`. This corrects
a source drift: the current source ignores `key_version` for key selection;
the spec now makes it functional.

### 3. `rotate` method

```rust
impl VaultServiceHandle {
    /// Re-encrypt an EncryptedData blob from one key version to another.
    ///
    /// Decrypts with the key at the blob's current key_version,
    /// re-encrypts with the key at `to_version`. Returns the new
    /// EncryptedData. Does not update storage — the caller replaces the
    /// blob in storage.
    pub fn rotate(
        &self,
        encrypted: &EncryptedData,
        to_version: u32,
    ) -> Result<EncryptedData, VaultServiceError> {
        let plaintext = self.decrypt(encrypted)?;
        self.encrypt(&plaintext, to_version)
    }
}
```

`rotate` is a vault method, not a storage operation. It decrypts and
re-encrypts; the caller (the assembly layer or a migration tool) handles
replacing the blob in storage. This keeps the vault focused on crypto and
the storage system focused on storage.

### 4. `CURRENT_KEY_VERSION` and rotation policy

```rust
pub const CURRENT_KEY_VERSION: u32 = 2;
```

`encrypt()` stamps `CURRENT_KEY_VERSION` (or the explicitly-passed version)
onto new `EncryptedData` blobs. The assembly layer decides when to rotate:

- **Manual rotation**: an operator triggers rotation (e.g., a CLI command
  `alknet vault rotate --to v3` that loads all blobs, calls `rotate` on
  each, and writes them back to storage).
- **No automatic rotation**: the vault does not self-rotate. Rotation is
  an operational action, not a runtime behavior. The vault provides the
  mechanism; the policy is external.

### 5. Cache implications

The `KeyCache` is keyed by derivation path. Since each version has a
distinct path, the cache naturally holds multiple versions simultaneously.
This is correct — during a rotation, the vault may need to decrypt old
blobs (v2) and encrypt new blobs (v3), and both keys should be cached.

The cache's TTL and LRU eviction still apply. If the cache evicts an old
version's key during a long rotation, the next `decrypt` of an old blob
re-derives it (the seed hasn't changed). This is correct but slightly
slower — the rotation tool should be aware that cache misses on old keys
are expected.

## Consequences

**Positive:**
- Key rotation is a vault method (`rotate`), not a storage operation or a
  full mnemonic change. It's cheap (HD derivation) and local.
- Partial rotation is safe. Old and new keys coexist — the vault can
  decrypt any version. This means a rotation can be performed incrementally
  (rotate some blobs, verify, rotate the rest).
- No new mnemonic needed. The same seed produces all version keys. A
  backup node with the same mnemonic can decrypt any version.
- The derivation path is self-documenting. `m/74'/2'/0'/1'` is clearly
  "encryption key version 3."
- The `salt` field remains unused — no KDF complexity. Rotation is pure HD
  path indexing.
- The mechanism generalizes the TS→vault migration (v1→v2 is a rotation,
  though v1 requires the TS PBKDF2 `decrypt`, not the vault's `decrypt`).

**Negative:**
- `decrypt` now derives the key at the version-indicated path, which means
  a cache miss on an old version re-derives from the seed. This is a few
  HMAC operations — negligible, but the path construction and cache lookup
  add a small amount of complexity over the current "always use
  `PATHS::ENCRYPTION`" approach.
- The rotation tool (CLI command or migration script) must iterate all
  stored blobs and call `rotate` on each. This is an operational concern,
  not a vault concern — but the vault spec should document the expected
  usage pattern so the tool implementer knows the contract.
- Old version keys are always derivable (the seed doesn't change). This is
  a feature (partial rotation is safe) but also means a compromised seed
  allows decrypting all versions. If the seed itself is compromised, all
  versions are compromised — rotation doesn't help. This is inherent to
  HD derivation and not specific to this design.

## Assumptions

1. **The seed is not compromised.** If the seed is compromised, rotating
   the encryption key path doesn't help — the attacker can derive all
   version keys. Seed compromise requires a full mnemonic change (new
   seed, re-derive everything, re-deploy). This ADR covers encryption key
   rotation, not seed rotation. Seed rotation is an operational procedure
   (generate new mnemonic, unlock with it, re-encrypt all data) that is
   outside the vault's API.

2. **Rotation is infrequent.** The vault does not optimize for frequent
   rotation (e.g., per-request key derivation). Rotation is an operational
   event triggered by policy or incident. The cache and path-indexed
   approach are efficient for this usage pattern.

3. **The storage system tracks which blobs to rotate.** The vault's `rotate`
   method handles one blob at a time. Iterating all stored
   `EncryptedData` blobs is the storage system's job (or the CLI's). The
   vault doesn't know what's in storage — it only knows how to rotate a
   blob it's given.

4. **v1 (TS PBKDF2) data is not rotated through the vault.** v1 data is
   decrypted by the TS `decrypt()` function (PBKDF2), not the vault's
   `decrypt()` (which uses HD derivation). The v1→v2 migration is a
   separate tool that has access to both. Once data is at v2, future
   rotations (v2→v3, etc.) use the vault's `rotate` method.

## References

- ADR-020: HD derivation for encryption keys (this ADR builds on the
  version-indexed path scheme)
- OQ-22: Key rotation mechanism (resolved by this ADR)
- [encryption.md](../crates/vault/encryption.md) — AES-256-GCM, EncryptedData
- [service.md](../crates/vault/service.md) — encrypt, decrypt, rotate methods
- [mnemonic-derivation.md](../crates/vault/mnemonic-derivation.md) —
  derivation paths, `PATHS::ENCRYPTION`