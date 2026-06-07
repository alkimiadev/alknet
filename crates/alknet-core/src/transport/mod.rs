//! Pluggable transport layer for Alknet.
//!
//! The transport layer produces a duplex byte stream (`AsyncRead + AsyncWrite + Unpin + Send`)
//! that SSH consumes. This is the core architectural abstraction — SSH never opens its own
//! network connections; it runs entirely over whatever stream the transport provides.
//!
//! Available transports (feature-gated):
//! - `TcpTransport` / `TcpAcceptor` — always available, direct TCP
//! - `TlsTransport` / `TlsAcceptor` — behind the `tls` feature, TCP + rustls
//! - `IrohTransport` / `IrohAcceptor` — behind the `iroh` feature, QUIC P2P via iroh
//! - `AcmeTlsAcceptor` — behind the `acme` feature, auto-provision TLS certs via Let's Encrypt
//!
//! See [ADR-001](docs/architecture/decisions/001-pluggable-transport.md) and
//! [ADR-004](docs/architecture/decisions/004-ssh-over-transport.md) for design rationale.

#[cfg(feature = "iroh")]
mod iroh_transport;
mod tcp;

#[cfg(feature = "iroh")]
pub use iroh_transport::{IrohAcceptor, IrohTransport, ALPN as IROH_ALPN};
pub use tcp::{TcpAcceptor, TcpTransport};

#[cfg(feature = "tls")]
mod tls;

#[cfg(feature = "tls")]
pub use tls::{AcmeConfig, TlsAcceptor, TlsTransport};

#[cfg(feature = "acme")]
mod acme;

#[cfg(feature = "acme")]
pub use acme::{AcmeCertProvider, AcmeMode, AcmeTlsAcceptor};

use std::net::SocketAddr;

use anyhow::Result;
use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};

/// Client-side transport trait. Produces a single duplex stream per connection.
///
/// Implementations connect to a remote endpoint and return a stream that SSH
/// runs over via `russh::client::connect_stream()`. Each call to `connect()` creates
/// a new stream — multiple sessions need multiple calls or multiple transports.
#[async_trait]
pub trait Transport: Send + Sync + 'static {
    /// The duplex stream type produced by this transport.
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    /// Connect to the remote endpoint and return a duplex stream.
    async fn connect(&self) -> Result<Self::Stream>;

    /// Return a human-readable description of this transport for logging.
    fn describe(&self) -> String;
}

/// Server-side transport acceptor. Accepts incoming connections and returns streams.
///
/// Implementations bind to a local endpoint and produce streams that SSH
/// runs over via `russh::server::run_stream()`.
#[async_trait]
pub trait TransportAcceptor: Send + Sync + 'static {
    /// The duplex stream type produced by this acceptor.
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    /// Accept an incoming connection and return a duplex stream with metadata.
    async fn accept(&self) -> Result<(Self::Stream, TransportInfo)>;
}

/// Metadata about an incoming transport connection.
///
/// Carries the remote address (if available) and the kind of transport
/// used. The server handler uses this for logging and auth decisions.
/// See ADR-001 for the pluggable transport rationale and ADR-004
/// for why SSH runs entirely over the transport stream.
#[derive(Debug, Clone)]
pub struct TransportInfo {
    pub remote_addr: Option<SocketAddr>,
    pub transport_kind: TransportKind,
}

/// The kind of transport that produced a connection.
///
/// Each variant identifies the transport mechanism. Used by the
/// server handler for logging and authorization decisions.
/// See ADR-001 and ADR-004.
#[derive(Debug, Clone)]
pub enum TransportKind {
    Tcp,
    Tls { server_name: Option<String> },
    Iroh { endpoint_id: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{duplex, DuplexStream};

    struct MockTransport;

    #[async_trait]
    impl Transport for MockTransport {
        type Stream = DuplexStream;

        async fn connect(&self) -> Result<Self::Stream> {
            let (stream, _) = duplex(1024);
            Ok(stream)
        }

        fn describe(&self) -> String {
            "mock".to_string()
        }
    }

    struct MockAcceptor;

    #[async_trait]
    impl TransportAcceptor for MockAcceptor {
        type Stream = DuplexStream;

        async fn accept(&self) -> Result<(Self::Stream, TransportInfo)> {
            let (stream, _) = duplex(1024);
            let info = TransportInfo {
                remote_addr: None,
                transport_kind: TransportKind::Tcp,
            };
            Ok((stream, info))
        }
    }

    #[tokio::test]
    async fn transport_trait_object() {
        let _boxed: Box<dyn Transport<Stream = DuplexStream>> = Box::new(MockTransport);
    }

    #[tokio::test]
    async fn transport_acceptor_trait_object() {
        let _boxed: Box<dyn TransportAcceptor<Stream = DuplexStream>> = Box::new(MockAcceptor);
    }

    #[tokio::test]
    async fn transport_connect_returns_stream() {
        let t = MockTransport;
        let _stream = t.connect().await.unwrap();
    }

    #[tokio::test]
    async fn transport_describe_returns_string() {
        let t = MockTransport;
        assert_eq!(t.describe(), "mock");
    }

    #[tokio::test]
    async fn acceptor_accept_returns_stream_and_info() {
        let a = MockAcceptor;
        let (_, info) = a.accept().await.unwrap();
        assert!(info.remote_addr.is_none());
        assert!(matches!(info.transport_kind, TransportKind::Tcp));
    }

    #[test]
    fn transport_kind_variants() {
        let tcp = TransportKind::Tcp;
        let tls = TransportKind::Tls {
            server_name: Some("example.com".to_string()),
        };
        let iroh = TransportKind::Iroh {
            endpoint_id: "abc123".to_string(),
        };

        if let TransportKind::Tcp = tcp {}
        if let TransportKind::Tls {
            server_name: Some(name),
        } = tls
        {
            assert_eq!(name, "example.com");
        }
        if let TransportKind::Iroh { endpoint_id } = iroh {
            assert_eq!(endpoint_id, "abc123");
        }
    }
}
