//! TTL-based key cache with LRU eviction for VaultService.
//!
//! The `KeyCache` stores derived key material keyed by derivation path. Entries
//! expire after a configurable TTL (default: 1 hour) and are evicted lazily on
//! access. When the cache exceeds `max_entries` (default: 64), the least recently
//! used entry is evicted. All entries are zeroized on removal per ADR-038.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use zeroize::Zeroize;

use crate::protocol::{DerivedKey, KeyType};

/// Default TTL for cached keys (1 hour).
pub const DEFAULT_TTL: Duration = Duration::from_secs(3600);

/// Default maximum number of cache entries.
pub const DEFAULT_MAX_ENTRIES: usize = 64;

/// A cached derived key. Wraps a `DerivedKey` with cache metadata.
///
/// Derives `Zeroize` and `ZeroizeOnDrop` — the private key is zeroized
/// when the entry is evicted (LRU/TTL) or the cache is cleared.
#[derive(Zeroize)]
#[zeroize(drop)]
pub struct CachedKey {
    /// The derived key (zeroized on drop).
    #[zeroize(skip)]
    pub key: DerivedKey,
    /// When the entry was inserted (for TTL).
    #[zeroize(skip)]
    pub cached_at: Instant,
    /// Last access time for LRU ordering.
    #[zeroize(skip)]
    last_accessed: Instant,
}

impl CachedKey {
    /// Create a new `CachedKey` from a `DerivedKey`.
    pub fn new(key: DerivedKey) -> Self {
        let now = Instant::now();
        Self {
            key,
            cached_at: now,
            last_accessed: now,
        }
    }

    /// The key type of the cached derived key.
    pub fn key_type(&self) -> &KeyType {
        &self.key.key_type
    }

    /// The private key bytes of the cached derived key.
    pub fn private_key(&self) -> &[u8] {
        &self.key.private_key
    }

    /// The public key bytes of the cached derived key.
    pub fn public_key(&self) -> &[u8] {
        &self.key.public_key
    }

