use std::sync::Arc;

use arc_swap::ArcSwap;

use crate::auth::identity::{AuthToken, ConfigIdentityProvider, Identity, IdentityProvider};
use crate::config::DynamicConfig;

#[derive(Debug, Clone, PartialEq)]
pub enum AuthProtocol {
    VerifyPubkey {
        fingerprint: String,
        key_data: Vec<u8>,
    },
    VerifyToken {
        token_bytes: Vec<u8>,
        timestamp: u64,
    },
    ReloadKeys,
    CheckAccess {
        identity: Identity,
        operation: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum AuthResult {
    Ok(Identity),
    Denied(String),
}

pub struct AuthServiceImpl {
    provider: ConfigIdentityProvider,
    dynamic: Arc<ArcSwap<DynamicConfig>>,
}

impl AuthServiceImpl {
    pub fn new(dynamic: Arc<ArcSwap<DynamicConfig>>) -> Self {
        let provider = ConfigIdentityProvider::new(Arc::clone(&dynamic));
        Self { provider, dynamic }
    }

    pub fn verify_pubkey(&self, fingerprint: &str) -> AuthResult {
        match self.provider.resolve_from_fingerprint(fingerprint) {
            Some(identity) => AuthResult::Ok(identity),
            None => AuthResult::Denied(format!("key not authorized: {}", fingerprint)),
        }
    }

    pub fn verify_token(&self, token: &AuthToken) -> AuthResult {
        match self.provider.resolve_from_token(token) {
            Some(identity) => AuthResult::Ok(identity),
            None => AuthResult::Denied("token verification failed".to_string()),
        }
    }

    pub fn reload_keys(&self) {
        self.dynamic.rcu(Arc::clone);
    }

    pub fn check_access(&self, identity: &Identity, operation: &str) -> AuthResult {
        if identity.scopes.iter().any(|s| s == operation) {
            AuthResult::Ok(identity.clone())
        } else {
            AuthResult::Denied(format!(
                "identity {} lacks scope: {}",
                identity.id, operation
            ))
        }
    }
}

impl std::fmt::Debug for AuthServiceImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthServiceImpl").finish()
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

    fn make_service(keys_content: &str) -> (AuthServiceImpl, Arc<ArcSwap<DynamicConfig>>) {
        let f = make_authorized_keys_file(keys_content);
        let server_auth =
            ServerAuthConfig::from_keys_and_ca(Some(KeySource::File(f.path().to_path_buf())), None)
                .unwrap();
        let auth_policy = AuthPolicy::from_server_auth_config(server_auth);
        let dynamic = DynamicConfig::new(auth_policy);
        let arc_swap = Arc::new(ArcSwap::new(Arc::new(dynamic)));
        let service = AuthServiceImpl::new(Arc::clone(&arc_swap));
        (service, arc_swap)
    }

    #[test]
    fn auth_service_verify_pubkey_valid() {
        let (service, _) = make_service(ED25519_PUBLIC_KEY);
        let key = load_key().public_key().clone();
        let fingerprint = format!("{}", key.fingerprint(HashAlg::Sha256));
        let result = service.verify_pubkey(&fingerprint);
        assert!(matches!(result, AuthResult::Ok(_)));
        if let AuthResult::Ok(identity) = result {
            assert_eq!(identity.id, fingerprint);
        }
    }

    #[test]
    fn auth_service_verify_pubkey_invalid() {
        let (service, _) = make_service(ED25519_PUBLIC_KEY);
        let result = service.verify_pubkey("SHA256:invalid");
        assert!(matches!(result, AuthResult::Denied(_)));
    }

    #[test]
    fn auth_service_verify_pubkey_matches_identity_provider() {
        let (service, arc_swap) = make_service(ED25519_PUBLIC_KEY);
        let provider = ConfigIdentityProvider::new(Arc::clone(&arc_swap));
        let key = load_key().public_key().clone();
        let fingerprint = format!("{}", key.fingerprint(HashAlg::Sha256));

        let service_result = service.verify_pubkey(&fingerprint);
        let provider_result = provider.resolve_from_fingerprint(&fingerprint);

        match service_result {
            AuthResult::Ok(identity) => {
                assert_eq!(identity, provider_result.unwrap());
            }
            AuthResult::Denied(_) => {
                assert!(provider_result.is_none());
            }
        }
    }

    #[test]
    fn auth_service_verify_token_returns_denied() {
        let (service, _) = make_service(ED25519_PUBLIC_KEY);
        let token = AuthToken {
            raw: b"test-token".to_vec(),
        };
        let result = service.verify_token(&token);
        assert!(matches!(result, AuthResult::Denied(_)));
    }

    #[test]
    fn auth_service_check_access_granted() {
        let (service, _) = make_service(ED25519_PUBLIC_KEY);
        let key = load_key().public_key().clone();
        let fingerprint = format!("{}", key.fingerprint(HashAlg::Sha256));
        let identity = Identity {
            id: fingerprint,
            scopes: vec!["relay:connect".to_string()],
            resources: std::collections::HashMap::new(),
        };
        let result = service.check_access(&identity, "relay:connect");
        assert!(matches!(result, AuthResult::Ok(_)));
    }

    #[test]
    fn auth_service_check_access_denied() {
        let (service, _) = make_service(ED25519_PUBLIC_KEY);
        let identity = Identity {
            id: "test".to_string(),
            scopes: vec!["relay:connect".to_string()],
            resources: std::collections::HashMap::new(),
        };
        let result = service.check_access(&identity, "admin:write");
        assert!(matches!(result, AuthResult::Denied(_)));
    }

    #[test]
    fn auth_protocol_variants() {
        let identity = Identity {
            id: "SHA256:abc".to_string(),
            scopes: vec!["relay:connect".to_string()],
            resources: std::collections::HashMap::new(),
        };

        let verify_pubkey = AuthProtocol::VerifyPubkey {
            fingerprint: "SHA256:abc".to_string(),
            key_data: vec![1, 2, 3],
        };
        match &verify_pubkey {
            AuthProtocol::VerifyPubkey {
                fingerprint,
                key_data,
            } => {
                assert_eq!(fingerprint, "SHA256:abc");
                assert_eq!(key_data, &vec![1, 2, 3]);
            }
            _ => panic!("expected VerifyPubkey variant"),
        }

        let verify_token = AuthProtocol::VerifyToken {
            token_bytes: vec![4, 5, 6],
            timestamp: 12345,
        };
        match &verify_token {
            AuthProtocol::VerifyToken {
                token_bytes,
                timestamp,
            } => {
                assert_eq!(token_bytes, &vec![4, 5, 6]);
                assert_eq!(*timestamp, 12345);
            }
            _ => panic!("expected VerifyToken variant"),
        }

        assert!(matches!(AuthProtocol::ReloadKeys, AuthProtocol::ReloadKeys));

        let check = AuthProtocol::CheckAccess {
            identity: identity.clone(),
            operation: "relay:connect".to_string(),
        };
        match &check {
            AuthProtocol::CheckAccess {
                identity: id,
                operation,
            } => {
                assert_eq!(id.id, "SHA256:abc");
                assert_eq!(operation, "relay:connect");
            }
            _ => panic!("expected CheckAccess variant"),
        }
    }

    #[test]
    fn auth_result_ok_identity() {
        let identity = Identity {
            id: "test".to_string(),
            scopes: vec![],
            resources: std::collections::HashMap::new(),
        };
        let result = AuthResult::Ok(identity.clone());
        assert_eq!(result, AuthResult::Ok(identity));
    }

    #[test]
    fn auth_result_denied_message() {
        let result = AuthResult::Denied("access denied".to_string());
        assert_eq!(result, AuthResult::Denied("access denied".to_string()));
    }
}
