//! Interface layer (Layer 2) of the three-layer model (ADR-026).
//!
//! The Interface layer sits between Transport (Layer 1) and Protocol (Layer 3).
//! An Interface consumes a `TransportStream` and produces call protocol sessions
//! that yield `EventEnvelope` frames. This enables the call protocol handler to be
//! interface-agnostic — it receives `InterfaceEvent` frames from any interface.
//!
//! SSH is an interface, not a transport. It wraps a byte stream in session
//! semantics (handshake, auth, channel multiplexing). Raw framing (4-byte length
//! prefix + JSON `EventEnvelope`) is another interface, one without SSH overhead.
//!
//! # OQ-IF-01 Resolution
//!
//! Every Interface session implements the `InterfaceSession` trait, which provides
//! `recv()` and `send()` methods producing and consuming `InterfaceEvent` frames.
//! Each `InterfaceEvent` carries an `EventEnvelope` and an optional `Identity`
//! (authenticated by the interface layer, e.g., via SSH public key auth or
//! transport-level token auth).
//!
//! This means the call protocol handler (Layer 3) is completely interface-agnostic:
//! it receives `InterfaceEvent` frames and processes them uniformly, regardless
//! of whether they arrived over SSH or raw framing.

pub mod config;
pub mod pairs;
pub mod session;

use anyhow::Result;
use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};

pub use config::{InterfaceConfig, InterfaceKind, RawFramingConfig, SshInterfaceConfig};
pub use pairs::{is_valid_pair, TransportKindBase, VALID_TRANSPORT_INTERFACE_PAIRS};
pub use session::{InterfaceEvent, InterfaceSession};

pub trait TransportStream: AsyncRead + AsyncWrite + Unpin + Send + 'static {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send + 'static> TransportStream for T {}

#[async_trait]
pub trait Interface: Send + Sync + 'static {
    type Session: InterfaceSession;

    async fn accept(
        &self,
        stream: Box<dyn TransportStream>,
        config: &InterfaceConfig,
    ) -> Result<Self::Session>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[test]
    fn transport_stream_trait_bounds() {
        fn assert_transport_stream<S: TransportStream>() {}
        assert_transport_stream::<tokio::io::DuplexStream>();
    }

    #[tokio::test]
    async fn transport_stream_from_duplex() {
        let (client, server) = duplex(1024);
        let _boxed: Box<dyn TransportStream> = Box::new(server);
        let _: Box<dyn TransportStream> = Box::new(client);
    }
}
