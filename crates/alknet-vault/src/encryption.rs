//! AES-256-GCM encryption and decryption for external credentials.
//!
//! External credentials (API keys, OAuth tokens) that cannot be derived from the
//! seed are encrypted using a key derived from the seed at path `m/74'/2'/0'/0'`.
//! The `EncryptedData` type stores the key version, salt, IV, and ciphertext.
//!
//! # Salt Field (Reserved for Future KDF-Based Key Derivation)
//!
//! The `salt` field in `EncryptedData` is **reserved for future KDF-based key
//! derivation** (Phase B). In v2, the encryption key is derived directly from the
//! seed at path `m/74'/2'/0'/0'` without using the salt. The salt is generated
//! randomly (32 bytes) and stored in `EncryptedData.salt` for forward
//! compatibility, but it plays no role in the v2 key derivation process.
//!
//! When key rotation is implemented in Phase B, the salt will be used as input to
//! HKDF or PBKDF2 for stretch-based key derivation, allowing the same seed to
//! produce different encryption keys without changing the derivation path. This
//! design ensures that the wire format does not need to change — the `salt` field
//! is already present and populated.
//!
//! # Wire Format
//!
//! The `EncryptedData` struct is the stable wire format shared with alknet-storage.
//! This is type-level compatibility, not a crate dependency. Both crates must
//! agree on the serialization format.
//!
//! # Key Versioning
//!
//! Key versioning allows re-encryption when the encryption key is rotated. The
//! current key version is `2` (HD-derived at `m/74'/2'/0'/0'`). Version `1` is
//! reserved for the TypeScript predecessor's PBKDF2-encrypted data, which the
//! vault cannot decrypt (different key derivation) — migration is a one-time
//! re-encryption. Each version maps to a unique derivation path
//! (`m/74'/2'/0'/{version-2}'`, see ADR-021). To rotate:
//! 1. Decrypt all existing `EncryptedData` with the old key version
//! 2. Re-encrypt with the new key version (via `VaultServiceHandle::rotate`)
//! 3. Update storage

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::fmt;
use zeroize::Zeroize;

/// Current default key version for encryption.
///
/// Version `2` is HD-derived at `m/74'/2'/0'/0'` (`PATHS::ENCRYPTION`) per
/// ADR-020. Version `1` is reserved for the TypeScript predecessor's
/// PBKDF2-encrypted data, which the vault cannot decrypt.
pub const CURRENT_KEY_VERSION: u32 = 2;

/// Encrypted data blob stored in the metagraph.
///
/// This is the stable wire format shared with alknet-storage. The fields are
/// Base64-encoded strings for JSON serialization compatibility.
///
/// # Compatibility
///
/// The Rust `EncryptedData` is a superset of the TypeScript `EncryptedDataSchema`
/// from `@alkdev/storage`. Migration path: re-encrypt TypeScript-encrypted data
/// using the Rust vault with a new key version.
///
/// See OQ-SVC-03 for the compatibility tracking.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EncryptedData {
    /// Key version for rotation support.
    pub key_version: u32,
    /// Base64-encoded random salt.
    ///
    /// **Reserved for future KDF-based key derivation (Phase B).** In v2, the
    /// encryption key is derived directly from the seed at path `m/74'/2'/0'/0'`
    /// without using the salt. The salt is generated and stored for forward
    /// compatibility but does not participate in key derivation.
    pub salt: String,
    /// Base64-encoded initialization vector (12 bytes for AES-GCM).
    pub iv: String,
    /// Base64-encoded ciphertext (AES-256-GCM encrypted, includes auth tag).
    pub data: String,
}

/// Encryption key material derived from the seed.
///
/// Holds the 32-byte AES-256-GCM key and its derivation metadata.
/// Zeroized on drop per ADR-038. Not `Clone` — move-only, like `DerivedKey`.
/// Implements a custom redacting `Debug` (never prints key bytes).
#[derive(Zeroize)]
#[zeroize(drop)]
pub struct EncryptionKey {
    key_bytes: [u8; 32],
    key_version: u32,
}

impl EncryptionKey {
    /// Construct from raw 32 bytes. Private — for internal use (tests).
    #[cfg(test)]
    fn new(key_bytes: [u8; 32], key_version: u32) -> Self {
        Self {
            key_bytes,
            key_version,
        }
    }

    /// Take the first 32 bytes of derived key material (the private key
    /// bytes from SLIP-0010 derivation) and construct an `EncryptionKey`.
    /// This is the bridge from `DerivedKey` (SLIP-0010 output) to
    /// `EncryptionKey` (AES-256-GCM input). `VaultServiceHandle::encrypt`
    /// and `decrypt` call this on the cached `DerivedKey` to obtain the
    /// `EncryptionKey` for the crypto layer.
    pub fn from_derived_bytes(bytes: &[u8], key_version: u32) -> Self {
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes[..32]);
        Self {
            key_bytes: key,
            key_version,
        }
    }

    /// Return the key version (for rotation tracking).
    pub fn version(&self) -> u32 {
        self.key_version
    }

    /// Return the key bytes (crate-internal — for `encrypt`/`decrypt`).
    pub(crate) fn key_bytes(&self) -> &[u8; 32] {
        &self.key_bytes
    }
}

impl fmt::Debug for EncryptionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EncryptionKey")
            .field("key_version", &self.key_version)
            .field("key_bytes", &"[REDACTED]")
            .finish()
    }
}

