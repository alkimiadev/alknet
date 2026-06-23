//! Core types: `ProtocolHandler`, `HandlerError`, `Connection`, `BiStream`,
//! `SendStream`, `RecvStream`, `StreamError`, `Capabilities`.
//!
//! See `docs/architecture/crates/core/core-types.md` for the full specification.

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};

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
    Mock(Box<dyn AsyncWrite + Send + Unpin>),
}

enum RecvStreamKind {
    #[cfg(feature = "quinn")]
    Quinn(quinn::RecvStream),
    #[cfg(feature = "iroh")]
    Iroh(iroh::endpoint::RecvStream),
    Mock(Box<dyn AsyncRead + Send + Unpin>),
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

    #[allow(dead_code)]
    pub fn from_mock(stream: impl AsyncWrite + Send + Unpin + 'static) -> Self {
        Self {
            kind: SendStreamKind::Mock(Box::new(stream)),
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

    #[allow(dead_code)]
    pub fn from_mock(stream: impl AsyncRead + Send + Unpin + 'static) -> Self {
        Self {
            kind: RecvStreamKind::Mock(Box::new(stream)),
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
            SendStreamKind::Mock(s) => {
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
            SendStreamKind::Mock(s) => AsyncWrite::poll_flush(std::pin::Pin::new(s.as_mut()), cx),
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
            SendStreamKind::Mock(s) => {
                AsyncWrite::poll_shutdown(std::pin::Pin::new(s.as_mut()), cx)
            }
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
            RecvStreamKind::Mock(s) => {
                AsyncRead::poll_read(std::pin::Pin::new(s.as_mut()), cx, buf)
            }
        }
    }
}

enum ConnectionKind {
    #[cfg(feature = "quinn")]
    Quinn(quinn::Connection),
    #[cfg(feature = "iroh")]
    Iroh(iroh::endpoint::Connection),
    Mock(Arc<dyn MockConnection + Send + Sync>),
}

#[allow(dead_code)]
pub trait MockConnection: Send + Sync {
    fn remote_alpn(&self) -> &[u8];
    fn remote_addr(&self) -> Option<SocketAddr>;
    fn close(&self, code: u32, reason: &str);
}

pub struct Connection {
    kind: ConnectionKind,
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
            kind: ConnectionKind::Quinn(conn),
            alpn,
            identity: OnceLock::new(),
        }
    }

    #[cfg(feature = "iroh")]
    pub fn from_iroh(conn: iroh::endpoint::Connection) -> Self {
        let alpn = conn.alpn().unwrap_or_default();
        Self {
            kind: ConnectionKind::Iroh(conn),
            alpn,
            identity: OnceLock::new(),
        }
    }

    #[allow(dead_code)]
    pub fn from_mock(mock: Arc<dyn MockConnection + Send + Sync>) -> Self {
        let alpn = mock.remote_alpn().to_vec();
        Self {
            kind: ConnectionKind::Mock(mock),
            alpn,
            identity: OnceLock::new(),
        }
    }

    pub async fn accept_bi(&self) -> Result<(SendStream, RecvStream), StreamError> {
        match &self.kind {
            #[cfg(feature = "quinn")]
            ConnectionKind::Quinn(c) => {
                let (send, recv) = c.accept_bi().await.map_err(map_quinn_connection_error)?;
                Ok((SendStream::from_quinn(send), RecvStream::from_quinn(recv)))
            }
            #[cfg(feature = "iroh")]
            ConnectionKind::Iroh(c) => {
                let (send, recv) = c.accept_bi().await.map_err(map_iroh_connection_error)?;
                Ok((SendStream::from_iroh(send), RecvStream::from_iroh(recv)))
            }
            ConnectionKind::Mock(_) => Err(StreamError::StreamClosed),
        }
    }

    pub async fn open_bi(&self) -> Result<(SendStream, RecvStream), StreamError> {
        match &self.kind {
            #[cfg(feature = "quinn")]
            ConnectionKind::Quinn(c) => {
                let (send, recv) = c.open_bi().await.map_err(map_quinn_connection_error)?;
                Ok((SendStream::from_quinn(send), RecvStream::from_quinn(recv)))
            }
            #[cfg(feature = "iroh")]
            ConnectionKind::Iroh(c) => {
                let (send, recv) = c.open_bi().await.map_err(map_iroh_connection_error)?;
                Ok((SendStream::from_iroh(send), RecvStream::from_iroh(recv)))
            }
            ConnectionKind::Mock(_) => Err(StreamError::StreamClosed),
        }
    }

    pub fn remote_alpn(&self) -> &[u8] {
        &self.alpn
    }

    pub fn remote_addr(&self) -> Option<SocketAddr> {
        match &self.kind {
            #[cfg(feature = "quinn")]
            ConnectionKind::Quinn(c) => Some(c.remote_address()),
            #[cfg(feature = "iroh")]
            ConnectionKind::Iroh(_) => None,
            ConnectionKind::Mock(m) => m.remote_addr(),
        }
    }

    pub fn close(&self, code: u32, reason: &str) {
        match &self.kind {
            #[cfg(feature = "quinn")]
            ConnectionKind::Quinn(c) => {
                let code = quinn::VarInt::from(code);
                c.close(code, reason.as_bytes());
            }
            #[cfg(feature = "iroh")]
            ConnectionKind::Iroh(c) => {
                let code = iroh::endpoint::VarInt::from(code);
                c.close(code, reason.as_bytes());
            }
            ConnectionKind::Mock(m) => m.close(code, reason),
        }
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
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    struct MockConn {
        alpn: &'static [u8],
        addr: Option<SocketAddr>,
        closed: std::sync::Mutex<Option<(u32, String)>>,
    }

    #[allow(dead_code)]
    impl MockConnection for MockConn {
        fn remote_alpn(&self) -> &[u8] {
            self.alpn
        }
        fn remote_addr(&self) -> Option<SocketAddr> {
            self.addr
        }
        fn close(&self, code: u32, reason: &str) {
            *self.closed.lock().unwrap() = Some((code, reason.to_string()));
        }
    }

    fn mock_connection() -> Connection {
        Connection::from_mock(Arc::new(MockConn {
            alpn: b"alknet/test",
            addr: Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1234)),
            closed: std::sync::Mutex::new(None),
        }))
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
        let conn = mock_connection();
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
        let conn = mock_connection();
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
    fn connection_remote_alpn_and_addr_from_mock() {
        let conn = mock_connection();
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
}
