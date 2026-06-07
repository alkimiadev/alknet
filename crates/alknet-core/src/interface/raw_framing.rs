use anyhow::Result;
use async_trait::async_trait;

use crate::interface::session::{InterfaceEvent, InterfaceSession};
use crate::interface::{Interface, InterfaceConfig, TransportStream};

pub struct RawFramingInterface;

pub struct RawFramingSession;

#[async_trait]
impl Interface for RawFramingInterface {
    type Session = RawFramingSession;

    async fn accept(
        &self,
        _stream: Box<dyn TransportStream>,
        _config: &InterfaceConfig,
    ) -> Result<Self::Session> {
        Err(anyhow::anyhow!(
            "RawFramingInterface is not yet implemented (Phase 4+)"
        ))
    }
}

#[async_trait]
impl InterfaceSession for RawFramingSession {
    async fn recv(&mut self) -> Option<InterfaceEvent> {
        None
    }

    async fn send(&mut self, _envelope: crate::call::EventEnvelope) -> Result<()> {
        Err(anyhow::anyhow!(
            "RawFramingSession is not yet implemented (Phase 4+)"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_framing_interface_type_exists() {
        let _iface = RawFramingInterface;
    }

    #[test]
    fn raw_framing_session_type_exists() {
        let _session = RawFramingSession;
    }

    #[tokio::test]
    async fn raw_framing_interface_accept_returns_error() {
        let iface = RawFramingInterface;
        let (_client, server) = tokio::io::duplex(1024);
        let stream: Box<dyn TransportStream> = Box::new(server);
        let config = InterfaceConfig::RawFraming(crate::interface::RawFramingConfig {});
        let result = iface.accept(stream, &config).await;
        assert!(result.is_err());
    }
}
