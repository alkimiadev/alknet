//! VaultServiceHandle — the sole runtime API for the vault.
//!
//! The `VaultServiceHandle` wraps the vault's state in an
//! `Arc<std::sync::RwLock<>>` and provides direct, synchronous method calls
//! for the unlock/lock lifecycle, key derivation, and encryption/decryption.
//!
//! # Lifecycle
//!
//! ```text
//! Unlock(passphrase)
//!   → validate mnemonic (if restoring) or generate new
//!   → derive master key from seed
//!   → store seed in SeedHolder (Zeroize-protected)
//!   → cache empty (keys derived on demand)
//!
//! DeriveEd25519/DeriveEncryptionKey/Encrypt/Decrypt
//!   → require unlocked state (VaultLocked error if locked)
//!   → derive key, return result
//!   → optionally cache derived key
//!
//! Lock
//!   → zeroize all cached derived keys
//!   → zeroize seed
//!   → drop all sensitive material
//!   → vault returns to locked state
//! ```
//!
//! # Dispatch
//!
//! The vault uses **direct method calls** on `VaultServiceHandle` — no actor,
//! no message enum, no channels, no serialization (ADR-025). The handle is
//! `Arc<std::sync::RwLock<VaultServiceInner>>` — clone it, share it, call
//! methods directly. All methods are synchronous (no `async`, no `.await`).
//! The vault does not depend on `tokio` (ADR-025).
//!
//! # Assembly
//!
//! The `VaultServiceHandle` is assembled by the CLI binary. The CLI unlocks
//! the vault at startup and injects derived/decrypted material into operation
//! contexts. No handler crate accesses the vault directly — they receive keys
//! through their operation context or via the call protocol.

use std::sync::{Arc, RwLock};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

use crate::cache::{CacheConfig, CachedKey, KeyCache};
use crate::derivation::{self, DerivationError, PATHS};
use crate::encryption::{self, EncryptedData, EncryptionKey};
use crate::mnemonic::{Language, Mnemonic, Seed};
use crate::protocol::{DerivedKey, KeyType};

/// Handle to a running VaultService for local (in-process) use.
///
/// This is the primary API for local secret operations. It wraps the
/// service state in an `Arc<RwLock<>>` for thread-safe access.
#[derive(Clone)]
pub struct VaultServiceHandle {
    inner: Arc<RwLock<VaultServiceInner>>,
}

/// Internal state of the secret service.
struct VaultServiceInner {
    /// The mnemonic phrase, if unlocked. None if locked.
    mnemonic: Option<Mnemonic>,
    /// The master seed, if unlocked. None if locked.
    seed: Option<Seed>,
    /// Whether the service is unlocked.
    unlocked: bool,
    /// TTL-based key cache with LRU eviction.
    cache: KeyCache,
}

/// Errors that can occur during vault operations.
#[derive(Debug, thiserror::Error)]
pub enum VaultServiceError {
    #[error("vault is locked; call Unlock first")]
    VaultLocked,
    #[error("vault is already unlocked")]
    AlreadyUnlocked,
    #[error("mnemonic error: {0}")]
    Mnemonic(String),
    #[error("derivation error: {0}")]
    Derivation(String),
    #[error("encryption error: {0}")]
    Encryption(String),
    #[error("invalid path: {0}")]
    InvalidPath(String),
    #[error("unsupported key type")]
    UnsupportedKeyType,
}

impl From<crate::mnemonic::MnemonicError> for VaultServiceError {
    fn from(e: crate::mnemonic::MnemonicError) -> Self {
        VaultServiceError::Mnemonic(e.to_string())
    }
}

impl From<DerivationError> for VaultServiceError {
    fn from(e: DerivationError) -> Self {
        VaultServiceError::Derivation(e.to_string())
    }
}

impl From<encryption::EncryptionError> for VaultServiceError {
    fn from(e: encryption::EncryptionError) -> Self {
        VaultServiceError::Encryption(e.to_string())
    }
}

impl VaultServiceHandle {
    /// Create a new VaultServiceHandle in the locked state with default cache config.
    pub fn new() -> Self {
        Self::with_cache_config(CacheConfig::default())
    }

