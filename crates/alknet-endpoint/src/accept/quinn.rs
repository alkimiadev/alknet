//! Quinn (QUIC) accept loop.
//!
//! Accepts QUIC connections, performs the TLS handshake, extracts
//! ALPN + client fingerprint, converts to `Connection`, and dispatches.

use std::sync::Arc;

use tokio::sync::watch;
use tracing::{debug, warn};

use alknet_core::auth::IdentityProvider;
use alknet_core::types::Connection;

use crate::registry::HandlerRegistry;

pub(crate) async fn run_accept_loop(
    quinn: quinn::Endpoint,
    handlers: Arc<HandlerRegistry>,
    identity_provider: Arc<dyn IdentityProvider>,
    shutdown_rx: &mut watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                debug!("quinn accept loop: shutdown signaled");
                break;
            }
            incoming = quinn.accept() => {
                let Some(incoming) = incoming else {
                    debug!("quinn accept loop: endpoint closed");
                    break;
                };
                let connecting = match incoming.accept() {
                    Ok(c) => c,
                    Err(e) => {
                        warn!("quinn accept failed: {e}");
                        continue;
                    }
                };
                let handlers = handlers.clone();
                let identity_provider = identity_provider.clone();
                tokio::spawn(async move {
                    let connection = match connecting.await {
                        Ok(conn) => conn,
                        Err(e) => {
                            warn!("quinn TLS handshake failure: {e}");
                            return;
                        }
                    };
                    let alpn = extract_alpn(&connection);
                    let remote_addr = Some(connection.remote_address());
                    let fingerprint = extract_client_fingerprint(&connection);
                    let conn = Connection::from_quinn_with_alpn(connection, alpn.clone());
                    crate::dispatch::dispatch_connection(
                        conn, alpn, fingerprint, remote_addr,
                        &handlers, &identity_provider,
                    );
                });
            }
        }
    }
}

fn extract_alpn(connection: &quinn::Connection) -> Vec<u8> {
    use quinn::crypto::rustls::HandshakeData;
    if let Some(data) = connection.handshake_data() {
        if let Ok(hs) = data.downcast::<HandshakeData>() {
            if let Some(protocol) = hs.protocol {
                return protocol;
            }
        }
    }
    Vec::new()
}

fn extract_client_fingerprint(connection: &quinn::Connection) -> Option<String> {
    let identity = connection.peer_identity()?;
    let certs = identity
        .downcast::<Vec<rustls::pki_types::CertificateDer>>()
        .ok()?;
    let leaf = certs.first()?;
    alknet_core::fingerprint::fingerprint_from_cert_der(leaf.as_ref())
}
