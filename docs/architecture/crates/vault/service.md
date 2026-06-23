---
status: draft
last_updated: 2026-06-22-25
---

# Service

The `VaultServiceHandle` runtime API: unlock/lock lifecycle, key
derivation, encryption, caching, and the direct method-call dispatch
path.

## What

The service layer wraps the vault's cryptographic primitives in a
stateful runtime with a clear lifecycle. It holds the master seed in
`Zeroize`-protected memory and provides methods for the unlock/lock
lifecycle, key derivation, and encryption/decryption.

This is the API the assembly layer (CLI binary) calls. No other component
calls these methods directly (ADR-019). The vault is local-only by
construction (ADR-025) — direct method calls, no actor, no message enum,
no remote dispatch.

## VaultServiceHandle

The primary API for local (in-process) use. Thread-safe via
`Arc<RwLock<VaultServiceInner>>`.

```rust
#[derive(Clone)]
pub struct VaultServiceHandle {
    inner: Arc<RwLock<VaultServiceInner>>,
}

struct VaultServiceInner {
    mnemonic: Option<Mnemonic>,  // None if locked
    seed: Option<Seed>,         // None if locked
    unlocked: bool,
    cache: KeyCache,            // TTL + LRU, see Cache section
}
```

`VaultServiceHandle` is `Clone` — cloning shares the underlying state via
`Arc`. This is how the actor and the assembly layer share the same vault.

## Lifecycle

```
Locked (initial state)
  │
  │ unlock(phrase, passphrase) / unlock_new(word_count)
  ▼
Unlocked — derive, encrypt, decrypt available
  │
  │ lock()
  ▼
Locked — seed and cache purged
```

### unlock(phrase, passphrase)

```rust
pub fn unlock(&self, phrase: &str, passphrase: Option<&str>) -> Result<(), VaultServiceError>;
```

Unlock with an existing mnemonic phrase. Validates the phrase against the
BIP39 word list, derives the seed, and stores both in `VaultServiceInner`.
Returns `AlreadyUnlocked` if the vault is already unlocked.

The passphrase is the BIP39 password extension (the "25th word"). `None`
means no passphrase (equivalent to empty string). Different passphrases
produce different seeds.

### unlock_new(word_count) → phrase

```rust
pub fn unlock_new(&self, word_count: usize) -> Result<Zeroizing<String>, VaultServiceError>;
```

Generate a new random mnemonic, unlock with it, and return the phrase as
a `Zeroizing<String>`. The returned phrase is the root of trust — it is
heap-allocated and zeroized on drop, so it does not linger in freed
memory. The caller should extract the phrase for secure storage (write
down, display to user) and let the `Zeroizing<String>` drop when done.
Do not clone the returned value or store it in a non-zeroizing container.
Supported word counts: 12, 15, 18, 21, 24.

This is the "first run" path — a new node generates its mnemonic, writes
it down, and the vault is unlocked for the process lifetime. The
`Zeroizing<String>` wrapper (from the `zeroize` crate) ensures the
mnemonic is wiped from memory once the caller is done with it, matching
the `Mnemonic` type's own `ZeroizeOnDrop` behavior. This resolves review
#002 W7.

### lock()

```rust
pub fn lock(&self);
```

Purge the seed, mnemonic, and all cached derived keys. Calls `zeroize()`
on all sensitive material. After locking, no derive/encrypt/decrypt
operations are possible until `unlock` is called again.

`lock()` on an already-locked service is a no-op (not an error).

### is_unlocked()

```rust
pub fn is_unlocked(&self) -> bool;
```

Check whether the vault is currently unlocked. Cheap (read lock only).

## Derive Methods

All derive methods require an unlocked vault and return
`VaultServiceError::VaultLocked` if called while locked.

### derive_ed25519(path) → DerivedKey

```rust
pub fn derive_ed25519(&self, path: &str) -> Result<DerivedKey, VaultServiceError>;
```

Derive an Ed25519 keypair at the given SLIP-0010 path. Checks the cache
first; on a miss, derives from the seed and caches the result. Returns a
`DerivedKey` with `KeyType::Ed25519`.

### derive_encryption_key(path) → DerivedKey

```rust
pub fn derive_encryption_key(&self, path: &str) -> Result<DerivedKey, VaultServiceError>;
```

Derive an AES-256-GCM encryption key at the given path. Same cache
behavior as `derive_ed25519`. Returns a `DerivedKey` with
`KeyType::Aes256Gcm`.

### derive_encryption_key_for_version(version) → EncryptionKey

```rust
pub fn derive_encryption_key_for_version(&self, version: u32) -> Result<EncryptionKey, VaultServiceError>;
```

Derive the encryption key for a specific key version. Maps the version to
its derivation path via `encryption_path_for_version(version)` (ADR-021):
v2 → `m/74'/2'/0'/0'`, v3 → `m/74'/2'/0'/1'`, etc. Cached by path. This is
the version-aware method that `decrypt` uses to select the correct key for
each blob — see [encryption.md](encryption.md) and ADR-021.

