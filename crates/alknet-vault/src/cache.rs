//! TTL-based key cache with LRU eviction for VaultService.
//!
//! The `KeyCache` stores derived key material keyed by derivation path. Entries
//! expire after a configurable TTL (default: 1 hour) and are evicted lazily on
//! access. When the cache exceeds `max_entries` (default: 64), the least recently
//! used entry is evicted. All entries are zeroized on removal per ADR-038.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use zeroize::Zeroize;

use crate::protocol::KeyType;

/// Default TTL for cached keys (1 hour).
pub const DEFAULT_TTL: Duration = Duration::from_secs(3600);

/// Default maximum number of cache entries.
pub const DEFAULT_MAX_ENTRIES: usize = 64;

/// A cached derived key with metadata for TTL and LRU tracking.
///
/// The `private_key` field is zeroized on drop via `#[zeroize(drop)]`.
/// This is a separate internal type from `DerivedKey` — it holds the same
/// data but is managed within the cache lifecycle.
#[derive(Zeroize)]
#[zeroize(drop)]
pub struct CachedKey {
    /// When this key was derived (for TTL checking).
    #[zeroize(skip)]
    pub derived_at: Instant,
    /// The type of key that was derived.
    #[zeroize(skip)]
    pub key_type: KeyType,
    /// The private key bytes (sensitive — zeroized on drop).
    #[zeroize]
    pub private_key: Vec<u8>,
    /// The public key bytes.
    #[zeroize(skip)]
    pub public_key: Vec<u8>,
    /// Last access time for LRU ordering.
    #[zeroize(skip)]
    last_accessed: Instant,
}

impl CachedKey {
    /// Create a new `CachedKey` from derived key material.
    pub fn new(key_type: KeyType, private_key: Vec<u8>, public_key: Vec<u8>) -> Self {
        let now = Instant::now();
        Self {
            derived_at: now,
            key_type,
            private_key,
            public_key,
            last_accessed: now,
        }
    }

    /// Check whether this cached entry has expired.
    pub fn is_expired(&self, ttl: Duration) -> bool {
        Instant::now().duration_since(self.derived_at) > ttl
    }

    /// Touch the entry to update its last-accessed time (for LRU).
    pub fn touch(&mut self) {
        self.last_accessed = Instant::now();
    }
}

/// Configuration for the key cache.
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// Time-to-live for cached entries. Expired entries are evicted lazily on access.
    pub ttl: Duration,
    /// Maximum number of entries. When exceeded, the least recently used entry is evicted.
    pub max_entries: usize,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            ttl: DEFAULT_TTL,
            max_entries: DEFAULT_MAX_ENTRIES,
        }
    }
}

impl CacheConfig {
    /// Create a new `CacheConfig` with the given TTL and max entries.
    pub fn new(ttl: Duration, max_entries: usize) -> Self {
        Self { ttl, max_entries }
    }
}

/// LRU key cache backed by a HashMap with access-order tracking.
///
/// The cache uses a `HashMap` for O(1) lookups and a separate ordering list
/// for LRU eviction. For the default 64 entries, this is efficient enough
/// without needing the `lru` crate.
pub struct KeyCache {
    entries: HashMap<String, CachedKey>,
    /// Access order: most recently used at the back, least recently at the front.
    order: Vec<String>,
    config: CacheConfig,
}

impl KeyCache {
    /// Create a new empty `KeyCache` with the given configuration.
    pub fn new(config: CacheConfig) -> Self {
        Self {
            entries: HashMap::new(),
            order: Vec::with_capacity(config.max_entries),
            config,
        }
    }

    /// Create a new empty `KeyCache` with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(CacheConfig::default())
    }

    /// Get a cached entry by derivation path if it exists and is within TTL.
    ///
    /// Returns `None` if the entry does not exist or has expired (expired entries
    /// are evicted). A successful get updates the LRU ordering.
    pub fn get(&mut self, path: &str) -> Option<&CachedKey> {
        if let Some(entry) = self.entries.get_mut(path) {
            if entry.is_expired(self.config.ttl) {
                self.remove_entry(path);
                return None;
            }
            entry.touch();
            self.move_to_back(path);
            Some(self.entries.get(path)?)
        } else {
            None
        }
    }

    /// Insert a cached key by derivation path.
    ///
    /// If the cache is at capacity, the least recently used entry is evicted
    /// (and zeroized). If an entry with the same path already exists, it is
    /// replaced (the old entry is zeroized on drop).
    pub fn insert(&mut self, path: &str, key: CachedKey) {
        if self.entries.contains_key(path) {
            self.remove_entry(path);
        } else if self.entries.len() >= self.config.max_entries {
            self.evict_lru();
        }
        self.entries.insert(path.to_string(), key);
        self.order.push(path.to_string());
    }

    /// Remove all entries that have exceeded the TTL, zeroizing them.
    pub fn evict_expired(&mut self) {
        let ttl = self.config.ttl;
        let expired: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, v)| v.is_expired(ttl))
            .map(|(k, _)| k.clone())
            .collect();

        for path in expired {
            self.remove_entry(&path);
        }
    }

    /// Clear all cache entries, zeroizing each one before removal.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }

    /// Returns the number of entries currently in the cache.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if the cache contains no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn remove_entry(&mut self, path: &str) {
        self.entries.remove(path);
        self.order.retain(|p| p != path);
    }

    fn evict_lru(&mut self) {
        if let Some(lru_path) = self.order.first().cloned() {
            self.remove_entry(&lru_path);
        }
    }

    fn move_to_back(&mut self, path: &str) {
        self.order.retain(|p| p != path);
        self.order.push(path.to_string());
    }
}