    /// Create a new VaultServiceHandle with the given cache configuration.
    pub fn with_cache_config(config: CacheConfig) -> Self {
        Self {
            inner: Arc::new(RwLock::new(VaultServiceInner {
                mnemonic: None,
                seed: None,
                unlocked: false,
                cache: KeyCache::new(config),
            })),
        }
    }

    /// Unlock the service with an existing mnemonic phrase.
    ///
    /// The passphrase is the BIP39 password (may be empty string for none).
    /// After unlocking, derive and encrypt/decrypt operations are available.
    pub fn unlock(&self, phrase: &str, passphrase: Option<&str>) -> Result<(), VaultServiceError> {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        if inner.unlocked {
            return Err(VaultServiceError::AlreadyUnlocked);
        }

        let mnemonic = Mnemonic::from_phrase(phrase, Language::English)?;
        let seed = mnemonic.to_seed(passphrase);

        inner.mnemonic = Some(mnemonic);
        inner.seed = Some(seed);
        inner.unlocked = true;
        Ok(())
    }

    /// Unlock the service with a new randomly generated mnemonic.
    ///
    /// Returns the generated mnemonic phrase. Store this phrase securely —
    /// it is the root of trust for all derived keys.
    pub fn unlock_new(&self, word_count: usize) -> Result<String, VaultServiceError> {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        if inner.unlocked {
            return Err(VaultServiceError::AlreadyUnlocked);
        }

        let mnemonic = Mnemonic::generate(word_count)?;
        let seed = mnemonic.to_seed(None);
        let phrase = mnemonic.phrase().to_string();

        inner.mnemonic = Some(mnemonic);
        inner.seed = Some(seed);
        inner.unlocked = true;
        Ok(phrase)
    }

    /// Lock the service, purging the seed and all cached derived keys.
    ///
    /// After locking, no derive/encrypt/decrypt operations are possible
    /// until `unlock` is called again. Calls `zeroize()` on all sensitive
    /// material per ADR-038.
    pub fn lock(&self) {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        inner.cache.clear();
        inner.seed = None;
        inner.mnemonic = None;
        inner.unlocked = false;
    }

    /// Check whether the service is currently unlocked.
    pub fn is_unlocked(&self) -> bool {
        self.inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .unlocked
    }

    /// Derive an Ed25519 keypair at the given path.
    pub fn derive_ed25519(&self, path: &str) -> Result<DerivedKey, VaultServiceError> {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        if !inner.unlocked {
            return Err(VaultServiceError::VaultLocked);
        }

        if let Some(cached) = inner.cache.get(path) {
            return Ok(DerivedKey {
                key_type: cached.key_type.clone(),
                private_key: cached.private_key.clone(),
                public_key: cached.public_key.clone(),
            });
        }

        let seed = inner.seed.as_ref().ok_or(VaultServiceError::VaultLocked)?;
        let key = derivation::derive_path_from_seed(seed.as_bytes(), path)?;
        let private_key = key.private_key().to_vec();
        let public_key = key.public_key().to_vec();
        let cached = CachedKey::new(KeyType::Ed25519, private_key.clone(), public_key.clone());
        inner.cache.insert(path, cached);
        Ok(DerivedKey {
            key_type: KeyType::Ed25519,
            private_key,
            public_key,
        })
    }

    /// Derive an AES-256-GCM encryption key at the given path.
    pub fn derive_encryption_key(&self, path: &str) -> Result<DerivedKey, VaultServiceError> {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        if !inner.unlocked {
            return Err(VaultServiceError::VaultLocked);
        }

        if let Some(cached) = inner.cache.get(path) {
            return Ok(DerivedKey {
                key_type: cached.key_type.clone(),
                private_key: cached.private_key.clone(),
                public_key: cached.public_key.clone(),
            });
        }

        let seed = inner.seed.as_ref().ok_or(VaultServiceError::VaultLocked)?;
        let key = derivation::derive_path_from_seed(seed.as_bytes(), path)?;
        let private_key = key.private_key().to_vec();
        let public_key = key.public_key().to_vec();
        let cached = CachedKey::new(KeyType::Aes256Gcm, private_key.clone(), public_key.clone());
        inner.cache.insert(path, cached);
        Ok(DerivedKey {
            key_type: KeyType::Aes256Gcm,
            private_key,
            public_key,
        })
    }

