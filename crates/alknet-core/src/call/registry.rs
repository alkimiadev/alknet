use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use crate::call::context::OperationContext;
use crate::call::response::ResponseEnvelope;
use crate::call::spec::OperationSpec;

pub type Handler = Arc<dyn Fn(Value, OperationContext) -> ResponseEnvelope + Send + Sync>;

pub struct OperationRegistry {
    operations: HashMap<String, (OperationSpec, Handler)>,
}

impl std::fmt::Debug for OperationRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OperationRegistry")
            .field("operation_count", &self.operations.len())
            .finish()
    }
}

impl OperationRegistry {
    pub fn new() -> Self {
        Self {
            operations: HashMap::new(),
        }
    }

    pub fn register(&mut self, spec: OperationSpec, handler: Handler) {
        self.operations.insert(spec.name.clone(), (spec, handler));
    }

    pub fn lookup(&self, name: &str) -> Option<(&OperationSpec, &Handler)> {
        self.operations
            .get(name)
            .map(|(spec, handler)| (spec, handler))
    }

    pub fn invoke(&self, name: &str, input: Value, context: OperationContext) -> ResponseEnvelope {
        match self.lookup(name) {
            Some((spec, handler)) => {
                if !context.trusted {
                    if let Some(ref identity) = context.identity {
                        if !spec.access_control.check(identity) {
                            return ResponseEnvelope::err(
                                &context.request_id,
                                "FORBIDDEN",
                                "access denied",
                                false,
                            );
                        }
                    } else if spec.access_control.has_restrictions() {
                        return ResponseEnvelope::err(
                            &context.request_id,
                            "FORBIDDEN",
                            "authentication required",
                            false,
                        );
                    }
                }
                handler(input, context)
            }
            None => ResponseEnvelope::err(
                &context.request_id,
                "NOT_FOUND",
                format!("operation not found: {name}"),
                false,
            ),
        }
    }

    pub fn list_operations(&self) -> Vec<&OperationSpec> {
        self.operations.values().map(|(spec, _)| spec).collect()
    }
}

impl Default for OperationRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub struct OperationRegistryBuilder {
    registry: OperationRegistry,
}

impl OperationRegistryBuilder {
    pub fn new() -> Self {
        Self {
            registry: OperationRegistry::new(),
        }
    }

    pub fn with(mut self, spec: OperationSpec, handler: Handler) -> Self {
        self.registry.register(spec, handler);
        self
    }

    pub fn build(self) -> OperationRegistry {
        self.registry
    }
}

