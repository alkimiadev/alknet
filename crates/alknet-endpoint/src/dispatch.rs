//! Shared dispatch path for all transports.
//!
//! `dispatch_connection` is the free function called by every accept loop
//! after transport-specific extraction. `AlknetEndpoint::dispatch` delegates
//! to it. `build_auth_context` resolves the caller's identity from the
//! TLS fingerprint.

#[cfg(any(feature = "quinn", feature = "iroh", feature = "tcp"))]
use std::net::SocketAddr;
#[cfg(any(feature = "quinn", feature = "iroh", feature = "tcp"))]
use std::sync::Arc;

#[cfg(any(feature = "quinn", feature = "iroh", feature = "tcp"))]
use tracing::{error, warn};

#[cfg(any(feature = "quinn", feature = "iroh", feature = "tcp"))]
use alknet_core::auth::{AuthContext, IdentityProvider};
#[cfg(any(feature = "quinn", feature = "iroh", feature = "tcp"))]
use alknet_core::types::Connection;

#[cfg(any(feature = "quinn", feature = "iroh", feature = "tcp"))]
use crate::registry::HandlerRegistry;

#[cfg(any(feature = "quinn", feature = "iroh", feature = "tcp"))]
pub(crate) fn dispatch_connection(
    connection: Connection,
    alpn: Vec<u8>,
    fingerprint: Option<String>,
    remote_addr: Option<SocketAddr>,
    handlers: &HandlerRegistry,
    identity_provider: &Arc<dyn IdentityProvider>,
) {
    #[cfg(feature = "acme")]
    if alpn == b"acme-tls/1" {
        tracing::debug!("acme-tls/1 challenge connection; closing");
        connection.close(0, "acme done");
        return;
    }

    let handler = match handlers.get(&alpn) {
        Some(h) => h.clone(),
        None => {
            connection.close(0, "no handler");
            warn!(
                "dispatch: no handler for ALPN {:?}",
                String::from_utf8_lossy(&alpn)
            );
            return;
        }
    };

    let auth = build_auth_context(&alpn, remote_addr, fingerprint, identity_provider);
    tokio::spawn(async move {
        if let Err(e) = handler.handle(connection, &auth).await {
            error!("handler returned error: {e}");
        }
    });
}

#[cfg(any(feature = "quinn", feature = "iroh", feature = "tcp"))]
pub(crate) fn build_auth_context(
    alpn: &[u8],
    remote_addr: Option<SocketAddr>,
    tls_client_fingerprint: Option<String>,
    identity_provider: &Arc<dyn IdentityProvider>,
) -> AuthContext {
    let identity = tls_client_fingerprint
        .as_ref()
        .and_then(|fp| identity_provider.resolve_from_fingerprint(fp));
    AuthContext {
        identity,
        alpn: alpn.to_vec(),
        remote_addr,
        tls_client_fingerprint,
    }
}

#[cfg(test)]
mod tests {
    #[cfg(any(feature = "quinn", feature = "iroh", feature = "tcp"))]
    use super::build_auth_context;
    #[cfg(any(feature = "quinn", feature = "iroh", feature = "tcp"))]
    use std::collections::HashMap;
    #[cfg(any(feature = "quinn", feature = "iroh", feature = "tcp"))]
    use std::sync::Arc;

    #[cfg(any(feature = "quinn", feature = "iroh", feature = "tcp"))]
    use alknet_core::auth::{AuthToken, Identity, IdentityProvider};
    #[cfg(any(feature = "quinn", feature = "iroh", feature = "tcp"))]
    use alknet_core::types::{Connection, HandlerError};
    #[cfg(any(feature = "quinn", feature = "iroh", feature = "tcp"))]
    use async_trait::async_trait;

    #[cfg(any(feature = "quinn", feature = "iroh", feature = "tcp"))]
    use crate::registry::HandlerRegistry;

    #[cfg(any(feature = "quinn", feature = "iroh", feature = "tcp"))]
    struct DummyHandler {
        alpn: &'static [u8],
    }

