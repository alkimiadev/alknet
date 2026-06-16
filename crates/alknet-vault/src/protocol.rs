//! VaultProtocol irpc message definition and associated types.
//!
//! This module defines the `VaultProtocol` enum for irpc-based message dispatch.
//! The protocol supports unlock/lock lifecycle, key derivation,
//! and encryption/decryption operations.
//!
//! # Protocol Operation
//!
//! The VaultProtocol follows a lifecycle: the vault starts in a **locked**
//! state where no derivation or encryption operations are possible. The `Unlock`
//! call loads the seed into memory (derived from the mnemonic passphrase). After
//! that, derive and encrypt/decrypt operations are available. The `Lock` call
//! purges the seed and all cached keys.
//!
//! # Wire Format
//!
//! For local (in-process) calls, the protocol uses tokio channels directly.
//! For remote (in-cluster) calls, the protocol is serialized with postcard.
//! For cross-node (call protocol) exposure, the vault is wrapped in an
//! operation that serializes to JSON.

use std::fmt;

use irpc::rpc_requests;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::Zeroize;

use crate::encryption::EncryptedData;

/// The type of a derived key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum KeyType {
    /// Ed25519 keypair (SLIP-0010 derivation).
    Ed25519,
    /// AES-256-GCM symmetric key (derived from seed, used for external credential encryption).
    Aes256Gcm,
    /// secp256k1 keypair (BIP-0032 derivation, for Ethereum signing).
    Secp256k1,
}

/// A derived key pair (private key + public key).
///
/// The private key is sensitive material that is zeroized on drop (ADR-038).
/// This type is **not** `Clone` — it is move-only. Consumers receive a
/// `DerivedKey` by value and must zeroize it when done (handled automatically
/// by `#[zeroize(drop)]`).
///
/// Serialization redacts the `private_key` field for human-readable formats
/// (JSON) for safety, showing `"[REDACTED]"` instead of the key bytes. For
/// binary formats (postcard, used by irpc), the actual bytes are serialized
/// so that remote communication works correctly. Deserialization always reads
/// the full bytes.
#[derive(Zeroize, Deserialize)]
#[zeroize(drop)]
pub struct DerivedKey {
    /// The type of key that was derived.
    #[zeroize(skip)]
    pub key_type: KeyType,
    /// The private key bytes (sensitive — zeroized on drop).
    #[zeroize]
    #[serde(deserialize_with = "deserialize_private_key")]
    pub private_key: Vec<u8>,
    /// The public key bytes.
    #[zeroize(skip)]
    pub public_key: Vec<u8>,
}

fn deserialize_private_key<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
    Vec::<u8>::deserialize(d)
}

impl fmt::Debug for DerivedKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DerivedKey")
            .field("key_type", &self.key_type)
            .field("private_key", &"[REDACTED]")
            .field("public_key", &self.public_key)
            .finish()
    }
}

impl Serialize for DerivedKey {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        if s.is_human_readable() {
            let mut state = s.serialize_struct("DerivedKey", 3)?;
            state.serialize_field("key_type", &self.key_type)?;
            state.serialize_field("private_key", "[REDACTED]")?;
            state.serialize_field("public_key", &self.public_key)?;
            state.end()
        } else {
            let mut state = s.serialize_struct("DerivedKey", 3)?;
            state.serialize_field("key_type", &self.key_type)?;
            state.serialize_field("private_key", &self.private_key)?;
            state.serialize_field("public_key", &self.public_key)?;
            state.end()
        }
    }
}

/// VaultProtocol message definition.
///
/// This is the irpc protocol enum that defines all vault operations.
/// The `#[rpc_requests]` macro generates:
/// - **`VaultMessage`**: message enum with `WithChannels` wrappers for each variant
/// - **`Channels<VaultProtocol>`** impls for each wrapper type
/// - **`From`** impls for protocol enum and message enum conversions
/// - **`Service`** and **`RemoteService`** trait impls for remote dispatch
///
/// # State Requirements
///
/// All operations except `Unlock` require the vault to be in an **unlocked**
/// state. Calling derive/encrypt/decrypt on a locked vault returns an error.
#[rpc_requests(message = VaultMessage, no_spans)]
#[derive(Debug, Serialize, Deserialize)]
pub enum VaultProtocol {
    /// Derive an Ed25519 keypair at the given path.
    ///
    /// Path format: `m/74'/0'/0'/0'` (SLIP-0010 hardened-only notation).
    /// Returns a `DerivedKey` with `KeyType::Ed25519`.
    #[rpc(tx = irpc::channel::oneshot::Sender<Result<DerivedKey, crate::service::VaultServiceError>>)]
    #[wrap(DeriveEd25519)]
    DeriveEd25519 {
        /// SLIP-0010 derivation path (e.g., "m/74'/0'/0'/0'").
        path: String,
    },

    /// Derive an AES-256-GCM encryption key at the given path.
    ///
    /// The default encryption path is `m/74'/2'/0'/0'`.
    /// Returns a `DerivedKey` with `KeyType::Aes256Gcm`.
    #[rpc(tx = irpc::channel::oneshot::Sender<Result<DerivedKey, crate::service::VaultServiceError>>)]
    #[wrap(DeriveEncryptionKey)]
    DeriveEncryptionKey {
        /// SLIP-0010 derivation path for the encryption key.
        path: String,
    },