impl Default for KeyCache {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_cached_key(key_type: KeyType) -> CachedKey {
        CachedKey::new(key_type, vec![0xABu8; 32], vec![0xCDu8; 32])
    }

    #[test]
    fn test_cache_insert_and_get() {
        let mut cache = KeyCache::with_defaults();
        cache.insert("m/74'/0'/0'/0'", make_cached_key(KeyType::Ed25519));

        let entry = cache.get("m/74'/0'/0'/0'").unwrap();
        assert_eq!(entry.key_type, KeyType::Ed25519);
    }

    #[test]
    fn test_cache_miss_returns_none() {
        let mut cache = KeyCache::with_defaults();
        assert!(cache.get("m/74'/0'/0'/0'").is_none());
    }

    #[test]
    fn test_cache_expired_entry_evicted_on_access() {
        let mut config = CacheConfig::default();
        config.ttl = Duration::from_millis(1);

        let mut cache = KeyCache::new(config);
        cache.insert("m/74'/0'/0'/0'", make_cached_key(KeyType::Ed25519));

        std::thread::sleep(Duration::from_millis(5));

        assert!(cache.get("m/74'/0'/0'/0'").is_none());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_cache_lru_eviction() {
        let mut config = CacheConfig::default();
        config.max_entries = 3;

        let mut cache = KeyCache::new(config);

        cache.insert("path1", make_cached_key(KeyType::Ed25519));
        cache.insert("path2", make_cached_key(KeyType::Aes256Gcm));
        cache.insert("path3", make_cached_key(KeyType::Secp256k1));

        assert_eq!(cache.len(), 3);

        cache.insert("path4", make_cached_key(KeyType::Ed25519));

        assert_eq!(cache.len(), 3);
        assert!(cache.get("path1").is_none());
        assert!(cache.get("path2").is_some());
        assert!(cache.get("path3").is_some());
        assert!(cache.get("path4").is_some());
    }

    #[test]
    fn test_cache_lru_access_reorders() {
        let mut config = CacheConfig::default();
        config.max_entries = 3;

        let mut cache = KeyCache::new(config);

        cache.insert("path1", make_cached_key(KeyType::Ed25519));
        cache.insert("path2", make_cached_key(KeyType::Aes256Gcm));
        cache.insert("path3", make_cached_key(KeyType::Secp256k1));

        cache.get("path1");

        cache.insert("path4", make_cached_key(KeyType::Ed25519));

        assert_eq!(cache.len(), 3);
        assert!(cache.get("path1").is_some());
        assert!(cache.get("path2").is_none());
        assert!(cache.get("path3").is_some());
        assert!(cache.get("path4").is_some());
    }

    #[test]
    fn test_cache_clear_zeroizes_and_removes_all() {
        let mut cache = KeyCache::with_defaults();
        cache.insert("path1", make_cached_key(KeyType::Ed25519));
        cache.insert("path2", make_cached_key(KeyType::Aes256Gcm));

        assert_eq!(cache.len(), 2);

        cache.clear();

        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
    }

    #[test]
    fn test_evict_expired_removes_only_expired() {
        let mut config = CacheConfig::default();
        config.ttl = Duration::from_millis(10);

        let mut cache = KeyCache::new(config);
        cache.insert("path1", make_cached_key(KeyType::Ed25519));

        std::thread::sleep(Duration::from_millis(20));

        cache.insert("path2", make_cached_key(KeyType::Aes256Gcm));

        cache.evict_expired();

        assert_eq!(cache.len(), 1);
        assert!(cache.get("path2").is_some());
    }

    #[test]
    fn test_cache_replace_existing_path() {
        let mut cache = KeyCache::with_defaults();
        cache.insert(
            "path1",
            CachedKey::new(KeyType::Ed25519, vec![1u8; 32], vec![2u8; 32]),
        );
        cache.insert(
            "path1",
            CachedKey::new(KeyType::Aes256Gcm, vec![3u8; 32], vec![4u8; 32]),
        );

        let entry = cache.get("path1").unwrap();
        assert_eq!(entry.key_type, KeyType::Aes256Gcm);
        assert_eq!(entry.private_key, vec![3u8; 32]);
        assert_eq!(cache.len(), 1);
    }
}