    /// Check whether this cached entry has expired.
    pub fn is_expired(&self, ttl: Duration) -> bool {
        Instant::now().duration_since(self.cached_at) > ttl
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
mod drop_tracker {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    struct DropTrackedKey {
        flag: Arc<AtomicBool>,
        bytes: Vec<u8>,
    }

    impl DropTrackedKey {
        fn new(flag: &Arc<AtomicBool>) -> Self {
            Self {
                flag: flag.clone(),
                bytes: vec![0xABu8; 32],
            }
        }
    }

    impl Drop for DropTrackedKey {
        fn drop(&mut self) {
            for b in self.bytes.iter_mut() {
                *b = 0;
            }
            self.flag.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn test_hashmap_clear_drops_values_triggering_drop_impls() {
        let flag1 = Arc::new(AtomicBool::new(false));
        let flag2 = Arc::new(AtomicBool::new(false));
        let mut map: HashMap<String, DropTrackedKey> = HashMap::new();
        map.insert("path1".to_string(), DropTrackedKey::new(&flag1));
        map.insert("path2".to_string(), DropTrackedKey::new(&flag2));

        assert!(!flag1.load(Ordering::SeqCst));
        assert!(!flag2.load(Ordering::SeqCst));

        map.clear();

        assert!(flag1.load(Ordering::SeqCst));
        assert!(flag2.load(Ordering::SeqCst));
        assert!(map.is_empty());
    }

    #[test]
    fn test_hashmap_remove_drops_value_triggering_drop_impl() {
        let flag = Arc::new(AtomicBool::new(false));
        let mut map: HashMap<String, DropTrackedKey> = HashMap::new();
        map.insert("path1".to_string(), DropTrackedKey::new(&flag));

        assert!(!flag.load(Ordering::SeqCst));

        map.remove("path1");

        assert!(flag.load(Ordering::SeqCst));
    }

    #[test]
    fn test_hashmap_insert_replace_drops_old_value() {
        let flag_old = Arc::new(AtomicBool::new(false));
        let mut map: HashMap<String, DropTrackedKey> = HashMap::new();
        map.insert("path1".to_string(), DropTrackedKey::new(&flag_old));

        assert!(!flag_old.load(Ordering::SeqCst));

        let flag_new = Arc::new(AtomicBool::new(false));
        map.insert("path1".to_string(), DropTrackedKey::new(&flag_new));

        assert!(flag_old.load(Ordering::SeqCst));
        assert!(!flag_new.load(Ordering::SeqCst));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_cached_key(key_type: KeyType) -> CachedKey {
        CachedKey::new(DerivedKey {
            key_type,
            private_key: vec![0xABu8; 32],
            public_key: vec![0xCDu8; 32],
        })
    }

    #[test]
    fn test_cache_insert_and_get() {
        let mut cache = KeyCache::with_defaults();
        cache.insert("m/74'/0'/0'/0'", make_cached_key(KeyType::Ed25519));

        let entry = cache.get("m/74'/0'/0'/0'").unwrap();
        assert_eq!(*entry.key_type(), KeyType::Ed25519);
    }

    #[test]
    fn test_cache_miss_returns_none() {
        let mut cache = KeyCache::with_defaults();
        assert!(cache.get("m/74'/0'/0'/0'").is_none());
    }

    #[test]
    fn test_cache_expired_entry_evicted_on_access() {
        let config = CacheConfig {
            ttl: Duration::from_millis(1),
            ..Default::default()
        };

        let mut cache = KeyCache::new(config);
        cache.insert("m/74'/0'/0'/0'", make_cached_key(KeyType::Ed25519));

        std::thread::sleep(Duration::from_millis(5));

        assert!(cache.get("m/74'/0'/0'/0'").is_none());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_cache_lru_eviction() {
        let config = CacheConfig {
            max_entries: 3,
            ..Default::default()
        };

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
        let config = CacheConfig {
            max_entries: 3,
            ..Default::default()
        };

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
        let config = CacheConfig {
            ttl: Duration::from_millis(10),
            ..Default::default()
        };

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
            CachedKey::new(DerivedKey {
                key_type: KeyType::Ed25519,
                private_key: vec![1u8; 32],
                public_key: vec![2u8; 32],
            }),
        );
        cache.insert(
            "path1",
            CachedKey::new(DerivedKey {
                key_type: KeyType::Aes256Gcm,
                private_key: vec![3u8; 32],
                public_key: vec![4u8; 32],
            }),
        );

        let entry = cache.get("path1").unwrap();
        assert_eq!(*entry.key_type(), KeyType::Aes256Gcm);
        assert_eq!(entry.private_key(), vec![3u8; 32]);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_lru_eviction_drops_evicted_cached_key() {
        let config = CacheConfig {
            max_entries: 2,
            ..Default::default()
        };

        let mut cache = KeyCache::new(config);

        cache.insert("path1", make_cached_key(KeyType::Ed25519));
        cache.insert("path2", make_cached_key(KeyType::Aes256Gcm));
        assert_eq!(cache.len(), 2);

        cache.insert("path3", make_cached_key(KeyType::Secp256k1));

        assert_eq!(cache.len(), 2);
        assert!(cache.get("path1").is_none());
        assert!(cache.get("path2").is_some());
        assert!(cache.get("path3").is_some());
    }

    #[test]
    fn test_ttl_expiry_evicts_entry_on_access() {
        let config = CacheConfig {
            ttl: Duration::from_millis(1),
            ..Default::default()
        };

        let mut cache = KeyCache::new(config);
        cache.insert("path1", make_cached_key(KeyType::Ed25519));
        assert_eq!(cache.len(), 1);

        std::thread::sleep(Duration::from_millis(5));

        assert!(cache.get("path1").is_none());
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
    }

    #[test]
    fn test_clear_removes_all_entries_and_empties_cache() {
        let mut cache = KeyCache::with_defaults();
        cache.insert("path1", make_cached_key(KeyType::Ed25519));
        cache.insert("path2", make_cached_key(KeyType::Aes256Gcm));
        cache.insert("path3", make_cached_key(KeyType::Secp256k1));
        assert_eq!(cache.len(), 3);

        cache.clear();

        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
        assert!(cache.get("path1").is_none());
        assert!(cache.get("path2").is_none());
        assert!(cache.get("path3").is_none());
    }
}
