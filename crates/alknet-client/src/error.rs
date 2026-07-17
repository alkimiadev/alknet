//! `ClientDialError` — error type for all three dial methods.

use thiserror::Error;

/// Errors produced by `AlknetClient` dial methods.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ClientDialError {
    /// TLS config construction failure — `TlsClientConfig::new` failed
    /// (verifier build, cert load, provider init). Wraps `TlsError`
    /// from alknet-tls.
    #[error("TLS config construction: {0}")]
    TlsConfig(#[from] alknet_tls::TlsError),

    /// Transport connect failure — quinn connect, TcpStream::connect,
    /// or iroh connect. The transport's own error type, stringified.
    #[error("transport connect: {0}")]
    Connect(String),

    /// TLS handshake failure — the handshake started but failed
    /// (rejected cert, ALPN mismatch, unknown raw-key remote
    /// fail-closed). Distinct from TlsConfig (which is pre-handshake).
    #[error("TLS handshake: {0}")]
    Handshake(String),

    /// No transport handle configured for the requested dial — e.g.,
    /// `dial_quic` called but `with_quinn` was not set.
    #[error("no transport handle configured for {transport}")]
    NoTransport { transport: &'static str },

    /// SOCKS5 proxy failure — handshake rejected, UDP ASSOCIATE
    /// unsupported, auth failed, or the proxy closed the control
    /// connection (ADR-090). The dial did not reach the remote; the
    /// caller decides whether to fall back to a direct dial or
    /// surface the error. The dial never silently falls back — that
    /// would defeat the privacy posture.
    #[cfg(feature = "socks5")]
    #[error("SOCKS5 proxy: {0}")]
    Proxy(String),
}
