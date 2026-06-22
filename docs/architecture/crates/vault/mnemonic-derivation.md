---
status: draft
last_updated: 2026-06-22-19
---

# Mnemonic and Key Derivation

BIP39 mnemonic generation, SLIP-0010 Ed25519 HD key derivation, BIP-0032
secp256k1 derivation (feature-gated), and the derivation path constants that
alknet uses.

## What

The vault derives keys from a single root: a BIP39 mnemonic. From one
mnemonic, all self-generated secrets are derived on demand via
hierarchical deterministic (HD) derivation. This is the same model as
cryptocurrency wallets — one seed phrase, many derived keys.

Two derivation schemes are supported:

| Scheme | Curve | Standard | Paths | Feature |
|--------|-------|----------|-------|---------|
| SLIP-0010 | Ed25519 | HMAC-SHA512 with `"ed25519 seed"` | Hardened only | default |
| BIP-0032 | secp256k1 | HMAC-SHA512 with `"Bitcoin seed"` | Hardened + unhardened | `secp256k1` |

Ed25519 is the default — it's what alknet's TLS identity (ADR-010), SSH
host keys, and signing keys use. secp256k1 is feature-gated for Ethereum
signing (the standard Ethereum path `m/44'/60'/0'/0/0` requires
unhardened indices, which SLIP-0010 cannot handle).

## Why HD Derivation

HD derivation lets one seed produce an unlimited number of keys at
deterministic paths. This means:

- **No key storage**: keys are derived on demand, not stored. The vault
  caches derived keys for performance, but the cache is rebuildable from
  the seed.
- **Reproducible across nodes**: the same mnemonic on a different node
  produces the same keys. A backup node derives the same identity key.
- **Domain separation**: different paths produce different keys. The
  identity key, SSH host key, encryption key, and signing keys are all
  cryptographically independent despite coming from one seed.
- **Auditable derivation**: the path records what a key is for.
  `m/74'/0'/0'/0'` is the identity key; `m/74'/0'/1'/0'` is the SSH host
  key. The path is the documentation.

## BIP39 Mnemonic

The root of trust is a BIP39 mnemonic seed phrase. The vault generates,
validates, and derives seeds from mnemonics.

```rust
pub struct Mnemonic {
    phrase: String,  // zeroized on drop
}

impl Mnemonic {
    pub fn generate(word_count: usize) -> Result<Self, MnemonicError>;
    pub fn from_phrase(phrase: &str, language: Language) -> Result<Self, MnemonicError>;
    pub fn to_seed(&self, passphrase: Option<&str>) -> Seed;
    pub fn phrase(&self) -> &str;
}
```

- `generate(word_count)`: Generate a new random mnemonic. Supported word
  counts: 12, 15, 18, 21, 24. The mnemonic is the root of trust — store it
  securely.
- `from_phrase(phrase, language)`: Restore from an existing phrase.
  Validates against the BIP39 word list and checksum.
- `to_seed(passphrase)`: Derive the 64-byte master seed. The passphrase is
  the optional BIP39 password extension (the "25th word"). Different
  passphrases produce different seeds.
- `phrase()`: Return the phrase string. Handle with care — this is the
  root of trust.

`Mnemonic` implements `Zeroize` and `Drop` — the phrase is zeroized
before deallocation. Only English is supported (matching the BIP39
reference and the majority of wallet software).

### Seed

```rust
#[derive(Clone, Zeroize)]
#[zeroize(drop)]
pub struct Seed {
    bytes: Vec<u8>,  // 64 bytes, zeroized on drop
}
```

The 64-byte seed from which all HD keys are derived. Zeroized on drop.
This is the input to SLIP-0010 / BIP-0032 master key derivation.

## SLIP-0010 Ed25519 Derivation

The default derivation scheme. SLIP-0010 specifies Ed25519 HD key
derivation using HMAC-SHA512 with the key `"ed25519 seed"`.

```rust
pub fn derive_path_from_seed(seed: &[u8], path: &str) -> Result<ExtendedPrivKey, DerivationError>;
```

### Master key derivation

The master key is derived from the seed via HMAC-SHA512:

```
HMAC-SHA512(key = "ed25519 seed", data = seed)
  → first 32 bytes: private key (kL)
  → next 32 bytes: chain code
```

The `ed25519-bip32` crate handles the extended key format (kL || kR ||
chain code). The vault extracts the first 32 bytes as the private key and
the public key (32 bytes) via `XPrv::public()`.

### Child derivation

SLIP-0010 Ed25519 supports **hardened child derivation only**. Every child
index must have the `'` (or `h`) suffix, meaning `index + 0x80000000`.
Unhardened indices are rejected by the derivation logic (Ed25519 cannot
support them because public key derivation is not possible without the
private key).

### Path parsing

```rust
pub fn parse_derivation_path(path: &str) -> Result<Vec<u32>, DerivationError>;
```

Parses paths like `m/74'/0'/0'/0'` into child indices. The `m` prefix is
required. Hardened indices have `'` or `h` suffix; unhardened indices are
allowed in the parser (for BIP-0032 paths) but Ed25519 derivation will
fail on them.

### ExtendedPrivKey

```rust
#[derive(Clone, Zeroize)]
#[zeroize(drop)]
pub struct ExtendedPrivKey {
    private_key: Vec<u8>,   // 32 bytes
    public_key: Vec<u8>,    // 32 bytes
    chain_code: Vec<u8>,    // 32 bytes
    path: String,           // the path that produced this key
}
```

The result of SLIP-0010 derivation. Zeroized on drop. Accessors return
slices — the caller copies what it needs.

## BIP-0032 secp256k1 Derivation (Ethereum)

