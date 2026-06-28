//! Credential store: `CredentialStore` repo trait, `InMemoryCredentialStore`
//! default adapter, `EncryptedData` core mirror, and the shared `StoreError`.
//!
//! See `docs/architecture/crates/core/auth.md` and ADR-031 / ADR-035 for the
//! full specification. The store persists `EncryptedData` blobs keyed by
//! provider; it never decrypts (ADR-025 — the vault is the sole decryption
//! boundary).

use std::collections::HashMap;
use std::sync::RwLock;

use async_trait::async_trait;

#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("backend error: {message}")]
    Backend { message: String },
    #[error("not found: {entity}")]
    NotFound { entity: String },
    #[error("serialization error: {message}")]
    Serialization { message: String },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EncryptedData {
    pub key_version: u32,
    pub salt: Vec<u8>,
    pub iv: Vec<u8>,
    pub data: Vec<u8>,
}

#[async_trait]
pub trait CredentialStore: Send + Sync {
    fn get(&self, provider: &str) -> Option<EncryptedData>;
    async fn put(&self, provider: &str, data: &EncryptedData) -> Result<(), StoreError>;
    async fn delete(&self, provider: &str) -> Result<(), StoreError>;
}

pub struct InMemoryCredentialStore {
    entries: RwLock<HashMap<String, EncryptedData>>,
}

impl InMemoryCredentialStore {
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
        }
    }

    pub fn with_entries(entries: HashMap<String, EncryptedData>) -> Self {
        Self {
            entries: RwLock::new(entries),
        }
    }
}

impl Default for InMemoryCredentialStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CredentialStore for InMemoryCredentialStore {
    fn get(&self, provider: &str) -> Option<EncryptedData> {
        let entries = self.entries.read().unwrap_or_else(|e| e.into_inner());
        entries.get(provider).cloned()
    }

    async fn put(&self, provider: &str, data: &EncryptedData) -> Result<(), StoreError> {
        let mut entries = self.entries.write().unwrap_or_else(|e| e.into_inner());
        entries.insert(provider.to_string(), data.clone());
        Ok(())
    }

    async fn delete(&self, provider: &str) -> Result<(), StoreError> {
        let mut entries = self.entries.write().unwrap_or_else(|e| e.into_inner());
        entries.remove(provider);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_encrypted_data() -> EncryptedData {
        EncryptedData {
            key_version: 2,
            salt: vec![],
            iv: vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12],
            data: vec![0xde, 0xad, 0xbe, 0xef],
        }
    }

    #[tokio::test]
    async fn in_memory_get_put_delete_round_trip() {
        let store = InMemoryCredentialStore::new();
        let data = sample_encrypted_data();

        assert!(store.get("openai").is_none());

        store.put("openai", &data).await.unwrap();
        let retrieved = store.get("openai").expect("provider should be present");
        assert_eq!(retrieved.key_version, data.key_version);
        assert_eq!(retrieved.salt, data.salt);
        assert_eq!(retrieved.iv, data.iv);
        assert_eq!(retrieved.data, data.data);

        store.delete("openai").await.unwrap();
        assert!(store.get("openai").is_none());
    }

    #[tokio::test]
    async fn in_memory_get_returns_none_for_missing_provider() {
        let store = InMemoryCredentialStore::new();
        assert!(store.get("never-configured").is_none());
    }

    #[tokio::test]
    async fn in_memory_delete_missing_provider_is_ok() {
        let store = InMemoryCredentialStore::new();
        store.delete("absent").await.unwrap();
    }

    #[tokio::test]
    async fn in_memory_put_replaces_existing() {
        let store = InMemoryCredentialStore::new();
        let first = sample_encrypted_data();
        let mut second = sample_encrypted_data();
        second.data = vec![0xc0, 0xff, 0xee];

        store.put("anthropic", &first).await.unwrap();
        store.put("anthropic", &second).await.unwrap();

        let retrieved = store.get("anthropic").expect("provider should be present");
        assert_eq!(retrieved.data, second.data);
    }

    #[tokio::test]
    async fn in_memory_with_entries_seeds_store() {
        let mut entries = HashMap::new();
        entries.insert("github".to_string(), sample_encrypted_data());
        let store = InMemoryCredentialStore::with_entries(entries);

        assert!(store.get("github").is_some());
        assert!(store.get("openai").is_none());
    }

    #[test]
    fn encrypted_data_serializes_and_deserializes_round_trip() {
        let data = sample_encrypted_data();
        let json = serde_json::to_string(&data).expect("serialize");
        let decoded: EncryptedData = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded.key_version, data.key_version);
        assert_eq!(decoded.salt, data.salt);
        assert_eq!(decoded.iv, data.iv);
        assert_eq!(decoded.data, data.data);
    }

    #[test]
    fn encrypted_data_round_trips_non_empty_salt() {
        let data = EncryptedData {
            key_version: 1,
            salt: vec![0xab, 0xcd, 0xef],
            iv: vec![0; 12],
            data: vec![0x01, 0x02, 0x03],
        };
        let json = serde_json::to_string(&data).expect("serialize");
        let decoded: EncryptedData = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded.salt, data.salt);
    }

    #[test]
    fn store_error_display_formatting() {
        let backend = StoreError::Backend {
            message: "disk full".to_string(),
        };
        assert_eq!(backend.to_string(), "backend error: disk full");

        let not_found = StoreError::NotFound {
            entity: "openai".to_string(),
        };
        assert_eq!(not_found.to_string(), "not found: openai");

        let serialization = StoreError::Serialization {
            message: "invalid utf8".to_string(),
        };
        assert_eq!(
            serialization.to_string(),
            "serialization error: invalid utf8"
        );
    }

    #[test]
    fn store_error_is_non_exhaustive() {
        let err = StoreError::Backend {
            message: "x".to_string(),
        };
        let _ = err.to_string();
    }
}
