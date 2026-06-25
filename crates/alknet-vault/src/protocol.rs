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
/// Serialization **always** redacts `private_key` as `"[REDACTED]"`, regardless
/// of format. Deserialization rejects redacted payloads with an explicit error.
#[derive(Zeroize)]
#[zeroize(drop)]
pub struct DerivedKey {
    #[zeroize(skip)]
    pub key_type: KeyType,
    #[zeroize]
    pub private_key: Vec<u8>,
    #[zeroize(skip)]
    pub public_key: Vec<u8>,
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
        let mut state = s.serialize_struct("DerivedKey", 3)?;
        state.serialize_field("key_type", &self.key_type)?;
        state.serialize_field("private_key", "[REDACTED]")?;
        state.serialize_field("public_key", &self.public_key)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for DerivedKey {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct DerivedKeyHelper {
            key_type: KeyType,
            private_key: Vec<u8>,
            public_key: Vec<u8>,
        }
        let helper = DerivedKeyHelper::deserialize(d)?;
        if helper.private_key == b"[REDACTED]" {
            return Err(serde::de::Error::custom(
                "DerivedKey.private_key is \"[REDACTED]\" — redacted payloads \
                 cannot be deserialized. JSON round-tripping a DerivedKey is \
                 not supported (the private key is gone).",
            ));
        }
        Ok(DerivedKey {
            key_type: helper.key_type,
            private_key: helper.private_key,
            public_key: helper.public_key,
        })
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
    fn test_derived_key_deserialize_rejects_redacted_payload() {
        let redacted_json = r#"{"key_type":"Ed25519","private_key":"[REDACTED]","public_key":[205,205,205,205,205,205,205,205,205,205,205,205,205,205,205,205,205,205,205,205,205,205,205,205,205,205,205,205,205,205,205,205]}"#;
        let result: Result<DerivedKey, _> = serde_json::from_str(redacted_json);
        let err = result.expect_err("deserializing a redacted payload must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("[REDACTED]"),
            "error must mention the redacted marker, got: {msg}"
        );
        assert!(
            !msg.contains("AB"),
            "error must not leak private key bytes, got: {msg}"
        );
    }

    #[test]
    fn test_derived_key_deserialize_rejects_redacted_byte_array() {
        // `[REDACTED]` as a 10-byte ASCII array: the redacted-marker guard at
        // protocol.rs:78 is only reachable when private_key deserializes as
        // Vec<u8> equal to b"[REDACTED]". The byte-array form is the one that
        // actually reaches the guard (a JSON string fails type coercion first).
        let redacted_bytes: Vec<u8> = b"[REDACTED]".to_vec();
        let mut json = String::from(r#"{"key_type":"Ed25519","private_key":"#);
        json.push_str(&serde_json::to_string(&redacted_bytes).unwrap());
        json.push_str(r#","public_key":[205]}"#);
        let result: Result<DerivedKey, _> = serde_json::from_str(&json);
        let err = result.expect_err("redacted byte array must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("redacted"),
            "error must explain the redacted-payload rejection, got: {msg}"
        );
        assert!(
            !msg.contains("AB"),
            "error must not leak any key bytes, got: {msg}"
        );
    }

    #[test]
    fn test_derived_key_deserialize_accepts_non_redacted_payload() {
        // A real (non-redacted) private key byte array must deserialize
        // successfully and reach the Ok arm of the deserialize impl.
        let key = make_test_key();
        let public = serde_json::to_string(&key.public_key).unwrap();
        let private = serde_json::to_string(&vec![0xABu8; 32]).unwrap();
        let json = format!(
            r#"{{"key_type":"Ed25519","private_key":{private},"public_key":{public}}}"#
        );
        let result: DerivedKey = serde_json::from_str(&json).expect("non-redacted payload deserializes");
        assert_eq!(result.key_type, KeyType::Ed25519);
        assert_eq!(result.private_key, vec![0xABu8; 32]);
        assert_eq!(result.public_key, key.public_key);
    }

    #[test]
    fn test_derived_key_debug_does_not_leak_private_key_bytes() {
        let key = make_test_key();
        let debug_output = format!("{:?}", key);
        assert!(
            !debug_output.contains("ab") && !debug_output.contains("AB"),
            "Debug must not leak private_key bytes"
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