/// Encrypt plaintext using an AES-256-GCM key.
///
/// Generates a random 12-byte IV and a random 32-byte salt for each encryption.
/// The salt allows key rotation without re-deriving from the seed.
///
/// # Arguments
///
/// * `plaintext` - The string to encrypt
/// * `key` - The encryption key derived from the seed
/// * `key_version` - The key version for rotation tracking
///
/// # Returns
///
/// An `EncryptedData` struct suitable for storage in the metagraph.
pub(crate) fn encrypt(
    plaintext: &str,
    key: &EncryptionKey,
) -> Result<EncryptedData, EncryptionError> {
    let cipher = Aes256Gcm::new_from_slice(key.key_bytes())
        .map_err(|e| EncryptionError::Encryption(format!("invalid key length: {e}")))?;

    // Generate random IV (12 bytes for AES-GCM) using OsRng CSPRNG
    let mut iv_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut iv_bytes);
    let nonce = Nonce::from_slice(&iv_bytes);

    // TODO(Phase B): Use salt in HKDF-based key derivation
    let mut salt_bytes = [0u8; 32];
    OsRng.fill_bytes(&mut salt_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| EncryptionError::Encryption(e.to_string()))?;

    Ok(EncryptedData {
        key_version: key.key_version,
        salt: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, salt_bytes),
        iv: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, iv_bytes),
        data: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &ciphertext),
    })
}

/// Decrypt an `EncryptedData` blob back to plaintext.
///
/// # Arguments
///
/// * `encrypted` - The encrypted data blob from storage
/// * `key` - The encryption key derived from the seed (must match `key_version`)
///
/// # Returns
///
/// The decrypted plaintext string.
pub(crate) fn decrypt(
    encrypted: &EncryptedData,
    key: &EncryptionKey,
) -> Result<String, EncryptionError> {
    let cipher = Aes256Gcm::new_from_slice(key.key_bytes())
        .map_err(|e| EncryptionError::Decryption(format!("invalid key length: {e}")))?;

    let iv_bytes =
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &encrypted.iv)
            .map_err(|e| EncryptionError::Decoding(e.to_string()))?;
    let nonce = Nonce::from_slice(&iv_bytes);

    let ciphertext =
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &encrypted.data)
            .map_err(|e| EncryptionError::Decoding(e.to_string()))?;

    let plaintext = cipher
        .decrypt(nonce, ciphertext.as_ref())
        .map_err(|e| EncryptionError::Decryption(e.to_string()))?;

    String::from_utf8(plaintext).map_err(|e| EncryptionError::Decryption(e.to_string()))
}

/// Errors that can occur during encryption/decryption operations.
#[derive(Debug, thiserror::Error)]
pub enum EncryptionError {
    #[error("encryption error: {0}")]
    Encryption(String),
    #[error("decryption error: {0}")]
    Decryption(String),
    #[error("base64 decoding error: {0}")]
    Decoding(String),
    #[error("key version mismatch: expected {expected}, got {actual}")]
    KeyVersionMismatch { expected: u32, actual: u32 },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_key() -> EncryptionKey {
        let key_bytes = [42u8; 32];
        EncryptionKey::new(key_bytes, CURRENT_KEY_VERSION)
    }

    #[test]
    fn test_encrypt_decrypt_round_trip() {
        let key = make_test_key();
        let plaintext = "hello, world! this is a secret API key";

        let encrypted = encrypt(plaintext, &key).unwrap();
        let decrypted = decrypt(&encrypted, &key).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_encrypted_data_has_different_iv_each_time() {
        let key = make_test_key();
        let plaintext = "same input";

        let encrypted1 = encrypt(plaintext, &key).unwrap();
        let encrypted2 = encrypt(plaintext, &key).unwrap();

        // Same plaintext encrypted twice should have different IVs and ciphertexts
        assert_ne!(encrypted1.iv, encrypted2.iv);
        assert_ne!(encrypted1.data, encrypted2.data);
    }

    #[test]
    fn test_encrypt_decrypt_with_key_version() {
        let key = EncryptionKey::new([7u8; 32], 2);
        let plaintext = "versioned encryption test";

        let encrypted = encrypt(plaintext, &key).unwrap();
        assert_eq!(encrypted.key_version, 2);

        let decrypted = decrypt(&encrypted, &key).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_decrypt_with_wrong_key_fails() {
        let key1 = EncryptionKey::new([1u8; 32], 1);
        let key2 = EncryptionKey::new([2u8; 32], 1);

        let encrypted = encrypt("secret stuff", &key1).unwrap();
        let result = decrypt(&encrypted, &key2);
        assert!(result.is_err());
    }

    #[test]
    fn test_encryption_key_debug_redacts_key_bytes() {
        let key = EncryptionKey::new([0xABu8; 32], 2);
        let debug_output = format!("{:?}", key);
        assert!(
            debug_output.contains("[REDACTED]"),
            "Debug must redact key_bytes, got: {debug_output}"
        );
        assert!(
            !debug_output.contains("AB"),
            "Debug must not leak key bytes, got: {debug_output}"
        );
        assert!(
            debug_output.contains("key_version"),
            "Debug must show key_version, got: {debug_output}"
        );
    }

    #[test]
    fn test_encryption_key_version_accessor() {
        let key = EncryptionKey::new([0u8; 32], 7);
        assert_eq!(key.version(), 7);
    }

    #[test]
    fn test_encryption_key_key_bytes_accessor() {
        let key = EncryptionKey::new([0x42u8; 32], 2);
        assert_eq!(key.key_bytes(), &[0x42u8; 32]);
    }

    #[test]
    fn test_encryption_key_from_derived_bytes_takes_first_32() {
        let derived = [0xAAu8; 64];
        let key = EncryptionKey::from_derived_bytes(&derived, 3);
        assert_eq!(key.key_bytes(), &[0xAAu8; 32]);
        assert_eq!(key.version(), 3);
    }
}