`derive_encryption_key(path)` (above) remains as the path-based API for
deriving at arbitrary paths. `derive_encryption_key_for_version(version)`
is the version-aware API used by `encrypt` and `decrypt`. The two share
the same cache (keyed by derivation path).

### derive_ethereum_key(path) → DerivedKey (feature-gated)

```rust
pub fn derive_ethereum_key(&self, path: &str) -> Result<DerivedKey, VaultServiceError>;
```

Derive a secp256k1 keypair at the given BIP-0032 path. Returns
`UnsupportedKeyType` when the `secp256k1` feature is disabled. Returns a
`DerivedKey` with `KeyType::Secp256k1` (33-byte compressed public key).

## Encrypt and Decrypt

### encrypt(plaintext, key_version) → EncryptedData

```rust
pub fn encrypt(&self, plaintext: &str, key_version: u32) -> Result<EncryptedData, VaultServiceError>;
```

Encrypt plaintext using the encryption key derived at `PATHS::ENCRYPTION`.
Derives (and caches) the encryption key on first call, then uses the cache
for subsequent calls. See [encryption.md](encryption.md) for the
cryptographic details.

### decrypt(encrypted) → String

```rust
pub fn decrypt(&self, encrypted: &EncryptedData) -> Result<String, VaultServiceError>;
```

Decrypt an `EncryptedData` blob. Derives (and caches) the encryption key
at the version-indexed path indicated by `encrypted.key_version` via
`derive_encryption_key_for_version` (ADR-021). Each version maps to a
distinct path (`m/74'/2'/0'/{version-2}'`), so old and new keys can
coexist during partial rotation. See [encryption.md](encryption.md).

### rotate(encrypted, to_version) → EncryptedData

```rust
pub fn rotate(&self, encrypted: &EncryptedData, to_version: u32) -> Result<EncryptedData, VaultServiceError>;
```

Re-encrypt an `EncryptedData` blob from its current key version to a new
version. Decrypts with the old version's key, re-encrypts with the new
version's key. Returns the new `EncryptedData` — the caller replaces the
blob in storage. No new mnemonic needed; the same seed produces all
version keys via different derivation paths (ADR-021).

This is the rotation primitive. The assembly layer or a migration tool
iterates stored blobs and calls `rotate` on each. The vault does not
self-rotate — rotation is an operational action.

## Cache

Derived keys are cached for performance — HD derivation involves HMAC
operations that are not free. The cache is keyed by derivation path and
has TTL-based expiry and LRU eviction.

```rust
pub struct KeyCache {
    entries: HashMap<String, CachedKey>,
    order: Vec<String>,         // LRU ordering
    config: CacheConfig,
}

pub struct CacheConfig {
    pub ttl: Duration,          // default: 1 hour
    pub max_entries: usize,     // default: 64
}
```

- **TTL**: entries expire after `ttl` (default 1 hour). Expired entries are
  evicted lazily on access (`get` checks expiry) or via `evict_expired()`.
- **LRU**: when the cache exceeds `max_entries` (default 64), the least
  recently used entry is evicted. Access (`get`) updates the LRU order.
- **Zeroized**: `CachedKey` derives `Zeroize` and `ZeroizeOnDrop`. Evicted
  and cleared entries are zeroized — derived private keys do not linger in
  freed heap memory.
- **Cleared on lock**: `lock()` calls `cache.clear()`, which removes and
  zeroizes all entries.

### What is and isn't cached

| Operation | Cached? | Why |
|-----------|---------|-----|
| `derive_ed25519` | Yes | Derivation is expensive; keys are reused |
| `derive_encryption_key` | Yes | Same — encryption key reused across calls |
| `derive_ethereum_key` | Yes | Same |
| `encrypt` / `decrypt` | Key cached | The encryption key (at `PATHS::ENCRYPTION`) is cached; the plaintext is not |

## Dispatch

The vault uses **direct method calls** on `VaultServiceHandle` — no actor,
no message enum, no channels, no serialization (ADR-025). The handle is
`Arc<RwLock<VaultServiceInner>>` — clone it, share it, call methods
directly. The RwLock provides concurrent reads (derive operations) and
exclusive writes (unlock/lock).

```
Assembly layer (CLI binary):
  1. Create VaultServiceHandle
  2. Unlock with mnemonic (local, from secure prompt or file)
  3. Call derive/encrypt/decrypt methods directly
  4. Extract bytes, construct alknet-core types at the assembly boundary
  5. Inject into handler capabilities (ADR-014)
```

There is no `VaultProtocol` enum, no `VaultServiceActor`, no `Client<S>`,
and no remote dispatch capability. The vault is local-only by
construction (ADR-025). If remote vault access is ever needed, it requires
a separate vault-server crate with its own ADR (OQ-021, ADR-025).

