//! SecretProtocol irpc service definition and associated types.
//!
//! This module defines the `SecretProtocol` enum for irpc-based inter-service
//! communication. The protocol supports unlock/lock lifecycle, key derivation,
//! and encryption/decryption operations.
//!
//! # Protocol Operation
//!
//! The SecretProtocol follows a lifecycle: the service starts in a **locked**
//! state where no derivation or encryption operations are possible. The `Unlock`
//! call loads the seed into memory (derived from the mnemonic passphrase). After
//! that, derive and encrypt/decrypt operations are available. The `Lock` call
//! purges the seed and all cached keys.
//!
//! # Wire Format
//!
//! For local (in-process) calls, the protocol uses tokio channels directly.
//! For remote (in-cluster) calls, the protocol is serialized with postcard.
//! For cross-node (call protocol) exposure, the service is wrapped in an
//! operation that serializes to JSON.

use serde::{Deserialize, Serialize};

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
/// The private key is sensitive material. Consumers should zeroize
/// it when no longer needed. The `SecretServiceHandle` manages the lifecycle
/// of derived keys internally.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DerivedKey {
    /// The type of key that was derived.
    pub key_type: KeyType,
    /// The private key bytes.
    pub private_key: Vec<u8>,
    /// The public key bytes.
    pub public_key: Vec<u8>,
}

/// SecretProtocol service definition.
///
/// This is the irpc protocol enum that defines all secret service operations.
/// The `#[rpc_requests]` macro generates two versions:
/// - **Serializable** (`SecretMessage::Request`): for remote communication (postcard)
/// - **With channels** (`SecretMessage::RequestWithChannels`): for local communication (tokio)
///
/// # State Requirements
///
/// All operations except `Unlock` require the service to be in an **unlocked**
/// state. Calling derive/encrypt/decrypt on a locked service returns an error.
#[derive(Debug, Serialize, Deserialize)]
pub enum SecretProtocol {
    /// Derive an Ed25519 keypair at the given path.
    ///
    /// Path format: `m/74'/0'/0'/0'` (SLIP-0010 hardened-only notation).
    /// Returns a `DerivedKey` with `KeyType::Ed25519`.
    DeriveEd25519 {
        /// SLIP-0010 derivation path (e.g., "m/74'/0'/0'/0'").
        path: String,
    },

    /// Derive an AES-256-GCM encryption key at the given path.
    ///
    /// The default encryption path is `m/74'/2'/0'/0'`.
    /// Returns a `DerivedKey` with `KeyType::Aes256Gcm`.
    DeriveEncryptionKey {
        /// SLIP-0010 derivation path for the encryption key.
        path: String,
    },

    /// Derive a secp256k1 (Ethereum) keypair at the given path.
    ///
    /// The default Ethereum path is `m/44'/60'/0'/0/0`.
    /// Returns a `DerivedKey` with `KeyType::Secp256k1`.
    DeriveEthereumKey {
        /// BIP-0032 derivation path (e.g., "m/44'/60'/0'/0/0").
        path: String,
    },

    /// Derive a deterministic password at the given path.
    ///
    /// Path format: `m/74'/1'/0'/{hash}'` (SLIP-0010 hardened notation).
    /// The `length` parameter controls the output length.
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
    Encrypt {
        /// The plaintext string to encrypt.
        plaintext: String,
        /// The key version for rotation tracking.
        key_version: u32,
    },

    /// Decrypt an `EncryptedData` blob back to plaintext.
    ///
    /// The key is derived from the seed at the path indicated by the key version.
    Decrypt {
        /// The encrypted data blob to decrypt.
        encrypted: EncryptedData,
    },

    /// Lock the service, purging the seed and all cached derived keys.
    ///
    /// After locking, no derive/encrypt/decrypt operations are possible
    /// until `Unlock` is called again. Calls `zeroize()` on all sensitive
    /// material (ADR-038).
    Lock,

    /// Unlock the service with a BIP39 passphrase.
    ///
    /// The passphrase is used to derive the master seed from the mnemonic.
    /// After unlocking, derive and encrypt/decrypt operations are available.
    Unlock {
        /// The BIP39 passphrase (may be empty for no passphrase).
        passphrase: String,
    },
}

/// Message type for SecretProtocol irpc communication.
///
/// TODO: Replace with irpc `#[rpc_requests]` macro-generated type once
/// the irpc crate is integrated. For now, this is a placeholder type alias.
pub type SecretMessage = SecretProtocol;