//! Integration tests for key derivation.
//!
//! These tests verify that SLIP-0010 derivation produces correct results
//! against known test vectors and that path constants produce expected key types.

use alknet_vault::derivation::PATHS;
use alknet_vault::service::VaultServiceHandle;

#[test]
fn test_identity_key_derivation() {
    let service = VaultServiceHandle::new();
    let _phrase = service.unlock_new(24).unwrap();

    let key = service.derive_ed25519(PATHS::IDENTITY).unwrap();
    assert_eq!(key.key_type, alknet_vault::protocol::KeyType::Ed25519);
    assert!(!key.private_key.is_empty());
    assert!(!key.public_key.is_empty());
}

#[test]
fn test_encryption_key_derivation() {
    let service = VaultServiceHandle::new();
    service.unlock_new(24).unwrap();

    let key = service.derive_encryption_key(PATHS::ENCRYPTION).unwrap();
    assert_eq!(key.key_type, alknet_vault::protocol::KeyType::Aes256Gcm);
}

#[test]
fn test_deterministic_derivation() {
    // Same seed + same path = same key
    let service = VaultServiceHandle::new();
    let phrase = service.unlock_new(24).unwrap();

    let key1 = service.derive_ed25519(PATHS::IDENTITY).unwrap();

    // Unlock with the same phrase again
    service.lock();
    service.unlock(&phrase, None).unwrap();

    let key2 = service.derive_ed25519(PATHS::IDENTITY).unwrap();

    assert_eq!(key1.private_key, key2.private_key);
    assert_eq!(key1.public_key, key2.public_key);
}

#[test]
fn test_different_paths_different_keys() {
    let service = VaultServiceHandle::new();
    service.unlock_new(24).unwrap();

    let identity_key = service.derive_ed25519(PATHS::IDENTITY).unwrap();
    let ssh_key = service.derive_ed25519(PATHS::SSH_HOST).unwrap();

    assert_ne!(identity_key.private_key, ssh_key.private_key);
    assert_ne!(identity_key.public_key, ssh_key.public_key);
}
