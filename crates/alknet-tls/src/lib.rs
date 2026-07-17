//! alknet-tls: TLS setup types for alknet — server config, client config,
//! verifiers, cert resolvers, and shared signing helpers.
//!
//! Provides `TlsServerConfig` (server-side TLS setup) and `TlsClientConfig`
//! (client-side TLS setup), both transport-agnostic. Transport-specific
//! conversion (e.g. `for_quinn()`) is feature-gated.

pub mod client;
pub mod pem;
pub mod server;
pub mod signing;

use std::io;

#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    #[error("TLS config error: {0}")]
    Config(String),
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("certificate error: {0}")]
    Cert(String),
}
