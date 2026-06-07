---
status: draft
last_updated: 2026-06-07
---

# Secret Service

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
and derived keys are provided on demand through an irpc service interface.

## Architecture

### Security Model

| State | What's in memory | What's on disk |
|-------|-----------------|---------------|
| Locked | Nothing | Encrypted database, derivation path metadata |
| Unlocked | Master seed in RAM | Same (seed is never persisted) |
| After use | Derived keys cached in RAM | Derivation paths only |

The seed phrase is entered once (at node startup or via `Unlock` call), held
only in RAM, and never written to disk. The `Lock` call purges the seed and all
cached derived keys from memory.

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
    Unlock { passphrase: String },
}

#[derive(Debug, Serialize, Deserialize)]
struct DerivedKey {
    key_type: KeyType,
    private_key: Vec<u8>,
    public_key: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
enum KeyType {
    Ed25519,
    Aes256Gcm,
    Secp256k1,
}

#[derive(Debug, Serialize, Deserialize)]
struct EncryptedData {
    key_version: u32,
    salt: String,   // Base64-encoded
    iv: String,     // Base64-encoded
    data: String,   // Base64-encoded
}
```

### BIP39 Mnemonic and Seed Derivation

```rust
let mnemonic = Mnemonic::from_phrase(&phrase, Language::English)?;
let seed = mnemonic.to_seed(Some(&passphrase));
let master_key = ExtendedPrivKey::new_master(Network::Alknet, &seed)?;
```

### SLIP-0010 Ed25519 HD Key Derivation

The `74'` coin type is unallocated per SLIP-0044 and reserved for alknet.

### Derivation Path Constants

| Path | Purpose | Curve/Algorithm |
|------|---------|----------------|
| `m/74'/0'/0'/0'` | Primary identity keypair | Ed25519 (alknet auth) |
| `m/74'/0'/0'/{n}'` | Worker/device identity | Ed25519 |
| `m/74'/0'/1'/0'` | SSH host key | Ed25519 |
| `m/74'/1'/0'/{hash}'` | Site-specific password | Deterministic |
| `m/74'/2'/0'/0'` | Encryption key for external credentials | AES-256-GCM |
| `m/44'/60'/0'/0/0` | Ethereum signing key | secp256k1 |

### AES-256-GCM Encryption for External Credentials

External credentials (API keys, OAuth tokens) that cannot be derived are
encrypted using a key derived from the seed at path `m/74'/2'/0'/0'`. The
`EncryptedData` type stores the key version, salt, IV, and ciphertext. This
format is compatible with the existing `@alkdev/storage` `EncryptedDataSchema`.

1. The secret service derives an AES-256-GCM key via path `m/74'/2'/0'/0'`
2. External credentials are encrypted with this key
3. The encrypted data is stored as a `SecretNode` in the metagraph
4. Only the derivation path and key version are stored in plain attributes
5. The seed phrase (or derived encryption key) is held only by the secret
   service — never in the database

### Deployment Topologies

**Minimal (single node, CLI)**: Secret service runs in the same process. Seed
phrase entered at startup. All keys derived locally. No irpc overhead.

**Production (head node)**: Secret service runs on a dedicated node or as a
local irpc service. Workers request derived keys via irpc over QUIC. The seed
never leaves the secret service node.

## Constraints

- The seed phrase is never persisted to disk. It is entered at startup or via
  `Unlock` and held only in RAM.
- `Lock` purges the seed and all cached derived keys from memory.
- alknet-secret does not depend on alknet-core or alknet-storage. It is fully
  independent.
- The `EncryptedData` wire format (key_version, salt, iv, data) is shared with
  alknet-storage for compatibility, but this is type-level compatibility — not a
  crate dependency.
- Per ADR-032, the secret service's Honker streams (key derivation notifications)
  stay within the service boundary. External consumers use irpc calls or call
  protocol operations that project to integration events.
- The irpc service defines the wire format for in-cluster communication
  (postcard serialization). For call protocol exposure (e.g.,
  `/head/secrets/derive`), the service is wrapped in an operation that serializes
  to JSON.

## Open Questions

- **OQ-SVC-01**: Should the secret service support multiple seed phrases (one
  per tenant)? See [open-questions.md](open-questions.md).

- **OQ-SVC-02**: Should service protocols use postcard (binary) or JSON for
  remote calls? See [open-questions.md](open-questions.md).

- **OQ-SVC-03**: How does the secret service integrate with the existing
  `EncryptedDataSchema` from `@alkdev/storage`? See [open-questions.md](open-questions.md).

- **OQ-SVC-04**: Should workers cache derived keys locally? See [open-questions.md](open-questions.md).

## Design Decisions

| ADR | Decision | Summary |
|-----|----------|---------|
| [027](decisions/027-crate-decomposition.md) | Crate decomposition | alknet-secret is independent of core and storage |
| [032](decisions/032-event-boundary-discipline.md) | Event boundary | Secret service domain events stay internal |

## References

- [research/services.md](../research/services.md) — SecretProtocol definition, DerivedKey, KeyType
- [research/storage.md](../research/storage.md) — Secrets section, derivation paths, EncryptedData
- [research/integration-plan.md](../research/integration-plan.md) — Phase 2.1
- SLIP-0010 — https://github.com/satoshilabs/slips/blob/master/slip-0010.md
- BIP39 — https://github.com/bitcoin/bips/blob/master/bip-0039.mediawiki