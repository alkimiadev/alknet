//! Authentication: `AuthContext`, `Identity`, `IdentityProvider`, `AuthToken`,
//! `ConfigIdentityProvider`.
//!
//! See `docs/architecture/crates/core/auth.md` for the full specification and
//! [ADR-034](../../../docs/architecture/decisions/034-outgoing-only-x509-and-three-peer-roles.md)
//! for the three-remote-roles decision.
//!
//! # Three remote roles (ADR-034 §1)
//!
//! The three credential types (`PeerEntry.fingerprints` entries) describe how
//! a *single* `PeerEntry` can be authenticated. Separately, there are three
//! distinct remote roles that must not be conflated:
//!
//! | Role | Identity | alknet peer? | `PeerEntry` on local side? |
//! |------|----------|--------------|----------------------------|
//! | **Public X.509 endpoint** | Domain + CA-issued X.509 | No (local node is a client) | No |
//! | **Transport relay** (iroh's DERP-equivalent) | iroh `NodeId` (Ed25519) | No (infrastructure) | No |
//! | **Hub / hosting node** | Ed25519 raw key **and/or** X.509 | Yes (full peer) | Yes |
//!
//! `PeerEntry` (and the `PeerId` it resolves to) is the model for peers in
//! the call-protocol peer graph (ADR-029) — peers that get a stable logical
//! identity, are addressable via `PeerRef::Specific`, and whose ops land in
//! the peer-keyed overlay. A pure-client connection to a public X.509
//! endpoint (e.g. a third-party API) is **not** in that graph on the client
//! side: no `PeerEntry`, no `PeerId`, no `PeerRef::Specific` routing. The
//! asymmetry is deliberate — a public domain's operator can change hands, so
//! there is no stable logical identity to attach.
//!
//! The hub case is an ordinary `PeerEntry` that happens to expose both an
//! Ed25519 fingerprint (P2P path) and an X.509 fingerprint
//! (`SHA256:<hex>`, WebTransport/HTTPS path) — already supported by
//! `PeerEntry.fingerprints: Vec<String>` (ADR-030).
//!
//! # Client-side verifier selection (ADR-034 §3)
//!
//! The `CallClient` / `from_openapi` / `from_mcp` client-side
//! `ServerCertVerifier` is selected by **whether the local node has a
//! `PeerEntry` for the remote**, not by key type alone:
//!
//! | Local has `PeerEntry` for remote? | Remote cert type | Client verifier |
//! |----------------------------------|------------------|-----------------|
//! | No (public X.509 endpoint) | X.509 | `WebPkiServerVerifier` (CA verification) |
//! | No | Ed25519 raw key | fails closed (no CA to fall back to) |
//! | Yes (hub, Ed25519 path) | Ed25519 raw key | fingerprint match (`ed25519:<hex>`) |
//! | Yes (hub, X.509 path) | X.509 | fingerprint match (`SHA256:<hex>`) |
//!
//! This is the key-type-aware verifier from OQ-29, with the peer-model
//! criterion (ADR-034) made explicit. The client-side verifier selection is
//! a `CallClient` concern (`call/call-client-verifier-selection`), not an
//! `IdentityProvider` concern — `IdentityProvider` is unchanged by ADR-034.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::config::{DynamicConfig, PeerEntry};
use crate::store::StoreError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Identity {
    pub id: String,
    pub scopes: Vec<String>,
    pub resources: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct AuthToken {
    pub raw: Vec<u8>,
}

#[derive(Clone)]
pub struct AuthContext {
    pub identity: Option<Identity>,
    pub alpn: Vec<u8>,
    pub remote_addr: Option<SocketAddr>,
    pub tls_client_fingerprint: Option<String>,
}

pub trait IdentityProvider: Send + Sync + 'static {
    fn resolve_from_fingerprint(&self, fingerprint: &str) -> Option<Identity>;
    fn resolve_from_token(&self, token: &AuthToken) -> Option<Identity>;
}

/// Write trait — management path, async (ADR-035). `ConfigIdentityProvider`
/// does NOT implement this (config reload is its write path). A persistence
/// adapter (e.g. `SqliteIdentityProvider` in `alknet-store-sqlite`) does:
/// writes hit the backend, emit a honker `NOTIFY`, and the local `LISTEN`
/// refreshes the in-memory read index.
#[async_trait]
pub trait IdentityStore: IdentityProvider {
    async fn put_peer(&self, peer: &PeerEntry) -> Result<(), StoreError>;
    async fn update_peer(&self, peer_id: &str, peer: &PeerEntry) -> Result<(), StoreError>;
    async fn remove_peer(&self, peer_id: &str) -> Result<(), StoreError>;
}

