//! # alknet-core
//!
//! Core library for [Alknet](https://git.alk.dev/alkdev/alknet), a self-hostable SSH-based
//! tunnel tool. This crate provides the transport abstraction, SOCKS5 server, port forwarding,
//! authentication, and server handler — everything needed to build an alknet client or server
//! on top of pluggable transports.
//!
//! > **Alpha software.** This crate depends on solid libraries (russh, tokio, rustls, iroh)
//! > for core functionality, but the integration layer has not been battle-tested. Use with
//! > caution and report issues.
//!
//! # Key concepts
//!
//! - **Transport trait** — produces a duplex byte stream (`AsyncRead + AsyncWrite + Unpin + Send`)
//!   that SSH consumes. Implementations: TCP, TLS, iroh (QUIC P2P).
//! - **SOCKS5 server** — the primary client interface, listening on a local port and routing
//!   traffic through SSH channels.
//! - **Port forwarding** — `-L` local and `-R` remote port forwards over SSH channels.
//! - **Authentication** — Ed25519 public key and OpenSSH certificate authority. No passwords.
//! - **Server handler** — accepts SSH connections via a `TransportAcceptor` and proxies
//!   `direct-tcpip` channel requests to targets (directly or via outbound proxy).
//!
//! # Feature flags
//!
//! | Feature | Default | Description |
//! |---------|---------|-------------|
//! | `tls` | yes | TLS transport via `tokio-rustls` |
//! | `iroh` | yes | iroh QUIC P2P transport |
//! | `acme` | no | ACME/Let's Encrypt auto-cert provisioning (implies `tls`) |
//! | `testutil` | no | Test utilities (for internal use) |
//!
//! # Quick example
//!
//! ```no_run
//! use std::sync::Arc;
//! use alknet_core::transport::TcpTransport;
//! use alknet_core::client::{ClientSession, ConnectOptions, TransportMode};
//! use alknet_core::auth::keys::KeySource;
//! use alknet_core::Transport;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let opts = ConnectOptions::new(KeySource::File("/path/to/key".into()))
//!         .server("example.com:22")
//!         .transport_mode(TransportMode::Tcp);
//!     let transport = Arc::new(TcpTransport::new("example.com:22".parse()?));
//!     let session = ClientSession::new(opts, transport).await?;
//!     session.run().await?;
//!     Ok(())
//! }
//! ```

pub mod auth;
pub mod client;
pub mod config;
pub mod error;
pub mod server;
pub mod socks5;
pub mod transport;

#[cfg(feature = "testutil")]
pub mod testutil;

pub use auth::{AuthToken, ConfigIdentityProvider, Identity, IdentityProvider};
pub use client::channel_manager::{ChannelManager, ForwardRequest};
pub use client::connect::{ClientSession, ConnectError, ConnectOptions, TransportMode};
pub use config::{
    AuthPolicy, ConfigReloadHandle, ConfigServiceImpl, DynamicConfig, ForwardingAction,
    ForwardingPolicy, ForwardingRule, RateLimitConfig, StaticConfig,
};
pub use error::{AuthError, ChannelError, ConfigError, ForwardError, TransportError};
pub use server::serve::{ListenerConfig, ServeError, ServeOptions, ServeTransportMode, Server};
pub use transport::{Transport, TransportAcceptor, TransportInfo, TransportKind};
