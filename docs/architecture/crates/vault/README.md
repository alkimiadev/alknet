---
status: draft
last_updated: 2026-06-23
---

# alknet-vault

Local key vault: BIP39 mnemonic generation, SLIP-0010 Ed25519 HD key
derivation, BIP-0032 secp256k1 derivation (feature-gated), and AES-256-GCM
encryption. Holds the master seed — the root of trust for all derived keys
and encrypted credentials in the alknet system.

## What This Crate Is

alknet-vault is a **standalone crate** with zero alknet crate dependencies
(ADR-018) and zero RPC framework dependencies (ADR-025). It provides the
cryptographic primitives and runtime API for managing the root of trust.
The CLI binary (the `alknet` crate) is the sole component that talks to the
vault directly (ADR-019) — handlers receive derived/decrypted material
through capabilities, never through a vault reference.

The vault is **not a network service**. It has no ALPN, no
`ProtocolHandler` implementation, no operations registered in the call
protocol (ADR-008, ADR-014), and no remote dispatch capability (ADR-025).
The vault is **local-only by construction** — direct method calls on
`VaultServiceHandle`, no actor, no message enum, no wire format. The master
seed and derived private keys never cross the network.

## Documents

| Document | Status | Description |
|----------|--------|-------------|
| [mnemonic-derivation.md](mnemonic-derivation.md) | draft | BIP39, SLIP-0010, BIP-0032, derivation paths, key types |
| [encryption.md](encryption.md) | draft | AES-256-GCM, EncryptedData, key versioning, HD derivation (ADR-020) |
| [service.md](service.md) | draft | VaultServiceHandle lifecycle, direct dispatch, cache, error model |
| [protocol.md](protocol.md) | draft | DerivedKey redaction, KeyType, serialization behavior |

## Applicable ADRs

| ADR | Title | Relevance |
|-----|-------|-----------|
| [003](../../decisions/003-crate-decomposition.md) | Crate Decomposition | alknet-vault's standalone position |
| [008](../../decisions/008-secret-service-integration.md) | Vault Integration Point | CLI-embedded, capability source |
| [010](../../decisions/010-alpn-router-and-endpoint.md) | ALPN Router and Endpoint | Ed25519 as default curve for TLS raw key identity |
| [014](../../decisions/014-secret-material-flow-and-capability-injection.md) | Secret Material Flow and Capability Injection | Capabilities carry vault-derived material |
| [018](../../decisions/018-vault-standalone-crate.md) | Vault as Standalone Crate | Zero alknet crate dependencies |
| [019](../../decisions/019-vault-assembly-layer-only.md) | Vault Assembly-Layer-Only Access | The assembly layer is the sole caller |
| [020](../../decisions/020-hd-derivation-for-encryption-keys.md) | HD Derivation for Encryption Keys | SLIP-0010 derivation, not PBKDF2; salt unused in v2 |
| [021](../../decisions/021-key-rotation-via-version-indexed-paths.md) | Key Rotation via Version-Indexed Paths | Version-indexed paths; `rotate` re-encrypts |
| [025](../../decisions/025-vault-local-only-dispatch.md) | Vault Local-Only Dispatch | Dropped irpc; direct method calls; local-only by construction |
| [026](../../decisions/026-vault-key-model-hd-derivation.md) | Vault Key Model — HD Derivation | HD derivation from BIP39 seed; `74'` coin type; AES-256-GCM |

## Relevant Open Questions

| OQ | Title | Status | Relevance |
|----|-------|--------|-----------|
| OQ-20 | Encryption key derivation | resolved (ADR-020) | HD derivation from seed; salt field unused in v2 |
| OQ-21 | Remote vault access | resolved (ADR-025) | Vault is local-only by construction; remote access requires a separate vault-server crate with its own ADR |
| OQ-22 | Key rotation mechanism | resolved (ADR-021) | Version-indexed paths; `rotate` method |

## Key Design Principles