    #[cfg(any(feature = "quinn", feature = "iroh", feature = "tcp"))]
    #[async_trait]
    impl alknet_core::types::ProtocolHandler for DummyHandler {
        fn alpn(&self) -> &'static [u8] {
            self.alpn
        }
        async fn handle(
            &self,
            _connection: Connection,
            _auth: &alknet_core::auth::AuthContext,
        ) -> Result<(), HandlerError> {
            Ok(())
        }
    }

    #[cfg(any(feature = "quinn", feature = "iroh", feature = "tcp"))]
    fn make_handler(alpn: &'static [u8]) -> Arc<dyn alknet_core::types::ProtocolHandler> {
        Arc::new(DummyHandler { alpn })
    }

    #[cfg(any(feature = "quinn", feature = "iroh", feature = "tcp"))]
    #[test]
    fn build_auth_context_resolves_identity_from_fingerprint() {
        struct StaticProvider;
        impl IdentityProvider for StaticProvider {
            fn resolve_from_fingerprint(&self, fp: &str) -> Option<Identity> {
                if fp == "SHA256:known" {
                    Some(Identity {
                        id: "SHA256:known".to_string(),
                        scopes: vec![],
                        resources: HashMap::new(),
                    })
                } else {
                    None
                }
            }
            fn resolve_from_token(&self, _token: &AuthToken) -> Option<Identity> {
                None
            }
        }
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticProvider);
        let auth = build_auth_context(
            b"alknet/test",
            None,
            Some("SHA256:known".to_string()),
            &provider,
        );
        assert_eq!(auth.identity.as_ref().unwrap().id, "SHA256:known");
        assert_eq!(auth.alpn, b"alknet/test");
        assert_eq!(auth.tls_client_fingerprint.as_deref(), Some("SHA256:known"));
    }

    #[cfg(any(feature = "quinn", feature = "iroh", feature = "tcp"))]
    #[test]
    fn build_auth_context_no_fingerprint_no_identity() {
        struct NoProvider;
        impl IdentityProvider for NoProvider {
            fn resolve_from_fingerprint(&self, _fp: &str) -> Option<Identity> {
                None
            }
            fn resolve_from_token(&self, _token: &AuthToken) -> Option<Identity> {
                None
            }
        }
        let provider: Arc<dyn IdentityProvider> = Arc::new(NoProvider);
        let auth = build_auth_context(b"alknet/test", None, None, &provider);
        assert!(auth.identity.is_none());
        assert!(auth.tls_client_fingerprint.is_none());
    }

    #[cfg(any(feature = "quinn", feature = "iroh", feature = "tcp"))]
    #[test]
    fn build_auth_context_fingerprint_unknown_identity_none() {
        struct StaticProvider;
        impl IdentityProvider for StaticProvider {
            fn resolve_from_fingerprint(&self, _fp: &str) -> Option<Identity> {
                None
            }
            fn resolve_from_token(&self, _token: &AuthToken) -> Option<Identity> {
                None
            }
        }
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticProvider);
        let auth = build_auth_context(
            b"alknet/test",
            None,
            Some("SHA256:unknown".to_string()),
            &provider,
        );
        assert!(auth.identity.is_none());
        assert!(auth.tls_client_fingerprint.is_some());
    }

    #[cfg(any(feature = "quinn", feature = "iroh", feature = "tcp"))]
    #[test]
    fn dispatch_decision_logic_lookup_and_auth() {
        let mut registry = HandlerRegistry::new();
        registry.register(make_handler(b"alknet/ssh"));
        registry.register(make_handler(b"alknet/call"));

        struct StaticProvider;
        impl IdentityProvider for StaticProvider {
            fn resolve_from_fingerprint(&self, fp: &str) -> Option<Identity> {
                if fp == "SHA256:caller" {
                    Some(Identity {
                        id: "SHA256:caller".to_string(),
                        scopes: vec!["relay:connect".to_string()],
                        resources: HashMap::new(),
                    })
                } else {
                    None
                }
            }
            fn resolve_from_token(&self, _: &AuthToken) -> Option<Identity> {
                None
            }
        }
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticProvider);

        let ssh_handler = registry.get(b"alknet/ssh").expect("ssh handler registered");
        assert_eq!(ssh_handler.alpn(), b"alknet/ssh");
        let auth = build_auth_context(
            b"alknet/ssh",
            Some(std::net::SocketAddr::new(
                std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                1234,
            )),
            Some("SHA256:caller".to_string()),
            &provider,
        );
        assert_eq!(auth.identity.as_ref().unwrap().id, "SHA256:caller");
        assert_eq!(auth.alpn, b"alknet/ssh");

        let unknown = registry.get(b"alknet/unknown");
        assert!(unknown.is_none(), "unknown ALPN has no handler");
    }
}
