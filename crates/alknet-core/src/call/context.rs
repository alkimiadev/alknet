use std::collections::HashMap;

use serde_json::Value;

use crate::call::OperationEnv;

#[derive(Debug, Clone)]
pub struct OperationContext {
    pub request_id: String,
    pub parent_request_id: Option<String>,
    pub identity: Option<crate::auth::Identity>,
    pub metadata: HashMap<String, Value>,
    pub env: OperationEnv,
    pub trusted: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call::OperationRegistry;

    fn make_context() -> OperationContext {
        let registry = OperationRegistry::new();
        OperationContext {
            request_id: "req-1".to_string(),
            parent_request_id: None,
            identity: None,
            metadata: HashMap::new(),
            env: OperationEnv::local(registry),
            trusted: false,
        }
    }

    #[test]
    fn operation_context_fields() {
        let ctx = make_context();
        assert_eq!(ctx.request_id, "req-1");
        assert!(ctx.parent_request_id.is_none());
        assert!(ctx.identity.is_none());
        assert!(ctx.metadata.is_empty());
        assert!(!ctx.trusted);
    }

    #[test]
    fn operation_context_with_parent() {
        let registry = OperationRegistry::new();
        let ctx = OperationContext {
            request_id: "req-2".to_string(),
            parent_request_id: Some("req-1".to_string()),
            identity: None,
            metadata: HashMap::new(),
            env: OperationEnv::local(registry),
            trusted: true,
        };
        assert_eq!(ctx.parent_request_id, Some("req-1".to_string()));
        assert!(ctx.trusted);
    }
}
