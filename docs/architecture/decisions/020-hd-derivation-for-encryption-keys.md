# ADR-020: HD Derivation for Encryption Keys

## Status

Accepted

## Context

The vault encrypts external credentials (API keys, OAuth tokens) that cannot
be derived from the BIP39 seed — they're arbitrary bytes. The encryption
key for AES-256-GCM must come from somewhere. Two approaches exist:

### The TypeScript predecessor

The `@alkdev/storage` library (`/workspace/@alkdev/storage/src/graphs/crypto.ts`)
implemented credential encryption before the vault existed. It uses
**PBKDF2** (Password-Based Key Derivation Function 2) with a password and
salt:

```
key = PBKDF2(password, salt, iterations=100_000, hash=SHA-256, output=32 bytes)
```

- The **password** is the secret (a user-provided string, not a BIP39 seed)
- The **salt** is 16 bytes, randomly generated per encryption, and is
  load-bearing — it participates in key derivation
- **Iterations**: 100,000 for key_version=1, 200,000 for key_version=2
- The resulting key is used for AES-256-GCM encryption

This was the right design before the vault existed: without a BIP39 seed,
PBKDF2 from a password was the only option. The salt prevents rainbow-table
attacks, and the iteration count slows brute-force.

### The vault's approach

The vault derives the encryption key from the BIP39 seed via **SLIP-0010
HD derivation** at path `m/74'/2'/0'/0'`:

```
seed → SLIP-0010 derive(m/74'/2'/0'/0') → first 32 bytes → AES-256-GCM key
```

- The **seed** is the secret (64 bytes, derived from the BIP39 mnemonic)
- The **salt** is generated (32 bytes) but **not used** in key derivation —
  it's stored in `EncryptedData.salt` for forward compatibility
- No PBKDF2, no iteration count, no password stretching
- The key is deterministic: the same mnemonic + path always produces the
  same key

### Why HD derivation is better now

With the vault in place, HD derivation is strictly better than PBKDF2 for
credential encryption:

1. **No password to manage.** The BIP39 mnemonic is already the root of
   trust. PBKDF2 requires a separate password — another secret to manage,
   lose, or have stolen. HD derivation uses the seed that already exists.

2. **Deterministic and reproducible.** The same mnemonic always produces the
   same encryption key at the same path. A backup node derives the same key.
   PBKDF2 with a different password produces a different key — there's no
   way to reproduce the key without the exact password.

