//! `HandlerRegistry` — maps ALPN byte strings to `ProtocolHandler` instances.
//!
//! Registered statically at startup by the assembly layer; the endpoint
//! dispatches by looking up the negotiated ALPN.

use std::collections::HashMap;
use std::sync::Arc;

use alknet_core::types::ProtocolHandler;

pub struct HandlerRegistry {
    handlers: HashMap<&'static [u8], Arc<dyn ProtocolHandler>>,
}

impl HandlerRegistry {
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    pub fn register(&mut self, handler: Arc<dyn ProtocolHandler>) {
        let alpn = handler.alpn();
        if self.handlers.contains_key(alpn) {
            panic!(
                "HandlerRegistry: ALPN already registered: {:?}",
                String::from_utf8_lossy(alpn)
            );
        }
        self.handlers.insert(alpn, handler);
    }

    pub fn get(&self, alpn: &[u8]) -> Option<&Arc<dyn ProtocolHandler>> {
        self.handlers.get(alpn)
    }

    pub fn alpn_strings(&self) -> Vec<Vec<u8>> {
        self.handlers.keys().map(|k| k.to_vec()).collect()
    }
}

impl Default for HandlerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for HandlerRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HandlerRegistry")
            .field(
                "alpns",
                &self
                    .handlers
                    .keys()
                    .map(|k| String::from_utf8_lossy(k).to_string())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alknet_core::auth::AuthContext;
    use alknet_core::types::{Connection, HandlerError};
    use async_trait::async_trait;

    struct DummyHandler {
        alpn: &'static [u8],
    }

    #[async_trait]
    impl ProtocolHandler for DummyHandler {
        fn alpn(&self) -> &'static [u8] {
            self.alpn
        }
        async fn handle(
            &self,
            _connection: Connection,
            _auth: &AuthContext,
        ) -> Result<(), HandlerError> {
            Ok(())
        }
    }

    fn make_handler(alpn: &'static [u8]) -> Arc<dyn ProtocolHandler> {
        Arc::new(DummyHandler { alpn })
    }

    #[test]
    fn handler_registry_new_is_empty() {
        let reg = HandlerRegistry::new();
        assert!(reg.alpn_strings().is_empty());
        assert!(reg.get(b"alknet/test").is_none());
    }

    #[test]
    fn handler_registry_register_then_get() {
        let mut reg = HandlerRegistry::new();
        reg.register(make_handler(b"alknet/test"));
        assert_eq!(reg.alpn_strings(), vec![b"alknet/test".to_vec()]);
        assert!(reg.get(b"alknet/test").is_some());
        assert!(reg.get(b"alknet/other").is_none());
    }

    #[test]
    fn handler_registry_multiple_alpns() {
        let mut reg = HandlerRegistry::new();
        reg.register(make_handler(b"alknet/ssh"));
        reg.register(make_handler(b"alknet/call"));
        let mut alpns = reg
            .alpn_strings()
            .into_iter()
            .map(|a| String::from_utf8(a).unwrap())
            .collect::<Vec<_>>();
        alpns.sort();
        assert_eq!(alpns, vec!["alknet/call", "alknet/ssh"]);
        assert!(reg.get(b"alknet/ssh").is_some());
        assert!(reg.get(b"alknet/call").is_some());
    }

    #[test]
    #[should_panic(expected = "ALPN already registered")]
    fn handler_registry_register_panics_on_duplicate() {
        let mut reg = HandlerRegistry::new();
        reg.register(make_handler(b"alknet/test"));
        reg.register(make_handler(b"alknet/test"));
    }

    #[test]
    fn handler_registry_debug_lists_alpns() {
        let mut reg = HandlerRegistry::new();
        reg.register(make_handler(b"alknet/test"));
        let s = format!("{:?}", reg);
        assert!(s.contains("alknet/test"));
    }

    #[test]
    fn handler_registry_default_is_empty() {
        let reg = HandlerRegistry::default();
        assert!(reg.alpn_strings().is_empty());
        assert!(reg.get(b"alknet/test").is_none());
    }

    #[test]
    fn handler_registry_debug_lists_alpns_via_default() {
        let reg = HandlerRegistry::default();
        let s = format!("{reg:?}");
        assert!(s.contains("HandlerRegistry"));
    }
}