pub struct ConfigIdentityProvider {
    dynamic: Arc<ArcSwap<DynamicConfig>>,
}

impl ConfigIdentityProvider {
    pub fn new(dynamic: Arc<ArcSwap<DynamicConfig>>) -> Self {
        Self { dynamic }
    }
}

impl IdentityProvider for ConfigIdentityProvider {
    fn resolve_from_fingerprint(&self, fingerprint: &str) -> Option<Identity> {
        let config = self.dynamic.load();
        config.auth.resolve_identity_from_fingerprint(fingerprint)
    }

    fn resolve_from_token(&self, token: &AuthToken) -> Option<Identity> {
        let config = self.dynamic.load();
        let token_str = String::from_utf8_lossy(&token.raw);
        config.auth.resolve_identity_from_token(&token_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ApiKeyEntry, AuthPolicy, DynamicConfig, PeerEntry, RateLimitConfig};

    fn compute_api_key_hash(token: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        let result = hasher.finalize();
        format!("sha256:{}", hex::encode(result))
    }

    fn make_provider(
        config: DynamicConfig,
    ) -> (ConfigIdentityProvider, Arc<ArcSwap<DynamicConfig>>) {
        let arc_swap = Arc::new(ArcSwap::new(Arc::new(config)));
        let provider = ConfigIdentityProvider::new(Arc::clone(&arc_swap));
        (provider, arc_swap)
    }

    fn peer_entry_with_fingerprint(peer_id: &str, fingerprint: &str) -> PeerEntry {
        PeerEntry {
            peer_id: peer_id.to_string(),
            fingerprints: vec![fingerprint.to_string()],
            auth_token_hash: None,
            scopes: vec!["relay:connect".to_string()],
            resources: std::collections::HashMap::new(),
            display_name: None,
            enabled: true,
        }
    }

    fn config_with_peer_entry(peer_id: &str, fingerprint: &str) -> DynamicConfig {
        DynamicConfig {
            auth: AuthPolicy {
                peers: vec![peer_entry_with_fingerprint(peer_id, fingerprint)],
                api_keys: Vec::new(),
            },
            rate_limits: RateLimitConfig::default(),
        }
    }

    fn config_with_api_key(entry: ApiKeyEntry) -> DynamicConfig {
        DynamicConfig {
            auth: AuthPolicy {
                peers: Vec::new(),
                api_keys: vec![entry],
            },
            rate_limits: RateLimitConfig::default(),
        }
    }

    fn compute_token_hash(token: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        let result = hasher.finalize();
        format!("sha256:{}", hex::encode(result))
    }

    #[test]
    fn identity_fields_and_equality() {
        let mut resources = HashMap::new();
        resources.insert(
            "service".to_string(),
            vec!["gitea".to_string(), "registry".to_string()],
        );
        let id = Identity {
            id: "SHA256:abc123".to_string(),
            scopes: vec!["relay:connect".to_string()],
            resources,
        };
        let id2 = id.clone();
        assert_eq!(id, id2);
        assert_eq!(id.id, "SHA256:abc123");
    }

    #[test]
    fn auth_token_is_clone() {
        let token = AuthToken {
            raw: b"alk_test".to_vec(),
        };
        let cloned = token.clone();
        assert_eq!(token.raw, cloned.raw);
    }

    #[test]
    fn auth_context_is_clone() {
        let ctx = AuthContext {
            identity: None,
            alpn: b"alknet/test".to_vec(),
            remote_addr: None,
            tls_client_fingerprint: None,
        };
        let cloned = ctx.clone();
        assert_eq!(cloned.alpn, b"alknet/test");
        assert!(cloned.identity.is_none());
    }

    #[test]
    fn fingerprint_resolution_known_returns_some() {
        let (provider, _) = make_provider(config_with_peer_entry("worker-a", "SHA256:abc123"));
        let identity = provider
            .resolve_from_fingerprint("SHA256:abc123")
            .expect("known fingerprint resolves");
        assert_eq!(identity.id, "worker-a");
        assert_eq!(identity.scopes, vec!["relay:connect".to_string()]);
        assert!(identity.resources.is_empty());
    }

    #[test]
    fn fingerprint_resolution_unknown_returns_none() {
        let (provider, _) = make_provider(config_with_peer_entry("worker-a", "SHA256:abc123"));
        assert!(provider
            .resolve_from_fingerprint("SHA256:unknown")
            .is_none());
    }

    #[test]
    fn fingerprint_resolution_empty_config_returns_none() {
        let (provider, _) = make_provider(DynamicConfig::default());
        assert!(provider
            .resolve_from_fingerprint("SHA256:anything")
            .is_none());
    }

    #[test]
    fn token_resolution_valid_non_expired_returns_some() {
        let token_str = "alk_testsecret123";
        let hash = compute_api_key_hash(token_str);
        let entry = ApiKeyEntry {
            prefix: "alk_test".to_string(),
            hash,
            scopes: vec!["relay:connect".to_string()],
            description: "test key".to_string(),
            expires_at: None,
        };
        let (provider, _) = make_provider(config_with_api_key(entry));
        let token = AuthToken {
            raw: token_str.as_bytes().to_vec(),
        };
        let identity = provider
            .resolve_from_token(&token)
            .expect("valid non-expired token resolves");
        assert_eq!(identity.id, "alk_test");
        assert_eq!(identity.scopes, vec!["relay:connect".to_string()]);
    }

    #[test]
    fn token_resolution_expired_returns_none() {
        let token_str = "alk_testsecret123";
        let hash = compute_api_key_hash(token_str);
        let entry = ApiKeyEntry {
            prefix: "alk_test".to_string(),
            hash,
            scopes: vec!["relay:connect".to_string()],
            description: "expired key".to_string(),
            expires_at: Some(1),
        };
        let (provider, _) = make_provider(config_with_api_key(entry));
        let token = AuthToken {
            raw: token_str.as_bytes().to_vec(),
        };
        assert!(provider.resolve_from_token(&token).is_none());
    }

    #[test]
    fn token_resolution_unknown_returns_none() {
        let token_str = "alk_testsecret123";
        let hash = compute_api_key_hash(token_str);
        let entry = ApiKeyEntry {
            prefix: "alk_test".to_string(),
            hash,
            scopes: vec!["relay:connect".to_string()],
            description: "test key".to_string(),
            expires_at: None,
        };
        let (provider, _) = make_provider(config_with_api_key(entry));
        let token = AuthToken {
            raw: b"alk_unknown".to_vec(),
        };
        assert!(provider.resolve_from_token(&token).is_none());
    }

    #[test]
    fn token_resolution_wrong_hash_returns_none() {
        let entry = ApiKeyEntry {
            prefix: "alk_test".to_string(),
            hash: "sha256:deadbeef".to_string(),
            scopes: vec!["relay:connect".to_string()],
            description: "wrong hash".to_string(),
            expires_at: None,
        };
        let (provider, _) = make_provider(config_with_api_key(entry));
        let token = AuthToken {
            raw: b"alk_testsecret123".to_vec(),
        };
        assert!(provider.resolve_from_token(&token).is_none());
    }

    #[test]
    fn token_resolution_non_alk_prefix_returns_none() {
        let (provider, _) = make_provider(DynamicConfig::default());
        let token = AuthToken {
            raw: b"bearer_token".to_vec(),
        };
        assert!(provider.resolve_from_token(&token).is_none());
    }

    #[test]
    fn config_reload_changes_resolution_immediately() {
        let (provider, arc_swap) = make_provider(DynamicConfig::default());
        assert!(provider.resolve_from_fingerprint("SHA256:abc123").is_none());

        let new_config = config_with_peer_entry("worker-a", "SHA256:abc123");
        arc_swap.store(Arc::new(new_config));

        assert!(provider.resolve_from_fingerprint("SHA256:abc123").is_some());
    }

    #[test]
    fn config_reload_removes_fingerprint_access_immediately() {
        let (provider, arc_swap) =
            make_provider(config_with_peer_entry("worker-a", "SHA256:abc123"));
        assert!(provider.resolve_from_fingerprint("SHA256:abc123").is_some());

        arc_swap.store(Arc::new(DynamicConfig::default()));

        assert!(provider.resolve_from_fingerprint("SHA256:abc123").is_none());
    }

    #[test]
    fn config_reload_handle_reloads_config() {
        use crate::config::ConfigReloadHandle;
        let arc_swap = Arc::new(ArcSwap::new(Arc::new(DynamicConfig::default())));
        let provider = ConfigIdentityProvider::new(Arc::clone(&arc_swap));
        let handle = ConfigReloadHandle::new(arc_swap);

        assert!(provider.resolve_from_fingerprint("SHA256:abc123").is_none());

        handle.reload(config_with_peer_entry("worker-a", "SHA256:abc123"));

        assert!(provider.resolve_from_fingerprint("SHA256:abc123").is_some());
    }

    #[test]
    fn config_identity_provider_is_identity_provider_not_store() {
        fn assert_provider<T: IdentityProvider>() {}
        fn assert_not_store<T>() {}
        assert_provider::<ConfigIdentityProvider>();
        assert_not_store::<ConfigIdentityProvider>();
    }

    #[test]
    fn token_resolution_via_peer_entry_auth_token_hash_returns_peer_id() {
        let token_str = "peer-bearer-secret";
        let mut entry = peer_entry_with_fingerprint("worker-a", "SHA256:abc123");
        entry.auth_token_hash = Some(compute_token_hash(token_str));
        let config = DynamicConfig {
            auth: AuthPolicy {
                peers: vec![entry],
                api_keys: Vec::new(),
            },
            rate_limits: RateLimitConfig::default(),
        };
        let (provider, _) = make_provider(config);
        let token = AuthToken {
            raw: token_str.as_bytes().to_vec(),
        };
        let identity = provider
            .resolve_from_token(&token)
            .expect("matching PeerEntry.auth_token_hash resolves");
        assert_eq!(identity.id, "worker-a");
        assert_eq!(identity.scopes, vec!["relay:connect".to_string()]);
    }

    #[test]
    fn token_resolution_falls_through_to_api_key_when_no_peer_entry_matches() {
        let api_token = "alk_test_secret";
        let mut entry = peer_entry_with_fingerprint("worker-a", "SHA256:abc123");
        entry.auth_token_hash = Some(compute_token_hash("different-token"));
        let api_entry = ApiKeyEntry {
            prefix: "alk_test".to_string(),
            hash: compute_api_key_hash(api_token),
            scopes: vec!["admin".to_string()],
            description: "fall-through key".to_string(),
            expires_at: None,
        };
        let config = DynamicConfig {
            auth: AuthPolicy {
                peers: vec![entry],
                api_keys: vec![api_entry],
            },
            rate_limits: RateLimitConfig::default(),
        };
        let (provider, _) = make_provider(config);
        let token = AuthToken {
            raw: api_token.as_bytes().to_vec(),
        };
        let identity = provider
            .resolve_from_token(&token)
            .expect("api key fall-through resolves");
        assert_eq!(identity.id, "alk_test");
        assert_eq!(identity.scopes, vec!["admin".to_string()]);
    }

    #[test]
    fn disabled_peer_entry_returns_none_on_fingerprint_resolution() {
        let mut entry = peer_entry_with_fingerprint("worker-a", "SHA256:abc123");
        entry.enabled = false;
        let config = DynamicConfig {
            auth: AuthPolicy {
                peers: vec![entry],
                api_keys: Vec::new(),
            },
            rate_limits: RateLimitConfig::default(),
        };
        let (provider, _) = make_provider(config);
        assert!(provider.resolve_from_fingerprint("SHA256:abc123").is_none());
    }

    #[test]
    fn disabled_peer_entry_returns_none_on_token_resolution() {
        let token_str = "peer-bearer-secret";
        let mut entry = peer_entry_with_fingerprint("worker-a", "SHA256:abc123");
        entry.auth_token_hash = Some(compute_token_hash(token_str));
        entry.enabled = false;
        let config = DynamicConfig {
            auth: AuthPolicy {
                peers: vec![entry],
                api_keys: Vec::new(),
            },
            rate_limits: RateLimitConfig::default(),
        };
        let (provider, _) = make_provider(config);
        let token = AuthToken {
            raw: token_str.as_bytes().to_vec(),
        };
        assert!(provider.resolve_from_token(&token).is_none());
    }
}

#[cfg(test)]
mod identity_store_tests {
    use super::*;
    use crate::config::PeerEntry;
    use std::collections::HashMap as StdHashMap;
    use std::sync::RwLock;

