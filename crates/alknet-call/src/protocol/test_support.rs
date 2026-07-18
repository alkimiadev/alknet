//! Shared test helpers for the call protocol's inline `#[cfg(test)]`
//! modules. Kept here (not in each test module) so the `stub_connection()`
//! shape is defined once — `Connection::from_stream` was removed (ADR-092)
//! and every test stub that previously called it now calls
//! `Connection::from_bidi(SinkEmpty, ...)` via `sink_empty_connection()`.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::task::{Context, Poll};

use alknet_core::types::Connection;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// A test-only `AsyncRead + AsyncWrite` pair equivalent to
/// `tokio::io::sink() + tokio::io::empty()`: reads yield EOF immediately
/// (zero bytes), writes discard. Exists because `Connection::from_bidi`
/// (ADR-092 — the only public stream constructor, replacing
/// `from_stream`) requires a single value that implements both traits.
/// Used only to construct a `Connection` for tests that exercise
/// `Connection`-level state (alpn, addr, identity, dispatcher run loop
/// with an immediately-closed accept stream) without ever reading or
/// writing real bytes.
pub(crate) struct SinkEmpty;

impl AsyncRead for SinkEmpty {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        // EOF immediately — mirrors `tokio::io::empty()`.
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for SinkEmpty {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        // Discard — mirrors `tokio::io::sink()`.
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// Construct a `Connection` whose `accept_bi` yields a `SinkEmpty` once,
/// then `ConnectionClosed`. Used by tests that need a `Connection` for
/// `CallConnection::new(conn)` or `adapter.handle(conn, &auth)` without
/// exercising the wire protocol — `SinkEmpty` reads EOF (so the dispatch
/// loop closes immediately) and discards writes.
pub(crate) fn sink_empty_connection() -> Connection {
    Connection::from_bidi(
        SinkEmpty,
        b"alknet/call".to_vec(),
        Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4321)),
    )
}
