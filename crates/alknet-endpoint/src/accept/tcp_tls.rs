//! TCP+TLS accept loop.
//!
//! Accepts TCP connections, performs a TLS handshake, extracts
//! ALPN + client fingerprint, converts to `Connection::from_bidi`,
//! and dispatches.

use std::sync::Arc;

use tokio::sync::watch;
use tracing::{debug, warn};

use alknet_core::auth::IdentityProvider;
use alknet_core::types::Connection;

use crate::registry::HandlerRegistry;

pub(crate) async fn run_accept_loop(
    listener: tokio::net::TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
    handlers: Arc<HandlerRegistry>,
    identity_provider: Arc<dyn IdentityProvider>,
    shutdown_rx: &mut watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                debug!("tcp+tls accept loop: shutdown signaled");
                break;
            }
            result = listener.accept() => {
                let (tcp_stream, remote_addr) = match result {
                    Ok(r) => r,
                    Err(e) => {
                        warn!("tcp+tls accept failed: {e}");
                        continue;
                    }
                };
                let acceptor = acceptor.clone();
                let handlers = handlers.clone();
                let identity_provider = identity_provider.clone();
                tokio::spawn(async move {
                    let tls_stream = match acceptor.accept(tcp_stream).await {
                        Ok(s) => s,
                        Err(e) => {
                            warn!("tcp+tls TLS handshake failure: {e}");
                            return;
                        }
                    };
                    let (alpn, fingerprint) = extract_tls_session_info(&tls_stream);
                    let conn = Connection::from_bidi(tls_stream, alpn.clone(), Some(remote_addr));
                    crate::dispatch::dispatch_connection(
                        conn, alpn, fingerprint, Some(remote_addr),
                        &handlers, &identity_provider,
                    );
                });
            }
        }
    }
}

fn extract_tls_session_info(
    tls_stream: &tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
) -> (Vec<u8>, Option<String>) {
    let (_, session) = tls_stream.get_ref();
    let alpn = session
        .alpn_protocol()
        .map(|a| a.to_vec())
        .unwrap_or_default();
    let fingerprint = session
        .peer_certificates()
        .and_then(|certs| certs.first())
        .and_then(|cert| alknet_core::fingerprint::fingerprint_from_cert_der(cert.as_ref()));
    (alpn, fingerprint)
}
