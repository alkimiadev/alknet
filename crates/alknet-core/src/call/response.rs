use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseEnvelope {
    pub request_id: String,
    pub result: Result<Value, CallError>,
}

impl ResponseEnvelope {
    pub fn ok(request_id: impl Into<String>, value: Value) -> Self {
        Self {
            request_id: request_id.into(),
            result: Ok(value),
        }
    }

    pub fn err(
        request_id: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            result: Err(CallError {
                code: code.into(),
                message: message.into(),
                retryable,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn call_error_fields() {
        let err = CallError {
            code: "NOT_FOUND".to_string(),
            message: "operation not found".to_string(),
            retryable: false,
        };
        assert_eq!(err.code, "NOT_FOUND");
        assert_eq!(err.message, "operation not found");
        assert!(!err.retryable);
    }

    #[test]
    fn response_envelope_ok() {
        let env = ResponseEnvelope::ok("req-1", json!({"status": "ok"}));
        assert_eq!(env.request_id, "req-1");
        assert!(env.result.is_ok());
        assert_eq!(env.result.unwrap(), json!({"status": "ok"}));
    }

    #[test]
    fn response_envelope_err() {
        let env = ResponseEnvelope::err("req-1", "NOT_FOUND", "operation not found", false);
        assert_eq!(env.request_id, "req-1");
        assert!(env.result.is_err());
        let err = env.result.unwrap_err();
        assert_eq!(err.code, "NOT_FOUND");
        assert_eq!(err.message, "operation not found");
        assert!(!err.retryable);
    }

    #[test]
    fn response_envelope_serialization() {
        let env = ResponseEnvelope::ok("req-1", json!({"key": "value"}));
        let serialized = serde_json::to_string(&env).unwrap();
        let deserialized: ResponseEnvelope = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.request_id, "req-1");
        assert!(deserialized.result.is_ok());
    }

    #[test]
    fn response_envelope_err_serialization() {
        let env = ResponseEnvelope::err("req-2", "TIMEOUT", "timed out", true);
        let serialized = serde_json::to_string(&env).unwrap();
        let deserialized: ResponseEnvelope = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.request_id, "req-2");
        let err = deserialized.result.unwrap_err();
        assert_eq!(err.code, "TIMEOUT");
        assert!(err.retryable);
    }
}
