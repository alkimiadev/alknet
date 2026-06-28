//! Configuration: `DynamicConfig`, `AuthPolicy`, `ApiKeyEntry`,
//! `RateLimitConfig`, `ConfigReloadHandle`.
//!
//! See `docs/architecture/crates/core/config.md` for the full specification.
//!
//! This module provides the dynamic-config types required by
//! `auth::ConfigIdentityProvider`. The remaining types (`StaticConfig`,
//! `TlsIdentity`, `ConfigError`) are filled in by the core/config task.

use std::collections::HashMap;
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

#[derive(Clone)]
pub struct Ed25519SecretKey(ed25519_dalek::SigningKey);

impl Ed25519SecretKey {
    pub fn generate() -> Self {
        let mut csprng = rand::rngs::OsRng;
        Self(ed25519_dalek::SigningKey::generate(&mut csprng))
    }

    pub fn from_bytes(bytes: &[u8; 32]) -> Self {
        Self(ed25519_dalek::SigningKey::from_bytes(bytes))
    }

    pub fn as_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }

    pub fn public(&self) -> ed25519_dalek::VerifyingKey {
        self.0.verifying_key()
    }

    pub fn sign(&self, message: &[u8]) -> ed25519_dalek::Signature {
        use ed25519_dalek::Signer;
        self.0.sign(message)
    }
}

impl std::fmt::Debug for Ed25519SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ed25519SecretKey").finish_non_exhaustive()
    }
}

impl zeroize::ZeroizeOnDrop for Ed25519SecretKey {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcmeDirectory {
    Production,
    Staging,
    Custom(String),
}

impl AcmeDirectory {
    pub fn url(&self) -> &str {
        match self {
            AcmeDirectory::Production => "https://acme-v02.api.letsencrypt.org/directory",
            AcmeDirectory::Staging => "https://acme-staging-v02.api.letsencrypt.org/directory",
            AcmeDirectory::Custom(url) => url,
        }
    }
}

#[derive(Debug, Clone)]
pub enum TlsIdentity {
    X509 {
        cert: PathBuf,
        key: PathBuf,
    },
    RawKey(Ed25519SecretKey),
    SelfSigned,
    Acme {
        domains: Vec<String>,
        cache_dir: PathBuf,
        directory: AcmeDirectory,
        contact: Vec<String>,
    },
}

#[derive(Debug, Clone, Default)]
pub struct DynamicConfig {
    pub auth: AuthPolicy,
    pub rate_limits: RateLimitConfig,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PeerEntry {
    pub peer_id: String,
    pub fingerprints: Vec<String>,
    pub auth_token_hash: Option<String>,
    pub scopes: Vec<String>,
    pub resources: HashMap<String, Vec<String>>,
    pub display_name: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Default)]
pub struct AuthPolicy {
    pub peers: Vec<PeerEntry>,
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
        self.peers
            .iter()
            .find(|p| p.enabled && p.fingerprints.iter().any(|f| f == fingerprint))
            .map(|p| Identity {
                id: p.peer_id.clone(),
                scopes: p.scopes.clone(),
                resources: p.resources.clone(),
            })
    }

    pub fn resolve_identity_from_token(&self, token: &str) -> Option<Identity> {
        let token_hash = sha256_hex(token);
        self.peers
            .iter()
            .find(|p| p.enabled && p.auth_token_hash.as_deref() == Some(&token_hash))
            .map(|p| Identity {
                id: p.peer_id.clone(),
                scopes: p.scopes.clone(),
                resources: p.resources.clone(),
            })
            .or_else(|| self.resolve_api_key(token))
    }