1. **Standalone**: The vault depends on no alknet crate and no RPC framework.
   It defines its own types and errors. External crates depend on the vault;
   the vault depends on nothing in alknet.
2. **Assembly-layer only**: The vault's API is consumed by the CLI binary,
   not by handlers. Handlers receive material through capabilities
   (ADR-014). The vault is not on the wire.
3. **Local-only by construction**: The vault has no remote dispatch
   capability. Direct method calls on `VaultServiceHandle` — no actor, no
   message enum, no wire format (ADR-025). Remote access, if ever needed,
   requires a separate crate with its own ADR.
4. **Zeroize everything sensitive**: The mnemonic, seed, derived private
   keys, encryption keys, and cached keys all implement `Zeroize` and
   `ZeroizeOnDrop`. Secret material does not linger in freed heap memory.
5. **Deterministic derivation**: The same mnemonic + passphrase + path
   always produces the same key. Derivation is reproducible across runs
   and across nodes.
6. **OsRng for nonces**: AES-GCM IVs and any cryptographic nonces use
   `OsRng` (or equivalent CSPRNG), never `rand::random()`. IV reuse under
   the same key is catastrophic for GCM.
7. **No `unwrap()` or `expect()` outside tests**: vault operations
   propagate errors. A poisoned lock is recovered with
   `unwrap_or_else(|e| e.into_inner())`, not `unwrap()`. A panic in one
   vault operation must not brick the vault for all other operations.

## Security Constraints

