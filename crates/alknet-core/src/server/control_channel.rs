//! Control channel routing for reserved `alknet-*` destinations.
//!
//! SSH channels opened with a destination starting with `alknet-` are intercepted
//! by the server and routed to a `ControlChannelHandler` instead of proxied to a
//! TCP target. See ADR-018 for the design rationale.

use std::io;

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};

pub const ALKNET_CONTROL_DESTINATION: &str = "alknet-control";
pub const ALKNET_PREFIX: &str = "alknet-";

pub fn is_reserved_destination(host: &str) -> bool {
    host.starts_with(ALKNET_PREFIX)
}

pub trait DuplexStream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> DuplexStream for T {}

#[async_trait]
pub trait ControlChannelHandler: Send + Sync {
    async fn handle_channel(&self, stream: Box<dyn DuplexStream>);
}

pub struct ControlChannelRouter {
    handler: Option<Box<dyn ControlChannelHandler>>,
}

impl ControlChannelRouter {
    pub fn new(handler: Option<Box<dyn ControlChannelHandler>>) -> Self {
        Self { handler }
    }

    pub fn without_handler() -> Self {
        Self { handler: None }
    }

    pub fn with_handler(handler: Box<dyn ControlChannelHandler>) -> Self {
        Self {
            handler: Some(handler),
        }
    }

    pub fn has_handler(&self) -> bool {
        self.handler.is_some()
    }

    pub async fn route(&self, stream: Box<dyn DuplexStream>) -> io::Result<()> {
        match &self.handler {
            Some(handler) => {
                handler.handle_channel(stream).await;
                Ok(())
            }
            None => Err(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                "no control channel handler configured",
            )),
        }
    }

    pub fn take_handler(&mut self) -> Option<Box<dyn ControlChannelHandler>> {
        self.handler.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[test]
    fn alknet_control_destination_constant() {
        assert_eq!(ALKNET_CONTROL_DESTINATION, "alknet-control");
    }

    #[test]
    fn alknet_prefix_constant() {
        assert_eq!(ALKNET_PREFIX, "alknet-");
    }

    #[test]
    fn reserved_destination_detected() {
        assert!(is_reserved_destination("alknet-control"));
        assert!(is_reserved_destination("alknet-status"));
        assert!(is_reserved_destination("alknet-events"));
        assert!(is_reserved_destination("alknet-"));
    }

    #[test]
    fn non_reserved_destination_passes_through() {
        assert!(!is_reserved_destination("example.com"));
        assert!(!is_reserved_destination("localhost"));
        assert!(!is_reserved_destination("192.168.1.1"));
        assert!(!is_reserved_destination("alknet.example.com"));
        assert!(!is_reserved_destination(""));
        assert!(!is_reserved_destination("alkne-control"));
        assert!(!is_reserved_destination("ALKNET-control"));
    }

    #[test]
    fn prefix_matching_case_sensitive() {
        assert!(!is_reserved_destination("Alknet-control"));
        assert!(!is_reserved_destination("ALKNET-control"));
        assert!(is_reserved_destination("alknet-Control"));
    }

    #[test]
    fn router_without_handler_has_no_handler() {
        let router = ControlChannelRouter::without_handler();
        assert!(!router.has_handler());
    }

    #[test]
    fn router_with_handler_has_handler() {
        struct DummyHandler;
        #[async_trait]
        impl ControlChannelHandler for DummyHandler {
            async fn handle_channel(&self, _stream: Box<dyn DuplexStream>) {}
        }
        let router = ControlChannelRouter::with_handler(Box::new(DummyHandler));
        assert!(router.has_handler());
    }

    #[tokio::test]
    async fn route_without_handler_returns_error() {
        let router = ControlChannelRouter::without_handler();
        let (_client, server) = duplex(64);
        let stream: Box<dyn DuplexStream> = Box::new(server);
        let result = router.route(stream).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::ConnectionRefused);
    }

    #[tokio::test]
    async fn route_with_handler_succeeds() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        struct TrackedHandler {
            called: Arc<AtomicBool>,
        }
        #[async_trait]
        impl ControlChannelHandler for TrackedHandler {
            async fn handle_channel(&self, _stream: Box<dyn DuplexStream>) {
                self.called.store(true, Ordering::SeqCst);
            }
        }

        let called = Arc::new(AtomicBool::new(false));
        let handler = TrackedHandler {
            called: called.clone(),
        };
        let router = ControlChannelRouter::with_handler(Box::new(handler));
        let (_client, server) = duplex(64);
        let stream: Box<dyn DuplexStream> = Box::new(server);
        let result = router.route(stream).await;
        assert!(result.is_ok());
        assert!(called.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn route_with_handler_can_read_write() {
        struct EchoHandler;
        #[async_trait]
        impl ControlChannelHandler for EchoHandler {
            async fn handle_channel(&self, mut stream: Box<dyn DuplexStream>) {
                let mut buf = [0u8; 64];
                let n = stream.read(&mut buf).await.unwrap();
                stream.write_all(&buf[..n]).await.unwrap();
            }
        }

        let router = ControlChannelRouter::with_handler(Box::new(EchoHandler));
        let (client, server) = duplex(64);
        let stream: Box<dyn DuplexStream> = Box::new(server);
        tokio::spawn(async move {
            router.route(stream).await.unwrap();
        });

        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut client = client;
        client.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 5];
        client.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");
    }

    #[test]
    fn control_channel_destination_matches_prefix() {
        assert!(is_reserved_destination(ALKNET_CONTROL_DESTINATION));
    }
}
