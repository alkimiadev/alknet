use std::sync::Arc;

use serde_json::{json, Value};

use super::context::OperationContext;
use super::registration::{Handler, OperationRegistry};
use super::spec::{AccessControl, OperationSpec, OperationType, Visibility};
use crate::protocol::wire::{CallError, ResponseEnvelope};

const NAME_SERVICES_LIST: &str = "services/list";
const NAME_SERVICES_LIST_PEERS: &str = "services/list-peers";
const NAME_SERVICES_SCHEMA: &str = "services/schema";

pub fn services_list_spec() -> OperationSpec {
    OperationSpec::new(
        NAME_SERVICES_LIST,
        OperationType::Query,
        Visibility::External,
        json!({}),
        json!({
            "type": "object",
            "properties": {
                "operations": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" },
                            "namespace": { "type": "string" },
                            "op_type": {
                                "type": "string",
                                "enum": ["query", "mutation", "subscription"]
                            }
                        }
                    }
                }
            }
        }),
        vec![],
        AccessControl::default(),
    )
}

pub fn services_schema_spec() -> OperationSpec {
    OperationSpec::new(
        NAME_SERVICES_SCHEMA,
        OperationType::Query,
        Visibility::External,
        json!({
            "type": "object",
            "properties": { "name": { "type": "string" } },
            "required": ["name"]
        }),
        operation_spec_schema(),
        vec![],
        AccessControl::default(),
    )
}

pub fn services_list_peers_spec() -> OperationSpec {
    OperationSpec::new(
        NAME_SERVICES_LIST_PEERS,
        OperationType::Query,
        Visibility::External,
        json!({}),
        json!({
            "type": "object",
            "properties": {
                "peers": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "peer_id": { "type": "string" },
                            "operations": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "name": { "type": "string" },
                                        "namespace": { "type": "string" },
                                        "op_type": {
                                            "type": "string",
                                            "enum": ["query", "mutation", "subscription"]
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }),
        vec![],
        AccessControl::default(),
    )
}

fn operation_spec_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "namespace": { "type": "string" },
            "op_type": {
                "type": "string",
                "enum": ["query", "mutation", "subscription"]
            },
            "visibility": {
                "type": "string",
                "enum": ["external", "internal"]
            },
            "input_schema": {},
            "output_schema": {},
            "error_schemas": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "code": { "type": "string" },
                        "description": { "type": "string" },
                        "schema": {},
                        "http_status": { "type": ["integer", "null"] }
                    }
                }
            },
            "access_control": {
                "type": "object",
                "properties": {
                    "required_scopes": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "required_scopes_any": {
                        "type": ["array", "null"],
                        "items": { "type": "string" }
                    },
                    "resource_type": { "type": ["string", "null"] },
                    "resource_action": { "type": ["string", "null"] }
                }
            }
        },
        "required": [
            "name",
            "namespace",
            "op_type",
            "visibility",
            "input_schema",
            "output_schema",
            "error_schemas",
            "access_control"
        ]
    })
}

fn op_type_str(op_type: OperationType) -> &'static str {
    match op_type {
        OperationType::Query => "query",
        OperationType::Mutation => "mutation",
        OperationType::Subscription => "subscription",
    }
}

fn visibility_str(visibility: Visibility) -> &'static str {
    match visibility {
        Visibility::External => "external",
        Visibility::Internal => "internal",
    }
}

fn access_control_to_json(acl: &AccessControl) -> Value {
    json!({
        "required_scopes": acl.required_scopes,
        "required_scopes_any": acl.required_scopes_any,
        "resource_type": acl.resource_type,
        "resource_action": acl.resource_action,
    })
}

fn error_definition_to_json(def: &super::spec::ErrorDefinition) -> Value {
    json!({
        "code": def.code,
        "description": def.description,
        "schema": def.schema,
        "http_status": def.http_status,
    })
}

fn spec_to_json(spec: &OperationSpec) -> Value {
    let error_schemas: Vec<Value> = spec
        .error_schemas
        .iter()
        .map(error_definition_to_json)
        .collect();
    json!({
        "name": spec.name,
        "namespace": spec.namespace,
        "op_type": op_type_str(spec.op_type),
        "visibility": visibility_str(spec.visibility),
        "input_schema": spec.input_schema,
        "output_schema": spec.output_schema,
        "error_schemas": error_schemas,
        "access_control": access_control_to_json(&spec.access_control),
    })
}

