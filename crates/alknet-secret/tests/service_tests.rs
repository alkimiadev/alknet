//! Integration tests for the SecretService lifecycle.
//!
//! These tests verify the unlock/lock lifecycle, error conditions,
//! and that the service correctly manages state transitions.

use alknet_secret::service::{SecretServiceError, SecretServiceHandle};
use alknet_secret::derivation::PATHS;

#[test]
fn test_full_lifecycle() {
    let service = SecretServiceHandle::new();

    // Starts locked
    assert!(!service.is_unlocked());

    // Can't derive while locked
    let result = service.derive_ed25519(PATHS::IDENTITY);
    assert!(matches!(result, Err(SecretServiceError::ServiceLocked)));

    // Unlock
    let phrase = service.unlock_new(24).unwrap();
    assert!(service.is_unlocked());
    assert!(!phrase.is_empty());

    // Can derive while unlocked
    let key = service.derive_ed25519(PATHS::IDENTITY).unwrap();
    assert!(!key.private_key.is_empty());

    // Lock
    service.lock();
    assert!(!service.is_unlocked());

    // Can't derive again
    let result = service.derive_ed25519(PATHS::IDENTITY);
    assert!(matches!(result, Err(SecretServiceError::ServiceLocked)));
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

    // Different passphrase produces different seed
    // (tested by deriving keys with different passphrases)
}

#[test]
fn test_double_unlock_fails() {
    let service = SecretServiceHandle::new();
    service.unlock_new(24).unwrap();

    let result = service.unlock_new(12);
    assert!(matches!(result, Err(SecretServiceError::AlreadyUnlocked)));
}

#[test]
fn test_lock_when_already_locked_is_noop() {
    let service = SecretServiceHandle::new();
    assert!(!service.is_unlocked());

    // Lock on already-locked service is a no-op
    service.lock();
    assert!(!service.is_unlocked());
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
    let result = service.decrypt(&encrypted);
    assert!(matches!(result, Err(SecretServiceError::ServiceLocked)));
}

#[test]
fn test_multiple_derive_paths_succeed() {
    let service = SecretServiceHandle::new();
    service.unlock_new(24).unwrap();

    // All standard paths should work
    let _identity = service.derive_ed25519(PATHS::IDENTITY).unwrap();
    let _ssh = service.derive_ed25519(PATHS::SSH_HOST).unwrap();
    let _enc = service
        .derive_encryption_key(PATHS::ENCRYPTION)
        .unwrap();
}