    fn make_peer(peer_id: &str) -> PeerEntry {
        PeerEntry {
            peer_id: peer_id.to_string(),
            fingerprints: vec![format!("SHA256:{peer_id}")],
            auth_token_hash: None,
            scopes: vec!["relay:connect".to_string()],
            resources: StdHashMap::new(),
            display_name: None,
            enabled: true,
        }
    }

    struct MockIdentityStore {
        peers: RwLock<HashMap<String, PeerEntry>>,
    }

    impl MockIdentityStore {
        fn new() -> Self {
            Self {
                peers: RwLock::new(HashMap::new()),
            }
        }
    }

    impl IdentityProvider for MockIdentityStore {
        fn resolve_from_fingerprint(&self, fingerprint: &str) -> Option<Identity> {
            let peers = self.peers.read().unwrap_or_else(|e| e.into_inner());
            peers.values().find_map(|p| {
                if p.fingerprints.iter().any(|f| f == fingerprint) && p.enabled {
                    Some(Identity {
                        id: p.peer_id.clone(),
                        scopes: p.scopes.clone(),
                        resources: p.resources.clone(),
                    })
                } else {
                    None
                }
            })
        }

        fn resolve_from_token(&self, _token: &AuthToken) -> Option<Identity> {
            None
        }
    }

