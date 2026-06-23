//! Vault key types: `DerivedKey` and `KeyType`.
//!
//! The vault's dispatch is direct method calls on `VaultServiceHandle`
//! (ADR-025). The types defined here — `DerivedKey`, `KeyType` — are the
//! return types from those methods. There is no `VaultProtocol` enum, no
//! `VaultMessage`, no `VaultServiceActor`, and no remote dispatch capability.
//!
//! The vault is **local-only by construction**. If remote vault access is
//! ever needed, it requires a separate crate that wraps the vault and adds
//! remote transport + auth (ADR-025, OQ-021).

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::Zeroize;

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
/// (JSON) for safety, showing `"[REDACTED]"` instead of the key bytes.
/// Deserialization always reads the full bytes.
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
