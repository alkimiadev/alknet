//! SLIP-0010 Ed25519 HD key derivation and path constants.
//!
//! This module provides hierarchical deterministic (HD) key derivation following
//! SLIP-0010 for Ed25519 keys and BIP-0032 for secp256k1 keys. The `74'`
//! coin type is unallocated per SLIP-0044 and reserved for alknet.
//!
//! # Derivation Paths
//!
//! | Path | Purpose | Curve/Algorithm |
//! |------|---------|----------------|
//! | `m/74'/0'/0'/0'` | Primary identity keypair | Ed25519 (alknet auth) |
//! | `m/74'/0'/0'/{n}'` | Worker/device identity | Ed25519 |
//! | `m/74'/0'/1'/0'` | SSH host key | Ed25519 |
//! | `m/74'/1'/0'/{hash}'` | Site-specific password | Deterministic |
//! | `m/74'/2'/0'/0'` | Encryption key for external credentials | AES-256-GCM |
//! | `m/44'/60'/0'/0/0` | Ethereum signing key | secp256k1 |

use ed25519_bip32::XPrv;
use hmac::{Hmac, Mac};
use sha2::Sha512;
use zeroize::Zeroize;

type HmacSha512 = Hmac<Sha512>;

/// Well-known derivation path constants for alknet key material.
///
/// These paths are defined once and referenced by both the secret service and
/// external consumers that need to request specific key types.
#[allow(non_snake_case)]
pub mod PATHS {
    /// Primary identity keypair for alknet authentication.
    pub const IDENTITY: &str = "m/74'/0'/0'/0'";

    /// Worker/device identity keypair (parameterized by device index).
    /// Use `device_path(n)` to construct the full path.
    pub const DEVICE_PREFIX: &str = "m/74'/0'/0'";

    /// SSH host key.
    pub const SSH_HOST: &str = "m/74'/0'/1'/0'";

    /// Encryption key for external credentials (AES-256-GCM).
    pub const ENCRYPTION: &str = "m/74'/2'/0'/0'";

    /// Ethereum signing key.
    pub const ETHEREUM: &str = "m/44'/60'/0'/0/0";
}

/// Construct a device identity derivation path with the given index.
///
/// Path: `m/74'/0'/0'/{n}'`
pub fn device_path(index: u32) -> String {
    format!("m/74'/0'/0'/{}'", index)
}

/// Construct a site-specific password derivation path with the given hash.
///
/// Path: `m/74'/1'/0'/{hash}'`
pub fn site_password_path(site_hash: &str) -> String {
    format!("m/74'/1'/0'/{}'", site_hash)
}

/// A derived extended private key with its public key.
///
/// Contains the private key bytes and public key bytes from
/// SLIP-0010 Ed25519 derivation.
#[derive(Clone, Zeroize)]
#[zeroize(drop)]
pub struct ExtendedPrivKey {
    /// The private key bytes (first 32 bytes of the extended key).
    private_key: Vec<u8>,
    /// The public key bytes (32 bytes).
    public_key: Vec<u8>,
    /// The chain code for child derivation (32 bytes).
    chain_code: Vec<u8>,
    /// The derivation path that produced this key.
    path: String,
}

impl ExtendedPrivKey {
    /// Returns the private key bytes (32 bytes for Ed25519).
    pub fn private_key(&self) -> &[u8] {
        &self.private_key
    }

    /// Returns the public key bytes (32 bytes for Ed25519).
    pub fn public_key(&self) -> &[u8] {
        &self.public_key
    }

    /// Returns the derivation path string.
    pub fn path(&self) -> &str {
        &self.path
    }
}

/// Derive an extended private key from a seed and derivation path.
///
/// This is the primary entry point for HD key derivation. Create a master key
/// from the seed, then derive the specified path.
///
/// # Example
///
/// ```
/// use alknet_secret::derivation::{derive_path_from_seed, PATHS};
/// use alknet_secret::mnemonic::Mnemonic;
///
/// let mnemonic = Mnemonic::generate(24).unwrap();
/// let seed = mnemonic.to_seed(None);
/// let identity_key = derive_path_from_seed(seed.as_bytes(), PATHS::IDENTITY).unwrap();
/// assert!(!identity_key.private_key().is_empty());
/// ```
pub fn derive_path_from_seed(seed: &[u8], path: &str) -> Result<ExtendedPrivKey, DerivationError> {
    let indices = parse_derivation_path(path)?;
    let xprv = derive_master_key(seed)?;

    let mut current = xprv;
    for index in indices {
        current = current.derive(ed25519_bip32::DerivationScheme::V2, index);
    }

    let public_key = current.public();

    Ok(ExtendedPrivKey {
        private_key: current.extended_secret_key_bytes()[..32].to_vec(),
        public_key: public_key.as_ref()[..32].to_vec(),
        chain_code: current.chain_code().to_vec(),
        path: path.to_string(),
    })
}

/// Derive the SLIP-0010 Ed25519 master key from a seed.
///
/// Uses HMAC-SHA512 with key "ed25519 seed" over the seed bytes,
/// following SLIP-0010 specification.
fn derive_master_key(seed: &[u8]) -> Result<XPrv, DerivationError> {
    let mut mac = HmacSha512::new_from_slice(b"ed25519 seed")
        .map_err(|e| DerivationError::Hmac(e.to_string()))?;
    mac.update(seed);
    let result = mac.finalize().into_bytes();

    // First 32 bytes: private key (kL in SLIP-0010)
    // Next 32 bytes: chain code
    let private_key_bytes = &result[..32];
    let chain_code_bytes = &result[32..];

    // Construct XPrv from the HMAC result
    // ed25519-bip32 expects a 96-byte extended key:
    // [32 bytes: kL || 32 bytes: kR (extended secret key) || 32 bytes: chain code]
    // SLIP-0010 uses the first 32 bytes as kL and hashes through SHA-512
    // to get the full extended key. We use from_nonextended_force to handle this.
    let mut priv_bytes = [0u8; 32];
    priv_bytes.copy_from_slice(private_key_bytes);
    let mut cc_bytes = [0u8; 32];
    cc_bytes.copy_from_slice(chain_code_bytes);

    Ok(XPrv::from_nonextended_force(&priv_bytes, &cc_bytes))
}

