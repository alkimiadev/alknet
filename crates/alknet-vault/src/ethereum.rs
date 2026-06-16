//! BIP-0032 secp256k1 HD key derivation for Ethereum keys.
//!
//! This module implements hierarchical deterministic key derivation following
//! BIP-0032 for secp256k1 curves. It is gated behind the `secp256k1` feature flag.
//!
//! Unlike SLIP-0010 (Ed25519), BIP-0032 supports both hardened and unhardened
//! child derivation and uses HMAC-SHA512 with the key "Bitcoin seed" (not
//! "ed25519 seed").
//!
//! # Ethereum Path
//!
//! The standard Ethereum derivation path is `m/44'/60'/0'/0/0` (EIP-84).
//! The last two indices (`0/0`) are unhardened, which SLIP-0010 cannot handle.

use hmac::{Hmac, Mac};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use sha2::Sha512;
use zeroize::Zeroize;

use crate::derivation::{parse_derivation_path, DerivationError};

type HmacSha512 = Hmac<Sha512>;

const HARDENED_OFFSET: u32 = 0x80000000;

/// An extended private key for BIP-0032 secp256k1 derivation.
///
/// Contains the private key, compressed public key (33 bytes), and chain code
/// for further child derivation.
#[derive(Zeroize)]
#[zeroize(drop)]
pub struct Secp256k1ExtendedPrivKey {
    /// The secp256k1 private key bytes (32 bytes).
    #[zeroize]
    private_key: Vec<u8>,
    /// The compressed public key bytes (33 bytes).
    public_key: Vec<u8>,
    /// The chain code for child derivation (32 bytes).
    chain_code: Vec<u8>,
}

impl Secp256k1ExtendedPrivKey {
    /// Returns the private key bytes (32 bytes).
    pub fn private_key(&self) -> &[u8] {
        &self.private_key
    }

    /// Returns the compressed public key bytes (33 bytes).
    pub fn public_key(&self) -> &[u8] {
        &self.public_key
    }

    /// Returns the chain code bytes (32 bytes).
    pub fn chain_code(&self) -> &[u8] {
        &self.chain_code
    }
}

/// Derive the BIP-0032 secp256k1 master key from a seed.
///
/// Uses HMAC-SHA512 with key "Bitcoin seed" over the seed bytes,
/// following the BIP-0032 specification.
pub fn derive_secp256k1_master_key(
    seed: &[u8],
) -> Result<Secp256k1ExtendedPrivKey, DerivationError> {
    let mut mac = HmacSha512::new_from_slice(b"Bitcoin seed")
        .map_err(|e| DerivationError::Hmac(e.to_string()))?;
    mac.update(seed);
    let result = mac.finalize().into_bytes();

    let private_key_bytes = &result[..32];
    let chain_code_bytes = &result[32..];

    let secp = Secp256k1::new();
    let secret_key = SecretKey::from_slice(private_key_bytes)
        .map_err(|e| DerivationError::Secp256k1(e.to_string()))?;
    let public_key = PublicKey::from_secret_key(&secp, &secret_key);

    Ok(Secp256k1ExtendedPrivKey {
        private_key: secret_key.secret_bytes().to_vec(),
        public_key: public_key.serialize().to_vec(),
        chain_code: chain_code_bytes.to_vec(),
    })
}

/// Derive a child extended private key from a parent key at the given index.
///
/// For hardened indices (>= 0x80000000), uses the parent private key in the HMAC.
/// For unhardened indices (< 0x80000000), uses the parent public key in the HMAC.
fn derive_child(
    parent: &Secp256k1ExtendedPrivKey,
    index: u32,
) -> Result<Secp256k1ExtendedPrivKey, DerivationError> {
    let secp = Secp256k1::new();

    let mut mac = HmacSha512::new_from_slice(parent.chain_code())
        .map_err(|e| DerivationError::Hmac(e.to_string()))?;

    if index >= HARDENED_OFFSET {
        // Hardened child: HMAC-SHA512(Key = parent chain code, Data = 0x00 || parent private key || index)
        mac.update(&[0x00]);
        mac.update(parent.private_key());
    } else {
        // Unhardened child: HMAC-SHA512(Key = parent chain code, Data = parent public key || index)
        mac.update(parent.public_key());
    }
    mac.update(&index.to_be_bytes());

    let result = mac.finalize().into_bytes();
    let child_key_bytes = &result[..32];
    let child_chain_code = &result[32..];

    // Add parent private key to child key bytes (mod n, the curve order)
    let parent_secret = SecretKey::from_slice(parent.private_key())
        .map_err(|e| DerivationError::Secp256k1(e.to_string()))?;
    let child_key_raw = SecretKey::from_slice(child_key_bytes)
        .map_err(|e| DerivationError::Secp256k1(e.to_string()))?;

    // Tweak: child_key = (parent_key + tweak) mod n
    let child_secret = parent_secret
        .add_tweak(&child_key_raw.into())
        .map_err(|e| DerivationError::Secp256k1(e.to_string()))?;

    let child_public = PublicKey::from_secret_key(&secp, &child_secret);

    Ok(Secp256k1ExtendedPrivKey {
        private_key: child_secret.secret_bytes().to_vec(),
        public_key: child_public.serialize().to_vec(),
        chain_code: child_chain_code.to_vec(),
    })
}

