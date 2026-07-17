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
