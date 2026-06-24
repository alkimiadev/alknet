//! BIP39 mnemonic generation, validation, and seed derivation.
//!
//! This module handles the root of trust: the BIP39 mnemonic seed phrase. From
//! a single mnemonic, all self-generated secrets can be derived on demand.
//!
//! # Security
//!
//! Seed material is protected with `Zeroize` to ensure it is overwritten in
//! memory before deallocation (ADR-038). The seed is never written to disk.

use bip39::Mnemonic as Bip39Mnemonic;
use zeroize::Zeroize;

/// BIP39 word list language.
///
/// Currently only English is supported, matching the BIP39 reference
/// implementation and the vast majority of wallet software.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    English,
}

impl From<Language> for bip39::Language {
    fn from(lang: Language) -> Self {
        match lang {
            Language::English => bip39::Language::English,
        }
    }
}

/// A BIP39 mnemonic seed phrase.
///
/// Wraps the `bip39` crate's `Mnemonic` type and provides seed derivation.
/// The internal phrase is zeroized on drop.
pub struct Mnemonic {
    inner: Bip39Mnemonic,
    phrase: String,
}

impl std::fmt::Debug for Mnemonic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mnemonic")
            .field("phrase", &"[REDACTED]")
            .finish()
    }
}

impl Mnemonic {
    /// Generate a new random mnemonic with the given word count.
    ///
    /// Supported word counts: 12, 15, 18, 21, 24.
    pub fn generate(word_count: usize) -> Result<Self, MnemonicError> {
        let mnemonic: Bip39Mnemonic = Bip39Mnemonic::generate(word_count)
            .map_err(|e: bip39::Error| MnemonicError::Generation(e.to_string()))?;
        Ok(Self::from_bip39(mnemonic))
    }

    /// Create a mnemonic from an existing phrase string.
    ///
    /// Validates the phrase against the BIP39 word list and checksum.
    pub fn from_phrase(phrase: &str, _language: Language) -> Result<Self, MnemonicError> {
        let mnemonic: Bip39Mnemonic = Bip39Mnemonic::parse_normalized(phrase)
            .map_err(|e: bip39::Error| MnemonicError::InvalidPhrase(e.to_string()))?;
        Ok(Self::from_bip39(mnemonic))
    }

    fn from_bip39(mnemonic: Bip39Mnemonic) -> Self {
        let phrase = mnemonic.to_string();
        Self {
            inner: mnemonic,
            phrase,
        }
    }

    /// Derive the master seed from this mnemonic.
    ///
    /// The optional passphrase is used as the BIP39 password for PBKDF2
    /// key derivation (BIP39 standard). An empty string means no passphrase.
    pub fn to_seed(&self, passphrase: Option<&str>) -> Seed {
        let normalized_passphrase = passphrase.unwrap_or("");
        let seed_bytes = self.inner.to_seed_normalized(normalized_passphrase);
        Seed {
            bytes: seed_bytes.to_vec(),
        }
    }

    /// Returns the mnemonic phrase as a string.
    ///
    /// Handle with care — this is the root of trust for all derived keys.
    pub fn phrase(&self) -> &str {
        &self.phrase
    }
}

impl Zeroize for Mnemonic {
    fn zeroize(&mut self) {
        self.phrase.zeroize();
        self.inner.zeroize();
    }
}

impl Drop for Mnemonic {
    fn drop(&mut self) {
        self.zeroize();
    }
}

/// A BIP39-derived master seed.
///
/// Contains the 64-byte seed material from which all HD keys are derived.
/// Zeroized on drop per ADR-038.
#[derive(Clone, Zeroize)]
#[zeroize(drop)]
pub struct Seed {
    bytes: Vec<u8>,
}

impl Seed {
    /// Returns the seed bytes.
    ///
    /// These bytes are the input to SLIP-0010 master key derivation.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Returns the length of the seed (always 64 bytes for BIP39).
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Returns whether the seed is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

/// Errors that can occur during mnemonic operations.
#[derive(Debug, thiserror::Error)]
pub enum MnemonicError {
    #[error("failed to generate mnemonic: {0}")]
    Generation(String),
    #[error("invalid mnemonic phrase: {0}")]
    InvalidPhrase(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_mnemonic_24_words() {
        let mnemonic = Mnemonic::generate(24).unwrap();
        let words: Vec<&str> = mnemonic.phrase().split_whitespace().collect();
        assert_eq!(words.len(), 24);
    }

    #[test]
    fn test_mnemonic_round_trip() {
        let original = Mnemonic::generate(12).unwrap();
        let phrase = original.phrase().to_string();
        let restored = Mnemonic::from_phrase(&phrase, Language::English).unwrap();
        assert_eq!(original.phrase(), restored.phrase());
    }

    #[test]
    fn test_seed_derivation() {
        let mnemonic = Mnemonic::generate(24).unwrap();
        let seed = mnemonic.to_seed(None);
        assert_eq!(seed.len(), 64);
        assert!(!seed.is_empty());
    }

    #[test]
    fn test_mnemonic_debug_redacts_phrase() {
        let mnemonic = Mnemonic::generate(24).unwrap();
        let debug_output = format!("{:?}", mnemonic);
        assert!(
            debug_output.contains("[REDACTED]"),
            "Debug must show [REDACTED] for phrase, got: {debug_output}"
        );
        for word in mnemonic.phrase().split_whitespace() {
            assert!(
                !debug_output.contains(word),
                "Debug must not leak phrase word '{word}', got: {debug_output}"
            );
        }
    }
}
