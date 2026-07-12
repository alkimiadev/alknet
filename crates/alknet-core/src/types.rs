//! Core types: `ProtocolHandler`, `HandlerError`, `Connection`, `BiStream`,
//! `SendStream`, `RecvStream`, `StreamError`, `Capabilities`.
//!
//! See `docs/architecture/crates/core/core-types.md` for the full specification.

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::{Mutex, OnceLock};

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::auth::{AuthContext, Identity};

pub struct Secret<T: Zeroize + Clone> {
    inner: T,
}

impl<T: Zeroize + Clone> Secret<T> {
    pub fn new(value: T) -> Self {
        Self { inner: value }
    }

    pub fn expose_secret(&self) -> &T {
        &self.inner
    }
}

impl<T: Zeroize + Clone> Clone for Secret<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<T: Zeroize + Clone> Zeroize for Secret<T> {
    fn zeroize(&mut self) {
        self.inner.zeroize();
    }
}

impl<T: Zeroize + Clone> Drop for Secret<T> {
    fn drop(&mut self) {
        self.inner.zeroize();
    }
}

impl<T: Zeroize + Clone> std::fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

pub struct Capabilities {
    entries: HashMap<String, Secret<String>>,
}

impl Zeroize for Capabilities {
    fn zeroize(&mut self) {
        for (_, v) in self.entries.iter_mut() {
            v.zeroize();
        }
        self.entries.clear();
    }
}

impl ZeroizeOnDrop for Capabilities {}

impl Clone for Capabilities {
    fn clone(&self) -> Self {
        Self {
            entries: self.entries.clone(),
        }
    }
}

impl Capabilities {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub fn with_api_key(mut self, service: &str, key: String) -> Self {
        self.entries
            .insert(format!("api_key:{service}"), Secret::new(key));
        self
    }

    pub fn with_http_token(mut self, service: &str, token: String) -> Self {
        self.entries
            .insert(format!("http_token:{service}"), Secret::new(token));
        self
    }

    pub fn get(&self, service: &str) -> Option<&Secret<String>> {
        self.entries
            .get(&format!("api_key:{service}"))
            .or_else(|| self.entries.get(&format!("http_token:{service}")))
    }
}

impl Default for Capabilities {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Capabilities {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Capabilities")
            .field("entries", &format!("[{} redacted]", self.entries.len()))
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum IdentityAlreadySet {
    #[error("connection identity already set")]
    AlreadySet,
}

pub enum HandlerError {
    ConnectionClosed,
    StreamError(io::Error),
    AuthRequired,
    Internal(Box<dyn std::error::Error + Send + Sync>),
}

impl std::fmt::Debug for HandlerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConnectionClosed => f.write_str("HandlerError::ConnectionClosed"),
            Self::StreamError(e) => f.debug_tuple("HandlerError::StreamError").field(e).finish(),
            Self::AuthRequired => f.write_str("HandlerError::AuthRequired"),
            Self::Internal(e) => f.debug_tuple("HandlerError::Internal").field(e).finish(),
        }
    }
}

impl std::fmt::Display for HandlerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConnectionClosed => f.write_str("connection closed"),
            Self::StreamError(e) => write!(f, "stream error: {e}"),
            Self::AuthRequired => f.write_str("authentication required"),
            Self::Internal(e) => write!(f, "internal handler error: {e}"),
        }
    }
}

impl std::error::Error for HandlerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::StreamError(e) => Some(e),
            Self::Internal(e) => Some(e.as_ref()),
            _ => None,
        }
    }
}

pub enum StreamError {
    ConnectionClosed,
    StreamClosed,
    Timeout,
    Internal(io::Error),
}

impl From<StreamError> for HandlerError {
    fn from(e: StreamError) -> Self {
        match e {
            StreamError::ConnectionClosed => HandlerError::ConnectionClosed,
            StreamError::StreamClosed => HandlerError::StreamError(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "stream closed",
            )),
            StreamError::Timeout => HandlerError::StreamError(io::Error::new(
                io::ErrorKind::TimedOut,
                "stream timed out",
            )),
            StreamError::Internal(e) => HandlerError::StreamError(e),
        }
    }
}

impl std::fmt::Debug for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConnectionClosed => f.write_str("StreamError::ConnectionClosed"),
            Self::StreamClosed => f.write_str("StreamError::StreamClosed"),
            Self::Timeout => f.write_str("StreamError::Timeout"),
            Self::Internal(e) => f.debug_tuple("StreamError::Internal").field(e).finish(),
        }
    }
}