fn normalize_name(name: &str) -> String {
    if let Some(rest) = name.strip_prefix('/') {
        rest.to_string()
    } else {
        name.to_string()
    }
}

pub fn services_list_handler(registry: Arc<OperationRegistry>) -> Handler {
    Arc::new(move |input: Value, ctx: OperationContext| {
        let registry = Arc::clone(&registry);
        Box::pin(async move {
            let _ = input;
            let calling_identity = ctx.identity.as_ref();
            let ops: Vec<Value> = registry
                .list_operations()
                .into_iter()
                .filter(|spec| spec.access_control.check(calling_identity).is_allowed())
                .map(|s| {
                    json!({
                        "name": s.name,
                        "namespace": s.namespace,
                        "op_type": op_type_str(s.op_type),
                    })
                })
                .collect();
            ResponseEnvelope::ok(ctx.request_id, json!({ "operations": ops }))
        })
    })
}

pub fn services_list_peers_handler(registry: Arc<OperationRegistry>) -> Handler {
    Arc::new(move |input: Value, ctx: OperationContext| {
        let registry = Arc::clone(&registry);
        Box::pin(async move {
            let _ = input;
            let calling_identity = ctx.identity.as_ref();
            let local_ops: Vec<Value> = registry
                .list_operations()
                .into_iter()
                .filter(|spec| spec.access_control.check(calling_identity).is_allowed())
                .map(|s| {
                    json!({
                        "name": s.name,
                        "namespace": s.namespace,
                        "op_type": op_type_str(s.op_type),
                    })
                })
                .collect();
            let mut peers: Vec<Value> = Vec::new();
            if !local_ops.is_empty() {
                peers.push(json!({ "peer_id": "local", "operations": local_ops }));
            }
            for peer_id in ctx.env.peer_ids() {
                let peer_ops: Vec<Value> = ctx
                    .env
                    .peer_operations(&peer_id)
                    .into_iter()
                    .filter(|name| {
                        let spec = registry.registration(name);
                        match spec {
                            Some(reg) => {
                                reg.spec.access_control.check(calling_identity).is_allowed()
                            }
                            None => true,
                        }
                    })
                    .map(name_to_listing_json)
                    .collect();
                if !peer_ops.is_empty() {
                    peers.push(json!({ "peer_id": peer_id, "operations": peer_ops }));
                }
            }
            ResponseEnvelope::ok(ctx.request_id, json!({ "peers": peers }))
        })
    })
}

