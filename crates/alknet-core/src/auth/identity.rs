use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;

use crate::config::DynamicConfig;

#[derive(Debug, Clone, PartialEq)]
pub struct Identity {
    pub id: String,
    pub scopes: Vec<String>,
    pub resources: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct AuthToken {
    pub raw: Vec<u8>,
}

pub trait IdentityProvider: Send + Sync + 'static {
    fn resolve_from_fingerprint(&self, fingerprint: &str) -> Option<Identity>;

    fn resolve_from_token(&self, token: &AuthToken) -> Option<Identity>;
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
        let auth = &config.auth;
        auth.resolve_identity_from_fingerprint(fingerprint)
    }

    fn resolve_from_token(&self, _token: &AuthToken) -> Option<Identity> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::keys::KeySource;
    use crate::auth::ServerAuthConfig;
    use crate::config::AuthPolicy;
    use russh::keys::ssh_key::HashAlg;
    use russh::keys::PrivateKey;
    use std::io::Write;

    const ED25519_PRIVATE_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAACBOfInDyRS33JEeDNT8xd10qRdwFN8z/QukCOgEIkv01QAAAJiQ+NvMkPjb\nzAAAAAtzc2gtZWQyNTUxOQAAACBOfInDyRS33JEeDNT8xd10qRdwFN8z/QukCOgEIkv01Q\nAAAECIWwJf7+7MOuZAOOWmoQbE9i/5GxjKsFrtJHjZ34E/fk58icPJFLfckR4M1PzF3XSp\nF3AU3zP9C6QI6AQiS/TVAAAAD3VidW50dUBuczUyODA5NgECAwQFBg==\n-----END OPENSSH PRIVATE KEY-----\n";

    const ED25519_PUBLIC_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIE58icPJFLfckR4M1PzF3XSpF3AU3zP9C6QI6AQiS/TV ubuntu@ns528096";

    fn load_key() -> PrivateKey {
        russh::keys::decode_secret_key(ED25519_PRIVATE_KEY, None).unwrap()
    }

    fn make_authorized_keys_file(keys_content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(keys_content.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    fn make_provider(keys_content: &str) -> (ConfigIdentityProvider, Arc<ArcSwap<DynamicConfig>>) {
        let f = make_authorized_keys_file(keys_content);
        let server_auth =
            ServerAuthConfig::from_keys_and_ca(Some(KeySource::File(f.path().to_path_buf())), None)
                .unwrap();
        let auth_policy = AuthPolicy::from_server_auth_config(server_auth);
        let dynamic = DynamicConfig::new(auth_policy);
        let arc_swap = Arc::new(ArcSwap::new(Arc::new(dynamic)));
        let provider = ConfigIdentityProvider::new(Arc::clone(&arc_swap));
        (provider, arc_swap)
    }

    #[test]
    fn identity_fields() {
        let mut resources = HashMap::new();
        resources.insert(
            "service".to_string(),
            vec!["gitea".to_string(), "registry".to_string()],
        );
        let identity = Identity {
            id: "SHA256:abc123".to_string(),
            scopes: vec![
                "relay:connect".to_string(),
                "service:gitea:read".to_string(),
            ],
            resources,
        };
        assert_eq!(identity.id, "SHA256:abc123");
        assert_eq!(identity.scopes, vec!["relay:connect", "service:gitea:read"]);
        assert_eq!(
            identity.resources.get("service").unwrap(),
            &vec!["gitea".to_string(), "registry".to_string()]
        );
    }

    #[test]
    fn identity_equality() {
        let id1 = Identity {
            id: "test".to_string(),
            scopes: vec!["relay:connect".to_string()],
            resources: HashMap::new(),
        };
        let id2 = Identity {
            id: "test".to_string(),
            scopes: vec!["relay:connect".to_string()],
            resources: HashMap::new(),
        };
        assert_eq!(id1, id2);
    }

    #[test]
    fn identity_inequality_different_id() {
        let id1 = Identity {
            id: "a".to_string(),
            scopes: vec![],
            resources: HashMap::new(),
        };
        let id2 = Identity {
            id: "b".to_string(),
            scopes: vec![],
            resources: HashMap::new(),
        };
        assert_ne!(id1, id2);
    }

    #[test]
    fn config_identity_provider_resolves_valid_fingerprint() {
        let (provider, _) = make_provider(ED25519_PUBLIC_KEY);
        let key = load_key().public_key().clone();
        let fingerprint = format!("{}", key.fingerprint(HashAlg::Sha256));
        let identity = provider.resolve_from_fingerprint(&fingerprint);
        assert!(identity.is_some());
        let identity = identity.unwrap();
        assert_eq!(identity.id, fingerprint);
        assert!(!identity.scopes.is_empty());
    }

    #[test]
    fn config_identity_provider_rejects_invalid_fingerprint() {
        let (provider, _) = make_provider(ED25519_PUBLIC_KEY);
        let identity = provider.resolve_from_fingerprint("SHA256:invalid");
        assert!(identity.is_none());
    }

    #[test]
    fn config_identity_provider_empty_config_rejects_all() {
        let dynamic = DynamicConfig::default();
        let arc_swap = Arc::new(ArcSwap::new(Arc::new(dynamic)));
        let provider = ConfigIdentityProvider::new(arc_swap);
        let identity = provider.resolve_from_fingerprint("SHA256:anything");
        assert!(identity.is_none());
    }

    #[test]
    fn config_identity_provider_resolve_from_token_returns_none() {
        let (provider, _) = make_provider(ED25519_PUBLIC_KEY);
        let token = AuthToken {
            raw: b"test-token".to_vec(),
        };
        assert!(provider.resolve_from_token(&token).is_none());
    }

    #[test]
    fn auth_token_holds_raw_bytes() {
        let token = AuthToken { raw: vec![1, 2, 3] };
        assert_eq!(token.raw, vec![1, 2, 3]);
    }

    #[test]
    fn config_identity_provider_reflects_config_reload() {
        let (provider, arc_swap) = make_provider(ED25519_PUBLIC_KEY);
        let key = load_key().public_key().clone();
        let fingerprint = format!("{}", key.fingerprint(HashAlg::Sha256));

        let identity = provider.resolve_from_fingerprint(&fingerprint);
        assert!(identity.is_some());

        let new_dynamic = DynamicConfig::default();
        arc_swap.store(Arc::new(new_dynamic));

        let identity = provider.resolve_from_fingerprint(&fingerprint);
        assert!(identity.is_none());
    }
}
