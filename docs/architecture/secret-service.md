---
status: reviewed
last_updated: 2026-06-10
---

# Secret Service (alknet-secret)

## What

The `alknet-secret` crate provides BIP39 mnemonic generation, SLIP-0010 Ed25519
HD key derivation, AES-256-GCM encryption for external credentials, and the
`SecretProtocol` irpc service. It is the only component that holds the master
seed phrase.

## Why

Operations like SSH key generation, API key storage, and Ethereum transaction
signing all need deterministic key derivation from a single root of trust. The
seed phrase is the single recovery mechanism — from it, all self-generated
secrets can be derived on demand. External credentials (third-party API keys,
OAuth tokens) cannot be derived and must be stored encrypted, with the
encryption key itself derived from the seed.

The secret service isolates this responsibility: no other crate sees the seed,
and derived keys are provided on demand through an irpc service interface. This
follows ADR-027 (crate decomposition) — alknet-secret is fully independent of
alknet-core and alknet-storage.

## Architecture

### Crate Structure

```
alknet-secret/
├── Cargo.toml
├── src/
│   ├── lib.rs           # Crate root, re-exports
│   ├── mnemonic.rs       # BIP39: phrase generation, validation, seed derivation
│   ├── derivation.rs     # SLIP-0010: HD key derivation, path constants
│   ├── encryption.rs     # AES-256-GCM: encrypt/decrypt, EncryptedData type
│   ├── protocol.rs       # SecretProtocol irpc service enum, DerivedKey, KeyType
│   ├── service.rs        # SecretService, SecretServiceHandle, SecretServiceActor
│   ├── cache.rs          # Key caching: LRU cache with TTL, derivation path as key
│   └── ethereum.rs       # BIP-0032 secp256k1 HD key derivation (behind feature flag)
└── tests/
    ├── derivation_tests.rs  # Path derivation, coin type 74' consistency
    ├── encryption_tests.rs  # Round-trip encrypt/decrypt, key version
    ├── service_tests.rs     # Unlock/Lock lifecycle, derive on locked = error
    └── test_vectors.rs      # Known-answer tests: BIP39, SLIP-0010, AES-256-GCM
```

### Dependencies

```toml
[dependencies]
bip39 = { version = "2", features = ["rand"] }
ed25519-bip32 = "0.4"        # IOHK SLIP-0010 Ed25519 HD derivation
aes-gcm = "0.10"              # AES-256-GCM
sha2 = "0.10"                 # SHA-256 (also used for HMAC-SHA512 in password derivation)
hmac = "0.12"                 # HMAC-SHA512 for key derivation
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
irpc = { workspace = true }   # Always-on, not feature-gated (ADR-027)
irpc-derive = { workspace = true }  # Proc-macro for #[rpc_requests]
tokio = { version = "1", features = ["sync", "rt", "macros"] }  # Async runtime for SecretServiceActor
zeroize = { version = "1", features = ["derive"] }  # Secure memory wiping (ADR-038)
base64 = "0.22"               # Base64url encoding for derived passwords
rand = "0.8"                  # Random IV/salt generation for AES-256-GCM

[dependencies.secp256k1]
version = "0.29"
optional = true               # BIP-0032 secp256k1 derivation (behind feature flag)

[features]
default = []
secp256k1 = ["dep:secp256k1"]  # Enable Ethereum/secp256k1 key derivation

# Future (Phase B): key rotation via KDF
# hkdf = "0.12"               # HKDF for salt-based key stretching (deferred)
# pbkdf2 = "0.12"             # PBKDF2 for password-based key derivation (deferred)
```

irpc is always a dependency (not behind a feature flag). Per ADR-027, irpc
in alknet-secret and alknet-storage is not feature-gated because these crates
are used in production deployments where the service layer is always active.
`irpc-derive` provides the `#[rpc_requests]` proc-macro that generates
`SecretMessage` and channel plumbing. `tokio` is needed for the
`SecretServiceActor` message loop (async channel receivers and task spawning).

