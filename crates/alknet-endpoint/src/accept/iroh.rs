//! Iroh accept loop.
//!
//! Accepts iroh connections, negotiates ALPN, extracts the client
//! fingerprint (NodeId), converts to `Connection`, and dispatches.

use std::sync::Arc;

use tokio::sync::watch;
use tracing::{debug, warn};

use alknet_core::auth::IdentityProvider;
use alknet_core::types::Connection;

use crate::registry::HandlerRegistry;

pub(crate) async fn run_accept_loop(
    iroh: iroh::Endpoint,
    handlers: Arc<HandlerRegistry>,
    identity_provider: Arc<dyn IdentityProvider>,
    shutdown_rx: &mut watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                debug!("iroh accept loop: shutdown signaled");
                break;
            }
            incoming = iroh.accept() => {
                let Some(incoming) = incoming else {
                    debug!("iroh accept loop: endpoint closed");
                    break;
                };
                let handlers = handlers.clone();
                let identity_provider = identity_provider.clone();
                tokio::spawn(async move {
                    let mut connecting = match incoming.accept() {
                        Ok(c) => c,
                        Err(e) => {
                            warn!("iroh accept failed: {e}");
                            return;
                        }
                    };
                    let alpn = match connecting.alpn().await {
                        Ok(alpn) => alpn,
                        Err(e) => {
                            warn!("iroh ALPN negotiation failed: {e}");
                            return;
                        }
                    };
                    let connection = match connecting.await {
                        Ok(conn) => conn,
                        Err(e) => {
                            warn!("iroh handshake completion failed: {e}");
                            return;
                        }
                    };
                    let fingerprint = extract_client_fingerprint(&connection);
                    let conn = Connection::from_iroh(connection);
                    crate::dispatch::dispatch_connection(
                        conn, alpn, fingerprint, None,
                        &handlers, &identity_provider,
                    );
                });
            }
        }
    }
}

fn extract_client_fingerprint(connection: &iroh::endpoint::Connection) -> Option<String> {
    let node_id = connection.remote_id();
    Some(format!("ed25519:{}", node_id))
}
