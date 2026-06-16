//! # alknet-vault
//!
//! Local key vault: BIP39 mnemonic generation, SLIP-0010 Ed25519 HD key derivation,
//! AES-256-GCM encryption for securing provider keys, credentials, and identity material.
//!
//! This crate is the only component that holds the master seed phrase. The CLI binary
//! unlocks the vault at startup and injects derived/decrypted material into operation
//! contexts. Other crates never access the vault directly — they receive keys through
//! their operation context or via the call protocol.
//!
//! ## Crate Independence
//!
//! alknet-vault does **not** depend on alknet-core or any other alknet crate. It is
//! fully independent and usable in contexts where QUIC networking doesn't exist (CLI
//! tools, test harnesses, WASM key derivation).
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
//! - [`protocol`] — `VaultProtocol` irpc message enum, `DerivedKey`, `KeyType`
//! - [`service`] — `VaultService` implementation with Unlock/Lock lifecycle
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
pub use protocol::{DerivedKey, KeyType, VaultMessage, VaultProtocol};
pub use service::{VaultService, VaultServiceActor, VaultServiceError, VaultServiceHandle};