The `secp256k1` crate is feature-gated behind the `secp256k1` feature because
Ethereum/BIP-0032 derivation is not needed in minimal deployments. Only
deployments that require `DeriveEthereumKey` should enable this feature. Note
that the crate name is `secp256k1` (the Rust library), not `libsecp256k1`
(the C library that the Rust crate wraps).

The `hkdf` and `pbkdf2` crates are deferred to Phase B. They will be needed for
salt-based key stretching when key rotation is implemented (see
[EncryptedData.salt](#aes-256-gcm-encryption-for-external-credentials)).

### Crate Interface (Public API)

The crate exposes these types as its stable public interface:

```rust
// Core types (always available)
pub use mnemonic::{Mnemonic, Language, Seed};
pub use derivation::{ExtendedPrivKey, DerivationError, PATHS};
pub use encryption::{EncryptedData, EncryptionError};
pub use protocol::{SecretProtocol, DerivedKey, KeyType, SecretMessage};
pub use service::{SecretService, SecretServiceHandle, SecretServiceActor, SecretServiceError};
pub use cache::CacheConfig;

// secp256k1 types (behind feature flag)
#[cfg(feature = "secp256k1")]
pub use ethereum::Secp256k1ExtendedPrivKey;
```

Other crates consume this interface:
- **alknet-storage** references `EncryptedData` for wire format compatibility
  (type-level, not a crate dependency)
- **alknet** (CLI binary) assembles `SecretService` and wires it to the
  `OperationEnv`
- **alknet-core** never depends on alknet-secret; `CredentialProvider` stub
  returns `None` until Phase A wiring

### Security Model

Per ADR-038 (seed lifecycle and memory security):

| State | What's in memory | What's on disk |
|-------|-----------------|---------------|
| Locked | Nothing | Encrypted database, derivation path metadata |
| Unlocked | Master seed in zeroize-protected RAM | Same (seed is never persisted) |
| After use | Derived keys cached in zeroize-protected RAM | Derivation paths only |

The seed phrase is entered once (at node startup or via `Unlock`), held only in
RAM, and never written to disk. `Lock` calls `zeroize()` on the seed and all
cached derived keys. The `SecretService` uses `Zeroize`-derived types for all
sensitive material.

#### Key Caching

Per OQ-SVC-04 (resolved), derived keys are cached in RAM with the following
properties:

- **Cache key**: The derivation path string (e.g., `m/74'/0'/0'/0'`). This
  uniquely identifies a derived key — the same path always produces the same
  key from the same seed.
- **TTL**: 1 hour (configurable). Cached entries expire after the TTL elapses,
  forcing re-derivation from the seed on next access.
- **Eviction policy**: LRU (least recently used). When the cache exceeds its
  maximum size, the least recently accessed entry is evicted.
- **Clearing**: The entire cache is cleared on `Lock`, and all entries are
  zeroized before removal per ADR-038.
- **Implementation**: The cache lives in `cache.rs` as an LRU map from
  derivation path to `Zeroize`-protected key bytes.

The cache avoids redundant derivation for frequently used keys (identity,
encryption) while ensuring that `Lock` purges all sensitive material.

### Key Derivation

#### BIP39 Mnemonic and Seed Derivation

```rust
let mnemonic = Mnemonic::from_phrase(&phrase, Language::English)?;
let seed = mnemonic.to_seed(None); // or Some("passphrase")
let key = derive_path_from_seed(seed.as_bytes(), PATHS::IDENTITY)?;
```

#### SLIP-0010 Ed25519 HD Key Derivation

The `74'` coin type is unallocated per SLIP-0044 and reserved for alknet.

#### Derivation Path Constants

| Path | Purpose | Curve/Algorithm |
|------|---------|----------------|
| `m/74'/0'/0'/0'` | Primary identity keypair | Ed25519 (alknet auth) |
| `m/74'/0'/0'/{n}'` | Worker/device identity | Ed25519 |
| `m/74'/0'/1'/0'` | SSH host key | Ed25519 |
| `m/74'/1'/0'/{hash}'` | Site-specific password | Deterministic (HMAC-SHA512) |
| `m/74'/2'/0'/0'` | Encryption key for external credentials | AES-256-GCM |
| `m/44'/60'/0'/0/0` | Ethereum signing key | secp256k1 |

These constants are defined in `derivation::PATHS` for programmatic access.

#### Password Derivation

`DerivePassword` produces a deterministic password from the seed using the
following algorithm:

1. Derive the extended private key at path `m/74'/1'/0'/{hash}'` using
   SLIP-0010 (HMAC-SHA512 with key "ed25519 seed"), where `{hash}'` is a
   site-specific hardened index derived from the site identifier.
2. Take the HMAC-SHA512 output (64 bytes) at that derivation level.
3. Truncate to the requested `length` bytes.
4. Encode as Base64url (RFC 4648 §5, no padding).

This produces a URL-safe, deterministic password of the requested length. v1
does not impose a special character set — the Base64url alphabet (`A-Z`,
`a-z`, `0-9`, `-`, `_`) provides sufficient entropy. If a specific character
set is required in the future, a versioned path can be introduced
(e.g., `m/74'/1'/1'/{hash}'`).

The `SecretServiceHandle` provides two methods for password derivation:
- `derive_password(path, length)` → `Vec<u8>` (raw truncated bytes)
- `derive_password_string(path, length)` → `String` (Base64url-encoded)

The irpc `DerivePassword` variant returns raw bytes (`Vec<u8>`). Consumers
who need a string representation can Base64url-encode the result.

#### secp256k1 Derivation (Ethereum)

`DeriveEthereumKey` uses **BIP-0032** (not SLIP-0010) at path
`m/44'/60'/0'/0/0`. This is a fundamentally different derivation algorithm from
Ed25519:

- SLIP-0010 (Ed25519) uses HMAC-SHA512 with key "ed25519 seed" and only
  supports hardened child derivation.
- BIP-0032 (secp256k1) uses HMAC-SHA512 with key "Bitcoin seed" and supports
  both hardened and unhardened child derivation.

The Ethereum path contains unhardened indices (`0/0`), which are invalid under
SLIP-0010. The `alknet-secret` crate gates secp256k1 derivation behind a
`secp256k1` feature flag, which pulls in the `libsecp256k1` crate. Deployments
that do not need Ethereum signing can omit this feature to avoid the
dependency.

#### DerivedKey Security Properties

Per ADR-038, the `private_key` field of `DerivedKey` must derive `Zeroize` and
use `#[zeroize(drop)]` to ensure sensitive key material is overwritten before
deallocation:

```rust
#[derive(Zeroize, Deserialize)]
#[zeroize(drop)]
pub struct DerivedKey {
    #[zeroize(skip)]
    pub key_type: KeyType,
    #[zeroize]
    #[serde(deserialize_with = "deserialize_private_key")]
    pub private_key: Vec<u8>,
    #[zeroize(skip)]
    pub public_key: Vec<u8>,
}
```

`DerivedKey` is **move-only** — it does not implement `Clone`. This is a
stronger security property than manual `Clone` with zeroization of the source:
a move-only type cannot be accidentally duplicated, and the `#[zeroize(drop)]`
annotation ensures the `private_key` is zeroized when the key goes out of scope.
There is no risk of use-after-zeroize from a manual `clone()` that destroys
the source.

Serialization redacts `private_key` in human-readable formats (JSON shows
`"[REDACTED]"`) but preserves the actual bytes in binary formats (postcard) so
that irpc remote communication works correctly. Deserialization always reads
the full bytes.

### AES-256-GCM Encryption for External Credentials

External credentials (API keys, OAuth tokens) that cannot be derived are
encrypted using a key derived from the seed at path `m/74'/2'/0'/0'`. The
`EncryptedData` type stores the key version, salt, IV, and ciphertext.

1. The secret service derives an AES-256-GCM key via path `m/74'/2'/0'/0'`
2. External credentials are encrypted with this key
3. The encrypted data is stored as a `SecretNode` in the metagraph
4. Only the derivation path and key version are stored in plain attributes
5. The seed phrase (or derived encryption key) is held only by the secret
   service — never in the database

#### EncryptedData.salt — Reserved for Future KDF-Based Key Rotation

In v1, the encryption key is derived directly from the seed at path
`m/74'/2'/0'/0'` without any salt-based key derivation. The `salt` field in
`EncryptedData` is **reserved for future KDF-based key rotation** (Phase B):

- The salt is generated randomly (32 bytes) and stored in `EncryptedData.salt`
  for forward compatibility, but it is **not used** in the v1 key derivation
  process.
- When key rotation is implemented, the salt will be used as input to HKDF or
  PBKDF2 for stretch-based key derivation, allowing the same seed to produce
  different encryption keys without changing the derivation path.
- This design ensures that the wire format does not need to change when key
  rotation is introduced — the `salt` field is already present and populated.

The `hkdf` and `pbkdf2` crates are listed as future dependencies in the
`Dependencies` section but are not included in v1.

### SecretProtocol irpc Service

```rust
#[rpc_requests(message = SecretMessage)]
#[derive(Debug, Serialize, Deserialize)]
enum SecretProtocol {
    #[rpc(tx=oneshot::Sender<DerivedKey>)]
    #[wrap(DeriveEd25519)]
    DeriveEd25519 { path: String },

    #[rpc(tx=oneshot::Sender<DerivedKey>)]
    #[wrap(DeriveEncryptionKey)]
    DeriveEncryptionKey { path: String },

    #[rpc(tx=oneshot::Sender<DerivedKey>)]
    #[wrap(DeriveEthereumKey)]
    DeriveEthereumKey { path: String },

    #[rpc(tx=oneshot::Sender<Vec<u8>>)]
    #[wrap(DerivePassword)]
    DerivePassword { path: String, length: usize },

    #[rpc(tx=oneshot::Sender<EncryptedData>)]
    #[wrap(Encrypt)]
    Encrypt { plaintext: String, key_version: u32 },

    #[rpc(tx=oneshot::Sender<String>)]
    #[wrap(Decrypt)]
    Decrypt { encrypted: EncryptedData },

    #[rpc(tx=oneshot::Sender<()>)]
    #[wrap(Lock)]
    Lock,

    #[rpc(tx=oneshot::Sender<()>)]
    #[wrap(Unlock)]
    Unlock { mnemonic: String, passphrase: Option<String> },
```

**Note**: The `Unlock` variant carries both the mnemonic phrase and an optional
BIP39 passphrase. The `mnemonic` field is the space-separated BIP39 word list.
The `passphrase` field is the optional BIP39 password extension (sometimes
called the "25th word"). Most deployments use `passphrase: None`, but the field
is available for users who need additional security beyond the mnemonic alone.

> **Implementation gap**: The current code has `Unlock { passphrase: String }`
> with only a single field (the mnemonic), and the actor handler passes `None`
> for the BIP39 passphrase. This needs to be updated to match the spec above.
> See the `unlock-passphrase-gap` task.

#### irpc Integration Model

The `SecretProtocol` enum defines the **wire protocol** — the set of operations
the secret service supports. The `#[rpc_requests(message = SecretMessage)]`
macro generates `SecretMessage` as the irpc wire type, which comes in two
variants:

- `SecretMessage::Request`: serialized form for remote (QUIC) communication,
  using postcard encoding.
- `SecretMessage::RequestWithChannels`: local form with `oneshot::Sender`
  channels for in-process communication.

There are two dispatch paths for consuming the secret service:

1. **Local (in-process)**: `SecretServiceHandle` wraps `SecretServiceInner`
   behind `Arc<RwLock<>>` and provides direct method calls
   (`derive_ed25519()`, `encrypt()`, etc.) without any serialization overhead.
   This is the path used by the CLI binary and single-node deployments. No irpc
   message passing is involved — the handle calls the implementation directly.

2. **Remote (in-cluster)**: `Client<SecretProtocol>` connects to the secret
   service node via irpc over QUIC. The client sends `SecretMessage::Request`
   messages (postcard-serialized) and receives responses. Workers on remote
   nodes use this path. The seed never leaves the secret service node — only
   derived keys are transmitted.

The `SecretServiceActor` processes incoming `SecretMessage` variants by
dispatching to the corresponding `SecretServiceHandle` methods. It provides
a `spawn(handle)` convenience method that creates an mpsc channel, spawns the
actor on a tokio task, and returns a `(Client<SecretProtocol>, SecretServiceActor)`
tuple for immediate use.

The `SecretService` type owns the irpc service handler and a
`SecretServiceHandle`. It dispatches incoming `SecretMessage` variants to the
handle's methods. For call protocol exposure (e.g., `/head/secrets/derive`),
the service is wrapped in an operation that serializes to JSON.

### Wire Format Compatibility with alknet-storage

The `EncryptedData` type (`key_version`, `salt`, `iv`, `data`) is the stable
wire format shared with alknet-storage. This is type-level compatibility — not a
crate dependency. alknet-storage stores encrypted nodes using this format;
alknet-secret encrypts and decrypts using this format.

The Rust `EncryptedData` struct in alknet-secret is a superset of the TypeScript
`EncryptedDataSchema` from `@alkdev/storage`. Migration path: re-encrypt
TypeScript-encrypted data using the Rust secret service with a new key version.
The wire format is stable — future key rotation will use the existing `salt`
field rather than adding new fields (see OQ-SVC-03).

### Deployment Topologies

**Minimal (single node, CLI)**: Secret service runs in the same process. Seed
phrase entered at startup. All keys derived locally via `SecretServiceHandle`.
No irpc overhead.

**Production (head node)**: Secret service runs on a dedicated node or as a
local irpc service. Workers request derived keys via `Client<SecretProtocol>`
over QUIC. The seed never leaves the secret service node.

### Test Vectors

Known-answer tests are required against published test vectors to verify
correctness of the cryptographic implementations:

#### BIP39 Test Vectors

The `mnemonic` module must produce identical output to the BIP39 reference
test vectors:

- Given a known mnemonic phrase and passphrase, the derived seed must match
  the reference output byte-for-byte.
- Test vectors from
  [BIP39 reference](https://github.com/bitcoin/bips/blob/master/bip-0039.mediawiki)
  and the `bip39` crate's own test suite.

#### SLIP-0010 Test Vectors

The `derivation` module must produce identical output to the SLIP-0010 reference
test vectors:

- Given a known seed, the derived master key (private key + chain code) must
  match the SLIP-0010 reference output.
- Given a known master key, the derived child key at path `m/74'/0'/0'/0'`
  must match the reference output.
- Test vectors from
  [SLIP-0010 reference](https://github.com/satoshilabs/slips/blob/master/slip-0010.md).

#### AES-256-GCM Test Vectors

The `encryption` module must produce identical results to published AES-256-GCM
test vectors:

- Given a known key, IV, and plaintext, the ciphertext must match the reference
  output.
- Use IEEE P802.1ASck or NIST SP 800-38D test vectors.
- Round-trip encryption/decryption must always succeed for valid inputs.

These tests ensure that the implementation is correct and compatible with
other BIP39/SLIP-0010/AES-256-GCM implementations. They are placed in
`tests/test_vectors.rs`.

## Constraints

- The seed phrase is never persisted to disk. It is entered at startup or via
  `Unlock` and held only in `Zeroize`-protected RAM (ADR-038).
- `Lock` calls `zeroize()` on the seed and all cached derived keys. The key
  cache is fully cleared and zeroized on `Lock` (OQ-SVC-04, resolved).
- alknet-secret does not depend on alknet-core or alknet-storage. It is fully
  independent (ADR-027).
- The `EncryptedData` wire format is shared with alknet-storage for type-level
  compatibility, not a crate dependency.
- Per ADR-032, secret service domain events (key derivation notifications) stay
  within the service boundary. External consumers use irpc calls or call
  protocol operations projected to integration events.
- irpc is always a dependency (not feature-gated) per ADR-027.
- `SecretProtocol` defines the wire format for in-cluster communication
  (postcard serialization). For call protocol exposure (e.g.,
  `/head/secrets/derive`), the service is wrapped in an operation that
  serializes to JSON.
- `DerivedKey.private_key` must derive `Zeroize` per ADR-038. `DerivedKey`
  is move-only (not `Clone`) — this is stronger than manual Clone with
  zeroization of the source, as it prevents accidental duplication.
- secp256k1 (Ethereum) derivation is gated behind the `secp256k1` feature flag
  because it requires a different derivation algorithm (BIP-0032) and an
  additional dependency (`secp256k1`).

## Phase Progression

| Phase | Scope | Notes |
|-------|-------|-------|
| Phase 3 (now) | Basic crate: mnemonic, derivation, encryption, irpc protocol, service lifecycle, key caching | Core key management |
| Phase A | Integration with alknet-storage via `EncryptedData` wire format. CLI commands for unlock/lock/derive. `SecretStoreCredentialProvider` wiring. | Full service integration |
| Phase B | Memory hardening: `mlock`/`VirtualLock` for seed RAM, constant-time comparison, audit logging of derivation requests. Key rotation: KDF-based key derivation using `EncryptedData.salt` with HKDF/PBKDF2. | Security hardening |
| Phase C | Multi-seed support (tenant isolation): indexed `Unlock` with tenant ID. | Multi-tenancy |

## Open Questions

- **OQ-SVC-01**: Should the secret service support multiple seed phrases (one
  per tenant)? See [open-questions.md](open-questions.md).

- **OQ-SVC-03**: How does the secret service integrate with the existing
  `EncryptedDataSchema` from `@alkdev/storage`? **Resolution**: The wire format
  is stable. `EncryptedData` (`key_version`, `salt`, `iv`, `data`) is shared
  type-level between alknet-secret and alknet-storage. The migration path is
  re-encryption with a new key version. The `salt` field is reserved for future
  KDF-based key rotation (see Phase B). See [open-questions.md](open-questions.md).

- **OQ-SVC-04**: Should workers cache derived keys locally? **Resolution**: Yes.
  Derived keys are cached in RAM using an LRU cache keyed by derivation path,
  with a TTL of 1 hour (configurable). The cache is fully cleared and zeroized
  on `Lock`. This avoids redundant derivation for frequently used keys while
  ensuring that `Lock` purges all sensitive material. See [open-questions.md](open-questions.md).

- **OQ-SEC-01**: Should alknet-secret use `mlock`/`VirtualLock` to prevent seed
  RAM from being paged to disk? See [open-questions.md](open-questions.md).
  Deferred to Phase B per ADR-038.

## Design Decisions

| ADR | Decision | Summary |
|-----|----------|---------|
| [027](decisions/027-crate-decomposition.md) | Crate decomposition | alknet-secret is independent of core and storage |
| [032](decisions/032-event-boundary-discipline.md) | Event boundary | Secret service domain events stay internal |
| [038](decisions/038-seed-lifecycle-memory-security.md) | Seed lifecycle and memory security | Zeroize for sensitive material, mlock deferred to Phase B |

## References

- [research/services.md](../research/services.md) — SecretProtocol definition, DerivedKey, KeyType
- [research/storage.md](../research/storage.md) — Secrets section, derivation paths, EncryptedData
- [research/integration-plan.md](../research/integration-plan.md) — Phase 3.1
- [credentials.md](credentials.md) — CredentialProvider (outbound auth, consumes SecretProtocol::Decrypt)
- SLIP-0010 — https://github.com/satoshilabs/slips/blob/master/slip-0010.md
- BIP39 — https://github.com/bitcoin/bips/blob/master/bip-0039.mediawiki
- BIP-0032 — https://github.com/bitcoin/bips/blob/master/bip-0032.mediawiki
- NIST SP 800-38D — AES-GCM test vectors