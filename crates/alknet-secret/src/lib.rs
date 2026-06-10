//! # alknet-secret
//!
//! BIP39 mnemonic generation, SLIP-0010 Ed25519 HD key derivation, AES-256-GCM
//! encryption for external credentials, and the `SecretProtocol` irpc service.
//!
//! This crate is the only component that holds the master seed phrase. All other
//! crates request derived keys through the `SecretProtocol` irpc service or the
//! `SecretServiceHandle` local API.
//!
//! ## Crate Independence
//!
//! alknet-secret does **not** depend on alknet-core or alknet-storage. Per ADR-027,
//! it is fully independent. The `EncryptedData` wire format is shared with
//! alknet-storage by type-level compatibility, not a crate dependency.
//!
//! ## Security Model
//!
//! The seed phrase is never persisted to disk. It is entered at startup or via
//! `Unlock` and held only in `Zeroize`-protected RAM (ADR-038). `Lock` purges
//! the seed and all cached derived keys.
//!
//! ## Module Organization
//!
//! - [`mnemonic`] — BIP39 mnemonic generation, validation, and seed derivation
//! - [`derivation`] — SLIP-0010 Ed25519 HD key derivation and path constants
//! - [`encryption`] — AES-256-GCM encrypt/decrypt and `EncryptedData` type
//! - [`protocol`] — `SecretProtocol` irpc service enum, `DerivedKey`, `KeyType`
//! - [`service`] — `SecretService` implementation with Unlock/Lock lifecycle
//! - [`ethereum`] — BIP-0032 secp256k1 HD key derivation (behind `secp256k1` feature)

pub mod cache;
pub mod derivation;
pub mod encryption;
pub mod mnemonic;
pub mod protocol;
pub mod service;

#[cfg(feature = "secp256k1")]
pub mod ethereum;

// Re-export primary public API
pub use cache::CacheConfig;
pub use derivation::{DerivationError, ExtendedPrivKey, PATHS};
pub use encryption::{EncryptedData, EncryptionError};
pub use mnemonic::{Language, Mnemonic, Seed};
pub use protocol::{DerivedKey, KeyType, SecretMessage, SecretProtocol};
pub use service::{SecretService, SecretServiceError, SecretServiceHandle};

#[cfg(feature = "secp256k1")]
pub use ethereum::Secp256k1ExtendedPrivKey;
