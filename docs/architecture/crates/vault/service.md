---
status: draft
last_updated: 2026-06-19
---

# Service

The `VaultServiceHandle` runtime API: unlock/lock lifecycle, key
derivation, encryption, caching, and the actor dispatch path.

## What

The service layer wraps the vault's cryptographic primitives in a
stateful runtime with a clear lifecycle. It holds the master seed in
`Zeroize`-protected memory and provides methods for the unlock/lock
lifecycle, key derivation, and encryption/decryption.

This is the API the assembly layer (CLI binary) calls. No other component
calls these methods directly (ADR-019).

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
pub fn unlock_new(&self, word_count: usize) -> Result<String, VaultServiceError>;
```

Generate a new random mnemonic, unlock with it, and return the phrase.
Store the returned phrase securely — it is the root of trust. Supported
word counts: 12, 15, 18, 21, 24.

This is the "first run" path — a new node generates its mnemonic, writes
it down, and the vault is unlocked for the process lifetime.

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

### derive_ethereum_key(path) → DerivedKey (feature-gated)

```rust
pub fn derive_ethereum_key(&self, path: &str) -> Result<DerivedKey, VaultServiceError>;
```

Derive a secp256k1 keypair at the given BIP-0032 path. Returns
`UnsupportedKeyType` when the `secp256k1` feature is disabled. Returns a
`DerivedKey` with `KeyType::Secp256k1` (33-byte compressed public key).

### derive_password(path, length) → Vec<u8>

```rust
pub fn derive_password(&self, path: &str, length: usize) -> Result<Vec<u8>, VaultServiceError>;
pub fn derive_password_string(&self, path: &str, length: usize) -> Result<String, VaultServiceError>;
```

Derive deterministic password bytes at the given path, truncated to
`length`. This is **not cached** — password derivation is cheap and
passwords are typically one-shot (derive, use, discard). The string
variant base64url-encodes the bytes (URL-safe, no padding).

`derive_password` is the mechanism for per-site deterministic passwords:
the same seed + path always produces the same password. The path includes
a site hash (`site_password_path(site_hash)`) so different sites get
different passwords.

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

Decrypt an `EncryptedData` blob. Derives (and caches) the encryption key at
`PATHS::ENCRYPTION` if not already cached. The `encrypted.key_version` is
stamped onto the `EncryptionKey` for forward compatibility but **does not
select a different derivation path in v1** — the same key (at
`m/74'/2'/0'/0'`) decrypts any version. Path-per-version routing is a Phase
B concern (OQ-22). See [encryption.md](encryption.md).

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
| `derive_password` | No | Cheap derivation; passwords are one-shot |
| `encrypt` / `decrypt` | Key cached | The encryption key (at `PATHS::ENCRYPTION`) is cached; the plaintext is not |

`derive_password` does not cache because it's a truncation of derived
bytes, not a keypair that's reused. Caching it would grow the cache with
unique paths (one per site hash) for no reuse benefit.

## Actor Dispatch

The `VaultServiceActor` processes `VaultMessage` variants from an mpsc
channel and dispatches to `VaultServiceHandle` methods. This is the irpc
dispatch mechanism (ADR-005) — the in-process actor pattern that irpc
services use.

```rust
pub struct VaultServiceActor {
    handle: VaultServiceHandle,
}

impl VaultServiceActor {
    pub fn new(handle: VaultServiceHandle) -> Self;
    pub async fn run(mut self, mut rx: mpsc::Receiver<VaultMessage>);
    pub fn spawn(handle: VaultServiceHandle) -> (Client<VaultProtocol>, VaultServiceActor);
}
```

- `run(rx)`: Message loop. Each `VaultMessage` variant is dispatched to the
  corresponding handle method, and the response is sent through the oneshot
  channel embedded in the message. Consumes `self`.
- `spawn(handle)`: Spawn the actor as a `tokio::task` and return a
  `Client<VaultProtocol>` for sending messages. **Source bug: the current
  `spawn` implementation returns a fresh, unspawned `VaultServiceActor` as
  the second tuple element (the spawned actor is consumed by `run`). The
  returned actor has no channel and is non-functional. This should be
  corrected during implementation sync — either drop the second return
  value (return only `Client<VaultProtocol>`) or restructure the API so
  the returned actor is the one that was spawned.**

The actor pattern is the irpc dispatch mechanism (ADR-005). For local
in-process use, prefer `VaultServiceHandle` directly — no channel, no
serialization. The actor exists for irpc service dispatch, which is an
in-process pattern (the actor and the handle share state via `Arc`).

### Dispatch paths

| Path | Type | Serialization | Use case |
|------|------|---------------|----------|
| Direct (in-process) | `VaultServiceHandle` method calls | None | CLI binary at startup (the supported path) |
| Actor (in-process) | `VaultMessage` over mpsc | None (channel) | irpc service dispatch (in-process) |

Remote (in-cluster) vault dispatch — where the vault runs as a sidecar
and other processes send `VaultMessage` over a network — is **not
supported** (ADR-019, OQ-21). The irpc `RemoteService` trait infrastructure
exists in the library, but exposing the vault over the network would
require its own ADR with an explicit threat model (the master seed must
never cross the network). The dispatch table above lists only the
supported paths.

The assembly layer (CLI binary) uses the direct path. The actor path
exists for in-process irpc dispatch but is not used by the assembly layer
— it's available for test harnesses and future in-process service
patterns. Neither path is on the alknet call protocol (ADR-008, ADR-014).

## Errors

```rust
#[derive(Debug, thiserror::Error, Serialize, Deserialize)]
pub enum VaultServiceError {
    VaultLocked,          // called derive/encrypt/decrypt while locked
    AlreadyUnlocked,      // called unlock while already unlocked
    Mnemonic(String),     // mnemonic generation/validation failed
    Derivation(String),   // HD derivation failed (bad path, HMAC error)
    Encryption(String),   // AES-GCM encrypt/decrypt failed
    InvalidPath(String),  // derivation path is malformed
    UnsupportedKeyType,   // secp256k1 called without the feature
}
```

`VaultServiceError` is `Serialize`/`Deserialize` (for irpc dispatch) and
wraps sub-errors as strings. It does not implement `From` for alknet-core
error types — the CLI binary converts at the assembly boundary (ADR-018).

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| Assembly layer is the sole caller | [ADR-019](../../decisions/019-vault-assembly-layer-only.md) | Handlers never hold a vault reference |
| Encryption key via HD derivation | [ADR-020](../../decisions/020-hd-derivation-for-encryption-keys.md) | Seed-derived key at `m/74'/2'/0'/0'`, not PBKDF2 |
| RwLock for thread safety | — | Multiple readers (derive), exclusive writer (unlock/lock) |
| TTL + LRU cache | — | Bounded memory, fresh keys, zeroized eviction |
| Actor for in-process irpc dispatch | [ADR-005](../../decisions/005-irpc-as-call-protocol-foundation.md) | irpc message dispatch; not on the call protocol |
| `derive_password` not cached | — | One-shot; caching grows cache with no reuse |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-21** (deferred): Remote vault administration — network unlock is not
  supported; needs an ADR if ever needed.

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
- [protocol.md](protocol.md) — `VaultMessage` and `DerivedKey`
- [encryption.md](encryption.md) — `encrypt` / `decrypt` cryptographic details