impl std::fmt::Display for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConnectionClosed => f.write_str("connection closed"),
            Self::StreamClosed => f.write_str("stream closed"),
            Self::Timeout => f.write_str("stream timed out"),
            Self::Internal(e) => write!(f, "stream error: {e}"),
        }
    }
}

impl std::error::Error for StreamError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Internal(e) => Some(e),
            _ => None,
        }
    }
}

#[async_trait]
pub trait ProtocolHandler: Send + Sync + 'static {
    fn alpn(&self) -> &'static [u8];
    async fn handle(&self, connection: Connection, auth: &AuthContext) -> Result<(), HandlerError>;
}

pub trait BiStream: AsyncRead + AsyncWrite + Send + Unpin {}

enum SendStreamKind {
    #[cfg(feature = "quinn")]
    Quinn(quinn::SendStream),
    #[cfg(feature = "iroh")]
    Iroh(iroh::endpoint::SendStream),
    Stream(Box<dyn AsyncWrite + Send + Unpin>),
}

enum RecvStreamKind {
    #[cfg(feature = "quinn")]
    Quinn(quinn::RecvStream),
    #[cfg(feature = "iroh")]
    Iroh(iroh::endpoint::RecvStream),
    Stream(Box<dyn AsyncRead + Send + Unpin>),
}

pub struct SendStream {
    kind: SendStreamKind,
}

pub struct RecvStream {
    kind: RecvStreamKind,
}

impl SendStream {
    #[cfg(feature = "quinn")]
    fn from_quinn(stream: quinn::SendStream) -> Self {
        Self {
            kind: SendStreamKind::Quinn(stream),
        }
    }

    #[cfg(feature = "iroh")]
    fn from_iroh(stream: iroh::endpoint::SendStream) -> Self {
        Self {
            kind: SendStreamKind::Iroh(stream),
        }
    }

    pub fn from_stream(stream: impl AsyncWrite + Send + Unpin + 'static) -> Self {
        Self {
            kind: SendStreamKind::Stream(Box::new(stream)),
        }
    }
}

impl RecvStream {
    #[cfg(feature = "quinn")]
    fn from_quinn(stream: quinn::RecvStream) -> Self {
        Self {
            kind: RecvStreamKind::Quinn(stream),
        }
    }

    #[cfg(feature = "iroh")]
    fn from_iroh(stream: iroh::endpoint::RecvStream) -> Self {
        Self {
            kind: RecvStreamKind::Iroh(stream),
        }
    }

    pub fn from_stream(stream: impl AsyncRead + Send + Unpin + 'static) -> Self {
        Self {
            kind: RecvStreamKind::Stream(Box::new(stream)),
        }
    }
}

impl AsyncWrite for SendStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        match &mut self.get_mut().kind {
            #[cfg(feature = "quinn")]
            SendStreamKind::Quinn(s) => AsyncWrite::poll_write(std::pin::Pin::new(s), cx, buf),
            #[cfg(feature = "iroh")]
            SendStreamKind::Iroh(s) => AsyncWrite::poll_write(std::pin::Pin::new(s), cx, buf),
            SendStreamKind::Stream(s) => {
                AsyncWrite::poll_write(std::pin::Pin::new(s.as_mut()), cx, buf)
            }
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match &mut self.get_mut().kind {
            #[cfg(feature = "quinn")]
            SendStreamKind::Quinn(s) => AsyncWrite::poll_flush(std::pin::Pin::new(s), cx),
            #[cfg(feature = "iroh")]
            SendStreamKind::Iroh(s) => AsyncWrite::poll_flush(std::pin::Pin::new(s), cx),
            SendStreamKind::Stream(s) => AsyncWrite::poll_flush(std::pin::Pin::new(s.as_mut()), cx),
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match &mut self.get_mut().kind {
            #[cfg(feature = "quinn")]
            SendStreamKind::Quinn(s) => AsyncWrite::poll_shutdown(std::pin::Pin::new(s), cx),
            #[cfg(feature = "iroh")]
            SendStreamKind::Iroh(s) => AsyncWrite::poll_shutdown(std::pin::Pin::new(s), cx),
            SendStreamKind::Stream(s) => AsyncWrite::poll_shutdown(std::pin::Pin::new(s), cx),
        }
    }
}

impl AsyncRead for RecvStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match &mut self.get_mut().kind {
            #[cfg(feature = "quinn")]
            RecvStreamKind::Quinn(s) => AsyncRead::poll_read(std::pin::Pin::new(s), cx, buf),
            #[cfg(feature = "iroh")]
            RecvStreamKind::Iroh(s) => AsyncRead::poll_read(std::pin::Pin::new(s), cx, buf),
            RecvStreamKind::Stream(s) => {
                AsyncRead::poll_read(std::pin::Pin::new(s.as_mut()), cx, buf)
            }
        }
    }
}

