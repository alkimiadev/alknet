# ADR-026: Vault Key Model — HD Derivation

## Status

Accepted

## Context

The vault's primary use of HD (hierarchical deterministic) derivation is
for identity keys, SSH host keys, and signing keys. ADR-020 covers HD
derivation for *encryption* keys specifically, but the broader decision —
"the vault uses HD derivation from a single BIP39 seed for all
self-generated secrets, not stored keys" — has no ADR. The rationale is
inline in `mnemonic-derivation.md`'s "Why HD Derivation" section, but the
choice is a one-way door: switching to stored keys would change the entire
trust model, the backup story, and the derivation path semantics.

Several related design choices also have inline rationale but no ADR:

- **`74'` coin type reservation** (SLIP-0044): alknet claims an unallocated
  coin type for its derivation paths. Once keys are derived at `m/74'/...`,
  changing the coin type would re-derive all keys — effectively one-way.
- **secp256k1 feature-gating**: the secp256k1/BIP-0032 dependency (needed
  only for Ethereum signing) is feature-gated to avoid pulling a heavy C
  dependency into nodes that don't do Ethereum signing.
- **AES-256-GCM cipher/mode choice**: the authenticated encryption scheme
  for credential storage. The rationale (authenticated, hardware-accelerated)
  is inline in `encryption.md` with no ADR.

These are foundational one-way doors that the entire vault model depends
on. They should be recorded as ADRs so a future reader sees *why* these
choices were made, not just *what* they are.

### Relationship to ADR-020

ADR-020 is a special case of this ADR — it covers HD derivation for the
*encryption key* specifically, including the v1→v2 migration from PBKDF2
to HD derivation. This ADR covers the *general* HD-derivation model that
ADR-020 builds on. ADR-020's decision (HD derivation at `m/74'/2'/0'/0'`
for encryption keys) is unchanged; this ADR records the overarching
principle.

## Decision

### 1. HD derivation from a single BIP39 seed is the vault's key model

All self-generated secrets in alknet are derived from a single BIP39
mnemonic via hierarchical deterministic (HD) derivation. The vault does
not store keys — it derives them on demand from the seed and caches them
for performance (the cache is rebuildable from the seed).

This is the same model as cryptocurrency wallets: one seed phrase, many
derived keys at deterministic paths. The properties that make this the
right model for alknet:

- **No key storage**: keys are derived on demand, not stored. The vault
  caches derived keys for performance, but the cache is rebuildable from
  the seed. No key file management, no key rotation infrastructure, no
  per-key backup.
- **Reproducible across nodes**: the same mnemonic on a different node
  produces the same keys. A backup node derives the same identity key.
  This is critical for disaster recovery — the mnemonic is the only thing
  that needs to be backed up.
- **Domain separation**: different paths produce cryptographically
  independent keys. The identity key, SSH host key, encryption key, and
  signing keys are all independent despite coming from one seed.
- **Auditable derivation**: the path records what a key is for.
  `m/74'/0'/0'/0'` is the identity key; `m/74'/0'/1'/0'` is the SSH host
  key. The path is the documentation.

### 2. SLIP-0010 (Ed25519) is the default derivation scheme

Ed25519 is alknet's default curve — it's what TLS raw key identity
(ADR-010), SSH host keys, and signing keys use. SLIP-0010 is the HD
derivation standard for Ed25519 (hardened-only, HMAC-SHA512 with
`"ed25519 seed"` as the key).

BIP-0032 (secp256k1) is supported for Ethereum signing (the standard
Ethereum path `m/44'/60'/0'/0/0` requires unhardened indices, which
SLIP-0010 cannot handle). secp256k1 is feature-gated (see Decision 4).

### 3. `74'` coin type is reserved for alknet

alknet reserves the `74'` coin type (unallocated per SLIP-0044) for its
derivation paths. All alknet paths start with `m/74'/...`:

| Path prefix | Purpose |
|-------------|---------|
| `m/74'/0'/...` | Identity keys (node, device, SSH host) |
| `m/74'/2'/...` | Encryption keys (credential storage) |
| `m/44'/60'/...` | Ethereum signing keys (secp256k1, standard BIP-44) |

Once keys are derived at `m/74'/...`, the coin type cannot be changed
without re-deriving all keys from a new path — which would produce
different keys, breaking all existing identity, TLS, SSH, and encryption
contexts. This is effectively one-way once any deployment generates keys.

