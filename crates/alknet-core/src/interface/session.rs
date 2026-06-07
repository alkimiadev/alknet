use anyhow::Result;
use async_trait::async_trait;

use crate::auth::Identity;
use crate::call::EventEnvelope;

#[derive(Debug, Clone)]
pub struct InterfaceEvent {
    pub envelope: EventEnvelope,
    pub identity: Option<Identity>,
}

impl InterfaceEvent {
    pub fn new(envelope: EventEnvelope) -> Self {
        Self {
            envelope,
            identity: None,
        }
    }

    pub fn with_identity(envelope: EventEnvelope, identity: Identity) -> Self {
        Self {
            envelope,
            identity: Some(identity),
        }
    }
}

#[async_trait]
pub trait InterfaceSession: Send {
    async fn recv(&mut self) -> Option<InterfaceEvent>;

    async fn send(&mut self, envelope: EventEnvelope) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn interface_event_new() {
        let envelope = EventEnvelope::call_requested("req-1", serde_json::json!({"op": "test"}));
        let event = InterfaceEvent::new(envelope.clone());
        assert_eq!(event.envelope, envelope);
        assert!(event.identity.is_none());
    }

    #[test]
    fn interface_event_with_identity() {
        let envelope = EventEnvelope::call_requested("req-1", serde_json::json!({"op": "test"}));
        let identity = Identity {
            id: "SHA256:abc123".to_string(),
            scopes: vec!["relay:connect".to_string()],
            resources: HashMap::new(),
        };
        let event = InterfaceEvent::with_identity(envelope.clone(), identity.clone());
        assert_eq!(event.envelope, envelope);
        assert!(event.identity.is_some());
        assert_eq!(event.identity.as_ref().unwrap().id, "SHA256:abc123");
    }
}
