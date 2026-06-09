//! Interface layer (Layer 2) of the three-layer model (ADR-026, ADR-035).
//!
//! The Interface layer sits between Transport (Layer 1) and Protocol (Layer 3).
//! It has two distinct patterns:
//!
//! - **StreamInterface** — consumes a `TransportStream`, produces a long-lived
//!   `Session` that yields `InterfaceEvent` frames. SSH and raw framing are
//!   `StreamInterface` implementations.
//!
//! - **MessageInterface** — handles individual `InterfaceRequest` →
//!   `InterfaceResponse` pairs. Manages its own transport (HTTP server, DNS
//!   server). HTTP and DNS are `MessageInterface` implementations.

pub mod config;
pub mod dns;
pub mod http;
pub mod pairs;
pub mod raw_framing;
pub mod session;
pub mod ssh;

use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};

pub use config::{
    DnsInterfaceConfig, HttpInterfaceConfig, InterfaceConfig, MessageInterfaceConfig,
    MessageInterfaceKind, RawFramingConfig, SshInterfaceConfig, StreamInterfaceConfig,
    StreamInterfaceKind,
};
pub use dns::DnsInterface;
pub use http::HttpInterface;
pub use pairs::{is_valid_pair, TransportKindBase, VALID_TRANSPORT_INTERFACE_PAIRS};
pub use raw_framing::{RawFramingInterface, RawFramingSession};
pub use session::{InterfaceEvent, InterfaceSession};
pub use ssh::{SshInterface, SshSession};

pub trait TransportStream: AsyncRead + AsyncWrite + Unpin + Send + 'static {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send + 'static> TransportStream for T {}

#[async_trait]
pub trait StreamInterface: Send + Sync + 'static {
    type Session: InterfaceSession;

    async fn accept(
        &self,
        stream: Box<dyn TransportStream>,
        config: &StreamInterfaceConfig,
    ) -> Result<Self::Session>;
}

#[async_trait]
pub trait MessageInterface: Send + Sync + 'static {
    async fn handle_request(&self, request: InterfaceRequest) -> Result<InterfaceResponse>;
}

#[derive(Debug, Clone)]
pub struct InterfaceRequest {
    pub operation_path: String,
    pub input: serde_json::Value,
    pub auth_token: Option<crate::auth::AuthToken>,
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct InterfaceResponse {
    pub result: Result<serde_json::Value, crate::call::CallError>,
    pub status: u16,
    pub headers: HashMap<String, String>,
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

    #[test]
    fn interface_request_fields() {
        let req = InterfaceRequest {
            operation_path: "/v1/head/auth/verify".to_string(),
            input: serde_json::json!({"key": "value"}),
            auth_token: None,
            metadata: HashMap::new(),
        };
        assert_eq!(req.operation_path, "/v1/head/auth/verify");
        assert!(req.auth_token.is_none());
    }

    #[test]
    fn interface_response_fields() {
        let resp = InterfaceResponse {
            result: Ok(serde_json::json!({"status": "ok"})),
            status: 200,
            headers: HashMap::new(),
        };
        assert_eq!(resp.status, 200);
    }

    struct MockMessageInterface;

    #[async_trait]
    impl MessageInterface for MockMessageInterface {
        async fn handle_request(&self, _request: InterfaceRequest) -> Result<InterfaceResponse> {
            Ok(InterfaceResponse {
                result: Ok(serde_json::json!({})),
                status: 200,
                headers: HashMap::new(),
            })
        }
    }

    #[tokio::test]
    async fn message_interface_trait_compiles() {
        let iface = MockMessageInterface;
        let req = InterfaceRequest {
            operation_path: "/test".to_string(),
            input: serde_json::json!({}),
            auth_token: None,
            metadata: HashMap::new(),
        };
        let resp = iface.handle_request(req).await.unwrap();
        assert_eq!(resp.status, 200);
    }
}