/// Yield bidirectional streams to a `Connection`. Downstream crates implement
/// this trait to add connection shapes (channels, a future transport, a test
/// double beyond the `from_stream` case) without editing `alknet-core`. See
/// ADR-070 for the full rationale and ADR-065 for the yield-once contract the
/// `StreamBidiStreamSource` impl preserves.
#[async_trait]
pub trait BidiStreamSource: Send + Sync + 'static {
    /// Yield the next bidirectional stream this connection provides.
    ///
    /// Transport semantics (carried from ADR-065):
    /// - QUIC (quinn/iroh): returns a new bidi stream on each call,
    ///   `ConnectionClosed` when the underlying connection closes.
    /// - Single-stream (TCP+TLS, SSH channel, WebTransport stream, wasm):
    ///   yields the underlying stream on the first call, then
    ///   `ConnectionClosed` on all subsequent calls.
    /// - Channels: yields one bidi stream per channel, `ConnectionClosed`
    ///   when the channels connection closes.
    async fn accept_bi(&self) -> Result<(SendStream, RecvStream), StreamError>;

    /// Open a bidirectional stream to the peer.
    ///
    /// Single-stream sources return `StreamClosed` (a single stream cannot
    /// open new application streams — ADR-065). QUIC and channels sources
    /// open new streams.
    async fn open_bi(&self) -> Result<(SendStream, RecvStream), StreamError>;

    /// The peer's address, if available. Informational (NAT/proxy).
    fn remote_addr(&self) -> Option<SocketAddr>;

    /// Close the connection. The `code`/`reason` args are QUIC application-
    /// level close codes; non-QUIC sources ignore them (the drop is the
    /// close — ADR-065 §"Negative"). See ADR-070 §"REQ-CORE-02" for the
    /// rationale for keeping the QUIC-shaped signature on the trait.
    fn close(&self, code: u32, reason: &str);
}

/// QUIC-backed `BidiStreamSource` (quinn). Crate-private; constructed via
/// `Connection::from_quinn` / `from_quinn_with_alpn` (feature `quinn`).
#[cfg(feature = "quinn")]
struct QuinnBidiStreamSource {
    conn: quinn::Connection,
}

#[cfg(feature = "quinn")]
#[async_trait]
impl BidiStreamSource for QuinnBidiStreamSource {
    async fn accept_bi(&self) -> Result<(SendStream, RecvStream), StreamError> {
        let (send, recv) = self
            .conn
            .accept_bi()
            .await
            .map_err(map_quinn_connection_error)?;
        Ok((SendStream::from_quinn(send), RecvStream::from_quinn(recv)))
    }

    async fn open_bi(&self) -> Result<(SendStream, RecvStream), StreamError> {
        let (send, recv) = self
            .conn
            .open_bi()
            .await
            .map_err(map_quinn_connection_error)?;
        Ok((SendStream::from_quinn(send), RecvStream::from_quinn(recv)))
    }

    fn remote_addr(&self) -> Option<SocketAddr> {
        Some(self.conn.remote_address())
    }

    fn close(&self, code: u32, reason: &str) {
        let code = quinn::VarInt::from(code);
        self.conn.close(code, reason.as_bytes());
    }
}

/// QUIC-backed `BidiStreamSource` (iroh). Crate-private; constructed via
/// `Connection::from_iroh` (feature `iroh`).
#[cfg(feature = "iroh")]
struct IrohBidiStreamSource {
    conn: iroh::endpoint::Connection,
}

#[cfg(feature = "iroh")]
#[async_trait]
impl BidiStreamSource for IrohBidiStreamSource {
    async fn accept_bi(&self) -> Result<(SendStream, RecvStream), StreamError> {
        let (send, recv) = self
            .conn
            .accept_bi()
            .await
            .map_err(map_iroh_connection_error)?;
        Ok((SendStream::from_iroh(send), RecvStream::from_iroh(recv)))
    }

    async fn open_bi(&self) -> Result<(SendStream, RecvStream), StreamError> {
        let (send, recv) = self
            .conn
            .open_bi()
            .await
            .map_err(map_iroh_connection_error)?;
        Ok((SendStream::from_iroh(send), RecvStream::from_iroh(recv)))
    }

    fn remote_addr(&self) -> Option<SocketAddr> {
        None
    }

