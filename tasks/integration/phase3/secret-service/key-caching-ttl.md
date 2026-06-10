---
id: key-caching-ttl
name: Implement TTL-based key cache with LRU eviction for SecretService
status: pending
depends_on: [spec-update-secret-service, derivedkey-zeroize-security]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

The `SecretServiceHandle` currently re-derives keys from the seed on every call. The spec (after update) requires a TTL-based key cache with LRU eviction, cleared on `Lock`. This is the resolution of OQ-SVC-04.

The current `SecretServiceInner` holds:
- `mnemonic: Option<Mnemonic>`
- `seed: Option<Seed>`
- `unlocked: bool`

It needs to add:
- `cache: HashMap<String, CachedKey>` where the key is the derivation path string
- `cache_ttl: Duration` (default 1 hour, configurable)
- LRU eviction when cache exceeds a configurable max size

**Design considerations:**

- The cache key is the derivation path string (e.g., `m/74'/0'/0'/0'`). This means caching at the path level — if you derive the same path multiple times, you get the cached key (within TTL).
- The cached value must hold the derived key material zeroize-protected, not just the public key.
- TTL is checked on access, not via a background timer. Expired entries are evicted lazily.
- `Lock` clears the cache entirely and zeroizes all cached entries.
- Cache hits avoid re-derivation from seed, which is the main performance win.
- The cache must be behind `RwLock` (already used for `SecretServiceInner`).

**Cache entry structure:**

```rust
#[derive(Zeroize)]
#[zeroize(drop)]
struct CachedKey {
    derived_at: Instant,
    key_type: KeyType,
    private_key: Vec<u8>,  // Zeroize-protected
    public_key: Vec<u8>,
}
```

**Cache configuration:**

```rust
pub struct CacheConfig {
    pub ttl: Duration,        // Default: 1 hour
    pub max_entries: usize,   // Default: 64
}
```

**Behavior:**

- On `derive_*` call: check cache. If hit and `Instant::now() - derived_at < ttl`, return cached. If expired, evict and re-derive. If miss, derive and insert.
- On `Lock`: zeroize all cache entries, clear the HashMap, zeroize seed and mnemonic (existing behavior).
- On `Encrypt`/`Decrypt`: the encryption key at `PATHS::ENCRYPTION` is also cached (same path, same behavior).

**Implementation:**

Add a `cache` module to `alknet-secret/src/cache.rs` implementing `KeyCache` with `get`, `insert`, `evict_expired`, `clear` (zeroize on clear).

Wire into `SecretServiceInner` behind the existing `RwLock`.

## Acceptance Criteria

- [ ] `cache.rs` module added to `alknet-secret/src/` with `KeyCache` struct
- [ ] `CachedKey` struct with Zeroize-derived private key, `derived_at: Instant`, `key_type`, `public_key`
- [ ] `CacheConfig` struct with `ttl: Duration` (default 1 hour) and `max_entries: usize` (default 64)
- [ ] `KeyCache::get(path: &str) -> Option<&CachedKey>` returns cached entry if within TTL
- [ ] `KeyCache::insert(path: &str, key: CachedKey)` inserts, evicts LRU if over max_entries
- [ ] `KeyCache::evict_expired()` removes entries past TTL, zeroizing them
- [ ] `KeyCache::clear()` zeroizes all entries and clears the HashMap
- [ ] `SecretServiceInner` gains a `cache: KeyCache` field (behind RwLock)
- [ ] `SecretServiceHandle::new()` accepts optional `CacheConfig` (defaults applied)
- [ ] `derive_ed25519`, `derive_encryption_key`, `derive_ethereum_key` check cache before re-deriving
- [ ] `Lock` clears the cache (zeroizes all cached entries)
- [ ] Unit test: cache hit avoids re-derivation
- [ ] Unit test: cache miss derives and caches
- [ ] Unit test: expired entry is evicted on access and re-derived
- [ ] Unit test: LRU eviction when cache exceeds max_entries
- [ ] Unit test: Lock clears all cache entries and zeroizes them
- [ ] Unit test: encrypt/decrypt uses cached encryption key

## References

- docs/architecture/secret-service.md — Key caching subsection (after spec update)
- docs/architecture/decisions/038-seed-lifecycle-memory-security.md — Zeroize requirement
- OQ-SVC-04 — Resolved: yes, cache with TTL default 1 hour
- crates/alknet-secret/src/service.rs — SecretServiceInner (to add cache)
- crates/alknet-secret/src/lib.rs — Module re-exports

## Notes

> This task depends on `derivedkey-zeroize-security` because `CachedKey` needs the same zeroize discipline that `DerivedKey` gets. If `DerivedKey` becomes non-Clone, `CachedKey` is a separate internal type that holds the same data but is managed within the cache.

> The LRU implementation can use `std::collections::HashMap` + a doubly-linked list (or `lru` crate for simplicity). Given the max_entries default of 64, even a simple scan-and-evict approach is fine. Prefer `lru` crate for correctness and simplicity.

> Cache configuration should be exposed through `SecretService::new()` or a `SecretServiceBuilder` pattern, not through `SecretServiceHandle::new()`. The handle wraps an already-configured service.

## Summary

> To be filled on completion