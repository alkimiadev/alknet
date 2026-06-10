//! Integration tests for AES-256-GCM encryption and decryption.
//!
//! These tests verify round-trip encryption, key version handling,
//! and wire format compatibility.

use alknet_secret::encryption::CURRENT_KEY_VERSION;
use alknet_secret::service::SecretServiceHandle;

#[test]
fn test_encrypt_decrypt_round_trip_via_service() {
    let service = SecretServiceHandle::new();
    service.unlock_new(24).unwrap();

    let plaintext = "sk-proj-abc123xyz789";

    let encrypted = service.encrypt(plaintext, CURRENT_KEY_VERSION).unwrap();
    let decrypted = service.decrypt(&encrypted).unwrap();

    assert_eq!(decrypted, plaintext);
}

#[test]
fn test_encrypt_produces_different_ciphertext_each_time() {
    let service = SecretServiceHandle::new();
    service.unlock_new(24).unwrap();

    let plaintext = "same input different ciphertexts";

    let encrypted1 = service.encrypt(plaintext, CURRENT_KEY_VERSION).unwrap();
    let encrypted2 = service.encrypt(plaintext, CURRENT_KEY_VERSION).unwrap();

    // Different IVs mean different ciphertexts
    assert_ne!(encrypted1.iv, encrypted2.iv);
    assert_ne!(encrypted1.data, encrypted2.data);
    // But same key version
    assert_eq!(encrypted1.key_version, encrypted2.key_version);
}

#[test]
fn test_encrypted_data_serialization() {
    let service = SecretServiceHandle::new();
    service.unlock_new(24).unwrap();

    let plaintext = "test serialization";
    let encrypted = service.encrypt(plaintext, CURRENT_KEY_VERSION).unwrap();

    // Verify EncryptedData serializes to JSON
    let json = serde_json::to_string(&encrypted).unwrap();
    assert!(json.contains("key_version"));
    assert!(json.contains("salt"));
    assert!(json.contains("iv"));
    assert!(json.contains("data"));

    // Verify round-trip through JSON
    let deserialized: alknet_secret::encryption::EncryptedData =
        serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, encrypted);
}