The pre-ADR-025 design had an actor path (mpsc channel + oneshot
backchannels, using irpc's `Service` trait) that was described as
"secondary" to direct calls. ADR-025 removed it — the actor existed only
to make irpc's dispatch work, and the direct path was always preferred.
The RwLock-based concurrency model is both simpler and better for
throughput (concurrent reads vs. sequential processing).

## Errors

```rust
#[derive(Debug, thiserror::Error)]
pub enum VaultServiceError {
    VaultLocked,          // called derive/encrypt/decrypt while locked
    AlreadyUnlocked,      // called unlock while already unlocked
    Mnemonic(String),     // mnemonic generation/validation failed
    Derivation(String),   // HD derivation failed (bad path, HMAC error)
    Encryption(String),    // AES-GCM encrypt/decrypt failed
    InvalidPath(String),   // derivation path is malformed
    UnsupportedKeyType,   // secp256k1 called without the feature
}
```

`VaultServiceError` is a plain `thiserror::Error` enum (ADR-025 dropped
the `Serialize`/`Deserialize` derives that were needed for irpc dispatch).
It wraps sub-errors as strings. The CLI binary converts vault errors to
alknet-core error types at the assembly boundary (ADR-018).

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| Assembly layer is the sole caller | [ADR-019](../../decisions/019-vault-assembly-layer-only.md) | Handlers never hold a vault reference |
| Encryption key via HD derivation | [ADR-020](../../decisions/020-hd-derivation-for-encryption-keys.md) | Seed-derived key at `m/74'/2'/0'/0'`, not PBKDF2 |
| Version-indexed paths for rotation | [ADR-021](../../decisions/021-key-rotation-via-version-indexed-paths.md) | `decrypt` selects key by version; `rotate` re-encrypts |
| RwLock for thread safety | — | Multiple readers (derive), exclusive writer (unlock/lock) |
| TTL + LRU cache | — | Bounded memory, fresh keys, zeroized eviction |
| Direct method calls (no actor) | [ADR-025](../../decisions/025-vault-local-only-dispatch.md) | No irpc, no message enum, no remote dispatch capability |
| `derive_password` removed | [ADR-025](../../decisions/025-vault-local-only-dispatch.md) | Password-manager pattern not relevant to RPC system's vault; resolves C9 |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-21** (resolved by ADR-025): Remote vault access is not a feature
  of the vault crate. The vault is local-only by construction — direct
  method calls on `VaultServiceHandle`, no remote dispatch capability.
  If remote access is ever needed, it requires a separate vault-server
  crate with its own ADR. See [protocol.md → Local-Only by
  Construction](protocol.md#local-only-by-construction).

## Security Constraints

These are security-critical implementation requirements, not
architectural decisions. They are documented here so implementation agents
don't miss them.

- **OsRng for IVs**: AES-GCM IVs and any cryptographic nonces must use
  `OsRng` (or equivalent CSPRNG), not `rand::random()`. IV reuse under the
  same key is catastrophic for GCM (authenticity breaks, two-time-pad on
  plaintext). **The current source uses `rand::random()` for IV generation
  in `encryption::encrypt()` — this is a known drift and must be corrected
  during implementation sync.**
- **Zeroized drop**: `Seed`, `Mnemonic`, `CachedKey`, `EncryptionKey`,
  `ExtendedPrivKey`, `Secp256k1ExtendedPrivKey`, and `DerivedKey` all
  derive `Zeroize` and `ZeroizeOnDrop`. The cache must clear on drop, not
  just on explicit `lock()`. **The current `KeyCache::clear()` removes
  entries but relies on `CachedKey`'s `Drop` impl for zeroization —
  verify that `HashMap::clear()` actually drops the values (it does, but
  this is worth a test).**
- **No `unwrap()` or `expect()` outside tests**: poisoned lock recovery
  uses `unwrap_or_else(|e| e.into_inner())` or explicit error propagation.
  A panic in one vault operation must not brick the vault for all other
  operations. **The current source uses `unwrap()` on every `RwLock`
  acquisition in `VaultServiceHandle` (lines 142, 161, 182, 191, 196, 227,
  264, 307, 340, 367) — this is a known drift and must be corrected. A
  poisoned lock should be recovered with `unwrap_or_else(|e|
  e.into_inner())`, not panicked.**
- **`DerivedKey` is move-only, not `Clone`**: `DerivedKey` does not derive
  `Clone`. It is move-only — consumers receive it by value and zeroize it
  when done (handled by `#[zeroize(drop)]`). This prevents accidental
  duplication of secret material. **The current source does not derive
  `Clone` on `DerivedKey` — this is correct.**
- **Cache eviction zeroizes**: when the cache evicts an entry (LRU or
  TTL), the `CachedKey` is dropped, which triggers `ZeroizeOnDrop`. Do not
  replace `CachedKey` with a type that doesn't zeroize.

## References

- Implementation: `crates/alknet-vault/src/service.rs`,
  `crates/alknet-vault/src/cache.rs`
- Tests: `crates/alknet-vault/tests/service_tests.rs`,
  `crates/alknet-vault/src/service.rs` (unit tests),
  `crates/alknet-vault/src/cache.rs` (unit tests)
- [protocol.md](protocol.md) — `DerivedKey` and `KeyType`
- [encryption.md](encryption.md) — `encrypt` / `decrypt` cryptographic details