/// Derive a secp256k1 extended private key from a seed and derivation path.
///
/// This is the primary entry point for BIP-0032 secp256k1 derivation.
/// Supports both hardened and unhardened indices.
///
/// # Example
///
/// ```ignore
/// use alknet_vault::ethereum::derive_secp256k1_path;
/// use alknet_vault::derivation::PATHS;
///
/// let key = derive_secp256k1_path(seed, PATHS::ETHEREUM).unwrap();
/// assert_eq!(key.private_key().len(), 32);
/// assert_eq!(key.public_key().len(), 33); // compressed
/// ```
pub fn derive_secp256k1_path(
    seed: &[u8],
    path: &str,
) -> Result<Secp256k1ExtendedPrivKey, DerivationError> {
    let indices = parse_derivation_path(path)?;
    let master = derive_secp256k1_master_key(seed)?;

    let mut current = master;
    for index in indices {
        current = derive_child(&current, index)?;
    }

    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PATHS;

    #[test]
    fn test_bip32_master_key_vector() {
        // BIP-0032 test vector 1: seed "000102030405060708090a0b0c0d0e0f"
        let seed = hex::decode("000102030405060708090a0b0c0d0e0f").unwrap();
        let master = derive_secp256k1_master_key(&seed).unwrap();

        // Expected master private key from BIP-0032 test vector 1
        let expected_priv =
            hex::decode("e8f32e723decf4051aefac8e2c93c9c5b214313817cdb01a1494b917c8436b35")
                .unwrap();
        assert_eq!(master.private_key(), expected_priv.as_slice());

        // Expected master public key (compressed) from BIP-0032 test vector 1
        let expected_pub =
            hex::decode("0339a36013301597daef41fbe593a02cc513d0b55527ec2df1050e2e8ff49c85c2")
                .unwrap();
        assert_eq!(master.public_key(), expected_pub.as_slice());

        // Expected chain code from BIP-0032 test vector 1
        let expected_cc =
            hex::decode("873dff81c02f525623fd1fe5167eac3a55a049de3d314bb42ee227ffed37d508")
                .unwrap();
        assert_eq!(master.chain_code(), expected_cc.as_slice());
    }

    #[test]
    fn test_bip32_derive_m_44h_60h_0h_0_0() {
        let seed = hex::decode("000102030405060708090a0b0c0d0e0f").unwrap();
        let key = derive_secp256k1_path(&seed, "m/44'/60'/0'/0/0").unwrap();
        assert_eq!(key.private_key().len(), 32);
        assert_eq!(key.public_key().len(), 33);
    }

    #[test]
    fn test_ethereum_keypair_is_valid() {
        let mnemonic = crate::mnemonic::Mnemonic::generate(24).unwrap();
        let seed = mnemonic.to_seed(None);
        let key = derive_secp256k1_path(seed.as_bytes(), PATHS::ETHEREUM).unwrap();

        let secp = Secp256k1::new();
        let secret_key = SecretKey::from_slice(key.private_key()).unwrap();
        let public_key = PublicKey::from_secret_key(&secp, &secret_key);
        assert_eq!(key.public_key(), public_key.serialize().as_slice());
    }

    #[test]
    fn test_ethereum_differs_from_ed25519() {
        let mnemonic = crate::mnemonic::Mnemonic::generate(24).unwrap();
        let seed = mnemonic.to_seed(None);

        let eth_key = derive_secp256k1_path(seed.as_bytes(), PATHS::ETHEREUM).unwrap();
        let ed_key =
            crate::derivation::derive_path_from_seed(seed.as_bytes(), PATHS::ETHEREUM).unwrap();

        assert_ne!(eth_key.private_key(), ed_key.private_key());
    }

    #[test]
    fn test_deterministic_derivation() {
        let mnemonic = crate::mnemonic::Mnemonic::generate(24).unwrap();
        let seed = mnemonic.to_seed(None);

        let key1 = derive_secp256k1_path(seed.as_bytes(), PATHS::ETHEREUM).unwrap();
        let key2 = derive_secp256k1_path(seed.as_bytes(), PATHS::ETHEREUM).unwrap();

        assert_eq!(key1.private_key(), key2.private_key());
        assert_eq!(key1.public_key(), key2.public_key());
    }

    #[test]
    fn test_compressed_public_key_is_33_bytes() {
        let mnemonic = crate::mnemonic::Mnemonic::generate(24).unwrap();
        let seed = mnemonic.to_seed(None);
        let key = derive_secp256k1_path(seed.as_bytes(), PATHS::ETHEREUM).unwrap();
        assert_eq!(key.public_key().len(), 33);
        // Compressed public key starts with 0x02 or 0x03
        assert!(key.public_key()[0] == 0x02 || key.public_key()[0] == 0x03);
    }
}
