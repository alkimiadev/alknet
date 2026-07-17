---
id: endpoint/accept-tcp-tls
name: Implement TCP+TLS accept loop and extractors in alknet-endpoint (new code)
status: pending
depends_on: [endpoint/dispatch]
scope: narrow
risk: medium
impact: component
level: implementation
---

## Description

Phase 2, Task 7 of the crate extraction. Implement the TCP+TLS accept loop and
transport-specific extractors in `crates/alknet-endpoint/src/accept/tcp_tls.rs`.

**This is new code** — the current `endpoint.rs` does not have a TCP+TLS accept loop.
It must be written fresh, following the same pattern as the quinn and iroh accept loops
but adapted for TCP+TLS transport.

### Design

The TCP+TLS accept loop follows the same pattern as quinn and iroh:

1. `tcp_listener.accept()` → get a `TcpStream`
2. `tls_acceptor.accept(tcp_stream)` → TLS handshake → get a `TlsStream<TcpStream>`
3. Extract ALPN from the TLS session
4. Extract client fingerprint from the peer certificate chain
5. Convert to `Connection::from_bidi(tls_stream)` (the `TlsStream<TcpStream>` is
   `AsyncRead + AsyncWrite`)
6. Call `crate::dispatch::dispatch_connection()`

### Implementation sketch

```rust
// accept/tcp_tls.rs
#[cfg(feature = "tcp")]
use std::net::SocketAddr;
#[cfg(feature = "tcp")]
use std::sync::Arc;
#[cfg(feature = "tcp")]
use tokio::sync::watch;
#[cfg(feature = "tcp")]
use tracing::{debug, warn};

#[cfg(feature = "tcp")]
use alknet_core::auth::IdentityProvider;
#[cfg(feature = "tcp")]
use alknet_core::types::Connection;

#[cfg(feature = "tcp")]
use crate::registry::HandlerRegistry;

#[cfg(feature = "tcp")]
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
                    let conn = Connection::from_bidi(tls_stream);
                    crate::dispatch::dispatch_connection(
                        conn, alpn, fingerprint, Some(remote_addr),
                        &handlers, &identity_provider,
                    );
                });
            }
        }
    }
}

#[cfg(feature = "tcp")]
fn extract_tls_session_info(
    tls_stream: &tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
) -> (Vec<u8>, Option<String>) {
    let (_, session) = tls_stream.get_ref();
    let alpn = session.alpn_protocol().map(|a| a.to_vec()).unwrap_or_default();
    let fingerprint = session
        .peer_certificates()
        .and_then(|certs| certs.first())
        .and_then(|cert| alknet_core::fingerprint::fingerprint_from_cert_der(cert.as_ref()));
    (alpn, fingerprint)
}
```

### Key design decisions

1. **`Connection::from_bidi`**: The `TlsStream<TcpStream>` implements `AsyncRead + AsyncWrite`,
   so it can be passed directly to `Connection::from_bidi`. No `QuicStream` wrapper needed
   (that's the Phase 6 fix for `alknet-http`).

2. **ALPN extraction**: `session.alpn_protocol()` returns the negotiated ALPN from the TLS
   session. This is the standard rustls API.

3. **Fingerprint extraction**: `session.peer_certificates()` returns the peer's certificate
   chain. The leaf cert's fingerprint is extracted via `alknet_core::fingerprint::fingerprint_from_cert_der`.

4. **`remote_addr`**: Available from `TcpListener::accept()` — passed to dispatch.

5. **Feature gate**: All code gated on `#[cfg(feature = "tcp")]`.

### What stays in core

There is no existing TCP+TLS accept loop in `endpoint.rs` — this is entirely new code.
No duplication, no prune needed.

## Acceptance Criteria

- [ ] `accept/tcp_tls.rs` contains `run_accept_loop`, `extract_tls_session_info`
- [ ] `run_accept_loop` accepts TCP connections, performs TLS handshake, spawns handler task
- [ ] `run_accept_loop` handles shutdown signal via `watch::Receiver`
- [ ] `extract_tls_session_info` extracts ALPN from TLS session
- [ ] `extract_tls_session_info` extracts client cert fingerprint via `alknet_core::fingerprint`
- [ ] Accept loop calls `crate::dispatch::dispatch_connection()` with extracted values
- [ ] `Connection::from_bidi(tls_stream)` used (no hand-rolled wrapper)
- [ ] `remote_addr` passed from `TcpListener::accept()`
- [ ] All code gated on `#[cfg(feature = "tcp")]`
- [ ] All imports use `alknet_core::` (not `crate::` from core)
- [ ] `cargo check -p alknet-endpoint --features tcp` succeeds
- [ ] `cargo clippy -p alknet-endpoint --features tcp` succeeds with no warnings
- [ ] `cargo test -p alknet-core` still passes (old code untouched)

## References

- docs/research/alknet-crate-extraction/findings.md — Phase 2, accept/tcp_tls module
- docs/architecture/crates/endpoint/README.md — Accept loops (lines 177-193), TcpTlsListener (lines 164-175)
- docs/architecture/decisions/083-endpoint-as-accept-loop-runner.md — ADR-083
- docs/architecture/decisions/065-connection-from-stream-generic-single-stream.md — ADR-065 (Connection::from_bidi)
- crates/alknet-core/src/endpoint.rs — lines 287-388 (quinn accept loop, reference pattern)

## Notes

> This is the only genuinely new code in Phase 2 — the current `endpoint.rs` has no
> TCP+TLS accept loop. It follows the same pattern as the quinn and iroh accept loops
> but uses `TcpListener::accept()` + `TlsAcceptor::accept()` + `Connection::from_bidi`.
> The `TlsStream<TcpStream>` is already `AsyncRead + AsyncWrite` — no wrapper needed.
> Risk is medium because it's new code, but the pattern is well-established by the
> quinn and iroh loops.

## Summary

> To be filled on completion