3. **No iteration overhead.** PBKDF2 with 100k iterations is intentionally
   slow (that's the point — it slows brute-force). HD derivation is a few
   HMAC operations — effectively instant. This matters when encrypting or
   decrypting multiple credentials at startup.

4. **Domain separation via paths.** Different encryption purposes can use
   different derivation paths (`m/74'/2'/0'/0'` for v2, `m/74'/2'/0'/1'`
   for a future v3). PBKDF2 has no equivalent — the only versioning knob is
   the iteration count or the password. See ADR-021 for the version-indexed
   path scheme.

5. **The salt becomes unnecessary for key derivation.** HD derivation
   doesn't need a salt — the path provides domain separation. The salt
   field in `EncryptedData` is kept for wire-format compatibility but does
   not participate in key derivation.

### The compatibility problem

The `EncryptedData` wire format is the same across both implementations
(`keyVersion`, `salt`, `iv`, `data` — all base64-encoded strings). But the
key derivation is different:

- **TS v1**: PBKDF2(password, salt, 100k iterations) → key
- **Rust v1**: SLIP-0010(seed, `m/74'/2'/0'/0'`) → key

Data encrypted by the TS implementation **cannot be decrypted by the vault**
— the keys are different even if the password equals the mnemonic. This is a
hard incompatibility at the crypto layer, not a format issue.

## Decision

### 1. HD derivation is the vault's encryption key derivation method

The vault uses SLIP-0010 HD derivation from the BIP39 seed at path
`m/74'/2'/0'/0'` (`PATHS::ENCRYPTION`) to produce the AES-256-GCM
encryption key. No PBKDF2. No password-based key derivation. The seed is
the sole secret input.

### 2. The salt field is unused in vault-encrypted data

The `EncryptedData.salt` field exists in the wire format for compatibility
with the TS `EncryptedDataSchema`, but the vault does not use it for key
derivation. The vault generates a random salt and stores it (for wire-format
consistency), but it plays no cryptographic role. If a future KDF-based
derivation is needed (see "Future KDF" below), the field is already present.

### 3. key_version semantics

| Version | Key derivation | Used by | Decryptable by vault? |
|---------|---------------|---------|----------------------|
| 1 | PBKDF2 (password + salt + 100k iterations) | TS `@alkdev/storage` | No — different key |
| 2 | SLIP-0010 HD derivation (seed → `m/74'/2'/0'/0'`) | Rust vault | Yes |

The vault stamps `key_version: 2` on new encryptions. `CURRENT_KEY_VERSION`
is `2`.

**The current source uses `CURRENT_KEY_VERSION = 1` with HD derivation.**
This is a drift from the spec — the source's v1 is HD-derived, but the TS
v1 is PBKDF2-derived. Same version number, different derivation. The source
must be updated to use `key_version: 2` for HD-derived data, reserving v1
for the TS PBKDF2 legacy.

### 4. Migration path: TS → vault

TS-encrypted credentials (PBKDF2, key_version=1) are migrated to vault-
encrypted credentials (HD derivation, key_version=2) through a one-time
re-encryption:

1. Decrypt the TS-encrypted data with the original password and PBKDF2
   (using the TS `@alkdev/storage` `decrypt()` function or a migration
   tool that implements PBKDF2)
2. Re-encrypt the plaintext with the vault at `key_version: 2`
3. Replace the old `EncryptedData` blob in storage

The vault does **not** implement PBKDF2. The migration is performed by a
separate tool or script that has access to both the TS `decrypt()` function
and the vault's `encrypt()`. This is a one-time migration — once all data
is at key_version=2, PBKDF2 is no longer needed.

### 5. No PBKDF2 in the vault

The vault does not implement PBKDF2 and does not support decrypting
key_version=1 (TS PBKDF2) data. The vault's `decrypt()` method derives the
key via HD derivation and attempts decryption. If the data was encrypted
with PBKDF2 (TS), decryption fails (wrong key) — this is correct behavior,
not a bug. The migration tool handles the TS→vault transition.

### 6. Future KDF (not v2)

If a future use case requires KDF-based key derivation (e.g., stretching a
key derived from a non-seed source, or using a salt for additional domain
separation), it would be a new key_version with its own derivation method.
The `salt` field is available for this.

**Clarification (review #002 W6)**: the salt field is reserved for *future
versions'* use. v2 data's salt is permanently unused — it was random, never
participated in key derivation, and cannot be retroactively made
load-bearing for v2 data. Introducing a KDF in v3 is a new derivation
method (not a version-indexed path), requiring its own design and a v2→v3
migration (re-encrypt with the new KDF, using a newly-generated v3 salt —
the v2 salt is not reused). The field's presence saves a wire-format struct
change only (ADR-018 locks the wire format); it does not make the KDF
design or migration trivial. A KDF doesn't fit the rotation scheme
(version-indexed paths, ADR-021) — it's a different derivation *family*,
not another version index. See OQ-22 (key rotation) and ADR-018
(`EncryptedData` wire format lock).

## Consequences

**Positive:**
- One secret (the BIP39 seed) is the root of trust for both derived keys
  and encryption keys. No separate password to manage.
- Encryption key derivation is instant (HD derivation) vs. slow (PBKDF2
  100k iterations). Startup with many credentials is fast.
- The encryption key is reproducible — a backup node with the same mnemonic
  derives the same key and can decrypt the same credentials.
- Domain separation via paths — future encryption purposes can use
  different paths without changing the wire format.
- Clean break from the TS approach. No PBKDF2 code in the vault. The vault
  is smaller and simpler.

**Negative:**
- TS-encrypted data cannot be decrypted by the vault. Migration requires a
  separate tool with access to both the TS `decrypt()` and the vault's
  `encrypt()`. This is expected — the TS implementation is being replaced,
  not integrated.
- The `salt` field is unused in v2. It occupies 44 bytes (base64-encoded 32
  bytes) per `EncryptedData` blob for no cryptographic purpose. This is the
  cost of wire-format compatibility — keeping the field means the struct
  doesn't need to change if a future KDF uses it.
- `CURRENT_KEY_VERSION` must change from 1 to 2 in the source. If any
  vault-encrypted data already exists at key_version=1 (with HD derivation),
  it would need re-encryption at key_version=2. In practice, the vault is
  pre-production, so this is a source change, not a data migration.

## Assumptions

1. **The TS `@alkdev/storage` encrypted data is the only legacy.** If other
   systems produce PBKDF2-encrypted `EncryptedData` blobs, they need the same
   migration treatment. The assumption is that `@alkdev/storage` is the only
   consumer.

2. **The vault is pre-production.** No significant amount of vault-encrypted
   data (HD derivation, key_version=1) exists in production. Bumping to
   key_version=2 is a source change, not a data migration. If vault-encrypted
   data does exist, it needs re-encryption at key_version=2 (decrypt with HD
   key at v1 path, re-encrypt at v2 — same key, just version bump).

3. **The migration is one-time and one-directional.** Once data is at
   key_version=2, there's no path back to PBKDF2. The TS `@alkdev/storage`
   crypto module becomes legacy after migration.

4. **The `salt` field's forward compatibility is worth the 44 bytes.** If a
   future KDF is never needed, the salt field is wasted space. The assumption
   is that the cost is negligible (credentials are small, not bulk data) and
   the flexibility is worth it.

## References

- ADR-018: Vault as standalone crate
- ADR-019: Vault assembly-layer-only access
- [encryption.md](../crates/vault/encryption.md) — AES-256-GCM, EncryptedData
- [mnemonic-derivation.md](../crates/vault/mnemonic-derivation.md) — SLIP-0010, PATHS::ENCRYPTION
- OQ-20: Salt/KDF Phase B (resolved by this ADR)
- OQ-22: Key rotation mechanism (still open — this ADR defines v2 but not the rotation workflow)
- TypeScript predecessor: `/workspace/@alkdev/storage/src/graphs/crypto.ts`
- TypeScript secret graph: `/workspace/@alkdev/storage/src/graphs/modules/secret-graph.ts`