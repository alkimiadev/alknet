//! Configuration: `DynamicConfig`, `AuthPolicy`, `ApiKeyEntry`,
//! `RateLimitConfig`, `ConfigReloadHandle`.
//!
//! See `docs/architecture/crates/core/config.md` for the full specification.
//!
//! This module provides the dynamic-config types required by
//! `auth::ConfigIdentityProvider`. The remaining types (`StaticConfig`,
//! `TlsIdentity`, `ConfigError`) are filled in by the core/config task.

use std::collections::HashSet;
use std::sync::Arc;

use arc_swap::ArcSwap;

use crate::auth::Identity;

pub const API_KEY_PREFIX: &str = "alk_";

#[derive(Debug, Clone)]
pub struct ApiKeyEntry {
    pub prefix: String,
    pub hash: String,
    pub scopes: Vec<String>,
    pub description: String,
    pub expires_at: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct AuthPolicy {
    pub authorized_fingerprints: HashSet<String>,
    pub api_keys: Vec<ApiKeyEntry>,
}

impl AuthPolicy {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn resolve_identity_from_fingerprint(&self, fingerprint: &str) -> Option<Identity> {
        if self.authorized_fingerprints.contains(fingerprint) {
            Some(Identity {
                id: fingerprint.to_string(),
                scopes: vec!["relay:connect".to_string()],
                resources: std::collections::HashMap::new(),
            })
        } else {
            None
        }
    }

    pub fn resolve_api_key(&self, token: &str) -> Option<Identity> {
        if !token.starts_with(API_KEY_PREFIX) {
            return None;
        }

        let prefix_part = &token[..token.len().min(8)];

        let entry = self
            .api_keys
            .iter()
            .find(|e| prefix_part.starts_with(&e.prefix))?;

        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        let result = hasher.finalize();
        let expected_hash = format!("sha256:{}", hex::encode(result));

        if entry.hash != expected_hash {
            return None;
        }

        if let Some(expires_at) = entry.expires_at {
            let now_secs = std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if now_secs >= expires_at {
                return None;
            }
        }

        Some(Identity {
            id: entry.prefix.clone(),
            scopes: entry.scopes.clone(),
            resources: std::collections::HashMap::new(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    pub max_connections_per_ip: usize,
    pub max_auth_attempts: usize,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            max_connections_per_ip: 100,
            max_auth_attempts: 5,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct DynamicConfig {
    pub auth: AuthPolicy,
    pub rate_limits: RateLimitConfig,
}

pub struct ConfigReloadHandle {
    dynamic: Arc<ArcSwap<DynamicConfig>>,
}

impl ConfigReloadHandle {
    pub fn new(dynamic: Arc<ArcSwap<DynamicConfig>>) -> Self {
        Self { dynamic }
    }

    pub fn reload(&self, new_config: DynamicConfig) {
        self.dynamic.store(Arc::new(new_config));
    }

    pub fn dynamic(&self) -> Arc<DynamicConfig> {
        self.dynamic.load_full()
    }
}