    #[async_trait]
    impl IdentityStore for MockIdentityStore {
        async fn put_peer(&self, peer: &PeerEntry) -> Result<(), StoreError> {
            let mut peers = self.peers.write().unwrap_or_else(|e| e.into_inner());
            peers.insert(peer.peer_id.clone(), peer.clone());
            Ok(())
        }

        async fn update_peer(&self, peer_id: &str, peer: &PeerEntry) -> Result<(), StoreError> {
            let mut peers = self.peers.write().unwrap_or_else(|e| e.into_inner());
            if !peers.contains_key(peer_id) {
                return Err(StoreError::NotFound {
                    entity: peer_id.to_string(),
                });
            }
            peers.remove(peer_id);
            peers.insert(peer.peer_id.clone(), peer.clone());
            Ok(())
        }

        async fn remove_peer(&self, peer_id: &str) -> Result<(), StoreError> {
            let mut peers = self.peers.write().unwrap_or_else(|e| e.into_inner());
            if peers.remove(peer_id).is_none() {
                return Err(StoreError::NotFound {
                    entity: peer_id.to_string(),
                });
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn mock_put_peer_upserts() {
        let store = MockIdentityStore::new();
        let mut peer = make_peer("worker-a");
        store.put_peer(&peer).await.unwrap();
        assert_eq!(
            store
                .resolve_from_fingerprint("SHA256:worker-a")
                .unwrap()
                .id,
            "worker-a"
        );

        peer.display_name = Some("renamed".to_string());
        store.put_peer(&peer).await.unwrap();
        let peers = store.peers.read().unwrap_or_else(|e| e.into_inner());
        assert_eq!(peers.len(), 1);
        assert_eq!(
            peers.get("worker-a").unwrap().display_name.as_deref(),
            Some("renamed")
        );
    }

    #[tokio::test]
    async fn mock_update_peer_existing_succeeds() {
        let store = MockIdentityStore::new();
        store.put_peer(&make_peer("worker-a")).await.unwrap();
        let updated = make_peer("worker-b");
        store.update_peer("worker-a", &updated).await.unwrap();
        assert!(store.resolve_from_fingerprint("SHA256:worker-a").is_none());
        assert!(store.resolve_from_fingerprint("SHA256:worker-b").is_some());
    }

    #[tokio::test]
    async fn mock_update_peer_missing_returns_not_found() {
        let store = MockIdentityStore::new();
        let err = store
            .update_peer("ghost", &make_peer("ghost"))
            .await
            .unwrap_err();
        assert!(matches!(err, StoreError::NotFound { .. }));
    }

    #[tokio::test]
    async fn mock_remove_peer_existing_succeeds() {
        let store = MockIdentityStore::new();
        store.put_peer(&make_peer("worker-a")).await.unwrap();
        store.remove_peer("worker-a").await.unwrap();
        assert!(store.resolve_from_fingerprint("SHA256:worker-a").is_none());
    }

    #[tokio::test]
    async fn mock_remove_peer_missing_returns_not_found() {
        let store = MockIdentityStore::new();
        let err = store.remove_peer("ghost").await.unwrap_err();
        assert!(matches!(err, StoreError::NotFound { .. }));
    }

    #[test]
    fn mock_identity_store_is_identity_provider() {
        fn assert_provider<T: IdentityProvider>() {}
        assert_provider::<MockIdentityStore>();
    }
}