    /// Derive a secp256k1 (Ethereum) keypair at the given path.
    ///
    /// Uses BIP-0032 derivation (HMAC-SHA512 with "Bitcoin seed") when the
    /// `secp256k1` feature is enabled. Returns `UnsupportedKeyType` when the
    /// feature is disabled.
    pub fn derive_ethereum_key(&self, path: &str) -> Result<DerivedKey, VaultServiceError> {
        #[cfg(feature = "secp256k1")]
        {
            let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
            if !inner.unlocked {
                return Err(VaultServiceError::VaultLocked);
            }

            if let Some(cached) = inner.cache.get(path) {
                return Ok(DerivedKey {
                    key_type: cached.key_type.clone(),
                    private_key: cached.private_key.clone(),
                    public_key: cached.public_key.clone(),
                });
            }

            let seed = inner.seed.as_ref().ok_or(VaultServiceError::VaultLocked)?;

            let key = crate::ethereum::derive_secp256k1_path(seed.as_bytes(), path)?;
            let private_key = key.private_key().to_vec();
            let public_key = key.public_key().to_vec();
            let cached =
                CachedKey::new(KeyType::Secp256k1, private_key.clone(), public_key.clone());
            inner.cache.insert(path, cached);
            Ok(DerivedKey {
                key_type: KeyType::Secp256k1,
                private_key,
                public_key,
            })
        }

        #[cfg(not(feature = "secp256k1"))]
        {
            let _ = path;
            Err(VaultServiceError::UnsupportedKeyType)
        }
    }

    pub fn derive_password(&self, path: &str, length: usize) -> Result<Vec<u8>, VaultServiceError> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        if !inner.unlocked {
            return Err(VaultServiceError::VaultLocked);
        }
        let seed = inner.seed.as_ref().ok_or(VaultServiceError::VaultLocked)?;

        let key = derivation::derive_path_from_seed(seed.as_bytes(), path)?;
        let private_key = key.private_key();
        let truncated_len = length.min(private_key.len());
        let result = private_key[..truncated_len].to_vec();
        Ok(result)
    }

    pub fn derive_password_string(
        &self,
        path: &str,
        length: usize,
    ) -> Result<String, VaultServiceError> {
        let bytes = self.derive_password(path, length)?;
        Ok(URL_SAFE_NO_PAD.encode(&bytes))
    }

    /// Encrypt plaintext using the derived encryption key.
    ///
    /// Uses the key at path `m/74'/2'/0'/0'` (PATHS::ENCRYPTION) by default.
    pub fn encrypt(
        &self,
        plaintext: &str,
        key_version: u32,
    ) -> Result<EncryptedData, VaultServiceError> {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        if !inner.unlocked {
            return Err(VaultServiceError::VaultLocked);
        }

        let private_key = if let Some(cached) = inner.cache.get(PATHS::ENCRYPTION) {
            cached.private_key.clone()
        } else {
            let seed = inner.seed.as_ref().ok_or(VaultServiceError::VaultLocked)?;
            let derived = derivation::derive_path_from_seed(seed.as_bytes(), PATHS::ENCRYPTION)?;
            let pk = derived.private_key().to_vec();
            let pubk = derived.public_key().to_vec();
            let cached = CachedKey::new(KeyType::Aes256Gcm, pk.clone(), pubk);
            inner.cache.insert(PATHS::ENCRYPTION, cached);
            pk
        };

        let enc_key = EncryptionKey::from_derived_bytes(&private_key, key_version);

        encryption::encrypt(plaintext, &enc_key).map_err(|e| e.into())
    }

    /// Decrypt an EncryptedData blob using the derived encryption key.
    pub fn decrypt(&self, encrypted: &EncryptedData) -> Result<String, VaultServiceError> {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        if !inner.unlocked {
            return Err(VaultServiceError::VaultLocked);
        }

        let private_key = if let Some(cached) = inner.cache.get(PATHS::ENCRYPTION) {
            cached.private_key.clone()
        } else {
            let seed = inner.seed.as_ref().ok_or(VaultServiceError::VaultLocked)?;
            let derived = derivation::derive_path_from_seed(seed.as_bytes(), PATHS::ENCRYPTION)?;
            let pk = derived.private_key().to_vec();
            let pubk = derived.public_key().to_vec();
            let cached = CachedKey::new(KeyType::Aes256Gcm, pk.clone(), pubk);
            inner.cache.insert(PATHS::ENCRYPTION, cached);
            pk
        };

        let enc_key = EncryptionKey::from_derived_bytes(&private_key, encrypted.key_version);

        encryption::decrypt(encrypted, &enc_key).map_err(|e| e.into())
    }
}

