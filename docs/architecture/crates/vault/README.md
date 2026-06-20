---
status: draft
last_updated: 2026-06-19
---

# alknet-vault

Local key vault: BIP39 mnemonic generation, SLIP-0010 Ed25519 HD key
derivation, BIP-0032 secp256k1 derivation (feature-gated), and AES-256-GCM
encryption. Holds the master seed — the root of trust for all derived keys
and encrypted credentials in the alknet system.

## What This Crate Is

alknet-vault is a **standalone crate** with zero alknet crate dependencies
(ADR-018). It provides the cryptographic primitives and runtime API for
managing the root of trust. The CLI binary (the `alknet` crate) is the sole
component that talks to the vault directly (ADR-019) — handlers receive
derived/decrypted material through capabilities, never through a vault
reference.

The vault is **not a network service**. It has no ALPN, no
`ProtocolHandler` implementation, and no operations registered in the call
protocol (ADR-008, ADR-014). The master seed and derived private keys never
cross the network.

## Documents

| Document | Status | Description |
|----------|--------|-------------|
| [mnemonic-derivation.md](mnemonic-derivation.md) | draft | BIP39, SLIP-0010, BIP-0032, derivation paths, key types |
| [encryption.md](encryption.md) | draft | AES-256-GCM, EncryptedData, key versioning, HD derivation (ADR-020) |
| [service.md](service.md) | draft | VaultServiceHandle lifecycle, actor dispatch, cache, error model |
| [protocol.md](protocol.md) | draft | VaultProtocol irpc messages, DerivedKey redaction, serialization |

## Applicable ADRs

| ADR | Title | Relevance |
|-----|-------|-----------|
| [003](../../decisions/003-crate-decomposition.md) | Crate Decomposition | alknet-vault's standalone position |
| [005](../../decisions/005-irpc-as-call-protocol-foundation.md) | irpc as Call Protocol Foundation | VaultProtocol uses irpc directly |
| [008](../../decisions/008-secret-service-integration.md) | Vault Integration Point | CLI-embedded, capability source |
| [014](../../decisions/014-secret-material-flow-and-capability-injection.md) | Secret Material Flow and Capability Injection | Capabilities carry vault-derived material |
| [018](../../decisions/018-vault-standalone-crate.md) | Vault as Standalone Crate | Zero alknet crate dependencies |
| [019](../../decisions/019-vault-assembly-layer-only.md) | Vault Assembly-Layer-Only Access | The assembly layer is the sole caller |
| [020](../../decisions/020-hd-derivation-for-encryption-keys.md) | HD Derivation for Encryption Keys | SLIP-0010 derivation, not PBKDF2; salt unused in v2 |
| [021](../../decisions/021-key-rotation-via-version-indexed-paths.md) | Key Rotation via Version-Indexed Paths | Version-indexed paths; `rotate` re-encrypts |

## Relevant Open Questions

| OQ | Title | Status | Relevance |
|----|-------|--------|-----------|
| OQ-20 | Encryption key derivation | resolved (ADR-020) | HD derivation from seed; salt field unused in v2 |
| OQ-21 | Remote vault access | deferred | Protocol is remote-capable by construction; enabling = server-setup change with auth-wrapping handler; Unlock/Lock local-only |
| OQ-22 | Key rotation mechanism | resolved (ADR-021) | Version-indexed paths; `rotate` method |

## Key Design Principles

1. **Standalone**: The vault depends on no alknet crate. It defines its own
   types, errors, and protocol. External crates depend on the vault; the
   vault depends on nothing in alknet.
2. **Assembly-layer only**: The vault's API is consumed by the CLI binary,
   not by handlers. Handlers receive material through capabilities
   (ADR-014). The vault is not on the wire.
3. **Zeroize everything sensitive**: The mnemonic, seed, derived private
   keys, encryption keys, and cached keys all implement `Zeroize` and
   `ZeroizeOnDrop`. Secret material does not linger in freed heap memory.
4. **Deterministic derivation**: The same mnemonic + passphrase + path
   always produces the same key. Derivation is reproducible across runs
   and across nodes.
5. **OsRng for nonces**: AES-GCM IVs and any cryptographic nonces use
   `OsRng` (or equivalent CSPRNG), never `rand::random()`. IV reuse under
   the same key is catastrophic for GCM.
6. **No `unwrap()` or `expect()` outside tests**: vault operations
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
- **DerivedKey redaction in JSON**: `DerivedKey` serializes the
  `private_key` as `"[REDACTED]"` in human-readable formats (JSON) and as
  raw bytes in binary formats (postcard). The redaction is a defense-in-
  depth measure, not the primary control — the primary control is that
  `DerivedKey` never crosses the call protocol wire (ADR-014).

## Public API

The vault re-exports its primary types from the crate root:

```rust
// Mnemonic and seed
pub use mnemonic::{Language, Mnemonic, Seed};

// Derivation
pub use derivation::{DerivationError, ExtendedPrivKey, PATHS};

// Encryption
pub use encryption::{EncryptedData, EncryptionError};

// Protocol (irpc messages)
pub use protocol::{DerivedKey, KeyType, VaultMessage, VaultProtocol};

// Service (runtime)
pub use service::{VaultService, VaultServiceActor, VaultServiceError, VaultServiceHandle};

// Cache
pub use cache::CacheConfig;
```

The `secp256k1` feature flag gates Ethereum (BIP-0032) derivation:

```rust
#[cfg(feature = "secp256k1")]
pub mod ethereum;
```