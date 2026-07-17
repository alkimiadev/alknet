//! alknet-client: Native client dial seam — multi-transport dialer that
//! produces `Connection`s for protocol take-overs.
//!
//! `AlknetClient` is the client-side analogue of `AlknetEndpoint`: a
//! multi-transport dialer that takes pre-built transport handles (quinn,
//! TCP+TLS, iroh), dials a remote `AlknetEndpoint` on a chosen ALPN, and
//! produces a `Connection`. The protocol take-overs
//! (`CallClient::spawn_dispatch`, `ChannelClient::from_connection`)
//! consume the `Connection` — the dial is below the protocol.
//!
//! An optional SOCKS5 proxy (ADR-090) routes the dials through a proxy
//! to hide the client's real IP from the hub.

pub mod client;
pub mod dial;
pub mod error;
#[cfg(feature = "socks5")]
pub mod socks5;

pub use client::AlknetClient;
pub use error::ClientDialError;
#[cfg(feature = "socks5")]
pub use socks5::{Socks5Credentials, Socks5ProxyConfig};
