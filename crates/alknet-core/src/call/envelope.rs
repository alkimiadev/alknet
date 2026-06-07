use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventEnvelope {
    #[serde(rename = "type")]
    pub r#type: String,
    pub id: String,
    pub payload: Value,
}

impl EventEnvelope {
    pub fn new(event_type: impl Into<String>, id: impl Into<String>, payload: Value) -> Self {
        Self {
            r#type: event_type.into(),
            id: id.into(),
            payload,
        }
    }

    pub fn call_requested(id: impl Into<String>, payload: Value) -> Self {
        Self::new(super::events::CALL_REQUESTED, id, payload)
    }

    pub fn call_responded(id: impl Into<String>, payload: Value) -> Self {
        Self::new(super::events::CALL_RESPONDED, id, payload)
    }

    pub fn call_completed(id: impl Into<String>, payload: Value) -> Self {
        Self::new(super::events::CALL_COMPLETED, id, payload)
    }

    pub fn call_aborted(id: impl Into<String>, payload: Value) -> Self {
        Self::new(super::events::CALL_ABORTED, id, payload)
    }

    pub fn call_error(
        id: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self::new(
            super::events::CALL_ERROR,
            id,
            serde_json::json!({
                "code": code.into(),
                "message": message.into(),
                "retryable": retryable,
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_envelope_new() {
        let env = EventEnvelope::new(
            "call.requested",
            "req-1",
            serde_json::json!({"key": "value"}),
        );
        assert_eq!(env.r#type, "call.requested");
        assert_eq!(env.id, "req-1");
        assert_eq!(env.payload, serde_json::json!({"key": "value"}));
    }

    #[test]
    fn event_envelope_serialization() {
        let env = EventEnvelope::new(
            "call.requested",
            "req-1",
            serde_json::json!({"key": "value"}),
        );
        let serialized = serde_json::to_string(&env).unwrap();
        let deserialized: EventEnvelope = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.r#type, "call.requested");
        assert_eq!(deserialized.id, "req-1");
        assert_eq!(deserialized.payload, serde_json::json!({"key": "value"}));
    }

    #[test]
    fn event_envelope_serialization_type_field() {
        let env = EventEnvelope::new("call.requested", "req-1", serde_json::json!(null));
        let serialized = serde_json::to_string(&env).unwrap();
        assert!(serialized.contains("\"type\""));
    }

    #[test]
    fn event_envelope_deserialization() {
        let json = r#"{"type":"call.responded","id":"req-42","payload":{"result":"ok"}}"#;
        let env: EventEnvelope = serde_json::from_str(json).unwrap();
        assert_eq!(env.r#type, "call.responded");
        assert_eq!(env.id, "req-42");
        assert_eq!(env.payload["result"], "ok");
    }

    #[test]
    fn event_envelope_call_requested() {
        let env = EventEnvelope::call_requested("req-1", serde_json::json!({"op": "test"}));
        assert_eq!(env.r#type, "call.requested");
        assert_eq!(env.id, "req-1");
    }

    #[test]
    fn event_envelope_call_responded() {
        let env = EventEnvelope::call_responded("req-1", serde_json::json!({"data": 42}));
        assert_eq!(env.r#type, "call.responded");
    }

    #[test]
    fn event_envelope_call_completed() {
        let env = EventEnvelope::call_completed("req-1", serde_json::json!(null));
        assert_eq!(env.r#type, "call.completed");
    }

    #[test]
    fn event_envelope_call_aborted() {
        let env = EventEnvelope::call_aborted("req-1", serde_json::json!({"reason": "cancelled"}));
        assert_eq!(env.r#type, "call.aborted");
    }

    #[test]
    fn event_envelope_call_error() {
        let env = EventEnvelope::call_error("req-1", "TIMEOUT", "timed out", true);
        assert_eq!(env.r#type, "call.error");
        assert_eq!(env.id, "req-1");
        assert_eq!(env.payload["code"], "TIMEOUT");
        assert_eq!(env.payload["message"], "timed out");
        assert_eq!(env.payload["retryable"], true);
    }

    #[test]
    fn event_envelope_empty_id() {
        let env = EventEnvelope::new("event.broadcast", "", serde_json::json!({"msg": "hello"}));
        assert_eq!(env.id, "");
    }
}
