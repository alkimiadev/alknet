use std::sync::Arc;

use async_trait::async_trait;
use russh::client;
use russh::keys::key::PrivateKeyWithHashAlg;
use russh::keys::{PrivateKey, PublicKey};

use crate::auth::keys::KeySource;
use crate::error::ConfigError;

/// Client-side SSH authentication configuration.
///
/// Holds the private key used for SSH authentication and an optional
/// public key override. When no public key is provided, it is derived
/// from the private key.
pub struct ClientAuthConfig {
    private_key: Arc<PrivateKey>,
    public_key: PublicKey,
}

impl ClientAuthConfig {
    /// Load a `ClientAuthConfig` from a key source (file or in-memory).
    pub fn from_key_source(source: KeySource) -> Result<Self, ConfigError> {
        let private_key = crate::auth::keys::load_private_key(source)?;
        let public_key = private_key.public_key().clone();
        Ok(Self {
            private_key: Arc::new(private_key),
            public_key,
        })
    }

    /// Returns the private key wrapped in `Arc` for use with russh authentication.
    pub fn private_key(&self) -> Arc<PrivateKey> {
        Arc::clone(&self.private_key)
    }

    /// Returns the public key derived from (or overridden for) this config.
    pub fn public_key(&self) -> &PublicKey {
        &self.public_key
    }

    /// Authenticate with the given SSH session handle and username.
    pub async fn authenticate<H: client::Handler>(
        &self,
        handle: &mut client::Handle<H>,
        username: &str,
    ) -> Result<bool, russh::Error> {
        let key_with_alg = PrivateKeyWithHashAlg::new(Arc::clone(&self.private_key), None)?;
        handle.authenticate_publickey(username, key_with_alg).await
    }
}

/// Client handler implementing `russh::client::Handler`.
///
/// Provides the callbacks required by russh during the SSH handshake.
/// Server key verification is delegated to a configurable callback;
/// the default accepts all server keys (suitable for testing or when
/// transport-layer verification — e.g. TLS — is already in place).
pub struct ClientHandler {
    pub_key: PublicKey,
    check_server_key_fn: Box<dyn Fn(&PublicKey) -> bool + Send + Sync>,
}

impl ClientHandler {
    /// Create a new client handler from a `ClientAuthConfig`.
    pub fn from_config(config: &ClientAuthConfig) -> Self {
        Self {
            pub_key: config.public_key().clone(),
            check_server_key_fn: Box::new(|_| true),
        }
    }

    /// Create a client handler with a custom server key verification callback.
    pub fn with_server_key_check(
        config: &ClientAuthConfig,
        check_fn: impl Fn(&PublicKey) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            pub_key: config.public_key().clone(),
            check_server_key_fn: Box::new(check_fn),
        }
    }

    /// Returns the public key associated with this handler.
    pub fn public_key(&self) -> &PublicKey {
        &self.pub_key
    }
}

#[async_trait]
impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok((self.check_server_key_fn)(server_public_key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::client::Handler;

    const ED25519_PRIVATE_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAACBOfInDyRS33JEeDNT8xd10qRdwFN8z/QukCOgEIkv01QAAAJiQ+NvMkPjb\nzAAAAAtzc2gtZWQyNTUxOQAAACBOfInDyRS33JEeDNT8xd10qRdwFN8z/QukCOgEIkv01Q\nAAAECIWwJf7+7MOuZAOOWmoQbE9i/5GxjKsFrtJHjZ34E/fk58icPJFLfckR4M1PzF3XSp\nF3AU3zP9C6QI6AQiS/TVAAAAD3VidW50dUBuczUyODA5NgECAwQFBg==\n-----END OPENSSH PRIVATE KEY-----\n";

    #[test]
    fn from_key_source_memory() {
        let source = KeySource::Memory(ED25519_PRIVATE_KEY.as_bytes().to_vec());
        let config = ClientAuthConfig::from_key_source(source).unwrap();
        assert_eq!(
            config.public_key().algorithm(),
            russh::keys::Algorithm::Ed25519
        );
    }

    #[test]
    fn handler_from_config() {
        let source = KeySource::Memory(ED25519_PRIVATE_KEY.as_bytes().to_vec());
        let config = ClientAuthConfig::from_key_source(source).unwrap();
        let handler = ClientHandler::from_config(&config);
        assert_eq!(
            handler.public_key().algorithm(),
            russh::keys::Algorithm::Ed25519
        );
    }

    #[test]
    fn handler_with_custom_server_key_check() {
        let source = KeySource::Memory(ED25519_PRIVATE_KEY.as_bytes().to_vec());
        let config = ClientAuthConfig::from_key_source(source).unwrap();
        let handler = ClientHandler::with_server_key_check(&config, |_pk| false);
        assert_eq!(
            handler.public_key().algorithm(),
            russh::keys::Algorithm::Ed25519
        );
    }

    #[test]
    fn from_key_source_invalid_key() {
        let source = KeySource::Memory(b"not a key".to_vec());
        let result = ClientAuthConfig::from_key_source(source);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn handler_check_server_key_accepts_by_default() {
        let source = KeySource::Memory(ED25519_PRIVATE_KEY.as_bytes().to_vec());
        let config = ClientAuthConfig::from_key_source(source).unwrap();
        let mut handler = ClientHandler::from_config(&config);
        let some_key = config.public_key().clone();
        let result = handler.check_server_key(&some_key).await.unwrap();
        assert!(result);
    }

    #[tokio::test]
    async fn handler_check_server_key_rejects_with_custom_fn() {
        let source = KeySource::Memory(ED25519_PRIVATE_KEY.as_bytes().to_vec());
        let config = ClientAuthConfig::from_key_source(source).unwrap();
        let mut handler = ClientHandler::with_server_key_check(&config, |_pk| false);
        let some_key = config.public_key().clone();
        let result = handler.check_server_key(&some_key).await.unwrap();
        assert!(!result);
    }

    #[test]
    fn private_key_arc_dedup() {
        let source = KeySource::Memory(ED25519_PRIVATE_KEY.as_bytes().to_vec());
        let config = ClientAuthConfig::from_key_source(source).unwrap();
        let key1 = config.private_key();
        let key2 = config.private_key();
        assert!(Arc::ptr_eq(&key1, &key2));
    }
}