//! SecretService implementation with Unlock/Lock lifecycle.
//!
//! The `SecretService` is the primary runtime interface for key management.
//! It holds the master seed in `Zeroize`-protected memory and provides methods
//! for the Unlock/Lock lifecycle, key derivation, and encryption/decryption.
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
//!   → require unlocked state (ServiceLocked error if locked)
//!   → derive key, return result
//!   → optionally cache derived key
//!
//! Lock
//!   → zeroize all cached derived keys
//!   → zeroize seed
//!   → drop all sensitive material
//!   → service returns to locked state
//! ```
//!
//! # Dispatch Paths
//!
//! There are two ways to interact with the secret service:
//!
//! 1. **Local (in-process)**: `SecretServiceHandle` wraps `SecretServiceInner`
//!    behind `Arc<RwLock<>>` and provides direct method calls without serialization.
//! 2. **Remote (in-cluster)**: `SecretServiceActor` processes `SecretMessage`
//!    variants from an mpsc channel and dispatches to the handle methods.
//!
//! # Assembly
//!
//! The `SecretService` is assembled by the CLI binary or NAPI layer. Per ADR-027,
//! alknet-core never sees the secret service directly — it is wired through the
//! `OperationEnv` dispatch mechanism. For minimal deployments, no secret service
//! is available (the `SecretStoreCredentialProvider` returns `None`).

use std::sync::{Arc, RwLock};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use irpc::WithChannels;
use serde::{Deserialize, Serialize};

use crate::cache::{CacheConfig, CachedKey, KeyCache};
use crate::derivation::{self, DerivationError, PATHS};
use crate::encryption::{self, EncryptedData, EncryptionKey};
use crate::mnemonic::{Language, Mnemonic, Seed};
use crate::protocol::{
    Decrypt, DeriveEd25519, DeriveEncryptionKey, DeriveEthereumKey, DerivePassword, Encrypt,
    SecretMessage, SecretProtocol, Unlock,
};
use crate::protocol::{DerivedKey, KeyType};

/// Handle to a running SecretService for local (in-process) use.
///
/// This is the primary API for local secret operations. It wraps the
/// service state in an `Arc<RwLock<>>` for thread-safe access.
#[derive(Clone)]
pub struct SecretServiceHandle {
    inner: Arc<RwLock<SecretServiceInner>>,
}

/// Internal state of the secret service.
struct SecretServiceInner {
    /// The mnemonic phrase, if unlocked. None if locked.
    mnemonic: Option<Mnemonic>,
    /// The master seed, if unlocked. None if locked.
    seed: Option<Seed>,
    /// Whether the service is unlocked.
    unlocked: bool,
    /// TTL-based key cache with LRU eviction.
    cache: KeyCache,
}

/// Errors that can occur during secret service operations.
#[derive(Debug, thiserror::Error, Serialize, Deserialize)]
pub enum SecretServiceError {
    #[error("service is locked; call Unlock first")]
    ServiceLocked,
    #[error("service is already unlocked")]
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

impl From<crate::mnemonic::MnemonicError> for SecretServiceError {
    fn from(e: crate::mnemonic::MnemonicError) -> Self {
        SecretServiceError::Mnemonic(e.to_string())
    }
}

impl From<DerivationError> for SecretServiceError {
    fn from(e: DerivationError) -> Self {
        SecretServiceError::Derivation(e.to_string())
    }
}

impl From<encryption::EncryptionError> for SecretServiceError {
    fn from(e: encryption::EncryptionError) -> Self {
        SecretServiceError::Encryption(e.to_string())
    }
}

impl SecretServiceHandle {
    /// Create a new SecretServiceHandle in the locked state with default cache config.
    pub fn new() -> Self {
        Self::with_cache_config(CacheConfig::default())
    }