    fn close(&self, code: u32, reason: &str) {
        let code = iroh::endpoint::VarInt::from(code);
        self.conn.close(code, reason.as_bytes());
    }
}

/// Single-stream `BidiStreamSource` (TCP+TLS, SSH channel, WebTransport
/// stream, wasm stream — ADR-065). Crate-private; constructed via
/// `Connection::from_stream` / `from_bidi` (no feature gate). `accept_bi`
/// yields the underlying stream once, then `ConnectionClosed`; `open_bi`
/// returns `StreamClosed`.
struct StreamBidiStreamSource {
    stream: Mutex<Option<(SendStream, RecvStream)>>,
    remote_addr: Option<SocketAddr>,
}

#[async_trait]
impl BidiStreamSource for StreamBidiStreamSource {
    async fn accept_bi(&self) -> Result<(SendStream, RecvStream), StreamError> {
        let mut guard = self.stream.lock().expect("stream mutex poisoned");
        match guard.take() {
            Some(pair) => Ok(pair),
            None => Err(StreamError::ConnectionClosed),
        }
    }

    async fn open_bi(&self) -> Result<(SendStream, RecvStream), StreamError> {
        Err(StreamError::StreamClosed)
    }

    fn remote_addr(&self) -> Option<SocketAddr> {
        self.remote_addr
    }

    /// `code`/`reason` are ignored: a single stream has no QUIC-shaped
    /// application-level close codes. The drop is the close (ADR-065
    /// §"Negative"). The `_` prefix is intentional — the signature matches
    /// the public `Connection::close` API (ADR-070 §"REQ-CORE-02").
    fn close(&self, _code: u32, _reason: &str) {
        let _ = self.stream.lock().expect("stream mutex poisoned").take();
    }
}

pub struct Connection {
    source: Box<dyn BidiStreamSource>,
    alpn: Vec<u8>,
    identity: OnceLock<Identity>,
}

impl Connection {
    #[cfg(feature = "quinn")]
    pub fn from_quinn(conn: quinn::Connection) -> Self {
        Self::from_quinn_with_alpn(conn, Vec::new())
    }

    #[cfg(feature = "quinn")]
    pub fn from_quinn_with_alpn(conn: quinn::Connection, alpn: Vec<u8>) -> Self {
        Self {
            source: Box::new(QuinnBidiStreamSource { conn }),
            alpn,
            identity: OnceLock::new(),
        }
    }

    #[cfg(feature = "iroh")]
    pub fn from_iroh(conn: iroh::endpoint::Connection) -> Self {
        let alpn = conn.alpn().to_vec();
        Self {
            source: Box::new(IrohBidiStreamSource { conn }),
            alpn,
            identity: OnceLock::new(),
        }
    }

    /// Construct a `Connection` from a pre-split read/write pair.
    /// `accept_bi()` yields this pair once, then returns `ConnectionClosed`.
    /// `open_bi()` returns `StreamClosed` (a single stream can't open new streams).
    pub fn from_stream(
        send: impl AsyncWrite + Send + Unpin + 'static,
        recv: impl AsyncRead + Send + Unpin + 'static,
        alpn: Vec<u8>,
        remote_addr: Option<SocketAddr>,
    ) -> Self {
        Self {
            source: Box::new(StreamBidiStreamSource {
                stream: Mutex::new(Some((
                    SendStream::from_stream(send),
                    RecvStream::from_stream(recv),
                ))),
                remote_addr,
            }),
            alpn,
            identity: OnceLock::new(),
        }
    }

    /// Convenience for a single bidirectional stream (e.g. `TlsStream<TcpStream>`).
    /// Splits internally via `tokio::io::split`.
    pub fn from_bidi(
        stream: impl AsyncRead + AsyncWrite + Send + Unpin + 'static,
        alpn: Vec<u8>,
        remote_addr: Option<SocketAddr>,
    ) -> Self {
        let (recv, send) = tokio::io::split(stream);
        Self::from_stream(send, recv, alpn, remote_addr)
    }

    /// Construct from a caller-supplied `BidiStreamSource` impl. The
    /// extension point for downstream crates — implement the trait and
    /// construct a `Connection` from it without editing core. See ADR-070.
    pub fn from_source(source: impl BidiStreamSource, alpn: Vec<u8>) -> Self {
        Self {
            source: Box::new(source),
            alpn,
            identity: OnceLock::new(),
        }
    }

