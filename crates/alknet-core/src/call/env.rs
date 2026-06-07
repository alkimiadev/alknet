use std::sync::Arc;

use serde_json::Value;

use crate::call::context::OperationContext;
use crate::call::registry::OperationRegistry;
use crate::call::response::ResponseEnvelope;

#[derive(Debug, Clone)]
pub struct OperationEnv {
    registry: Arc<OperationRegistry>,
}

impl OperationEnv {
    pub fn local(registry: OperationRegistry) -> Self {
        Self {
            registry: Arc::new(registry),
        }
    }

    pub fn invoke(&self, namespace: &str, operation: &str, input: Value) -> ResponseEnvelope {
        let name = format!("/{namespace}/{operation}");
        let request_id = format!("env{name}");
        let context = OperationContext {
            request_id: request_id.clone(),
            parent_request_id: None,
            identity: None,
            metadata: std::collections::HashMap::new(),
            env: self.clone(),
            trusted: true,
        };
        self.registry.invoke(&name, input, context)
    }

    pub fn registry_ref(&self) -> &OperationRegistry {
        &self.registry
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call::registry::OperationRegistryBuilder;
    use crate::call::spec::{AccessControl, OperationSpec, OperationType};

    fn make_spec(name: &str, namespace: &str) -> OperationSpec {
        OperationSpec {
            name: name.to_string(),
            namespace: namespace.to_string(),
            op_type: OperationType::Query,
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
            access_control: AccessControl {
                required_scopes: vec![],
                required_scopes_any: None,
                resource_type: None,
                resource_action: None,
            },
        }
    }

    #[test]
    fn operation_env_local_invoke() {
        let registry = OperationRegistryBuilder::new()
            .with(
                make_spec("/auth/verify", "auth"),
                Arc::new(|_input, _ctx| {
                    ResponseEnvelope::ok("env-/auth/verify", serde_json::json!({"verified": true}))
                }),
            )
            .build();

        let env = OperationEnv::local(registry);
        let result = env.invoke("auth", "verify", serde_json::json!({"token": "abc"}));
        assert!(result.result.is_ok());
    }

    #[test]
    fn operation_env_invoke_missing() {
        let registry = OperationRegistry::new();
        let env = OperationEnv::local(registry);
        let result = env.invoke("auth", "verify", serde_json::json!(null));
        assert!(result.result.is_err());
        let err = result.result.unwrap_err();
        assert_eq!(err.code, "NOT_FOUND");
    }

    #[test]
    fn operation_env_invoke_trusted() {
        let registry = OperationRegistryBuilder::new()
            .with(
                make_spec("/auth/verify", "auth"),
                Arc::new(|_input, ctx| {
                    assert!(ctx.trusted);
                    ResponseEnvelope::ok(&ctx.request_id, serde_json::json!({"ok": true}))
                }),
            )
            .build();

        let env = OperationEnv::local(registry);
        let result = env.invoke("auth", "verify", serde_json::json!(null));
        assert!(result.result.is_ok());
    }
}
