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
//! # Assembly
//!
//! The `SecretService` is assembled by the CLI binary or NAPI layer. Per ADR-027,
//! alknet-core never sees the secret service directly — it is wired through the
//! `OperationEnv` dispatch mechanism. For minimal deployments, no secret service
//! is available (the `SecretStoreCredentialProvider` returns `None`).

use std::sync::{Arc, RwLock};

use crate::derivation::{self, DerivationError, PATHS};
use crate::encryption::{self, EncryptedData, EncryptionKey};
use crate::mnemonic::{Language, Mnemonic, Seed};
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
}

/// Errors that can occur during secret service operations.
#[derive(Debug, thiserror::Error)]
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
    /// Create a new SecretServiceHandle in the locked state.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(SecretServiceInner {
                mnemonic: None,
                seed: None,
                unlocked: false,
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
        inner.seed = None; // Seed's Zeroize drop handles the zeroization
        inner.mnemonic = None; // Mnemonic's Zeroize drop handles the zeroization
        inner.unlocked = false;
    }

    /// Check whether the service is currently unlocked.
    pub fn is_unlocked(&self) -> bool {
        self.inner.read().unwrap().unlocked
    }

    /// Derive an Ed25519 keypair at the given path.
    pub fn derive_ed25519(&self, path: &str) -> Result<DerivedKey, SecretServiceError> {
        let inner = self.inner.read().unwrap();
        if !inner.unlocked {
            return Err(SecretServiceError::ServiceLocked);
        }
        let seed = inner
            .seed
            .as_ref()
            .ok_or(SecretServiceError::ServiceLocked)?;

        let key = derivation::derive_path_from_seed(seed.as_bytes(), path)?;
        Ok(DerivedKey {
            key_type: KeyType::Ed25519,
            private_key: key.private_key().to_vec(),
            public_key: key.public_key().to_vec(),
        })
    }

    /// Derive an AES-256-GCM encryption key at the given path.
    pub fn derive_encryption_key(&self, path: &str) -> Result<DerivedKey, SecretServiceError> {
        let inner = self.inner.read().unwrap();
        if !inner.unlocked {
            return Err(SecretServiceError::ServiceLocked);
        }
        let seed = inner
            .seed
            .as_ref()
            .ok_or(SecretServiceError::ServiceLocked)?;

        let key = derivation::derive_path_from_seed(seed.as_bytes(), path)?;
        Ok(DerivedKey {
            key_type: KeyType::Aes256Gcm,
            private_key: key.private_key().to_vec(),
            public_key: key.public_key().to_vec(),
        })
    }

    /// Derive a secp256k1 (Ethereum) keypair at the given path.
    pub fn derive_ethereum_key(&self, path: &str) -> Result<DerivedKey, SecretServiceError> {
        let inner = self.inner.read().unwrap();
        if !inner.unlocked {
            return Err(SecretServiceError::ServiceLocked);
        }
        let seed = inner
            .seed
            .as_ref()
            .ok_or(SecretServiceError::ServiceLocked)?;

        let key = derivation::derive_path_from_seed(seed.as_bytes(), path)?;
        Ok(DerivedKey {
            key_type: KeyType::Secp256k1,
            private_key: key.private_key().to_vec(),
            public_key: key.public_key().to_vec(),
        })
    }

    /// Encrypt plaintext using the derived encryption key.
    ///
    /// Uses the key at path `m/74'/2'/0'/0'` (PATHS::ENCRYPTION) by default.
    pub fn encrypt(
        &self,
        plaintext: &str,
        key_version: u32,
    ) -> Result<EncryptedData, SecretServiceError> {
        let inner = self.inner.read().unwrap();
        if !inner.unlocked {
            return Err(SecretServiceError::ServiceLocked);
        }
        let seed = inner
            .seed
            .as_ref()
            .ok_or(SecretServiceError::ServiceLocked)?;

        let derived = derivation::derive_path_from_seed(seed.as_bytes(), PATHS::ENCRYPTION)?;
        let enc_key = EncryptionKey::from_derived_bytes(derived.private_key(), key_version);

        encryption::encrypt(plaintext, &enc_key).map_err(|e| e.into())
    }

    /// Decrypt an EncryptedData blob using the derived encryption key.
    pub fn decrypt(&self, encrypted: &EncryptedData) -> Result<String, SecretServiceError> {
        let inner = self.inner.read().unwrap();
        if !inner.unlocked {
            return Err(SecretServiceError::ServiceLocked);
        }
        let seed = inner
            .seed
            .as_ref()
            .ok_or(SecretServiceError::ServiceLocked)?;

        let derived = derivation::derive_path_from_seed(seed.as_bytes(), PATHS::ENCRYPTION)?;
        let enc_key =
            EncryptionKey::from_derived_bytes(derived.private_key(), encrypted.key_version);

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

#[cfg(test)]
mod tests {
    use super::*;

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

        // Starts locked
        assert!(!service.is_unlocked());

        // Can't derive while locked
        assert!(service.derive_ed25519(PATHS::IDENTITY).is_err());

        // Unlock
        let phrase = service.unlock_new(24).unwrap();
        assert!(service.is_unlocked());

        // Can derive while unlocked
        let key = service.derive_ed25519(PATHS::IDENTITY).unwrap();
        assert!(!key.private_key.is_empty());

        // Lock
        service.lock();
        assert!(!service.is_unlocked());

        // Can't derive again
        assert!(service.derive_ed25519(PATHS::IDENTITY).is_err());
    }

    #[test]
    fn test_unlock_with_known_phrase() {
        let service = SecretServiceHandle::new();

        // Generate a phrase
        let phrase = service.unlock_new(24).unwrap();
        service.lock();

        // Re-unlock with the same phrase
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

        // After lock, can't decrypt
        service.lock();
        assert!(service.decrypt(&encrypted).is_err());
    }
}
