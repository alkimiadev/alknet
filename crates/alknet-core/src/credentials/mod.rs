use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};

use crate::config::DynamicConfig;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum CredentialSet {
    ApiKey {
        header_name: String,
        token: String,
    },
    Basic {
        username: String,
        password: String,
    },
    Bearer {
        token: String,
    },
    S3AccessKey {
        access_key: String,
        secret_key: String,
        session_token: Option<String>,
    },
    OidcToken {
        access_token: String,
        refresh_token: Option<String>,
        expires_at: Option<u64>,
    },
    Custom {
        scheme: String,
        params: HashMap<String, String>,
    },
}

pub trait CredentialProvider: Send + Sync + 'static {
    fn get_credentials(&self, service: &str) -> Option<CredentialSet>;
    fn refresh_credentials(&self, service: &str) -> Option<CredentialSet>;
}

pub struct ConfigCredentialProvider {
    dynamic: Arc<ArcSwap<DynamicConfig>>,
}

impl ConfigCredentialProvider {
    pub fn new(dynamic: Arc<ArcSwap<DynamicConfig>>) -> Self {
        Self { dynamic }
    }
}

impl CredentialProvider for ConfigCredentialProvider {
    fn get_credentials(&self, service: &str) -> Option<CredentialSet> {
        let config = self.dynamic.load();
        config.credentials.get(service).cloned()
    }

    fn refresh_credentials(&self, service: &str) -> Option<CredentialSet> {
        self.get_credentials(service)
    }
}

pub struct SecretStoreCredentialProvider;

impl SecretStoreCredentialProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SecretStoreCredentialProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl CredentialProvider for SecretStoreCredentialProvider {
    fn get_credentials(&self, _service: &str) -> Option<CredentialSet> {
        None
    }

    fn refresh_credentials(&self, _service: &str) -> Option<CredentialSet> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AuthPolicy;

    fn make_dynamic_with_credentials() -> Arc<ArcSwap<DynamicConfig>> {
        let mut credentials = HashMap::new();
        credentials.insert(
            "vast-ai".to_string(),
            CredentialSet::Bearer {
                token: "secret-token".to_string(),
            },
        );
        credentials.insert(
            "custom-service".to_string(),
            CredentialSet::ApiKey {
                header_name: "X-API-Key".to_string(),
                token: "api-key-123".to_string(),
            },
        );
        let config = DynamicConfig::new(AuthPolicy::empty()).with_credentials(credentials);
        Arc::new(ArcSwap::new(Arc::new(config)))
    }

    fn make_dynamic_empty() -> Arc<ArcSwap<DynamicConfig>> {
        let config = DynamicConfig::default();
        Arc::new(ArcSwap::new(Arc::new(config)))
    }

    #[test]
    fn config_credential_provider_returns_configured_credentials() {
        let dynamic = make_dynamic_with_credentials();
        let provider = ConfigCredentialProvider::new(dynamic);
        let creds = provider.get_credentials("vast-ai");
        assert!(creds.is_some());
        match creds.unwrap() {
            CredentialSet::Bearer { token } => assert_eq!(token, "secret-token"),
            _ => panic!("expected Bearer variant"),
        }
    }

    #[test]
    fn config_credential_provider_returns_api_key_variant() {
        let dynamic = make_dynamic_with_credentials();
        let provider = ConfigCredentialProvider::new(dynamic);
        let creds = provider.get_credentials("custom-service");
        assert!(creds.is_some());
        match creds.unwrap() {
            CredentialSet::ApiKey { header_name, token } => {
                assert_eq!(header_name, "X-API-Key");
                assert_eq!(token, "api-key-123");
            }
            _ => panic!("expected ApiKey variant"),
        }
    }

    #[test]
    fn config_credential_provider_returns_none_for_unknown_service() {
        let dynamic = make_dynamic_with_credentials();
        let provider = ConfigCredentialProvider::new(dynamic);
        let creds = provider.get_credentials("nonexistent");
        assert!(creds.is_none());
    }

    #[test]
    fn config_credential_provider_empty_config_returns_none() {
        let dynamic = make_dynamic_empty();
        let provider = ConfigCredentialProvider::new(dynamic);
        let creds = provider.get_credentials("vast-ai");
        assert!(creds.is_none());
    }

    #[test]
    fn secret_store_credential_provider_returns_none() {
        let provider = SecretStoreCredentialProvider::new();
        assert!(provider.get_credentials("vast-ai").is_none());
        assert!(provider.get_credentials("rustfs").is_none());
        assert!(provider.get_credentials("gitea").is_none());
    }

    #[test]
    fn secret_store_credential_provider_refresh_returns_none() {
        let provider = SecretStoreCredentialProvider::new();
        assert!(provider.refresh_credentials("vast-ai").is_none());
    }

    #[test]
    fn credential_set_bearer_serialization() {
        let creds = CredentialSet::Bearer {
            token: "tok".to_string(),
        };
        let json = serde_json::to_string(&creds).unwrap();
        let deserialized: CredentialSet = serde_json::from_str(&json).unwrap();
        assert_eq!(creds, deserialized);
    }

    #[test]
    fn credential_set_s3_access_key_serialization() {
        let creds = CredentialSet::S3AccessKey {
            access_key: "AKIA123".to_string(),
            secret_key: "secret".to_string(),
            session_token: Some("session".to_string()),
        };
        let json = serde_json::to_string(&creds).unwrap();
        let deserialized: CredentialSet = serde_json::from_str(&json).unwrap();
        assert_eq!(creds, deserialized);
    }

    #[test]
    fn credential_set_oidc_token_serialization() {
        let creds = CredentialSet::OidcToken {
            access_token: "access".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: Some(1234567890),
        };
        let json = serde_json::to_string(&creds).unwrap();
        let deserialized: CredentialSet = serde_json::from_str(&json).unwrap();
        assert_eq!(creds, deserialized);
    }

    #[test]
    fn credential_set_custom_serialization() {
        let mut params = HashMap::new();
        params.insert("key1".to_string(), "val1".to_string());
        let creds = CredentialSet::Custom {
            scheme: "X-Custom".to_string(),
            params,
        };
        let json = serde_json::to_string(&creds).unwrap();
        let deserialized: CredentialSet = serde_json::from_str(&json).unwrap();
        assert_eq!(creds, deserialized);
    }

    #[test]
    fn credential_set_basic_serialization() {
        let creds = CredentialSet::Basic {
            username: "user".to_string(),
            password: "pass".to_string(),
        };
        let json = serde_json::to_string(&creds).unwrap();
        let deserialized: CredentialSet = serde_json::from_str(&json).unwrap();
        assert_eq!(creds, deserialized);
    }

    #[test]
    fn credential_set_clone() {
        let creds = CredentialSet::Bearer {
            token: "tok".to_string(),
        };
        let cloned = creds.clone();
        assert_eq!(creds, cloned);
    }
}