fn name_to_listing_json(name: String) -> Value {
    let namespace = name
        .split('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("")
        .to_string();
    json!({
        "name": name,
        "namespace": namespace,
        "op_type": "query",
    })
}

pub fn services_schema_handler(registry: Arc<OperationRegistry>) -> Handler {
    Arc::new(move |input: Value, ctx: OperationContext| {
        let registry = Arc::clone(&registry);
        Box::pin(async move {
            let name = match input.get("name").and_then(|v| v.as_str()) {
                Some(n) => normalize_name(n),
                None => {
                    return ResponseEnvelope::error(
                        ctx.request_id,
                        CallError::invalid_input("missing required field: name"),
                    );
                }
            };
            match registry.registration(&name) {
                Some(reg) => {
                    let spec_json = spec_to_json(&reg.spec);
                    ResponseEnvelope::ok(ctx.request_id, spec_json)
                }
                None => ResponseEnvelope::not_found(ctx.request_id, &name),
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::context::{CompositionAuthority, ScopedOperationEnv};
    use crate::registry::registration::{make_handler, HandlerRegistration, OperationProvenance};
    use alknet_core::types::Capabilities;
    use std::collections::HashMap;
    use std::time::Duration;

    fn external_spec(name: &str) -> OperationSpec {
        OperationSpec::new(
            name,
            OperationType::Query,
            Visibility::External,
            json!({}),
            json!({}),
            vec![],
            AccessControl::default(),
        )
    }

    fn internal_spec(name: &str) -> OperationSpec {
        OperationSpec::new(
            name,
            OperationType::Mutation,
            Visibility::Internal,
            json!({}),
            json!({}),
            vec![],
            AccessControl::default(),
        )
    }

    fn echo_handler() -> Handler {
        make_handler(
            |input, context| async move { ResponseEnvelope::ok(context.request_id, input) },
        )
    }

    fn noop_env() -> Arc<dyn crate::registry::env::OperationEnv + Send + Sync> {
        struct NoopEnv;
        #[async_trait::async_trait]
        impl crate::registry::env::OperationEnv for NoopEnv {
            async fn invoke_with_policy(
                &self,
                _ns: &str,
                _op: &str,
                _input: Value,
                _parent: &OperationContext,
                _policy: crate::registry::context::AbortPolicy,
            ) -> ResponseEnvelope {
                ResponseEnvelope::error("test", CallError::internal("noop env does not dispatch"))
            }
            fn contains(&self, _name: &str) -> bool {
                false
            }
        }
        Arc::new(NoopEnv)
    }

    fn root_context(request_id: &str) -> OperationContext {
        OperationContext {
            request_id: request_id.to_string(),
            parent_request_id: None,
            identity: None,
            handler_identity: None,
            forwarded_for: None,
            capabilities: Capabilities::new(),
            metadata: HashMap::new(),
            scoped_env: ScopedOperationEnv::empty(),
            env: noop_env(),
            abort_policy: crate::registry::context::AbortPolicy::default(),
            deadline: Some(std::time::Instant::now() + Duration::from_secs(30)),
            internal: false,
        }
    }

    fn root_context_with_identity(
        request_id: &str,
        identity: Option<alknet_core::auth::Identity>,
    ) -> OperationContext {
        OperationContext {
            request_id: request_id.to_string(),
            parent_request_id: None,
            identity,
            handler_identity: None,
            forwarded_for: None,
            capabilities: Capabilities::new(),
            metadata: HashMap::new(),
            scoped_env: ScopedOperationEnv::empty(),
            env: noop_env(),
            abort_policy: crate::registry::context::AbortPolicy::default(),
            deadline: Some(std::time::Instant::now() + Duration::from_secs(30)),
            internal: false,
        }
    }

    fn identity_with_scopes(id: &str, scopes: &[&str]) -> alknet_core::auth::Identity {
        alknet_core::auth::Identity {
            id: id.to_string(),
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
            resources: HashMap::new(),
        }
    }

    fn external_spec_with_acl(name: &str, acl: AccessControl) -> OperationSpec {
        OperationSpec::new(
            name,
            OperationType::Query,
            Visibility::External,
            json!({}),
            json!({}),
            vec![],
            acl,
        )
    }

    fn registry_with_access_controlled_ops() -> Arc<OperationRegistry> {
        let mut registry = OperationRegistry::new();
        registry.register(HandlerRegistration::new(
            external_spec_with_acl("public/echo", AccessControl::default()),
            echo_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        registry.register(HandlerRegistration::new(
            external_spec_with_acl(
                "admin/secret",
                AccessControl {
                    required_scopes: vec!["admin".to_string()],
                    ..Default::default()
                },
            ),
            echo_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        registry.register(HandlerRegistration::new(
            internal_spec("internal/hidden"),
            echo_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        Arc::new(registry)
    }

    fn op_names(response: ResponseEnvelope) -> Vec<String> {
        let output = response.result.expect("ok response");
        output
            .get("operations")
            .and_then(|v| v.as_array())
            .expect("operations array")
            .iter()
            .filter_map(|o| o.get("name").and_then(|n| n.as_str()).map(String::from))
            .collect()
    }

    fn registry_with_ops() -> Arc<OperationRegistry> {
        let mut registry = OperationRegistry::new();
        registry.register(HandlerRegistration::new(
            external_spec("fs/readFile"),
            echo_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        registry.register(HandlerRegistration::new(
            internal_spec("secret/internal"),
            echo_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        registry.register(HandlerRegistration::new(
            OperationSpec::new(
                "events/subscribe",
                OperationType::Subscription,
                Visibility::External,
                json!({}),
                json!({}),
                vec![],
                AccessControl::default(),
            ),
            echo_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        registry.register(HandlerRegistration::new(
            OperationSpec::new(
                "fs/readFileErr",
                OperationType::Query,
                Visibility::External,
                json!({}),
                json!({}),
                vec![super::super::spec::ErrorDefinition {
                    code: "FILE_NOT_FOUND".to_string(),
                    description: "file not found".to_string(),
                    schema: json!({ "type": "object" }),
                    http_status: None,
                }],
                AccessControl::default(),
            ),
            echo_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        Arc::new(registry)
    }

    #[test]
    fn services_list_spec_has_correct_fields() {
        let spec = services_list_spec();
        assert_eq!(spec.name, NAME_SERVICES_LIST);
        assert_eq!(spec.namespace, "services");
        assert_eq!(spec.op_type, OperationType::Query);
        assert_eq!(spec.visibility, Visibility::External);
        assert_eq!(spec.input_schema, json!({}));
        assert!(spec.output_schema.get("properties").is_some());
        assert!(spec.error_schemas.is_empty());
        assert!(!spec.access_control.has_restrictions());
    }

    #[test]
    fn services_schema_spec_has_correct_fields() {
        let spec = services_schema_spec();
        assert_eq!(spec.name, NAME_SERVICES_SCHEMA);
        assert_eq!(spec.namespace, "services");
        assert_eq!(spec.op_type, OperationType::Query);
        assert_eq!(spec.visibility, Visibility::External);
        assert!(spec.input_schema.get("required").is_some());
        assert!(spec.output_schema.get("properties").is_some());
        assert!(spec.error_schemas.is_empty());
        assert!(!spec.access_control.has_restrictions());
    }

    #[tokio::test]
    async fn services_list_returns_external_ops_only() {
        let registry = registry_with_ops();
        let handler = services_list_handler(Arc::clone(&registry));
        let ctx = root_context("req-1");
        let response = handler(serde_json::json!({}), ctx).await;
        let output = response.result.expect("ok response");
        let ops = output
            .get("operations")
            .and_then(|v| v.as_array())
            .expect("operations array");
        let names: Vec<&str> = ops
            .iter()
            .filter_map(|o| o.get("name").and_then(|n| n.as_str()))
            .collect();
        assert!(names.contains(&"fs/readFile"));
        assert!(names.contains(&"events/subscribe"));
        assert!(names.contains(&"fs/readFileErr"));
        assert!(
            !names.contains(&"secret/internal"),
            "internal ops must not be listed"
        );
    }

    #[tokio::test]
    async fn services_list_output_format_matches_spec() {
        let registry = registry_with_ops();
        let handler = services_list_handler(Arc::clone(&registry));
        let ctx = root_context("req-1");
        let response = handler(serde_json::json!({}), ctx).await;
        let output = response.result.expect("ok response");
        let ops = output
            .get("operations")
            .and_then(|v| v.as_array())
            .expect("operations array");
        let fs_op = ops
            .iter()
            .find(|o| o.get("name").and_then(|n| n.as_str()) == Some("fs/readFile"))
            .expect("fs/readFile present");
        assert_eq!(fs_op.get("namespace"), Some(&json!("fs")));
        assert_eq!(fs_op.get("op_type"), Some(&json!("query")));
    }

    #[tokio::test]
    async fn services_schema_returns_spec_for_known_op() {
        let registry = registry_with_ops();
        let handler = services_schema_handler(Arc::clone(&registry));
        let ctx = root_context("req-2");
        let response = handler(serde_json::json!({ "name": "fs/readFileErr" }), ctx).await;
        let spec = response.result.expect("ok response");
        assert_eq!(spec.get("name"), Some(&json!("fs/readFileErr")));
        assert_eq!(spec.get("namespace"), Some(&json!("fs")));
        assert_eq!(spec.get("op_type"), Some(&json!("query")));
        let error_schemas = spec
            .get("error_schemas")
            .and_then(|v| v.as_array())
            .expect("error_schemas array");
        assert_eq!(error_schemas.len(), 1);
        assert_eq!(error_schemas[0].get("code"), Some(&json!("FILE_NOT_FOUND")));
    }

    #[tokio::test]
    async fn services_schema_returns_not_found_for_unknown_op() {
        let registry = registry_with_ops();
        let handler = services_schema_handler(Arc::clone(&registry));
        let ctx = root_context("req-3");
        let response = handler(serde_json::json!({ "name": "no/such" }), ctx).await;
        match response.result {
            Err(e) => assert_eq!(e.code, "NOT_FOUND"),
            other => panic!("expected NOT_FOUND, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn services_schema_accepts_name_with_leading_slash() {
        let registry = registry_with_ops();
        let handler = services_schema_handler(Arc::clone(&registry));
        let ctx = root_context("req-4");
        let response = handler(serde_json::json!({ "name": "/fs/readFile" }), ctx).await;
        let spec = response.result.expect("ok response");
        assert_eq!(spec.get("name"), Some(&json!("fs/readFile")));
    }

    #[tokio::test]
    async fn services_schema_rejects_missing_name() {
        let registry = registry_with_ops();
        let handler = services_schema_handler(Arc::clone(&registry));
        let ctx = root_context("req-5");
        let response = handler(serde_json::json!({}), ctx).await;
        match response.result {
            Err(e) => assert_eq!(e.code, "INVALID_INPUT"),
            other => panic!("expected INVALID_INPUT, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn services_list_handler_registered_and_invocable_via_registry() {
        let registry = registry_with_ops();
        let list_handler = services_list_handler(Arc::clone(&registry));
        let schema_handler = services_schema_handler(Arc::clone(&registry));

        let mut discovery_registry = OperationRegistry::new();
        discovery_registry.register(HandlerRegistration::new(
            services_list_spec(),
            list_handler,
            OperationProvenance::Local,
            CompositionAuthority::none(),
            ScopedOperationEnv::empty().into(),
            Capabilities::new(),
        ));
        discovery_registry.register(HandlerRegistration::new(
            services_schema_spec(),
            schema_handler,
            OperationProvenance::Local,
            CompositionAuthority::none(),
            ScopedOperationEnv::empty().into(),
            Capabilities::new(),
        ));
        let discovery = Arc::new(discovery_registry);

        let ctx = root_context("req-6");
        let response = discovery
            .invoke(NAME_SERVICES_LIST, serde_json::json!({}), ctx)
            .await;
        let output = response.result.expect("list ok");
        assert!(output.get("operations").is_some());
    }

    #[test]
    fn normalize_name_strips_leading_slash() {
        assert_eq!(normalize_name("/fs/readFile"), "fs/readFile");
        assert_eq!(normalize_name("fs/readFile"), "fs/readFile");
    }

    #[test]
    fn op_type_str_matches_wire_enum() {
        assert_eq!(op_type_str(OperationType::Query), "query");
        assert_eq!(op_type_str(OperationType::Mutation), "mutation");
        assert_eq!(op_type_str(OperationType::Subscription), "subscription");
    }

    #[test]
    fn visibility_str_matches_wire_enum() {
        assert_eq!(visibility_str(Visibility::External), "external");
        assert_eq!(visibility_str(Visibility::Internal), "internal");
    }

    #[test]
    fn spec_to_json_round_trips_error_schemas() {
        let spec = OperationSpec::new(
            "fs/readFile",
            OperationType::Query,
            Visibility::External,
            json!({ "type": "object" }),
            json!({ "type": "string" }),
            vec![super::super::spec::ErrorDefinition {
                code: "FILE_NOT_FOUND".to_string(),
                description: "file not found".to_string(),
                schema: json!({ "type": "object", "properties": { "path": { "type": "string" } } }),
                http_status: Some(404),
            }],
            AccessControl {
                required_scopes: vec!["fs:read".to_string()],
                ..Default::default()
            },
        );
        let json_val = spec_to_json(&spec);
        let error_schemas = json_val
            .get("error_schemas")
            .and_then(|v| v.as_array())
            .expect("error_schemas");
        assert_eq!(error_schemas.len(), 1);
        assert_eq!(error_schemas[0].get("code"), Some(&json!("FILE_NOT_FOUND")));
        assert_eq!(error_schemas[0].get("http_status"), Some(&json!(404)));
        let acl = json_val.get("access_control").expect("access_control");
        assert_eq!(acl.get("required_scopes"), Some(&json!(["fs:read"])));
    }

    #[tokio::test]
    async fn services_list_filters_by_access_control_authorized_peer() {
        let registry = registry_with_access_controlled_ops();
        let handler = services_list_handler(Arc::clone(&registry));
        let ctx = root_context_with_identity(
            "req-acl-1",
            Some(identity_with_scopes("admin-peer", &["admin"])),
        );
        let names = op_names(handler(serde_json::json!({}), ctx).await);
        assert!(names.contains(&"public/echo".to_string()));
        assert!(names.contains(&"admin/secret".to_string()));
        assert!(!names.contains(&"internal/hidden".to_string()));
    }

    #[tokio::test]
    async fn services_list_filters_by_access_control_unauthorized_peer() {
        let registry = registry_with_access_controlled_ops();
        let handler = services_list_handler(Arc::clone(&registry));
        let ctx = root_context_with_identity(
            "req-acl-2",
            Some(identity_with_scopes("regular-peer", &["user"])),
        );
        let names = op_names(handler(serde_json::json!({}), ctx).await);
        assert!(names.contains(&"public/echo".to_string()));
        assert!(
            !names.contains(&"admin/secret".to_string()),
            "unauthorized peer must not see admin/secret"
        );
        assert!(!names.contains(&"internal/hidden".to_string()));
    }

    #[tokio::test]
    async fn services_list_op_with_default_acl_listed_to_any_peer() {
        let registry = registry_with_access_controlled_ops();
        let handler = services_list_handler(Arc::clone(&registry));
        let ctx = root_context_with_identity("req-acl-3", None);
        let names = op_names(handler(serde_json::json!({}), ctx).await);
        assert!(
            names.contains(&"public/echo".to_string()),
            "default AccessControl op must be listed to unauthenticated peer"
        );
        assert!(!names.contains(&"admin/secret".to_string()));
    }

    #[tokio::test]
    async fn services_list_peers_attributes_ops_by_peer_id() {
        struct PeerEnv {
            peers: HashMap<String, Vec<String>>,
        }
        #[async_trait::async_trait]
        impl crate::registry::env::OperationEnv for PeerEnv {
            async fn invoke_with_policy(
                &self,
                _ns: &str,
                _op: &str,
                _input: Value,
                parent: &OperationContext,
                _policy: crate::registry::context::AbortPolicy,
            ) -> ResponseEnvelope {
                ResponseEnvelope::ok(parent.request_id.clone(), json!({}))
            }
            fn contains(&self, _name: &str) -> bool {
                false
            }
            fn peer_ids(&self) -> Vec<crate::registry::env::PeerId> {
                self.peers.keys().cloned().collect()
            }
            fn peer_operations(&self, peer: &crate::registry::env::PeerId) -> Vec<String> {
                self.peers.get(peer).cloned().unwrap_or_default()
            }
        }

        let mut peers = HashMap::new();
        peers.insert(
            "worker-a".to_string(),
            vec!["container/exec".to_string(), "container/logs".to_string()],
        );
        peers.insert("worker-b".to_string(), vec!["container/exec".to_string()]);
        let env: Arc<dyn crate::registry::env::OperationEnv + Send + Sync> =
            Arc::new(PeerEnv { peers });

        let registry = registry_with_access_controlled_ops();
        let handler = services_list_peers_handler(Arc::clone(&registry));
        let ctx = OperationContext {
            request_id: "req-peers-1".to_string(),
            parent_request_id: None,
            identity: None,
            handler_identity: None,
            forwarded_for: None,
            capabilities: Capabilities::new(),
            metadata: HashMap::new(),
            scoped_env: ScopedOperationEnv::empty(),
            env,
            abort_policy: crate::registry::context::AbortPolicy::default(),
            deadline: Some(std::time::Instant::now() + Duration::from_secs(30)),
            internal: false,
        };
        let response = handler(serde_json::json!({}), ctx).await;
        let output = response.result.expect("ok response");
        let peers_arr = output
            .get("peers")
            .and_then(|v| v.as_array())
            .expect("peers array");
        let peer_ids: Vec<&str> = peers_arr
            .iter()
            .filter_map(|p| p.get("peer_id").and_then(|v| v.as_str()))
            .collect();
        assert!(peer_ids.contains(&"local"));
        assert!(peer_ids.contains(&"worker-a"));
        assert!(peer_ids.contains(&"worker-b"));
        let worker_a = peers_arr
            .iter()
            .find(|p| p.get("peer_id").and_then(|v| v.as_str()) == Some("worker-a"))
            .expect("worker-a present");
        let worker_a_ops = worker_a
            .get("operations")
            .and_then(|v| v.as_array())
            .expect("worker-a operations");
        let worker_a_names: Vec<&str> = worker_a_ops
            .iter()
            .filter_map(|o| o.get("name").and_then(|n| n.as_str()))
            .collect();
        assert!(worker_a_names.contains(&"container/exec"));
        assert!(worker_a_names.contains(&"container/logs"));
    }

    #[test]
    fn services_list_peers_spec_has_correct_fields() {
        let spec = services_list_peers_spec();
        assert_eq!(spec.name, NAME_SERVICES_LIST_PEERS);
        assert_eq!(spec.namespace, "services");
        assert_eq!(spec.op_type, OperationType::Query);
        assert_eq!(spec.visibility, Visibility::External);
        assert!(spec.error_schemas.is_empty());
        assert!(!spec.access_control.has_restrictions());
    }

    #[tokio::test]
    async fn services_list_peers_filters_by_access_control() {
        struct PeerEnv;
        #[async_trait::async_trait]
        impl crate::registry::env::OperationEnv for PeerEnv {
            async fn invoke_with_policy(
                &self,
                _ns: &str,
                _op: &str,
                _input: Value,
                parent: &OperationContext,
                _policy: crate::registry::context::AbortPolicy,
            ) -> ResponseEnvelope {
                ResponseEnvelope::ok(parent.request_id.clone(), json!({}))
            }
            fn contains(&self, _name: &str) -> bool {
                false
            }
            fn peer_ids(&self) -> Vec<crate::registry::env::PeerId> {
                vec!["restricted-peer".to_string()]
            }
            fn peer_operations(&self, _peer: &crate::registry::env::PeerId) -> Vec<String> {
                vec!["admin/secret".to_string(), "public/echo".to_string()]
            }
        }

        let registry = registry_with_access_controlled_ops();
        let handler = services_list_peers_handler(Arc::clone(&registry));
        let env: Arc<dyn crate::registry::env::OperationEnv + Send + Sync> = Arc::new(PeerEnv);
        let ctx = OperationContext {
            request_id: "req-peers-2".to_string(),
            parent_request_id: None,
            identity: Some(identity_with_scopes("regular-peer", &["user"])),
            handler_identity: None,
            forwarded_for: None,
            capabilities: Capabilities::new(),
            metadata: HashMap::new(),
            scoped_env: ScopedOperationEnv::empty(),
            env,
            abort_policy: crate::registry::context::AbortPolicy::default(),
            deadline: Some(std::time::Instant::now() + Duration::from_secs(30)),
            internal: false,
        };
        let response = handler(serde_json::json!({}), ctx).await;
        let output = response.result.expect("ok response");
        let peers_arr = output
            .get("peers")
            .and_then(|v| v.as_array())
            .expect("peers array");
        let restricted = peers_arr
            .iter()
            .find(|p| p.get("peer_id").and_then(|v| v.as_str()) == Some("restricted-peer"))
            .expect("restricted-peer present");
        let ops = restricted
            .get("operations")
            .and_then(|v| v.as_array())
            .expect("operations");
        let names: Vec<&str> = ops
            .iter()
            .filter_map(|o| o.get("name").and_then(|n| n.as_str()))
            .collect();
        assert!(names.contains(&"public/echo"));
        assert!(
            !names.contains(&"admin/secret"),
            "unauthorized peer must not see admin op in list-peers"
        );
    }
}