    pub fn validate_peer_ids(&self) -> Result<(), DuplicatePeerId> {
        let mut seen = std::collections::HashSet::new();
        for peer in &self.peers {
            if !seen.insert(peer.peer_id.as_str()) {
                return Err(DuplicatePeerId {
                    peer_id: peer.peer_id.clone(),
                });
            }
        }
        Ok(())
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

        let expected_hash = sha256_hex(token);

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

fn sha256_hex(input: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let result = hasher.finalize();
    format!("sha256:{}", hex::encode(result))
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("duplicate peer_id: {peer_id}")]
pub struct DuplicatePeerId {
    pub peer_id: String,
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
        assert!(cfg.auth.peers.is_empty());
        assert!(cfg.auth.api_keys.is_empty());
        assert_eq!(cfg.rate_limits.max_connections_per_ip, 100);
        assert_eq!(cfg.rate_limits.max_auth_attempts, 5);
    }

    #[test]
    fn auth_policy_default() {
        let policy = AuthPolicy::default();
        assert!(policy.peers.is_empty());
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
        assert!(initial.auth.peers.is_empty());

        let new_auth = AuthPolicy {
            peers: vec![PeerEntry {
                peer_id: "worker-a".to_string(),
                fingerprints: vec!["aa:bb:cc".to_string()],
                auth_token_hash: None,
                scopes: vec!["relay:connect".to_string()],
                resources: HashMap::new(),
                display_name: None,
                enabled: true,
            }],
            api_keys: Vec::new(),
        };
        let new_config = DynamicConfig {
            auth: new_auth,
            rate_limits: RateLimitConfig::default(),
        };
        handle.reload(new_config);

        let after = handle.dynamic();
        assert_eq!(after.auth.peers.len(), 1);
        assert_eq!(after.auth.peers[0].peer_id, "worker-a");
        assert!(initial.auth.peers.is_empty());
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

    #[test]
    fn resolve_api_key_returns_empty_resources() {
        let token = "alk_test_secret";
        let hash = sha256_hex(token);

        let entry = ApiKeyEntry {
            prefix: "alk_tes".to_string(),
            hash,
            scopes: vec!["admin".to_string()],
            description: "test key".to_string(),
            expires_at: None,
        };
        let policy = AuthPolicy {
            peers: Vec::new(),
            api_keys: vec![entry],
        };

        let identity = policy.resolve_api_key(token);
        assert!(
            identity.is_some(),
            "api key with matching prefix and hash should resolve"
        );
        let identity = identity.unwrap();
        assert_eq!(identity.id, "alk_tes");
        assert_eq!(identity.scopes, vec!["admin"]);
        assert!(
            identity.resources.is_empty(),
            "token-resolved identities must have empty resources (Option B — scopes only)"
        );
    }

    #[test]
    fn resolve_identity_from_fingerprint_uses_peer_id() {
        let policy = AuthPolicy {
            peers: vec![PeerEntry {
                peer_id: "worker-a".to_string(),
                fingerprints: vec!["SHA256:known".to_string()],
                auth_token_hash: None,
                scopes: vec!["relay:connect".to_string()],
                resources: HashMap::new(),
                display_name: None,
                enabled: true,
            }],
            api_keys: vec![],
        };

        let identity = policy
            .resolve_identity_from_fingerprint("SHA256:known")
            .expect("known fingerprint should resolve");
        assert_eq!(identity.id, "worker-a");
        assert_eq!(identity.scopes, vec!["relay:connect"]);
    }

    // --- PeerEntry model (ADR-030) ---------------------------------------

    fn peer_entry(peer_id: &str, fingerprints: &[&str]) -> PeerEntry {
        PeerEntry {
            peer_id: peer_id.to_string(),
            fingerprints: fingerprints.iter().map(|s| s.to_string()).collect(),
            auth_token_hash: None,
            scopes: vec!["relay:connect".to_string()],
            resources: HashMap::new(),
            display_name: None,
            enabled: true,
        }
    }

    #[test]
    fn fingerprint_resolution_known_returns_some_with_peer_id() {
        let policy = AuthPolicy {
            peers: vec![peer_entry("worker-a", &["ed25519:abc"])],
            api_keys: vec![],
        };
        let identity = policy
            .resolve_identity_from_fingerprint("ed25519:abc")
            .expect("known fingerprint resolves");
        assert_eq!(identity.id, "worker-a");
        assert_eq!(identity.scopes, vec!["relay:connect"]);
    }

    #[test]
    fn fingerprint_resolution_unknown_returns_none() {
        let policy = AuthPolicy {
            peers: vec![peer_entry("worker-a", &["ed25519:abc"])],
            api_keys: vec![],
        };
        assert!(policy
            .resolve_identity_from_fingerprint("ed25519:unknown")
            .is_none());
    }

    #[test]
    fn fingerprint_resolution_disabled_returns_none() {
        let mut entry = peer_entry("worker-a", &["ed25519:abc"]);
        entry.enabled = false;
        let policy = AuthPolicy {
            peers: vec![entry],
            api_keys: vec![],
        };
        assert!(policy
            .resolve_identity_from_fingerprint("ed25519:abc")
            .is_none());
    }

    #[test]
    fn token_resolution_matching_peer_returns_some_with_peer_id() {
        let token = "bearer-secret";
        let mut entry = peer_entry("worker-a", &["ed25519:abc"]);
        entry.auth_token_hash = Some(sha256_hex(token));
        let policy = AuthPolicy {
            peers: vec![entry],
            api_keys: vec![],
        };
        let identity = policy
            .resolve_identity_from_token(token)
            .expect("matching auth_token_hash resolves");
        assert_eq!(identity.id, "worker-a");
    }

    #[test]
    fn token_resolution_non_matching_falls_through_to_api_key() {
        let api_token = "alk_test_secret";
        let mut entry = peer_entry("worker-a", &["ed25519:abc"]);
        entry.auth_token_hash = Some(sha256_hex("different-token"));
        let api_entry = ApiKeyEntry {
            prefix: "alk_tes".to_string(),
            hash: sha256_hex(api_token),
            scopes: vec!["admin".to_string()],
            description: "test key".to_string(),
            expires_at: None,
        };
        let policy = AuthPolicy {
            peers: vec![entry],
            api_keys: vec![api_entry],
        };
        let identity = policy
            .resolve_identity_from_token(api_token)
            .expect("api key fall-through resolves");
        assert_eq!(identity.id, "alk_tes");
        assert_eq!(identity.scopes, vec!["admin"]);
    }

    #[test]
    fn token_resolution_no_match_returns_none() {
        let policy = AuthPolicy {
            peers: vec![peer_entry("worker-a", &["ed25519:abc"])],
            api_keys: vec![],
        };
        assert!(policy.resolve_identity_from_token("unknown").is_none());
    }

    #[test]
    fn multi_fingerprint_peer_any_resolves_to_same_peer_id() {
        let policy = AuthPolicy {
            peers: vec![peer_entry("worker-a", &["ed25519:abc", "SHA256:def"])],
            api_keys: vec![],
        };
        let id1 = policy
            .resolve_identity_from_fingerprint("ed25519:abc")
            .expect("first fingerprint resolves");
        let id2 = policy
            .resolve_identity_from_fingerprint("SHA256:def")
            .expect("second fingerprint resolves");
        assert_eq!(id1.id, "worker-a");
        assert_eq!(id2.id, "worker-a");
    }

    #[test]
    fn resources_populated_on_fingerprint_path() {
        let mut resources = HashMap::new();
        resources.insert("service".to_string(), vec!["gitea".to_string()]);
        let mut entry = peer_entry("worker-a", &["ed25519:abc"]);
        entry.resources = resources.clone();
        let policy = AuthPolicy {
            peers: vec![entry],
            api_keys: vec![],
        };
        let identity = policy
            .resolve_identity_from_fingerprint("ed25519:abc")
            .expect("known fingerprint resolves");
        assert_eq!(identity.resources, resources);
    }

    #[test]
    fn resources_populated_on_token_path() {
        let token = "bearer-secret";
        let mut resources = HashMap::new();
        resources.insert("service".to_string(), vec!["gitea".to_string()]);
        let mut entry = peer_entry("worker-a", &["ed25519:abc"]);
        entry.auth_token_hash = Some(sha256_hex(token));
        entry.resources = resources.clone();
        let policy = AuthPolicy {
            peers: vec![entry],
            api_keys: vec![],
        };
        let identity = policy
            .resolve_identity_from_token(token)
            .expect("matching token resolves");
        assert_eq!(identity.resources, resources);
    }

    #[test]
    fn duplicate_peer_id_validation_rejects() {
        let policy = AuthPolicy {
            peers: vec![
                peer_entry("worker-a", &["ed25519:abc"]),
                peer_entry("worker-a", &["ed25519:def"]),
            ],
            api_keys: vec![],
        };
        let err = policy.validate_peer_ids().expect_err("duplicate detected");
        assert_eq!(err.peer_id, "worker-a");
    }

    #[test]
    fn unique_peer_ids_validate_ok() {
        let policy = AuthPolicy {
            peers: vec![
                peer_entry("worker-a", &["ed25519:abc"]),
                peer_entry("worker-b", &["ed25519:def"]),
            ],
            api_keys: vec![],
        };
        assert!(policy.validate_peer_ids().is_ok());
    }

    // --- Ed25519SecretKey -------------------------------------------------

    #[test]
    fn ed25519_secret_key_round_trips_bytes() {
        let key = Ed25519SecretKey::generate();
        let bytes = key.as_bytes();
        let restored = Ed25519SecretKey::from_bytes(&bytes);
        assert_eq!(restored.as_bytes(), bytes);
    }

    #[test]
    fn ed25519_secret_key_sign_verifies_against_public_key() {
        use ed25519_dalek::{Signature, Verifier};
        let key = Ed25519SecretKey::generate();
        let public = key.public();
        let message = b"alknet coverage check";
        let signature: Signature = key.sign(message);
        assert_eq!(signature.to_bytes().len(), 64);
        assert!(
            public.verify(message, &signature).is_ok(),
            "signature produced by Ed25519SecretKey::sign must verify under its public key"
        );
    }

    #[test]
    fn ed25519_secret_key_sign_rejects_tampered_message() {
        use ed25519_dalek::{Signature, Verifier};
        let key = Ed25519SecretKey::generate();
        let public = key.public();
        let signature: Signature = key.sign(b"original message");
        assert!(
            public.verify(b"tampered message", &signature).is_err(),
            "signature must not verify against a different message"
        );
    }

    #[test]
    fn ed25519_secret_key_debug_does_not_leak_material() {
        let key = Ed25519SecretKey::generate();
        let dbg = format!("{key:?}");
        assert!(dbg.contains("Ed25519SecretKey"));
        assert!(!dbg.contains("SigningKey"));
        let raw = hex::encode(key.as_bytes());
        assert!(
            !dbg.contains(&raw),
            "Debug output must not contain the raw key bytes"
        );
    }

    #[test]
    fn ed25519_secret_key_public_matches_underlying_signing_key() {
        let key = Ed25519SecretKey::generate();
        let public = key.public();
        assert_eq!(public.to_bytes().len(), 32);
    }
}