impl Default for VaultServiceHandle {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_starts_locked() {
        let service = VaultServiceHandle::new();
        assert!(!service.is_unlocked());
    }

    #[test]
    fn test_unlock_new_generates_mnemonic() {
        let service = VaultServiceHandle::new();
        let phrase = service.unlock_new(24).unwrap();
        assert!(!phrase.is_empty());
        assert!(service.is_unlocked());
    }

    #[test]
    fn test_lock_purges_state() {
        let service = VaultServiceHandle::new();
        service.unlock_new(24).unwrap();
        assert!(service.is_unlocked());

        service.lock();
        assert!(!service.is_unlocked());
    }

    #[test]
    fn test_derive_on_locked_fails() {
        let service = VaultServiceHandle::new();
        let result = service.derive_ed25519(PATHS::IDENTITY);
        assert!(result.is_err());
    }

    #[test]
    fn test_encrypt_on_locked_fails() {
        let service = VaultServiceHandle::new();
        let result = service.encrypt("secret", 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_full_lifecycle() {
        let service = VaultServiceHandle::new();

        assert!(!service.is_unlocked());

        assert!(service.derive_ed25519(PATHS::IDENTITY).is_err());

        let _phrase = service.unlock_new(24).unwrap();
        assert!(service.is_unlocked());

        let key = service.derive_ed25519(PATHS::IDENTITY).unwrap();
        assert!(!key.private_key.is_empty());

        service.lock();
        assert!(!service.is_unlocked());

        assert!(service.derive_ed25519(PATHS::IDENTITY).is_err());
    }

    #[test]
    fn test_poisoned_lock_recovery() {
        let service = VaultServiceHandle::new();
        service.unlock_new(24).unwrap();

        let inner_arc = service.inner.clone();
        std::thread::spawn(move || {
            let _guard = inner_arc.write().unwrap();
            panic!("simulated panic while holding write lock");
        })
        .join()
        .expect_err("thread must panic to poison the lock");

        assert!(
            service.is_unlocked(),
            "vault must remain usable after a poisoned lock"
        );

        let key = service.derive_ed25519(PATHS::IDENTITY).unwrap();
        assert!(!key.private_key.is_empty());
    }

    #[test]
    fn test_unlock_with_known_phrase() {
        let service = VaultServiceHandle::new();

        let phrase = service.unlock_new(24).unwrap();
        service.lock();

        service.unlock(&phrase, None).unwrap();
        assert!(service.is_unlocked());
    }

    #[test]
    fn test_double_unlock_fails() {
        let service = VaultServiceHandle::new();
        service.unlock_new(24).unwrap();

        let result = service.unlock_new(12);
        assert!(result.is_err());
    }

    #[test]
    fn test_encrypt_decrypt_lifecycle() {
        let service = VaultServiceHandle::new();
        service.unlock_new(24).unwrap();

        let plaintext = "my-api-key-12345";
        let encrypted = service.encrypt(plaintext, 1).unwrap();
        let decrypted = service.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, plaintext);

        service.lock();
        assert!(service.decrypt(&encrypted).is_err());
    }

    #[test]
    fn test_derive_password_deterministic() {
        let service = VaultServiceHandle::new();
        service.unlock_new(24).unwrap();

        let path = "m/74'/1'/0'/12345'";
        let pw1 = service.derive_password(path, 16).unwrap();
        let pw2 = service.derive_password(path, 16).unwrap();
        assert_eq!(pw1, pw2, "derive_password must be deterministic");
    }

    #[test]
    fn test_derive_password_different_paths() {
        let service = VaultServiceHandle::new();
        service.unlock_new(24).unwrap();

        let pw_a = service.derive_password("m/74'/1'/0'/100'", 16).unwrap();
        let pw_b = service.derive_password("m/74'/1'/0'/200'", 16).unwrap();
        assert_ne!(
            pw_a, pw_b,
            "different paths must produce different passwords"
        );
    }

    #[test]
    fn test_derive_password_length_truncation() {
        let service = VaultServiceHandle::new();
        service.unlock_new(24).unwrap();

        let path = "m/74'/1'/0'/999'";
        let pw_full = service.derive_password(path, 32).unwrap();
        let pw_short = service.derive_password(path, 16).unwrap();

        assert_eq!(pw_short.len(), 16);
        assert_eq!(pw_full.len(), 32);
        assert_eq!(
            &pw_full[..16],
            &pw_short[..],
            "truncated bytes must match prefix of full key"
        );
    }

    #[test]
    fn test_derive_password_locked_error() {
        let service = VaultServiceHandle::new();
        let result = service.derive_password("m/74'/1'/0'/1'", 16);
        assert!(matches!(result, Err(VaultServiceError::VaultLocked)));
    }

    #[test]
    fn test_derive_password_string_base64url() {
        let service = VaultServiceHandle::new();
        service.unlock_new(24).unwrap();

        let path = "m/74'/1'/0'/42'";
        let encoded = service.derive_password_string(path, 16).unwrap();

        assert!(!encoded.contains('='), "Base64url must not contain padding");
        assert!(
            encoded
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "Base64url must only contain URL-safe characters"
        );

        let raw_bytes = service.derive_password(path, 16).unwrap();
        let decoded = URL_SAFE_NO_PAD.decode(&encoded).unwrap();
        assert_eq!(raw_bytes, decoded);
    }

    #[cfg(feature = "secp256k1")]
    #[test]
    fn test_derive_ethereum_key_bip32() {
        let service = VaultServiceHandle::new();
        service.unlock_new(24).unwrap();

        let key = service.derive_ethereum_key(PATHS::ETHEREUM).unwrap();
        assert_eq!(key.key_type, KeyType::Secp256k1);
        assert_eq!(key.private_key.len(), 32);
        assert_eq!(key.public_key.len(), 33);
    }

    #[cfg(feature = "secp256k1")]
    #[test]
    fn test_ethereum_key_differs_from_ed25519() {
        let service = VaultServiceHandle::new();
        service.unlock_new(24).unwrap();

        let eth_key = service.derive_ethereum_key(PATHS::ETHEREUM).unwrap();
        let ed_key = service.derive_ed25519(PATHS::IDENTITY).unwrap();

        assert_ne!(eth_key.private_key, ed_key.private_key);
    }

    #[cfg(not(feature = "secp256k1"))]
    #[test]
    fn test_derive_ethereum_key_unsupported_without_feature() {
        let service = VaultServiceHandle::new();
        service.unlock_new(24).unwrap();

        let result = service.derive_ethereum_key(PATHS::ETHEREUM);
        assert!(matches!(result, Err(VaultServiceError::UnsupportedKeyType)));
    }

    #[test]
    fn test_cache_hit_avoids_re_derivation() {
        let service = VaultServiceHandle::new();
        service.unlock_new(24).unwrap();

        let key1 = service.derive_ed25519(PATHS::IDENTITY).unwrap();
        let key2 = service.derive_ed25519(PATHS::IDENTITY).unwrap();

        assert_eq!(key1.private_key, key2.private_key);
        assert_eq!(key1.public_key, key2.public_key);

        let cache_len = service.inner.read().unwrap().cache.len();
        assert_eq!(cache_len, 1);
    }

    #[test]
    fn test_cache_miss_derives_and_caches() {
        let service = VaultServiceHandle::new();
        service.unlock_new(24).unwrap();

        assert_eq!(service.inner.read().unwrap().cache.len(), 0);

        service.derive_ed25519(PATHS::IDENTITY).unwrap();

        assert_eq!(service.inner.read().unwrap().cache.len(), 1);
    }

    #[test]
    fn test_expired_entry_evicted_on_access() {
        let config = crate::cache::CacheConfig::new(std::time::Duration::from_millis(5), 64);
        let service = VaultServiceHandle::with_cache_config(config);
        service.unlock_new(24).unwrap();

        let key1 = service.derive_ed25519(PATHS::IDENTITY).unwrap();
        assert_eq!(service.inner.read().unwrap().cache.len(), 1);

        std::thread::sleep(std::time::Duration::from_millis(10));

        let key2 = service.derive_ed25519(PATHS::IDENTITY).unwrap();
        assert_eq!(key1.private_key, key2.private_key);
        assert_eq!(service.inner.read().unwrap().cache.len(), 1);
    }

    #[test]
    fn test_lru_eviction_when_over_max_entries() {
        let config = crate::cache::CacheConfig::new(std::time::Duration::from_secs(3600), 2);
        let service = VaultServiceHandle::with_cache_config(config);
        service.unlock_new(24).unwrap();

        service.derive_ed25519(PATHS::IDENTITY).unwrap();
        service.derive_ed25519(PATHS::SSH_HOST).unwrap();
        assert_eq!(service.inner.read().unwrap().cache.len(), 2);

        service.derive_ed25519(PATHS::ENCRYPTION).unwrap();
        assert_eq!(service.inner.read().unwrap().cache.len(), 2);

        let mut inner = service.inner.write().unwrap();
        assert!(inner.cache.get(PATHS::IDENTITY).is_none());
        assert!(inner.cache.get(PATHS::SSH_HOST).is_some());
        assert!(inner.cache.get(PATHS::ENCRYPTION).is_some());
    }

    #[test]
    fn test_lock_clears_all_cache_entries() {
        let service = VaultServiceHandle::new();
        service.unlock_new(24).unwrap();

        service.derive_ed25519(PATHS::IDENTITY).unwrap();
        service.derive_ed25519(PATHS::SSH_HOST).unwrap();
        assert_eq!(service.inner.read().unwrap().cache.len(), 2);

        service.lock();

        assert_eq!(service.inner.read().unwrap().cache.len(), 0);
    }

    #[test]
    fn test_encrypt_decrypt_uses_cached_encryption_key() {
        let service = VaultServiceHandle::new();
        service.unlock_new(24).unwrap();

        let plaintext = "cached-encryption-test";
        let encrypted = service.encrypt(plaintext, 1).unwrap();
        assert_eq!(service.inner.read().unwrap().cache.len(), 1);

        let decrypted = service.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, plaintext);

        assert_eq!(service.inner.read().unwrap().cache.len(), 1);
    }

    #[test]
    fn test_unlock_with_passphrase_produces_different_seed() {
        let service_a = VaultServiceHandle::new();
        let service_b = VaultServiceHandle::new();

        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

        service_a.unlock(phrase, None).unwrap();
        let key_a = service_a.derive_ed25519(PATHS::IDENTITY).unwrap();

        service_a.lock();

        service_a.unlock(phrase, Some("TREZOR")).unwrap();
        let key_b = service_a.derive_ed25519(PATHS::IDENTITY).unwrap();

        assert_ne!(
            key_a.private_key, key_b.private_key,
            "Unlock with passphrase must produce different seed than without"
        );

        service_a.lock();

        service_b.unlock(phrase, None).unwrap();
        let key_c = service_b.derive_ed25519(PATHS::IDENTITY).unwrap();

        assert_eq!(
            key_a.private_key, key_c.private_key,
            "Unlock with None passphrase must produce same seed as another None passphrase unlock"
        );
    }
}