These are security-critical implementation requirements, not architectural
decisions (the architecture is locked by the ADRs above). They are
documented here so implementation agents don't miss them. See
[service.md → Security Constraints](service.md#security-constraints) for
the full list.

- **OsRng for IVs**: AES-GCM IVs must use `OsRng`, not `rand::random()`. The
  current source uses `rand::random()` — this is a known drift from the
  spec and must be corrected during implementation sync.
- **Zeroized drop**: `Seed`, `Mnemonic`, `ExtendedPrivKey`,
  `Secp256k1ExtendedPrivKey`, `EncryptionKey`, `CachedKey`, and
  `DerivedKey` all derive `Zeroize` and `ZeroizeOnDrop`. The cache must
  clear on drop, not just on explicit `lock()`.
- **No `unwrap()` outside tests**: poisoned lock recovery uses
  `unwrap_or_else(|e| e.into_inner())` or explicit error propagation. The
  current source uses `unwrap()` in `VaultServiceHandle` methods — this
  is a known drift and must be corrected.
- **DerivedKey redaction in serialization**: `DerivedKey` serializes the
  `private_key` as `"[REDACTED]"` in all formats (ADR-025 dropped the
  postcard/remote path that previously preserved bytes in binary formats).
  Deserialization rejects `"[REDACTED]"` with an error (resolves review
  #002 W8). The redaction is a defense-in-depth measure for logging safety,
  not the primary control — the primary control is that `DerivedKey` never
  crosses the call protocol wire (ADR-014).

## Known Source Drift

The vault crate carries over source from the POC. The following items are
known divergences between the current source and the spec. All must be
corrected during implementation sync. This table is the single source of
truth for drift tracking — if an item is fixed in source, update this table.

| # | Item | Current source behavior | Target behavior (per spec) | Source location | Spec reference |
|---|------|------------------------|-----------------------------|-----------------|----------------|
| 1 | IV generation | `rand::random()` | `OsRng` (CSPRNG) | `encryption.rs` L133 | [encryption.md → Security Constraints](encryption.md#security-constraints), [service.md → Security Constraints](service.md#security-constraints) |
| 2 | RwLock `unwrap()` | `unwrap()` on every `RwLock` acquisition (L142, 161, 182, 191, 196, 227, 264, 307, 340, 367) | `unwrap_or_else(\|e\| e.into_inner())` for poisoned lock recovery | `service.rs` (see line numbers) | [service.md → Security Constraints](service.md#security-constraints) |
| 3 | `CURRENT_KEY_VERSION` | `1` (HD-derived, but v1 is reserved for TS PBKDF2 legacy per ADR-020) | `2` (HD-derived, per ADR-020) | `encryption.rs` | [encryption.md → Key Versioning](encryption.md#key-versioning), [ADR-020](../../decisions/020-hd-derivation-for-encryption-keys.md) |
| 4 | irpc dependency | `VaultProtocol` enum with `#[rpc_requests]`, `VaultServiceActor`, `Client<VaultProtocol>`, irpc/postcard deps | Remove entirely — direct method calls on `VaultServiceHandle` (ADR-025) | `protocol.rs`, `service.rs`, `Cargo.toml` | [ADR-025](../../decisions/025-vault-local-only-dispatch.md) |
| 5 | `DerivedKey` dual serialization | JSON redacts, postcard preserves bytes | Always redact on serialize; reject `"[REDACTED]"` on deserialize with error (ADR-025, resolves W8) | `protocol.rs` | [protocol.md → Serialization Redaction](protocol.md#serialization-redaction), [ADR-025](../../decisions/025-vault-local-only-dispatch.md) |
| 6 | `HashMap::clear` zeroization | `KeyCache::clear()` removes entries and relies on `CachedKey`'s `Drop` impl for zeroization | Verify `HashMap::clear()` actually drops values (it does, but worth a test) | `cache.rs` | [service.md → Security Constraints](service.md#security-constraints) |
| 7 | `derive_password` / `site_password_path` | `derive_password`, `derive_password_string`, `site_password_path` methods exist | Remove entirely — password-manager pattern not relevant to RPC system's vault (ADR-025, resolves C9) | `service.rs`, `mnemonic-derivation.rs` | [ADR-025](../../decisions/025-vault-local-only-dispatch.md) |
| 8 | `unlock_new` return type | Returns `String` (not zeroized on drop) | Return `Zeroizing<String>` — the mnemonic is the root of trust and must not linger in freed memory (resolves W7) | `service.rs` | [service.md → unlock_new](service.md#unlock_newword_count--phrase) |
| 9 | `key_version` ignored in encrypt/decrypt | `encrypt`/`decrypt` always derive at `PATHS::ENCRYPTION` regardless of `key_version` | Derive at `encryption_path_for_version(key_version)` — encrypt stamps the passed version, decrypt selects the key by the blob's version (ADR-021) | `service.rs` | [service.md → encrypt](service.md#encryptplaintext-key_version--encrypteddata), [ADR-021](../../decisions/021-key-rotation-via-version-indexed-paths.md) |
| 10 | `rotate` not implemented | No `rotate` method exists | Implement `rotate(encrypted, to_version)` — decrypt with old version's key, re-encrypt with new version's key (ADR-021) | `service.rs` | [service.md → rotate](service.md#rotateencrypted-to_version--encrypteddata), [ADR-021](../../decisions/021-key-rotation-via-version-indexed-paths.md) |

## Public API

The vault re-exports its primary types from the crate root:

```rust
// Mnemonic and seed
pub use mnemonic::{Language, Mnemonic, Seed};

// Derivation
pub use derivation::{DerivationError, ExtendedPrivKey, PATHS};
// Derivation helpers (derive_path_from_seed, parse_derivation_path,
// device_path, encryption_path_for_version) are accessible as
// alknet_vault::derivation::* — not re-exported at crate root to avoid
// clutter, but fully public.

// Encryption
pub use encryption::{EncryptedData, EncryptionError, EncryptionKey};
pub use encryption::CURRENT_KEY_VERSION;

// Key types (DerivedKey, KeyType)
pub use protocol::{DerivedKey, KeyType};

// Service (runtime)
pub use service::{VaultServiceError, VaultServiceHandle};

// Cache
pub use cache::CacheConfig;
```

The `secp256k1` feature flag gates Ethereum (BIP-0032) derivation:

```rust
#[cfg(feature = "secp256k1")]
pub mod ethereum;
```