    /// Yield the next bidirectional stream this connection provides.
    ///
    /// # Transport semantics
    ///
    /// - **QUIC (quinn/iroh)**: returns a new bidi stream on each call.
    ///   `ConnectionClosed` when the underlying connection closes.
    /// - **TCP+TLS / single-stream**: yields the underlying stream on the
    ///   first call, then `ConnectionClosed` on all subsequent calls.
    ///   A single transport stream cannot open new application streams.
    ///
    /// Handlers that loop `accept_bi` (e.g. `TtyAdapter`) get one session
    /// per single-stream connection; handlers that call once (e.g.
    /// `HttpAdapter`) get the stream directly. Both are correct.
    pub async fn accept_bi(&self) -> Result<(SendStream, RecvStream), StreamError> {
        self.source.accept_bi().await
    }

    pub async fn open_bi(&self) -> Result<(SendStream, RecvStream), StreamError> {
        self.source.open_bi().await
    }

    pub fn remote_alpn(&self) -> &[u8] {
        &self.alpn
    }

    pub fn remote_addr(&self) -> Option<SocketAddr> {
        self.source.remote_addr()
    }

    pub fn close(&self, code: u32, reason: &str) {
        self.source.close(code, reason)
    }

    pub fn set_identity(&self, identity: Identity) -> Result<(), IdentityAlreadySet> {
        self.identity
            .set(identity)
            .map_err(|_| IdentityAlreadySet::AlreadySet)
    }

    pub fn identity(&self) -> Option<&Identity> {
        self.identity.get()
    }
}

#[cfg(feature = "quinn")]
fn map_quinn_connection_error(e: quinn::ConnectionError) -> StreamError {
    use quinn::ConnectionError as E;
    match e {
        E::TimedOut => StreamError::Timeout,
        E::ConnectionClosed(_) | E::ApplicationClosed(_) | E::Reset => {
            StreamError::ConnectionClosed
        }
        other => StreamError::Internal(io::Error::other(other)),
    }
}

#[cfg(feature = "iroh")]
fn map_iroh_connection_error(e: iroh::endpoint::ConnectionError) -> StreamError {
    use iroh::endpoint::ConnectionError as E;
    match e {
        E::TimedOut => StreamError::Timeout,
        E::ConnectionClosed(_) | E::ApplicationClosed(_) | E::Reset => {
            StreamError::ConnectionClosed
        }
        other => StreamError::Internal(io::Error::other(other)),
    }
}