impl Default for OperationRegistryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Identity;
    use crate::call::env::OperationEnv;
    use crate::call::spec::{AccessControl, OperationType};
    use std::collections::HashMap;

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

    fn make_spec_with_acl(name: &str, namespace: &str, acl: AccessControl) -> OperationSpec {
        OperationSpec {
            name: name.to_string(),
            namespace: namespace.to_string(),
            op_type: OperationType::Mutation,
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
            access_control: acl,
        }
    }

    fn make_context(request_id: &str, identity: Option<Identity>) -> OperationContext {
        let registry = OperationRegistry::new();
        OperationContext {
            request_id: request_id.to_string(),
            parent_request_id: None,
            identity,
            metadata: HashMap::new(),
            env: OperationEnv::local(registry),
            trusted: false,
        }
    }

    #[test]
    fn register_and_lookup() {
        let mut registry = OperationRegistry::new();
        let spec = make_spec("fs/readFile", "fs");
        let handler: Handler = Arc::new(|input, _ctx| ResponseEnvelope::ok("req-1", input));
        registry.register(spec, handler);
        let found = registry.lookup("fs/readFile");
        assert!(found.is_some());
        let (spec, _) = found.unwrap();
        assert_eq!(spec.name, "fs/readFile");
        assert_eq!(spec.namespace, "fs");
    }

    #[test]
    fn lookup_missing_returns_none() {
        let registry = OperationRegistry::new();
        assert!(registry.lookup("missing").is_none());
    }

    #[test]
    fn invoke_operation() {
        let mut registry = OperationRegistry::new();
        let spec = make_spec("fs/readFile", "fs");
        let handler: Handler = Arc::new(|input, ctx| ResponseEnvelope::ok(&ctx.request_id, input));
        registry.register(spec, handler);
        let context = make_context("req-1", None);
        let result = registry.invoke("fs/readFile", serde_json::json!({"path": "/tmp"}), context);
        assert!(result.result.is_ok());
        assert_eq!(result.result.unwrap(), serde_json::json!({"path": "/tmp"}));
    }

    #[test]
    fn invoke_missing_operation() {
        let registry = OperationRegistry::new();
        let context = make_context("req-1", None);
        let result = registry.invoke("missing", serde_json::json!(null), context);
        assert!(result.result.is_err());
        let err = result.result.unwrap_err();
        assert_eq!(err.code, "NOT_FOUND");
    }

    #[test]
    fn invoke_with_acl_check_allowed() {
        let mut registry = OperationRegistry::new();
        let acl = AccessControl {
            required_scopes: vec!["read".to_string()],
            required_scopes_any: None,
            resource_type: None,
            resource_action: None,
        };
        let spec = make_spec_with_acl("bash/exec", "bash", acl);
        let handler: Handler = Arc::new(|_input, ctx| {
            ResponseEnvelope::ok(&ctx.request_id, serde_json::json!("done"))
        });
        registry.register(spec, handler);

        let identity = Identity {
            id: "user-1".to_string(),
            scopes: vec!["read".to_string()],
            resources: HashMap::new(),
        };
        let context = make_context("req-1", Some(identity));
        let result = registry.invoke("bash/exec", serde_json::json!(null), context);
        assert!(result.result.is_ok());
    }

    #[test]
    fn invoke_with_acl_check_denied() {
        let mut registry = OperationRegistry::new();
        let acl = AccessControl {
            required_scopes: vec!["admin".to_string()],
            required_scopes_any: None,
            resource_type: None,
            resource_action: None,
        };
        let spec = make_spec_with_acl("bash/exec", "bash", acl);
        let handler: Handler = Arc::new(|_input, ctx| {
            ResponseEnvelope::ok(&ctx.request_id, serde_json::json!("done"))
        });
        registry.register(spec, handler);

        let identity = Identity {
            id: "user-1".to_string(),
            scopes: vec!["read".to_string()],
            resources: HashMap::new(),
        };
        let context = make_context("req-1", Some(identity));
        let result = registry.invoke("bash/exec", serde_json::json!(null), context);
        assert!(result.result.is_err());
        let err = result.result.unwrap_err();
        assert_eq!(err.code, "FORBIDDEN");
    }

    #[test]
    fn invoke_trusted_skips_acl() {
        let mut registry = OperationRegistry::new();
        let acl = AccessControl {
            required_scopes: vec!["admin".to_string()],
            required_scopes_any: None,
            resource_type: None,
            resource_action: None,
        };
        let spec = make_spec_with_acl("bash/exec", "bash", acl);
        let handler: Handler = Arc::new(|_input, ctx| {
            ResponseEnvelope::ok(&ctx.request_id, serde_json::json!("done"))
        });
        registry.register(spec, handler);

        let identity = Identity {
            id: "user-1".to_string(),
            scopes: vec!["read".to_string()],
            resources: HashMap::new(),
        };
        let mut registry2 = OperationRegistry::new();
        let context = OperationContext {
            request_id: "req-1".to_string(),
            parent_request_id: None,
            identity: Some(identity),
            metadata: HashMap::new(),
            env: OperationEnv::local(registry2),
            trusted: true,
        };
        let result = registry.invoke("bash/exec", serde_json::json!(null), context);
        assert!(result.result.is_ok());
    }

    #[test]
    fn invoke_no_identity_with_acl_denied() {
        let mut registry = OperationRegistry::new();
        let acl = AccessControl {
            required_scopes: vec!["read".to_string()],
            required_scopes_any: None,
            resource_type: None,
            resource_action: None,
        };
        let spec = make_spec_with_acl("bash/exec", "bash", acl);
        let handler: Handler = Arc::new(|_input, ctx| {
            ResponseEnvelope::ok(&ctx.request_id, serde_json::json!("done"))
        });
        registry.register(spec, handler);

        let context = make_context("req-1", None);
        let result = registry.invoke("bash/exec", serde_json::json!(null), context);
        assert!(result.result.is_err());
        let err = result.result.unwrap_err();
        assert_eq!(err.code, "FORBIDDEN");
    }

    #[test]
    fn list_operations() {
        let mut registry = OperationRegistry::new();
        registry.register(
            make_spec("fs/readFile", "fs"),
            Arc::new(|_, ctx| ResponseEnvelope::ok(&ctx.request_id, serde_json::json!(null))),
        );
        registry.register(
            make_spec("bash/exec", "bash"),
            Arc::new(|_, ctx| ResponseEnvelope::ok(&ctx.request_id, serde_json::json!(null))),
        );
        let ops = registry.list_operations();
        assert_eq!(ops.len(), 2);
    }

    #[test]
    fn registry_builder() {
        let registry = OperationRegistryBuilder::new()
            .with(
                make_spec("fs/readFile", "fs"),
                Arc::new(|input, ctx| ResponseEnvelope::ok(&ctx.request_id, input)),
            )
            .with(
                make_spec("bash/exec", "bash"),
                Arc::new(|input, ctx| ResponseEnvelope::ok(&ctx.request_id, input)),
            )
            .build();
        assert!(registry.lookup("fs/readFile").is_some());
        assert!(registry.lookup("bash/exec").is_some());
    }
}