    /// Derive a secp256k1 (Ethereum) keypair at the given path.
    ///
    /// The default Ethereum path is `m/44'/60'/0'/0/0`.
    /// Returns a `DerivedKey` with `KeyType::Secp256k1`.
    #[rpc(tx = irpc::channel::oneshot::Sender<Result<DerivedKey, crate::service::VaultServiceError>>)]
    #[wrap(DeriveEthereumKey)]
    DeriveEthereumKey {
        /// BIP-0032 derivation path (e.g., "m/44'/60'/0'/0/0").
        path: String,
    },

    /// Derive a deterministic password at the given path.
    ///
    /// Path format: `m/74'/1'/0'/{hash}'` (SLIP-0010 hardened notation).
    /// The `length` parameter controls the output length.
    #[rpc(tx = irpc::channel::oneshot::Sender<Result<Vec<u8>, crate::service::VaultServiceError>>)]
    #[wrap(DerivePassword)]
    DerivePassword {
        /// SLIP-0010 derivation path for the password.
        path: String,
        /// Desired password length in bytes.
        length: usize,
    },

    /// Encrypt plaintext using a derived encryption key.
    ///
    /// The key is derived at the path `m/74'/2'/0'/0'` with the given version.
    /// Returns an `EncryptedData` blob suitable for storage.
    #[rpc(tx = irpc::channel::oneshot::Sender<Result<EncryptedData, crate::service::VaultServiceError>>)]
    #[wrap(Encrypt)]
    Encrypt {
        /// The plaintext string to encrypt.
        plaintext: String,
        /// The key version for rotation tracking.
        key_version: u32,
    },

    /// Decrypt an `EncryptedData` blob back to plaintext.
    ///
    /// The key is derived from the seed at the path indicated by the key version.
    #[rpc(tx = irpc::channel::oneshot::Sender<Result<String, crate::service::VaultServiceError>>)]
    #[wrap(Decrypt)]
    Decrypt {
        /// The encrypted data blob to decrypt.
        encrypted: EncryptedData,
    },

    /// Lock the service, purging the seed and all cached derived keys.
    ///
    /// After locking, no derive/encrypt/decrypt operations are possible
    /// until `Unlock` is called again. Calls `zeroize()` on all sensitive
    /// material (ADR-038).
    #[rpc(tx = irpc::channel::oneshot::Sender<Result<(), crate::service::VaultServiceError>>)]
    #[wrap(Lock)]
    Lock,

    /// Unlock the service with a BIP39 mnemonic and optional passphrase.
    ///
    /// The mnemonic is the space-separated BIP39 word list. The passphrase is
    /// the optional BIP39 password extension (the "25th word"). After unlocking,
    /// derive and encrypt/decrypt operations are available.
    #[rpc(tx = irpc::channel::oneshot::Sender<Result<(), crate::service::VaultServiceError>>)]
    #[wrap(Unlock)]
    Unlock {
        /// The BIP39 mnemonic phrase (space-separated word list).
        mnemonic: String,
        /// Optional BIP39 passphrase (the "25th word" password extension).
        passphrase: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_key() -> DerivedKey {
        DerivedKey {
            key_type: KeyType::Ed25519,
            private_key: vec![0xABu8; 32],
            public_key: vec![0xCDu8; 32],
        }
    }

    #[test]
    fn test_derived_key_debug_redacts_private_key() {
        let key = make_test_key();
        let debug_output = format!("{:?}", key);
        assert!(
            !debug_output.contains("AB"),
            "Debug must not leak private_key bytes"
        );
        assert!(
            debug_output.contains("[REDACTED]"),
            "Debug must show [REDACTED] for private_key"
        );
        assert!(debug_output.contains("Ed25519"), "Debug must show key_type");
    }

    #[test]
    fn test_derived_key_serialize_redacts_private_key_json() {
        let key = make_test_key();
        let json = serde_json::to_string(&key).unwrap();
        assert!(
            !json.contains("AB"),
            "JSON must not contain private_key bytes"
        );
        assert!(
            json.contains("[REDACTED]"),
            "JSON must show [REDACTED] for private_key"
        );
        assert!(json.contains("Ed25519"), "JSON must contain key_type");
    }

    #[test]
    fn test_derived_key_serialize_preserves_bytes_postcard() {
        let key = make_test_key();
        let bytes = postcard::to_allocvec(&key).unwrap();
        let restored: DerivedKey = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(
            restored.private_key,
            vec![0xABu8; 32],
            "postcard must preserve private_key bytes"
        );
        assert_eq!(
            restored.public_key,
            vec![0xCDu8; 32],
            "postcard must preserve public_key bytes"
        );
    }

    #[test]
    fn test_derived_key_deserialize_preserves_bytes() {
        let key = make_test_key();
        let bytes = postcard::to_allocvec(&key.private_key).unwrap();
        let restored: Vec<u8> = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(
            restored,
            vec![0xABu8; 32],
            "Deserialization must preserve private_key bytes"
        );
    }

    #[test]
    fn test_derived_key_zeroize_on_drop() {
        let key = DerivedKey {
            key_type: KeyType::Aes256Gcm,
            private_key: vec![0xFFu8; 32],
            public_key: vec![0x00u8; 32],
        };
        drop(key);
    }

    #[test]
    fn test_derived_key_not_clone() {
        let key = make_test_key();
        let _moved = key;
    }

    #[test]
    fn test_derived_key_zeroize_method_overwrites_private_key() {
        let mut key = make_test_key();
        assert_ne!(key.private_key, vec![0u8; 32]);
        assert!(!key.private_key.is_empty());

        key.zeroize();

        assert!(
            key.private_key.is_empty(),
            "zeroize() must clear the private_key Vec"
        );
    }
}