    /// Create a new SecretServiceHandle with the given cache configuration.
    pub fn with_cache_config(config: CacheConfig) -> Self {
        Self {
            inner: Arc::new(RwLock::new(SecretServiceInner {
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
    pub fn unlock(&self, phrase: &str, passphrase: Option<&str>) -> Result<(), SecretServiceError> {
        let mut inner = self.inner.write().unwrap();
        if inner.unlocked {
            return Err(SecretServiceError::AlreadyUnlocked);
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
    pub fn unlock_new(&self, word_count: usize) -> Result<String, SecretServiceError> {
        let mut inner = self.inner.write().unwrap();
        if inner.unlocked {
            return Err(SecretServiceError::AlreadyUnlocked);
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
        let mut inner = self.inner.write().unwrap();
        inner.cache.clear();
        inner.seed = None;
        inner.mnemonic = None;
        inner.unlocked = false;
    }

    /// Check whether the service is currently unlocked.
    pub fn is_unlocked(&self) -> bool {
        self.inner.read().unwrap().unlocked
    }

    /// Derive an Ed25519 keypair at the given path.
    pub fn derive_ed25519(&self, path: &str) -> Result<DerivedKey, SecretServiceError> {
        let mut inner = self.inner.write().unwrap();
        if !inner.unlocked {
            return Err(SecretServiceError::ServiceLocked);
        }

        if let Some(cached) = inner.cache.get(path) {
            return Ok(DerivedKey {
                key_type: cached.key_type.clone(),
                private_key: cached.private_key.clone(),
                public_key: cached.public_key.clone(),
            });
        }

        let seed = inner
            .seed
            .as_ref()
            .ok_or(SecretServiceError::ServiceLocked)?;
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
    pub fn derive_encryption_key(&self, path: &str) -> Result<DerivedKey, SecretServiceError> {
        let mut inner = self.inner.write().unwrap();
        if !inner.unlocked {
            return Err(SecretServiceError::ServiceLocked);
        }

        if let Some(cached) = inner.cache.get(path) {
            return Ok(DerivedKey {
                key_type: cached.key_type.clone(),
                private_key: cached.private_key.clone(),
                public_key: cached.public_key.clone(),
            });
        }

        let seed = inner
            .seed
            .as_ref()
            .ok_or(SecretServiceError::ServiceLocked)?;
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
    pub fn derive_ethereum_key(&self, path: &str) -> Result<DerivedKey, SecretServiceError> {
        #[cfg(feature = "secp256k1")]
        {
            let mut inner = self.inner.write().unwrap();
            if !inner.unlocked {
                return Err(SecretServiceError::ServiceLocked);
            }

            if let Some(cached) = inner.cache.get(path) {
                return Ok(DerivedKey {
                    key_type: cached.key_type.clone(),
                    private_key: cached.private_key.clone(),
                    public_key: cached.public_key.clone(),
                });
            }

            let seed = inner
                .seed
                .as_ref()
                .ok_or(SecretServiceError::ServiceLocked)?;

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
            Err(SecretServiceError::UnsupportedKeyType)
        }
    }

    pub fn derive_password(
        &self,
        path: &str,
        length: usize,
    ) -> Result<Vec<u8>, SecretServiceError> {
        let inner = self.inner.read().unwrap();
        if !inner.unlocked {
            return Err(SecretServiceError::ServiceLocked);
        }
        let seed = inner
            .seed
            .as_ref()
            .ok_or(SecretServiceError::ServiceLocked)?;

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
    ) -> Result<String, SecretServiceError> {
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
    ) -> Result<EncryptedData, SecretServiceError> {
        let mut inner = self.inner.write().unwrap();
        if !inner.unlocked {
            return Err(SecretServiceError::ServiceLocked);
        }

        let private_key = if let Some(cached) = inner.cache.get(PATHS::ENCRYPTION) {
            cached.private_key.clone()
        } else {
            let seed = inner
                .seed
                .as_ref()
                .ok_or(SecretServiceError::ServiceLocked)?;
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
    pub fn decrypt(&self, encrypted: &EncryptedData) -> Result<String, SecretServiceError> {
        let mut inner = self.inner.write().unwrap();
        if !inner.unlocked {
            return Err(SecretServiceError::ServiceLocked);
        }

        let private_key = if let Some(cached) = inner.cache.get(PATHS::ENCRYPTION) {
            cached.private_key.clone()
        } else {
            let seed = inner
                .seed
                .as_ref()
                .ok_or(SecretServiceError::ServiceLocked)?;
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

impl Default for SecretServiceHandle {
    fn default() -> Self {
        Self::new()
    }
}

/// The SecretService manages the lifecycle of the master seed and provides
/// secret operations. This is the type used by the irpc service handler.
///
/// For local (in-process) use, prefer `SecretServiceHandle` which wraps
/// this in thread-safe locks.
pub struct SecretService {
    handle: SecretServiceHandle,
}

impl SecretService {
    /// Create a new SecretService in the locked state.
    pub fn new() -> Self {
        Self {
            handle: SecretServiceHandle::new(),
        }
    }

    /// Get a handle for local (in-process) use.
    pub fn handle(&self) -> &SecretServiceHandle {
        &self.handle
    }
}

impl Default for SecretService {
    fn default() -> Self {
        Self::new()
    }
}

/// Actor that processes `SecretMessage` variants and dispatches to `SecretServiceHandle`.
///
/// The actor runs as a `tokio::task`, receives messages from an mpsc channel,
/// dispatches to the handle methods, and sends responses through oneshot channels.
///
/// # Usage
///
/// ```ignore
/// let handle = SecretServiceHandle::new();
/// let (client, actor) = SecretServiceActor::spawn(handle);
/// tokio::task::spawn(actor.run(rx));
/// // Use client to send messages
/// ```
pub struct SecretServiceActor {
    handle: SecretServiceHandle,
}

impl SecretServiceActor {
    /// Create a new actor wrapping the given handle.
    pub fn new(handle: SecretServiceHandle) -> Self {
        Self { handle }
    }

    /// Run the actor message loop, processing `SecretMessage` variants.
    ///
    /// This method runs until the receiver channel is closed. Each message
    /// variant is dispatched to the corresponding `SecretServiceHandle` method
    /// and the response is sent through the oneshot channel embedded in the message.
    pub async fn run(mut self, mut rx: tokio::sync::mpsc::Receiver<SecretMessage>) {
        while let Some(msg) = rx.recv().await {
            self.handle_message(msg);
        }
    }

    /// Spawn the actor as a `tokio::task` and return a `Client<SecretProtocol>` for sending messages.
    ///
    /// The actor runs on a tokio task and processes messages from the mpsc channel.
    /// The returned `Client<SecretProtocol>` can be used to send `SecretMessage` variants
    /// to the actor.
    pub fn spawn(
        handle: SecretServiceHandle,
    ) -> (irpc::Client<SecretProtocol>, SecretServiceActor) {
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let client = irpc::Client::local(tx);
        let actor = Self::new(handle.clone());
        tokio::task::spawn(actor.run(rx));
        (client, Self::new(handle))
    }

    /// Handle a single `SecretMessage` by dispatching to the appropriate handle method.
    fn handle_message(&mut self, msg: SecretMessage) {
        match msg {
            SecretMessage::DeriveEd25519(msg) => {
                let WithChannels { inner, tx, .. } = msg;
                let DeriveEd25519 { path } = inner;
                let result = self.handle.derive_ed25519(&path);
                tokio::spawn(async move {
                    let _ = tx.send(result).await;
                });
            }
            SecretMessage::DeriveEncryptionKey(msg) => {
                let WithChannels { inner, tx, .. } = msg;
                let DeriveEncryptionKey { path } = inner;
                let result = self.handle.derive_encryption_key(&path);
                tokio::spawn(async move {
                    let _ = tx.send(result).await;
                });
            }
            SecretMessage::DeriveEthereumKey(msg) => {
                let WithChannels { inner, tx, .. } = msg;
                let DeriveEthereumKey { path } = inner;
                let result = self.handle.derive_ethereum_key(&path);
                tokio::spawn(async move {
                    let _ = tx.send(result).await;
                });
            }
            SecretMessage::DerivePassword(msg) => {
                let WithChannels { inner, tx, .. } = msg;
                let DerivePassword { path, length } = inner;
                let result = self.handle.derive_password(&path, length);
                tokio::spawn(async move {
                    let _ = tx.send(result).await;
                });
            }
            SecretMessage::Encrypt(msg) => {
                let WithChannels { inner, tx, .. } = msg;
                let Encrypt {
                    plaintext,
                    key_version,
                } = inner;
                let result = self.handle.encrypt(&plaintext, key_version);
                tokio::spawn(async move {
                    let _ = tx.send(result).await;
                });
            }
            SecretMessage::Decrypt(msg) => {
                let WithChannels { inner, tx, .. } = msg;
                let Decrypt { encrypted } = inner;
                let result = self.handle.decrypt(&encrypted);
                tokio::spawn(async move {
                    let _ = tx.send(result).await;
                });
            }
            SecretMessage::Lock(msg) => {
                let WithChannels { inner: _, tx, .. } = msg;
                self.handle.lock();
                tokio::spawn(async move {
                    let _ = tx.send(Ok(())).await;
                });
            }
            SecretMessage::Unlock(msg) => {
                let WithChannels { inner, tx, .. } = msg;
                let Unlock { passphrase } = inner;
                let result = self.handle.unlock(&passphrase, None);
                tokio::spawn(async move {
                    let _ = tx.send(result).await;
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Lock;
    use irpc::channel::oneshot;
    use irpc::WithChannels;

    #[test]
    fn test_service_starts_locked() {
        let service = SecretServiceHandle::new();
        assert!(!service.is_unlocked());
    }

    #[test]
    fn test_unlock_new_generates_mnemonic() {
        let service = SecretServiceHandle::new();
        let phrase = service.unlock_new(24).unwrap();
        assert!(!phrase.is_empty());
        assert!(service.is_unlocked());
    }

    #[test]
    fn test_lock_purges_state() {
        let service = SecretServiceHandle::new();
        service.unlock_new(24).unwrap();
        assert!(service.is_unlocked());

        service.lock();
        assert!(!service.is_unlocked());
    }

    #[test]
    fn test_derive_on_locked_fails() {
        let service = SecretServiceHandle::new();
        let result = service.derive_ed25519(PATHS::IDENTITY);
        assert!(result.is_err());
    }

    #[test]
    fn test_encrypt_on_locked_fails() {
        let service = SecretServiceHandle::new();
        let result = service.encrypt("secret", 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_full_lifecycle() {
        let service = SecretServiceHandle::new();

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
    fn test_unlock_with_known_phrase() {
        let service = SecretServiceHandle::new();

        let phrase = service.unlock_new(24).unwrap();
        service.lock();

        service.unlock(&phrase, None).unwrap();
        assert!(service.is_unlocked());
    }

    #[test]
    fn test_double_unlock_fails() {
        let service = SecretServiceHandle::new();
        service.unlock_new(24).unwrap();

        let result = service.unlock_new(12);
        assert!(result.is_err());
    }

    #[test]
    fn test_encrypt_decrypt_lifecycle() {
        let service = SecretServiceHandle::new();
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
        let service = SecretServiceHandle::new();
        service.unlock_new(24).unwrap();

        let path = "m/74'/1'/0'/12345'";
        let pw1 = service.derive_password(path, 16).unwrap();
        let pw2 = service.derive_password(path, 16).unwrap();
        assert_eq!(pw1, pw2, "derive_password must be deterministic");
    }

    #[test]
    fn test_derive_password_different_paths() {
        let service = SecretServiceHandle::new();
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
        let service = SecretServiceHandle::new();
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
        let service = SecretServiceHandle::new();
        let result = service.derive_password("m/74'/1'/0'/1'", 16);
        assert!(matches!(result, Err(SecretServiceError::ServiceLocked)));
    }

    #[test]
    fn test_derive_password_string_base64url() {
        let service = SecretServiceHandle::new();
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
        let service = SecretServiceHandle::new();
        service.unlock_new(24).unwrap();

        let key = service.derive_ethereum_key(PATHS::ETHEREUM).unwrap();
        assert_eq!(key.key_type, KeyType::Secp256k1);
        assert_eq!(key.private_key.len(), 32);
        assert_eq!(key.public_key.len(), 33);
    }

    #[cfg(feature = "secp256k1")]
    #[test]
    fn test_ethereum_key_differs_from_ed25519() {
        let service = SecretServiceHandle::new();
        service.unlock_new(24).unwrap();

        let eth_key = service.derive_ethereum_key(PATHS::ETHEREUM).unwrap();
        let ed_key = service.derive_ed25519(PATHS::IDENTITY).unwrap();

        assert_ne!(eth_key.private_key, ed_key.private_key);
    }

    #[cfg(not(feature = "secp256k1"))]
    #[test]
    fn test_derive_ethereum_key_unsupported_without_feature() {
        let service = SecretServiceHandle::new();
        service.unlock_new(24).unwrap();

        let result = service.derive_ethereum_key(PATHS::ETHEREUM);
        assert!(matches!(
            result,
            Err(SecretServiceError::UnsupportedKeyType)
        ));
    }

    #[test]
    fn test_cache_hit_avoids_re_derivation() {
        let service = SecretServiceHandle::new();
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
        let service = SecretServiceHandle::new();
        service.unlock_new(24).unwrap();

        assert_eq!(service.inner.read().unwrap().cache.len(), 0);

        service.derive_ed25519(PATHS::IDENTITY).unwrap();

        assert_eq!(service.inner.read().unwrap().cache.len(), 1);
    }

    #[test]
    fn test_expired_entry_evicted_on_access() {
        let config = crate::cache::CacheConfig::new(std::time::Duration::from_millis(5), 64);
        let service = SecretServiceHandle::with_cache_config(config);
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
        let service = SecretServiceHandle::with_cache_config(config);
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
        let service = SecretServiceHandle::new();
        service.unlock_new(24).unwrap();

        service.derive_ed25519(PATHS::IDENTITY).unwrap();
        service.derive_ed25519(PATHS::SSH_HOST).unwrap();
        assert_eq!(service.inner.read().unwrap().cache.len(), 2);

        service.lock();

        assert_eq!(service.inner.read().unwrap().cache.len(), 0);
    }

    #[test]
    fn test_encrypt_decrypt_uses_cached_encryption_key() {
        let service = SecretServiceHandle::new();
        service.unlock_new(24).unwrap();

        let plaintext = "cached-encryption-test";
        let encrypted = service.encrypt(plaintext, 1).unwrap();
        assert_eq!(service.inner.read().unwrap().cache.len(), 1);

        let decrypted = service.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, plaintext);

        assert_eq!(service.inner.read().unwrap().cache.len(), 1);
    }

    #[tokio::test]
    async fn test_actor_unlock_responds_successfully() {
        let handle = SecretServiceHandle::new();
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let actor = SecretServiceActor::new(handle);
        tokio::task::spawn(actor.run(rx));

        let (resp_tx, resp_rx) = oneshot::channel();
        let msg = SecretMessage::Unlock(WithChannels::from((
            Unlock {
                passphrase: "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about".to_string(),
            },
            resp_tx,
        )));
        tx.send(msg).await.unwrap();

        let result = resp_rx.await.unwrap();
        assert!(result.is_ok(), "Unlock via actor must succeed");
    }

    #[tokio::test]
    async fn test_actor_derive_ed25519_returns_key() {
        let handle = SecretServiceHandle::new();
        handle.unlock_new(24).unwrap();
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let actor = SecretServiceActor::new(handle);
        tokio::task::spawn(actor.run(rx));

        let (resp_tx, resp_rx) = oneshot::channel();
        let msg = SecretMessage::DeriveEd25519(WithChannels::from((
            DeriveEd25519 {
                path: PATHS::IDENTITY.to_string(),
            },
            resp_tx,
        )));
        tx.send(msg).await.unwrap();

        let result = resp_rx.await.unwrap();
        assert!(result.is_ok(), "DeriveEd25519 via actor must succeed");
        let key = result.unwrap();
        assert!(
            !key.private_key.is_empty(),
            "DerivedKey must have private_key"
        );
        assert_eq!(key.key_type, KeyType::Ed25519);
    }

    #[tokio::test]
    async fn test_actor_lock_clears_state() {
        let handle = SecretServiceHandle::new();
        handle.unlock_new(24).unwrap();
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let actor = SecretServiceActor::new(handle.clone());
        tokio::task::spawn(actor.run(rx));

        let (resp_tx, resp_rx): (oneshot::Sender<Result<(), SecretServiceError>>, _) =
            oneshot::channel();
        let msg = SecretMessage::Lock(WithChannels::from((Lock, resp_tx)));
        tx.send(msg).await.unwrap();

        let result = resp_rx.await.unwrap();
        assert!(result.is_ok(), "Lock via actor must succeed");
        assert!(!handle.is_unlocked(), "Handle must be locked after Lock");
    }
}