### 4. secp256k1 is feature-gated

The `secp256k1` crate (BIP-0032 derivation for Ethereum) is a heavy C
dependency. Most alknet nodes do not do Ethereum signing and should not
pay the compilation cost. The `secp256k1` feature flag gates
Ethereum-specific derivation:

- Without the feature: `derive_ethereum_key` returns
  `VaultServiceError::UnsupportedKeyType`.
- With the feature: full BIP-0032 secp256k1 derivation at the standard
  Ethereum path.

### 5. AES-256-GCM for credential encryption

External credentials (API keys, OAuth tokens, bearer tokens) are encrypted
at rest using AES-256-GCM with a seed-derived key. AES-256-GCM is an
authenticated encryption scheme — it provides both confidentiality
(encryption) and integrity (authentication tag). A tampered ciphertext
fails decryption, which is the correct behavior for credential storage:
if an attacker modifies an encrypted API key in storage, decryption fails
rather than producing a different plaintext.

GCM is hardware-accelerated on modern CPUs (AES-NI), making it fast enough
that encryption is never a bottleneck. The 12-byte nonce (IV) is generated
with `OsRng` (CSPRNG) — IV reuse under the same key is catastrophic for
GCM.

The encryption key is derived from the seed at `m/74'/2'/0'/0'` via
SLIP-0010 — see ADR-020 for the full encryption key derivation rationale
and the v1→v2 migration from PBKDF2.

## Consequences

**Positive:**

- One seed, many keys, no key storage. The mnemonic is the only thing
  that needs to be backed up. Disaster recovery is "restore the mnemonic,
  re-derive everything."
- Reproducibility across nodes. A backup node with the same mnemonic
  derives the same identity key, SSH host key, and encryption key. This
  is critical for failover and migration.
- Domain separation via paths. The path *is* the documentation of what a
  key is for. No separate key registry or metadata needed.
- Ed25519 as the default curve aligns with TLS raw key identity (RFC 7250,
  ADR-010), SSH key-based auth, and iroh's NodeId model. One key type for
  all identity purposes.
- secp256k1 feature-gating keeps the default dependency tree lean. Nodes
  that don't do Ethereum signing don't pay the secp256k1 compilation cost.

**Negative:**

- The mnemonic is a single point of failure. If the mnemonic is lost, all
  derived keys are lost. If the mnemonic is compromised, all derived keys
  are compromised. Mitigated: the mnemonic is stored offline (written
  down), the vault is local-only (ADR-025), and the passphrase (BIP39
  password extension) adds a second factor.
- Changing the coin type (`74'`) or any path prefix is effectively
  one-way once keys are derived. This is inherent to HD derivation — the
  path *is* the key identity. Mitigated: the path scheme is designed to
  accommodate future use cases (device index, key version) without
  changing prefixes.
- Ed25519-only for the default derivation scheme means non-hardened
  derivation is not available (SLIP-0010 limitation). If a future use case
  needs non-hardened Ed25519 derivation (e.g., deriving public keys from a
  public key without the seed), SLIP-0010 cannot do it. Mitigated: this is
  not a current use case; if it becomes one, a different derivation scheme
  or a non-HD approach would be needed for that specific key.

## References

- ADR-020: HD derivation for encryption keys (a special case of this ADR —
  covers the encryption key at `m/74'/2'/0'/0'` and the v1→v2 migration
  from PBKDF2)
- ADR-010: ALPN router and endpoint (Ed25519 as the default curve for TLS
  raw key identity — the identity key at `m/74'/0'/0'/0'`)
- ADR-018: Vault as standalone crate (the vault defines its own key types
  and derivation paths)
- ADR-025: Vault local-only dispatch (the vault is local-only; the seed
  never crosses the network)
- [mnemonic-derivation.md](../crates/vault/mnemonic-derivation.md) —
  BIP39, SLIP-0010, BIP-0032, derivation paths, PATHS module
- [encryption.md](../crates/vault/encryption.md) — AES-256-GCM,
  EncryptedData, key versioning
- SLIP-0010: Universal hierarchical deterministic keys (Ed25519)
- SLIP-0044: Registered coin types for BIP-0032 / SLIP-0010 (`74'` is
  unallocated)
- BIP-0032: Hierarchical deterministic wallets (secp256k1)
- BIP-39: Mnemonic code for generating deterministic keys