#[cfg(test)]
mod from_source_tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::Arc;

    /// A minimal custom `BidiStreamSource` impl to prove `from_source`
    /// delegates to a caller-supplied impl. Not a built-in — the whole
    /// point of `from_source` is that a non-core type can drive `Connection`.
    struct RecordingSource {
        stream: Mutex<Option<(SendStream, RecvStream)>>,
        addr: Option<SocketAddr>,
        closed: Arc<Mutex<Option<(u32, String)>>>,
    }

    #[async_trait]
    impl BidiStreamSource for RecordingSource {
        async fn accept_bi(&self) -> Result<(SendStream, RecvStream), StreamError> {
            match self.stream.lock().expect("mock mutex poisoned").take() {
                Some(pair) => Ok(pair),
                None => Err(StreamError::ConnectionClosed),
            }
        }

        async fn open_bi(&self) -> Result<(SendStream, RecvStream), StreamError> {
            Err(StreamError::StreamClosed)
        }

        fn remote_addr(&self) -> Option<SocketAddr> {
            self.addr
        }

        fn close(&self, code: u32, reason: &str) {
            let _ = self
                .closed
                .lock()
                .expect("mock closed mutex poisoned")
                .replace((code, reason.to_string()));
        }
    }

    #[tokio::test]
    async fn from_source_delegates_to_custom_impl() {
        use tokio::io::AsyncReadExt;
        use tokio::io::AsyncWriteExt;

        // One duplex: the mock holds end `a` (split into send_a/recv_a); the
        // test driver holds end `b` (split into send_b/recv_b) to echo back.
        let (a, b) = tokio::io::duplex(64);
        let (recv_a, send_a) = tokio::io::split(a);
        let (mut recv_b, mut send_b) = tokio::io::split(b);
        let addr = Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 7777));
        let recorded = Arc::new(Mutex::new(None));
        let conn = Connection::from_source(
            RecordingSource {
                stream: Mutex::new(Some((
                    SendStream::from_stream(send_a),
                    RecvStream::from_stream(recv_a),
                ))),
                addr,
                closed: Arc::clone(&recorded),
            },
            b"alknet/test".to_vec(),
        );

        // remote_alpn reads self.alpn (Connection-level, unchanged).
        assert_eq!(conn.remote_alpn(), b"alknet/test");

        // remote_addr delegates to RecordingSource::remote_addr.
        assert_eq!(conn.remote_addr(), addr);

        // accept_bi delegates to RecordingSource::accept_bi and yields the pair.
        let (mut send, mut recv) = conn.accept_bi().await.expect("first accept_bi yields");

        // Write via the mock's SendStream -> arrives at the driver's recv_b.
        send.write_all(b"hello").await.expect("write round-trips");
        let mut buf = [0u8; 5];
        recv_b.read_exact(&mut buf).await.expect("driver reads");
        assert_eq!(&buf, b"hello");

        // Driver writes back -> arrives at the mock's RecvStream.
        send_b
            .write_all(b"world")
            .await
            .expect("driver writes back");
        let mut buf = [0u8; 5];
        recv.read_exact(&mut buf).await.expect("read round-trips");
        assert_eq!(&buf, b"world");

        // Second accept_bi delegates to RecordingSource::accept_bi -> ConnectionClosed.
        match conn.accept_bi().await {
            Err(StreamError::ConnectionClosed) => {}
            Err(e) => panic!("expected ConnectionClosed on second accept_bi, got {e}"),
            Ok(_) => panic!("expected ConnectionClosed on second accept_bi, got a stream"),
        }

        // open_bi delegates to RecordingSource::open_bi -> StreamClosed.
        match conn.open_bi().await {
            Err(StreamError::StreamClosed) => {}
            Err(e) => panic!("expected StreamClosed from open_bi, got {e}"),
            Ok(_) => panic!("expected StreamClosed from open_bi, got a stream"),
        }

        // close delegates to RecordingSource::close and the args thread through.
        conn.close(42, "shutting down");
        assert_eq!(
            recorded.lock().expect("recorded mutex poisoned").take(),
            Some((42, "shutting down".to_string()))
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn test_connection() -> Connection {
        Connection::from_stream(
            tokio::io::sink(),
            tokio::io::empty(),
            b"alknet/test".to_vec(),
            Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1234)),
        )
    }

    #[test]
    fn capabilities_new_is_empty() {
        let caps = Capabilities::new();
        assert!(caps.get("google").is_none());
    }

    #[test]
    fn capabilities_with_api_key_then_get() {
        let caps = Capabilities::new().with_api_key("google", "sekrit".to_string());
        let secret = caps.get("google").expect("api key present");
        assert_eq!(secret.expose_secret(), "sekrit");
    }

    #[test]
    fn capabilities_with_http_token_then_get() {
        let caps = Capabilities::new().with_http_token("github", "tok".to_string());
        let secret = caps.get("github").expect("http token present");
        assert_eq!(secret.expose_secret(), "tok");
    }

    #[test]
    fn capabilities_clone_preserves_entries() {
        let caps = Capabilities::new().with_api_key("google", "k".to_string());
        let cloned = caps.clone();
        assert_eq!(
            cloned.get("google").map(|s| s.expose_secret().clone()),
            Some("k".to_string())
        );
        assert_eq!(
            caps.get("google").map(|s| s.expose_secret().clone()),
            Some("k".to_string())
        );
    }

    #[test]
    fn capabilities_zeroize_on_drop_clears_secret() {
        let mut secret = Secret::new("sensitive".to_string());
        secret.zeroize();
        assert_eq!(secret.expose_secret(), "");
    }

    #[test]
    fn capabilities_does_not_derive_serialize() {
        fn assert_not_serialize<T>() {}
        assert_not_serialize::<Capabilities>();
    }

    #[test]
    fn capabilities_debug_redacts_entries() {
        let caps = Capabilities::new().with_api_key("google", "sekrit".to_string());
        let s = format!("{:?}", caps);
        assert!(s.contains("redacted"));
        assert!(!s.contains("sekrit"));
    }

    #[test]
    fn secret_debug_redacts() {
        let secret = Secret::new("hidden".to_string());
        assert_eq!(format!("{:?}", secret), "[REDACTED]");
    }

    #[test]
    fn set_identity_once_succeeds_twice_errors() {
        let conn = test_connection();
        let id = Identity {
            id: "alk_test".to_string(),
            scopes: vec!["relay:connect".to_string()],
            resources: HashMap::new(),
        };
        assert!(conn.set_identity(id.clone()).is_ok());
        assert!(matches!(
            conn.set_identity(id),
            Err(IdentityAlreadySet::AlreadySet)
        ));
    }

    #[test]
    fn identity_get_returns_set_value() {
        let conn = test_connection();
        assert!(conn.identity().is_none());
        let id = Identity {
            id: "alk_test".to_string(),
            scopes: vec![],
            resources: HashMap::new(),
        };
        conn.set_identity(id.clone()).unwrap();
        assert_eq!(conn.identity(), Some(&id));
    }

    #[test]
    fn connection_remote_alpn_and_addr_from_stream() {
        let conn = test_connection();
        assert_eq!(conn.remote_alpn(), b"alknet/test");
        assert_eq!(
            conn.remote_addr(),
            Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1234))
        );
    }

    #[test]
    fn stream_error_maps_to_handler_error() {
        assert!(matches!(
            HandlerError::from(StreamError::ConnectionClosed),
            HandlerError::ConnectionClosed
        ));
        match HandlerError::from(StreamError::StreamClosed) {
            HandlerError::StreamError(e) => assert_eq!(e.kind(), io::ErrorKind::ConnectionReset),
            other => panic!("expected StreamError, got {other:?}"),
        }
        match HandlerError::from(StreamError::Timeout) {
            HandlerError::StreamError(e) => assert_eq!(e.kind(), io::ErrorKind::TimedOut),
            other => panic!("expected StreamError, got {other:?}"),
        }
        match HandlerError::from(StreamError::Internal(io::Error::other("x"))) {
            HandlerError::StreamError(e) => assert_eq!(e.kind(), io::ErrorKind::Other),
            other => panic!("expected StreamError, got {other:?}"),
        }
    }

    #[test]
    fn handler_error_auth_required_constructible() {
        let e = HandlerError::AuthRequired;
        assert_eq!(format!("{e}"), "authentication required");
    }

    // --- HandlerError / StreamError Debug + Display + source ---------------

    #[test]
    fn handler_error_debug_covers_all_variants() {
        assert_eq!(
            format!("{:?}", HandlerError::ConnectionClosed),
            "HandlerError::ConnectionClosed"
        );
        let io_err = io::Error::new(io::ErrorKind::BrokenPipe, "boom");
        let dbg = format!("{:?}", HandlerError::StreamError(io_err));
        assert!(dbg.contains("HandlerError::StreamError"));
        assert_eq!(
            format!("{:?}", HandlerError::AuthRequired),
            "HandlerError::AuthRequired"
        );
        let inner: Box<dyn std::error::Error + Send + Sync> = "oops".into();
        let dbg = format!("{:?}", HandlerError::Internal(inner));
        assert!(dbg.contains("HandlerError::Internal"));
    }

    #[test]
    fn handler_error_display_covers_all_variants() {
        assert_eq!(
            format!("{}", HandlerError::ConnectionClosed),
            "connection closed"
        );
        let io_err = io::Error::new(io::ErrorKind::BrokenPipe, "boom");
        let s = format!("{}", HandlerError::StreamError(io_err));
        assert!(s.starts_with("stream error: "));
        assert_eq!(
            format!("{}", HandlerError::AuthRequired),
            "authentication required"
        );
        let inner: Box<dyn std::error::Error + Send + Sync> = "oops".into();
        assert_eq!(
            format!("{}", HandlerError::Internal(inner)),
            "internal handler error: oops"
        );
    }

    #[test]
    fn handler_error_source_covers_all_variants() {
        use std::error::Error;
        assert!(HandlerError::ConnectionClosed.source().is_none());
        assert!(HandlerError::AuthRequired.source().is_none());
        let stream_err =
            HandlerError::StreamError(io::Error::new(io::ErrorKind::BrokenPipe, "boom"));
        assert!(
            stream_err.source().is_some(),
            "StreamError must expose its io::Error as source"
        );
        let internal_inner: Box<dyn std::error::Error + Send + Sync> = "boom".into();
        let internal_err = HandlerError::Internal(internal_inner);
        assert!(
            internal_err.source().is_some(),
            "Internal must expose its inner error as source"
        );
    }

    #[test]
    fn stream_error_debug_covers_all_variants() {
        assert_eq!(
            format!("{:?}", StreamError::ConnectionClosed),
            "StreamError::ConnectionClosed"
        );
        assert_eq!(
            format!("{:?}", StreamError::StreamClosed),
            "StreamError::StreamClosed"
        );
        assert_eq!(
            format!("{:?}", StreamError::Timeout),
            "StreamError::Timeout"
        );
        let dbg = format!("{:?}", StreamError::Internal(io::Error::other("x")));
        assert!(dbg.contains("StreamError::Internal"));
    }

    #[test]
    fn stream_error_display_covers_all_variants() {
        assert_eq!(
            format!("{}", StreamError::ConnectionClosed),
            "connection closed"
        );
        assert_eq!(format!("{}", StreamError::StreamClosed), "stream closed");
        assert_eq!(format!("{}", StreamError::Timeout), "stream timed out");
        assert_eq!(
            format!("{}", StreamError::Internal(io::Error::other("boom"))),
            "stream error: boom"
        );
    }

    #[test]
    fn stream_error_source_covers_all_variants() {
        use std::error::Error;
        assert!(StreamError::ConnectionClosed.source().is_none());
        assert!(StreamError::StreamClosed.source().is_none());
        assert!(StreamError::Timeout.source().is_none());
        let internal = StreamError::Internal(io::Error::other("x"));
        assert!(
            internal.source().is_some(),
            "Internal must expose its io::Error as source"
        );
    }

    // --- map_*_connection_error -------------------------------------------

    #[cfg(feature = "quinn")]
    #[test]
    fn map_quinn_connection_error_timed_out_maps_to_timeout() {
        assert!(matches!(
            map_quinn_connection_error(quinn::ConnectionError::TimedOut),
            StreamError::Timeout
        ));
    }

    #[cfg(feature = "quinn")]
    #[test]
    fn map_quinn_connection_error_reset_maps_to_connection_closed() {
        assert!(matches!(
            map_quinn_connection_error(quinn::ConnectionError::Reset),
            StreamError::ConnectionClosed
        ));
    }

    #[cfg(feature = "quinn")]
    #[test]
    fn map_quinn_connection_error_application_closed_maps_to_connection_closed() {
        use bytes::Bytes;
        let close = quinn::ConnectionError::ApplicationClosed(quinn::ApplicationClose {
            error_code: quinn::VarInt::from_u32(1),
            reason: Bytes::new(),
        });
        assert!(matches!(
            map_quinn_connection_error(close),
            StreamError::ConnectionClosed
        ));
    }

    #[cfg(feature = "quinn")]
    #[test]
    fn map_quinn_connection_error_other_maps_to_internal() {
        let other = quinn::ConnectionError::VersionMismatch;
        match map_quinn_connection_error(other) {
            StreamError::Internal(e) => assert_eq!(e.kind(), io::ErrorKind::Other),
            other => panic!("expected StreamError::Internal, got {other:?}"),
        }
    }

    #[cfg(feature = "iroh")]
    #[test]
    fn map_iroh_connection_error_timed_out_maps_to_timeout() {
        assert!(matches!(
            map_iroh_connection_error(iroh::endpoint::ConnectionError::TimedOut),
            StreamError::Timeout
        ));
    }

    #[cfg(feature = "iroh")]
    #[test]
    fn map_iroh_connection_error_reset_maps_to_connection_closed() {
        assert!(matches!(
            map_iroh_connection_error(iroh::endpoint::ConnectionError::Reset),
            StreamError::ConnectionClosed
        ));
    }

    #[cfg(feature = "iroh")]
    #[test]
    fn map_iroh_connection_error_application_closed_maps_to_connection_closed() {
        use bytes::Bytes;
        let close =
            iroh::endpoint::ConnectionError::ApplicationClosed(iroh::endpoint::ApplicationClose {
                error_code: iroh::endpoint::VarInt::from_u32(1),
                reason: Bytes::new(),
            });
        assert!(matches!(
            map_iroh_connection_error(close),
            StreamError::ConnectionClosed
        ));
    }

    #[cfg(feature = "iroh")]
    #[test]
    fn map_iroh_connection_error_other_maps_to_internal() {
        let other = iroh::endpoint::ConnectionError::VersionMismatch;
        match map_iroh_connection_error(other) {
            StreamError::Internal(e) => assert_eq!(e.kind(), io::ErrorKind::Other),
            other => panic!("expected StreamError::Internal, got {other:?}"),
        }
    }

    // --- Capabilities zeroize + default -----------------------------------

    #[test]
    fn capabilities_default_is_empty() {
        let caps = Capabilities::default();
        assert!(caps.get("anything").is_none());
    }

    #[test]
    fn capabilities_zeroize_clears_entries() {
        let mut caps = Capabilities::new()
            .with_api_key("svc-a", "k1".to_string())
            .with_http_token("svc-b", "t1".to_string());
        assert!(caps.get("svc-a").is_some());
        assert!(caps.get("svc-b").is_some());
        caps.zeroize();
        assert!(caps.get("svc-a").is_none());
        assert!(caps.get("svc-b").is_none());
    }
}