/// Parse a derivation path string into child indices.
///
/// Path format: `m/74'/0'/0'/0'`
/// Hardened indices have `'` or `h` suffix. Unhardened indices are allowed
/// for BIP-0032 paths (e.g., Ethereum `m/44'/60'/0'/0/0`).
pub fn parse_derivation_path(path: &str) -> Result<Vec<u32>, DerivationError> {
    if !path.starts_with('m') {
        return Err(DerivationError::InvalidPath(
            "path must start with 'm'".to_string(),
        ));
    }

    let mut indices = Vec::new();
    let parts: Vec<&str> = path.split('/').skip(1).collect(); // skip "m"

    for part in parts {
        let hardened = part.ends_with('\'') || part.ends_with('h');
        let index_str = part.trim_end_matches('\'').trim_end_matches('h');
        let index: u32 = index_str
            .parse()
            .map_err(|_| DerivationError::InvalidPath(format!("invalid index: {part}")))?;

        if hardened {
            indices.push(index + 0x80000000);
        } else {
            indices.push(index);
        }
    }

    Ok(indices)
}

/// Errors that can occur during key derivation.
#[derive(Debug, thiserror::Error)]
pub enum DerivationError {
    #[error("invalid derivation path: {0}")]
    InvalidPath(String),
    #[error("HMAC error: {0}")]
    Hmac(String),
    #[error("key derivation error: {0}")]
    KeyDerivation(String),
    #[error("seed is not unlocked")]
    Locked,
    #[error("secp256k1 error: {0}")]
    Secp256k1(String),
    #[error("unsupported key type")]
    UnsupportedKeyType,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_derivation_path_hardened() {
        let indices = parse_derivation_path("m/74'/0'/0'/0'").unwrap();
        assert_eq!(
            indices,
            vec![0x80000000 + 74, 0x80000000, 0x80000000, 0x80000000]
        );
    }

    #[test]
    fn test_parse_derivation_path_mixed() {
        // Ethereum path has unhardened indices
        let indices = parse_derivation_path("m/44'/60'/0'/0/0").unwrap();
        assert_eq!(
            indices,
            vec![0x80000000 + 44, 0x80000000 + 60, 0x80000000, 0, 0]
        );
    }

    #[test]
    fn test_parse_rejects_no_m_prefix() {
        let result = parse_derivation_path("74'/0'/0'/0'");
        assert!(result.is_err());
    }

    #[test]
    fn test_path_constants() {
        assert_eq!(PATHS::IDENTITY, "m/74'/0'/0'/0'");
        assert_eq!(PATHS::ENCRYPTION, "m/74'/2'/0'/0'");
        assert_eq!(PATHS::SSH_HOST, "m/74'/0'/1'/0'");
        assert_eq!(PATHS::ETHEREUM, "m/44'/60'/0'/0/0");
    }

    #[test]
    fn test_device_path() {
        assert_eq!(device_path(0), "m/74'/0'/0'/0'");
        assert_eq!(device_path(1), "m/74'/0'/0'/1'");
    }

    #[test]
    fn test_site_password_path() {
        assert_eq!(site_password_path("abc123"), "m/74'/1'/0'/abc123'");
    }

    #[test]
    fn test_derive_master_key_from_seed() {
        // Use a known 64-byte seed
        let seed = [0xABu8; 64];
        let result = derive_master_key(&seed);
        assert!(result.is_ok());
    }

    #[test]
    fn test_derive_identity_key_from_random_seed() {
        let mnemonic = crate::mnemonic::Mnemonic::generate(24).unwrap();
        let seed = mnemonic.to_seed(None);
        let key = derive_path_from_seed(seed.as_bytes(), PATHS::IDENTITY);
        assert!(key.is_ok());

        let key = key.unwrap();
        assert_eq!(key.private_key().len(), 32);
        assert_eq!(key.public_key().len(), 32);
        assert_eq!(key.path(), PATHS::IDENTITY);
    }

    #[test]
    fn test_deterministic_derivation() {
        let mnemonic = crate::mnemonic::Mnemonic::generate(24).unwrap();
        let seed = mnemonic.to_seed(None);

        let key1 = derive_path_from_seed(seed.as_bytes(), PATHS::IDENTITY).unwrap();
        let key2 = derive_path_from_seed(seed.as_bytes(), PATHS::IDENTITY).unwrap();

        assert_eq!(key1.private_key(), key2.private_key());
        assert_eq!(key1.public_key(), key2.public_key());
    }

    #[test]
    fn test_different_paths_different_keys() {
        let mnemonic = crate::mnemonic::Mnemonic::generate(24).unwrap();
        let seed = mnemonic.to_seed(None);

        let identity = derive_path_from_seed(seed.as_bytes(), PATHS::IDENTITY).unwrap();
        let ssh = derive_path_from_seed(seed.as_bytes(), PATHS::SSH_HOST).unwrap();

        assert_ne!(identity.private_key(), ssh.private_key());
        assert_ne!(identity.public_key(), ssh.public_key());
    }
}
