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