Feature-gated behind `secp256k1`. Implements BIP-0032 HD key derivation for
the secp256k1 curve, used for Ethereum signing keys.

```rust
#[cfg(feature = "secp256k1")]
pub fn derive_secp256k1_path(seed: &[u8], path: &str) -> Result<Secp256k1ExtendedPrivKey, DerivationError>;
```

Unlike SLIP-0010 (Ed25519), BIP-0032 supports both hardened and
unhardened child derivation. The standard Ethereum path
`m/44'/60'/0'/0/0` uses unhardened indices for the last two levels.

### Why a separate module

SLIP-0010 and BIP-0032 differ in:

| Aspect | SLIP-0010 (Ed25519) | BIP-0032 (secp256k1) |
|--------|---------------------|----------------------|
| HMAC key | `"ed25519 seed"` | `"Bitcoin seed"` |
| Child derivation | Hardened only | Hardened + unhardened |
| Public key size | 32 bytes | 33 bytes (compressed) |
| Public derivation | Not possible | Possible (unhardened) |

The `secp256k1` crate is a heavy dependency (it includes a C library for
curve operations). Feature-gating it keeps the default vault lightweight —
nodes that don't need Ethereum signing don't pay the cost.

When the feature is disabled, `derive_ethereum_key` returns
`VaultServiceError::UnsupportedKeyType`.

## Derivation Paths

alknet reserves the `74'` coin type (unallocated per SLIP-0044) for its
keys. Well-known paths are constants in the `PATHS` module:

```rust
pub mod PATHS {
    pub const IDENTITY: &str = "m/74'/0'/0'/0'";       // Primary identity keypair
    pub const DEVICE_PREFIX: &str = "m/74'/0'/0'";      // Worker/device identity prefix
    pub const SSH_HOST: &str = "m/74'/0'/1'/0'";        // SSH host key
    pub const ENCRYPTION: &str = "m/74'/2'/0'/0'";     // AES-256-GCM encryption key
    pub const ETHEREUM: &str = "m/44'/60'/0'/0/0";     // Ethereum signing key (secp256k1)
}
```

Helper functions construct parameterized paths:

```rust
pub fn device_path(index: u32) -> String;           // m/74'/0'/0'/{index}'
pub fn site_password_path(site_hash: &str) -> String; // m/74'/1'/0'/{site_hash}'
pub fn encryption_path_for_version(version: u32) -> String; // m/74'/2'/0'/{version-2}'
```

### Path semantics

| Path | Purpose | Key type | Used by |
|------|---------|----------|---------|
| `m/74'/0'/0'/0'` | Primary node identity (Ed25519) | Ed25519 | TLS raw key (ADR-010), node identity |
| `m/74'/0'/0'/{n}'` | Worker/device identity | Ed25519 | Multi-device nodes, workers |
| `m/74'/0'/1'/0'` | SSH host key | Ed25519 | SSH handler |
| `m/74'/1'/0'/{hash}'` | Site-specific deterministic password | Ed25519 bytes | Per-site passwords (not cached) |
| `m/74'/2'/0'/0'` | Encryption key for external credentials | AES-256-GCM | Credential encryption (v2, see [encryption.md](encryption.md)) |
| `m/44'/60'/0'/0/0` | Ethereum signing key | secp256k1 | Ethereum signing (feature-gated) |

`encryption_path_for_version` maps a key version to its derivation path
(ADR-021). v2 (current) maps to `m/74'/2'/0'/0'` (which is `PATHS::ENCRYPTION`);
v3 maps to `m/74'/2'/0'/1'`; etc. This is the rotation mechanism — each
version gets a cryptographically independent key from the same seed.

`KeyType` tags `DerivedKey` (see [protocol.md](protocol.md)) and
`CachedKey` (see [service.md](service.md)) so consumers know what they
received without inspecting byte lengths.

## Determinism

Derivation is deterministic: the same mnemonic + passphrase + path
always produces the same key. This is verified by regression tests in
`tests/test_vectors.rs` against the BIP39 "abandon...about" test vector.

### Passphrase sensitivity

Different passphrases produce different seeds and therefore different
keys. The passphrase is a legitimate access-control mechanism: two
operators with the same mnemonic but different passphrases get different
keysets. The vault does not enforce a passphrase policy — that's an
assembly-layer concern.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| Vault is standalone | [ADR-018](../../decisions/018-vault-standalone-crate.md) | Zero alknet crate dependencies |
| HD derivation (not stored keys) | — | One seed, many keys, no key storage |
| `74'` coin type reserved for alknet | — | SLIP-0044 unallocated; alknet namespace |
| secp256k1 feature-gated | — | Heavy dep; only needed for Ethereum |
| Hardened-only for Ed25519 | SLIP-0010 | Ed25519 cannot do public derivation |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-20** (resolved by ADR-020): Encryption key derivation — HD derivation
  from seed, not PBKDF2. The salt field is unused in v2. See
  [encryption.md](encryption.md).

## References

- [BIP39](https://github.com/bitcoin/bips/blob/master/bip-0039.mediawiki) —
  mnemonic seed phrases
- [SLIP-0010](https://github.com/satoshilabs/slips/blob/master/slip-0010.md) —
  Ed25519 HD derivation
- [BIP-0032](https://github.com/bitcoin/bips/blob/master/bip-0032.mediawiki) —
  secp256k1 HD derivation
- [SLIP-0044](https://github.com/satoshilabs/slips/blob/master/slip-0044.md) —
  registered coin types (74' is unallocated)
- Implementation: `crates/alknet-vault/src/mnemonic.rs`,
  `crates/alknet-vault/src/derivation.rs`, `crates/alknet-vault/src/ethereum.rs`
- Test vectors: `crates/alknet-vault/tests/test_vectors.rs`