use std::sync::Arc;

use serde_json::Value;

use crate::call::context::OperationContext;
use crate::call::response::ResponseEnvelope;
use crate::call::spec::{AccessControl, OperationSpec, OperationType};

pub fn services_list_spec() -> OperationSpec {
    OperationSpec {
        name: super::events::SERVICE_LIST.to_string(),
        namespace: "services".to_string(),
        op_type: OperationType::Query,
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {},
        }),
        output_schema: serde_json::json!({
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "namespace": { "type": "string" },
                    "op_type": { "type": "string" },
                },
            },
        }),
        access_control: AccessControl {
            required_scopes: vec![],
            required_scopes_any: None,
            resource_type: None,
            resource_action: None,
        },
    }
}

pub fn services_schema_spec() -> OperationSpec {
    OperationSpec {
        name: super::events::SERVICE_SCHEMA.to_string(),
        namespace: "services".to_string(),
        op_type: OperationType::Query,
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
            },
            "required": ["name"],
        }),
        output_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "namespace": { "type": "string" },
                "op_type": { "type": "string" },
                "input_schema": { "type": "object" },
                "output_schema": { "type": "object" },
            },
        }),
        access_control: AccessControl {
            required_scopes: vec![],
            required_scopes_any: None,
            resource_type: None,
            resource_action: None,
        },
    }
}

pub fn register_default_operations(registry: &mut crate::call::OperationRegistry) {
    registry.register(services_list_spec(), Arc::new(services_list_handler));
    registry.register(services_schema_spec(), Arc::new(services_schema_handler));
}

fn services_list_handler(_input: Value, ctx: OperationContext) -> ResponseEnvelope {
    let registry = &ctx.env.registry_ref();
    let specs = registry.list_operations();
    let ops: Vec<Value> = specs
        .iter()
        .map(|spec| {
            serde_json::json!({
                "name": spec.name,
                "namespace": spec.namespace,
                "op_type": format!("{:?}", spec.op_type).to_lowercase(),
            })
        })
        .collect();
    ResponseEnvelope::ok(&ctx.request_id, serde_json::json!({ "operations": ops }))
}

fn services_schema_handler(input: Value, ctx: OperationContext) -> ResponseEnvelope {
    let name = match input.get("name").and_then(|v| v.as_str()) {
        Some(n) => n.to_string(),
        None => {
            return ResponseEnvelope::err(
                &ctx.request_id,
                "INVALID_INPUT",
                "missing required field: name",
                false,
            );
        }
    };
    let registry = &ctx.env.registry_ref();
    match registry.lookup(&name) {
        Some((spec, _)) => ResponseEnvelope::ok(
            &ctx.request_id,
            serde_json::json!({
                "name": spec.name,
                "namespace": spec.namespace,
                "op_type": format!("{:?}", spec.op_type).to_lowercase(),
                "input_schema": spec.input_schema,
                "output_schema": spec.output_schema,
            }),
        ),
        None => ResponseEnvelope::err(
            &ctx.request_id,
            "NOT_FOUND",
            format!("operation not found: {name}"),
            false,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call::env::OperationEnv;

    fn make_env() -> OperationEnv {
        let mut registry = crate::call::OperationRegistry::new();
        registry.register(services_list_spec(), Arc::new(services_list_handler));
        registry.register(services_schema_spec(), Arc::new(services_schema_handler));
        OperationEnv::local(registry)
    }

    #[test]
    fn services_list_returns_operations() {
        let env = make_env();
        let result = env.invoke("services", "list", serde_json::json!({}));
        assert!(result.result.is_ok());
        let value = result.result.unwrap();
        let ops = value.get("operations").unwrap().as_array().unwrap();
        assert_eq!(ops.len(), 2);
    }

    #[test]
    fn services_schema_returns_spec() {
        let env = make_env();
        let result = env.invoke(
            "services",
            "schema",
            serde_json::json!({"name": "/services/list"}),
        );
        assert!(result.result.is_ok());
        let value = result.result.unwrap();
        assert_eq!(value["name"], "/services/list");
        assert_eq!(value["namespace"], "services");
    }

    #[test]
    fn services_schema_missing_name() {
        let env = make_env();
        let result = env.invoke("services", "schema", serde_json::json!({}));
        assert!(result.result.is_err());
        let err = result.result.unwrap_err();
        assert_eq!(err.code, "INVALID_INPUT");
    }

    #[test]
    fn services_schema_not_found() {
        let env = make_env();
        let result = env.invoke(
            "services",
            "schema",
            serde_json::json!({"name": "/nonexistent/op"}),
        );
        assert!(result.result.is_err());
        let err = result.result.unwrap_err();
        assert_eq!(err.code, "NOT_FOUND");
    }

    #[test]
    fn services_list_spec_fields() {
        let spec = services_list_spec();
        assert_eq!(spec.name, "/services/list");
        assert_eq!(spec.namespace, "services");
        assert_eq!(spec.op_type, OperationType::Query);
        assert!(!spec.access_control.has_restrictions());
    }

    #[test]
    fn services_schema_spec_fields() {
        let spec = services_schema_spec();
        assert_eq!(spec.name, "/services/schema");
        assert_eq!(spec.namespace, "services");
        assert_eq!(spec.op_type, OperationType::Query);
        assert!(!spec.access_control.has_restrictions());
    }

    #[test]
    fn register_default_operations_adds_both() {
        let mut registry = crate::call::OperationRegistry::new();
        register_default_operations(&mut registry);
        assert!(registry.lookup("/services/list").is_some());
        assert!(registry.lookup("/services/schema").is_some());
        assert_eq!(registry.list_operations().len(), 2);
    }
}
