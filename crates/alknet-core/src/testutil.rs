use tokio::io::{DuplexStream, AsyncRead, AsyncWrite};
use anyhow::Result;

#[cfg(feature = "transport-traits")]
pub use crate::transport::{Transport, TransportAcceptor, TransportInfo, TransportKind};

#[cfg(not(feature = "transport-traits"))]
pub use local_traits::{Transport, TransportAcceptor, TransportInfo, TransportKind};

#[cfg(not(feature = "transport-traits"))]
mod local_traits {
    use std::net::SocketAddr;
    use anyhow::Result;
    use tokio::io::{AsyncRead, AsyncWrite};
    use async_trait::async_trait;

    #[async_trait]
    pub trait Transport: Send + Sync + 'static {
        type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;
        async fn connect(&self) -> Result<Self::Stream>;
        fn describe(&self) -> String;
    }

    #[async_trait]
    pub trait TransportAcceptor: Send + Sync + 'static {
        type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;
        async fn accept(&self) -> Result<(Self::Stream, TransportInfo)>;
    }

    #[derive(Debug, Clone)]
    pub struct TransportInfo {
        pub remote_addr: Option<SocketAddr>,
        pub transport_kind: TransportKind,
    }

    #[derive(Debug, Clone)]
    pub enum TransportKind {
        Tcp,
        Tls { server_name: Option<String> },
        Iroh { endpoint_id: String },
    }
}

pub struct MockStream {
    inner: DuplexStream,
}

impl MockStream {
    pub fn new(inner: DuplexStream) -> Self {
        Self { inner }
    }
}

impl AsyncRead for MockStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for MockStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

impl Unpin for MockStream {}

pub struct MockTransport {
    buf_size: usize,
}

impl MockTransport {
    pub fn new(buf_size: usize) -> Self {
        Self { buf_size }
    }
}

#[async_trait::async_trait]
impl Transport for MockTransport {
    type Stream = MockStream;

    async fn connect(&self) -> Result<Self::Stream> {
        let (client, _) = tokio::io::duplex(self.buf_size);
        Ok(MockStream::new(client))
    }

    fn describe(&self) -> String {
        "mock".to_string()
    }
}

pub struct MockTransportAcceptor {
    buf_size: usize,
}

impl MockTransportAcceptor {
    pub fn new(buf_size: usize) -> Self {
        Self { buf_size }
    }
}

#[async_trait::async_trait]
impl TransportAcceptor for MockTransportAcceptor {
    type Stream = MockStream;

    async fn accept(&self) -> Result<(Self::Stream, TransportInfo)> {
        let (_, server) = tokio::io::duplex(self.buf_size);
        let info = TransportInfo {
            remote_addr: None,
            transport_kind: TransportKind::Tcp,
        };
        Ok((MockStream::new(server), info))
    }
}

pub fn mock_pair(buf_size: usize) -> (MockStream, MockStream) {
    let (client, server) = tokio::io::duplex(buf_size);
    (MockStream::new(client), MockStream::new(server))
}