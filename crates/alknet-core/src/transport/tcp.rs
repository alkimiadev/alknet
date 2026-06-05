use std::net::SocketAddr;

use anyhow::Result;
use async_trait::async_trait;
use tokio::net::{TcpListener, TcpStream};

use super::{Transport, TransportAcceptor, TransportInfo, TransportKind};

/// A TCP-based client transport that connects to a remote address.
///
/// Connects via `TcpStream::connect(addr)`. Uses tokio's default
/// connect timeout behavior: the OS controls connection timeout
/// (typically ~2 minutes on Linux via `net.ipv4.tcp_syn_retries`).
/// For custom timeouts, wrap `TcpTransport` with
/// `tokio::time::timeout(duration, transport.connect())`.
pub struct TcpTransport {
    addr: SocketAddr,
}

impl TcpTransport {
    pub fn new(addr: SocketAddr) -> Self {
        Self { addr }
    }
}

#[async_trait]
impl Transport for TcpTransport {
    type Stream = TcpStream;

    async fn connect(&self) -> Result<Self::Stream> {
        let stream = TcpStream::connect(self.addr).await?;
        Ok(stream)
    }

    fn describe(&self) -> String {
        format!("tcp://{}", self.addr)
    }
}

/// A TCP-based server transport acceptor that listens for incoming connections.
///
/// Binds via `TcpListener::bind(addr)`. Accepts connections and returns
/// the stream together with `TransportInfo` containing the remote address
/// and `TransportKind::Tcp`.
pub struct TcpAcceptor {
    listener: TcpListener,
    listen_addr: SocketAddr,
}

impl TcpAcceptor {
    /// Bind a TCP listener on the given address.
    ///
    /// Returns the acceptor ready to receive connections.
    /// The actual bound address may differ from the requested one
    /// (e.g., when binding to port 0 the OS assigns an ephemeral port).
    pub async fn bind(addr: SocketAddr) -> Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        let listen_addr = listener.local_addr()?;
        Ok(Self {
            listener,
            listen_addr,
        })
    }

    pub fn listen_addr(&self) -> SocketAddr {
        self.listen_addr
    }
}

#[async_trait]
impl TransportAcceptor for TcpAcceptor {
    type Stream = TcpStream;

    async fn accept(&self) -> Result<(Self::Stream, TransportInfo)> {
        let (stream, remote_addr) = self.listener.accept().await?;
        let info = TransportInfo {
            remote_addr: Some(remote_addr),
            transport_kind: TransportKind::Tcp,
        };
        Ok((stream, info))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn tcp_transport_connect_creates_stream() {
        let acceptor = TcpAcceptor::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let addr = acceptor.listen_addr();
        let transport = TcpTransport::new(addr);

        let accept_handle = tokio::spawn(async move { acceptor.accept().await.unwrap() });

        let stream = transport.connect().await.unwrap();
        assert_eq!(stream.local_addr().unwrap().ip(), addr.ip());

        let (_server_stream, info) = accept_handle.await.unwrap();
        assert!(info.remote_addr.is_some());
        assert!(matches!(info.transport_kind, TransportKind::Tcp));
    }

    #[tokio::test]
    async fn tcp_acceptor_accept_receives_connection() {
        let acceptor = TcpAcceptor::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let addr = acceptor.listen_addr();

        tokio::spawn(async move {
            TcpStream::connect(addr).await.unwrap();
        });

        let (stream, info) = acceptor.accept().await.unwrap();
        assert!(info.remote_addr.is_some());
        assert!(matches!(info.transport_kind, TransportKind::Tcp));
        assert_eq!(
            info.remote_addr.unwrap().ip(),
            stream.peer_addr().unwrap().ip()
        );
    }

    #[test]
    fn tcp_transport_describe_format() {
        let addr: SocketAddr = "1.2.3.4:22".parse().unwrap();
        let transport = TcpTransport::new(addr);
        assert_eq!(transport.describe(), "tcp://1.2.3.4:22");
    }

    #[tokio::test]
    async fn tcp_stream_is_duplex() {
        let acceptor = TcpAcceptor::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let addr = acceptor.listen_addr();

        let mut client = TcpStream::connect(addr).await.unwrap();
        let (mut server, _) = acceptor.accept().await.unwrap();

        client.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 5];
        server.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");

        server.write_all(b"world").await.unwrap();
        let mut buf = [0u8; 5];
        client.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"world");
    }

    #[tokio::test]
    async fn tcp_acceptor_bind_port_zero_assigns_ephemeral() {
        let acceptor = TcpAcceptor::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        assert_ne!(acceptor.listen_addr().port(), 0);
    }
}