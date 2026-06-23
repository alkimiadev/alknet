//! Configuration: `DynamicConfig`, `AuthPolicy`, `ApiKeyEntry`,
//! `RateLimitConfig`, `ConfigReloadHandle`.
//!
//! See `docs/architecture/crates/core/config.md` for the full specification.
//!
//! This module provides the dynamic-config types required by
//! `auth::ConfigIdentityProvider`. The remaining types (`StaticConfig`,
//! `TlsIdentity`, `ConfigError`) are filled in by the core/config task.

use std::collections::HashSet;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;

use crate::auth::Identity;

pub const API_KEY_PREFIX: &str = "alk_";

#[derive(Debug, Clone)]
pub struct StaticConfig {
    pub listen_addr: Option<SocketAddr>,
    pub tls_identity: Option<TlsIdentity>,
    #[cfg(feature = "iroh")]
    pub iroh_relay: Option<iroh::RelayUrl>,
    pub drain_timeout: Duration,
}

#[derive(Debug, Clone)]
pub enum TlsIdentity {
    X509 {
        cert: PathBuf,
        key: PathBuf,
    },
    #[cfg(feature = "iroh")]
    RawKey(iroh::SecretKey),
    SelfSigned,
}

#[derive(Debug, Clone, Default)]
pub struct DynamicConfig {
    pub auth: AuthPolicy,
    pub rate_limits: RateLimitConfig,
}

#[derive(Debug, Clone, Default)]
pub struct AuthPolicy {
    pub authorized_fingerprints: HashSet<String>,
    pub api_keys: Vec<ApiKeyEntry>,
}

#[derive(Debug, Clone)]
pub struct ApiKeyEntry {
    pub prefix: String,
    pub hash: String,
    pub scopes: Vec<String>,
    pub description: String,
    pub expires_at: Option<u64>,
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

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("invalid flag: {name}")]
    InvalidFlag { name: String },
    #[error("key file not found: {path}")]
    KeyFileNotFound { path: String },
    #[error("bind failed: {0}")]
    BindFailed(#[from] io::Error),
    #[error("tls config error: {0}")]
    TlsConfig(io::Error),
    #[error("incompatible options")]
    IncompatibleOptions,
}

impl Default for StaticConfig {
    fn default() -> Self {
        Self {
            listen_addr: None,
            tls_identity: None,
            #[cfg(feature = "iroh")]
            iroh_relay: None,
            drain_timeout: Duration::from_secs(2),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_config_default() {
        let cfg = StaticConfig::default();
        assert!(cfg.listen_addr.is_none());
        assert!(cfg.tls_identity.is_none());
        assert_eq!(cfg.drain_timeout, Duration::from_secs(2));
    }

    #[test]
    fn dynamic_config_default() {
        let cfg = DynamicConfig::default();
        assert!(cfg.auth.authorized_fingerprints.is_empty());
        assert!(cfg.auth.api_keys.is_empty());
        assert_eq!(cfg.rate_limits.max_connections_per_ip, 100);
        assert_eq!(cfg.rate_limits.max_auth_attempts, 5);
    }

    #[test]
    fn auth_policy_default() {
        let policy = AuthPolicy::default();
        assert!(policy.authorized_fingerprints.is_empty());
        assert!(policy.api_keys.is_empty());
    }

    #[test]
    fn rate_limit_config_default() {
        let rl = RateLimitConfig::default();
        assert!(rl.max_connections_per_ip > 0);
        assert!(rl.max_auth_attempts > 0);
    }

    #[test]
    fn api_key_entry_construct() {
        let entry = ApiKeyEntry {
            prefix: "alk12345".to_string(),
            hash: "deadbeef".to_string(),
            scopes: vec!["admin".to_string()],
            description: "test key".to_string(),
            expires_at: Some(1_700_000_000),
        };
        assert_eq!(entry.prefix, "alk12345");
        assert_eq!(entry.scopes, vec!["admin"]);
        assert_eq!(entry.expires_at, Some(1_700_000_000));
    }

    #[test]
    fn tls_identity_x509_construct() {
        let id = TlsIdentity::X509 {
            cert: PathBuf::from("/etc/cert.pem"),
            key: PathBuf::from("/etc/key.pem"),
        };
        match id {
            TlsIdentity::X509 { cert, key } => {
                assert_eq!(cert, PathBuf::from("/etc/cert.pem"));
                assert_eq!(key, PathBuf::from("/etc/key.pem"));
            }
            _ => panic!("expected X509"),
        }
    }

    #[test]
    fn tls_identity_self_signed() {
        let id = TlsIdentity::SelfSigned;
        let s = format!("{id:?}");
        assert!(s.contains("SelfSigned"));
    }

    #[test]
    fn config_reload_handle_swaps_atomically() {
        let dynamic = Arc::new(ArcSwap::from_pointee(DynamicConfig::default()));
        let handle = ConfigReloadHandle::new(dynamic.clone());

        let initial = handle.dynamic();
        assert!(initial.auth.authorized_fingerprints.is_empty());

        let mut new_auth = AuthPolicy::default();
        new_auth
            .authorized_fingerprints
            .insert("aa:bb:cc".to_string());
        let new_config = DynamicConfig {
            auth: new_auth,
            rate_limits: RateLimitConfig::default(),
        };
        handle.reload(new_config);

        let after = handle.dynamic();
        assert!(after.auth.authorized_fingerprints.contains("aa:bb:cc"));
        assert!(initial.auth.authorized_fingerprints.is_empty());
    }

    #[test]
    fn config_reload_handle_dynamic_returns_current() {
        let dynamic = Arc::new(ArcSwap::from_pointee(DynamicConfig::default()));
        let handle = ConfigReloadHandle::new(dynamic);
        let a = handle.dynamic();
        let b = handle.dynamic();
        assert_eq!(
            a.rate_limits.max_auth_attempts,
            b.rate_limits.max_auth_attempts
        );
    }

    #[test]
    fn config_error_invalid_flag_display() {
        let e = ConfigError::InvalidFlag {
            name: "foo".to_string(),
        };
        assert_eq!(format!("{e}"), "invalid flag: foo");
    }

    #[test]
    fn config_error_key_file_not_found_display() {
        let e = ConfigError::KeyFileNotFound {
            path: "/x".to_string(),
        };
        assert_eq!(format!("{e}"), "key file not found: /x");
    }

    #[test]
    fn config_error_incompatible_options_display() {
        let e = ConfigError::IncompatibleOptions;
        assert_eq!(format!("{e}"), "incompatible options");
    }

    #[test]
    fn config_error_bind_failed_from_io() {
        let io_err = io::Error::new(io::ErrorKind::AddrInUse, "busy");
        let e: ConfigError = io_err.into();
        assert!(matches!(e, ConfigError::BindFailed(_)));
    }

    #[test]
    fn config_error_tls_config_display() {
        let e = ConfigError::TlsConfig(io::Error::new(io::ErrorKind::InvalidData, "bad"));
        let s = format!("{e}");
        assert!(s.starts_with("tls config error:"));
    }
}
