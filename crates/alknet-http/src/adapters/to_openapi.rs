//! `to_openapi`: the OpenAPI gateway projection (ADR-042). Generates a
//! fixed 5-endpoint gateway doc (`/search`, `/schema`, `/call`, `/batch`,
//! `/subscribe`) that gates access to the full operation registry — not one
//! path per operation. Served at `GET /openapi.json` by the HTTP server.
//!
//! Pure projection (ADR-017 §5): consumes the registry, does not produce
//! entries, is not an `OperationAdapter`. The per-caller operation surface
//! is discovered via `/search` (AccessControl-filtered at runtime), not
//! preloaded into the doc (ADR-042 §3). `info.version` is a constant
//! semver tracking the gateway endpoint contract, not the operation set
//! (ADR-045); the initial version is `1.0.0`.
//!
//! Error fidelity (ADR-023): `/call`'s responses include the protocol-
//! level errors (400/401/403/404/500/504) plus the operation-level errors
//! declared in registry `error_schemas` (mapped by `http_status`).

use alknet_call::registry::registration::OperationRegistry;
use alknet_call::registry::spec::Visibility;
use serde_json::{json, Map, Value};

use crate::adapters::OpenAPISpec;

const GATEWAY_VERSION: &str = "1.0.0";
const GATEWAY_TITLE: &str = "alknet gateway";

const PATH_SEARCH: &str = "/search";
const PATH_SCHEMA: &str = "/schema";
const PATH_CALL: &str = "/call";
const PATH_BATCH: &str = "/batch";
const PATH_SUBSCRIBE: &str = "/subscribe";

const CONTENT_JSON: &str = "application/json";
const CONTENT_SSE: &str = "text/event-stream";

const STATUS_BAD_REQUEST: &str = "400";
const STATUS_UNAUTHORIZED: &str = "401";
const STATUS_FORBIDDEN: &str = "403";
const STATUS_NOT_FOUND: &str = "404";
const STATUS_INTERNAL: &str = "500";
const STATUS_TIMEOUT: &str = "504";

pub fn to_openapi(registry: &OperationRegistry) -> OpenAPISpec {
    let mut paths_obj = Map::new();
    paths_obj.insert(
        PATH_SEARCH.to_string(),
        path_item("get", search_operation()),
    );
    paths_obj.insert(
        PATH_SCHEMA.to_string(),
        path_item("get", schema_operation()),
    );
    paths_obj.insert(
        PATH_CALL.to_string(),
        path_item("post", call_operation(registry)),
    );
    paths_obj.insert(PATH_BATCH.to_string(), path_item("post", batch_operation()));
    paths_obj.insert(
        PATH_SUBSCRIBE.to_string(),
        path_item("post", subscribe_operation()),
    );

    let doc = json!({
        "openapi": "3.0.0",
        "info": {
            "title": GATEWAY_TITLE,
            "version": GATEWAY_VERSION,
        },
        "paths": Value::Object(paths_obj),
    });

    OpenAPISpec::from_value(doc).expect("generated gateway doc is a valid OpenAPI 3.0 object")
}

fn path_item(method: &str, operation: Value) -> Value {
    let mut item = Map::new();
    item.insert(method.to_string(), operation);
    Value::Object(item)
}

fn search_operation() -> Value {
    json!({
        "operationId": "search",
        "summary": "List/search available operations (AccessControl-filtered). Returns names + descriptions.",
        "responses": {
            "200": json_response(search_output_schema()),
            STATUS_BAD_REQUEST: error_response("INVALID_INPUT", "Malformed query."),
            STATUS_UNAUTHORIZED: error_response("UNAUTHORIZED", "Missing bearer token."),
            STATUS_FORBIDDEN: error_response("FORBIDDEN", "Insufficient scopes."),
            STATUS_INTERNAL: error_response("INTERNAL", "Internal error."),
            STATUS_TIMEOUT: error_response("TIMEOUT", "Request timed out."),
        }
    })
}

fn schema_operation() -> Value {
    json!({
        "operationId": "schema",
        "summary": "Get an operation's full OperationSpec (input/output JSON Schemas, error schemas).",
        "parameters": [{
            "name": "name",
            "in": "query",
            "required": true,
            "schema": { "type": "string" }
        }],
        "responses": {
            "200": json_response(schema_output_schema()),
            STATUS_BAD_REQUEST: error_response("INVALID_INPUT", "Missing or malformed `name` parameter."),
            STATUS_UNAUTHORIZED: error_response("UNAUTHORIZED", "Missing bearer token."),
            STATUS_FORBIDDEN: error_response("FORBIDDEN", "Insufficient scopes for the requested operation."),
            STATUS_NOT_FOUND: error_response("NOT_FOUND", "Operation not registered."),
            STATUS_INTERNAL: error_response("INTERNAL", "Internal error."),
            STATUS_TIMEOUT: error_response("TIMEOUT", "Request timed out."),
        }
    })
}

fn call_operation(registry: &OperationRegistry) -> Value {
    let mut responses = Map::new();
    responses.insert("200".to_string(), json_response(call_success_schema()));
    responses.insert(
        STATUS_BAD_REQUEST.to_string(),
        error_response(
            "INVALID_INPUT",
            "The request body was not a valid `{ operation, input }` object.",
        ),
    );
    responses.insert(
        STATUS_UNAUTHORIZED.to_string(),
        error_response("UNAUTHORIZED", "No bearer token provided."),
    );
    responses.insert(
        STATUS_FORBIDDEN.to_string(),
        error_response(
            "FORBIDDEN",
            "Insufficient scopes to invoke the requested operation.",
        ),
    );
    responses.insert(
        STATUS_NOT_FOUND.to_string(),
        error_response("NOT_FOUND", "Operation not registered (or is Internal)."),
    );
    responses.insert(
        STATUS_INTERNAL.to_string(),
        error_response("INTERNAL", "Internal error."),
    );
    responses.insert(
        STATUS_TIMEOUT.to_string(),
        error_response("TIMEOUT", "Request timed out."),
    );

    for spec in registry.list_operations() {
        if spec.visibility != Visibility::External {
            continue;
        }
        for error in &spec.error_schemas {
            let Some(status) = error.http_status else {
                continue;
            };
            let code = format!("{status}");
            if responses.contains_key(&code) {
                continue;
            }
            responses.insert(code, json_response(error.schema.clone()));
        }
    }

    json!({
        "operationId": "call",
        "summary": "Invoke an operation by name with a flat JSON body `{ operation, input }`.",
        "requestBody": {
            "required": true,
            "content": {
                CONTENT_JSON: {
                    "schema": call_input_schema(),
                }
            }
        },
        "responses": Value::Object(responses),
    })
}

fn batch_operation() -> Value {
    json!({
        "operationId": "batch",
        "summary": "Invoke multiple operations in one request. Array of `{ operation, input }`.",
        "requestBody": {
            "required": true,
            "content": {
                CONTENT_JSON: {
                    "schema": batch_input_schema(),
                }
            }
        },
        "responses": {
            "200": json_response(batch_output_schema()),
            STATUS_BAD_REQUEST: error_response("INVALID_INPUT", "The request body was not a JSON array of call requests."),
            STATUS_UNAUTHORIZED: error_response("UNAUTHORIZED", "Missing bearer token."),
            STATUS_FORBIDDEN: error_response("FORBIDDEN", "Insufficient scopes."),
            STATUS_INTERNAL: error_response("INTERNAL", "Internal error."),
            STATUS_TIMEOUT: error_response("TIMEOUT", "Request timed out."),
        }
    })
}

fn subscribe_operation() -> Value {
    let mut responses = Map::new();
    responses.insert("200".to_string(), sse_response(call_success_schema()));
    responses.insert(
        STATUS_BAD_REQUEST.to_string(),
        error_response(
            "INVALID_INPUT",
            "The request body was not a valid `{ operation, input }` object.",
        ),
    );
    responses.insert(
        STATUS_UNAUTHORIZED.to_string(),
        error_response("UNAUTHORIZED", "No bearer token provided."),
    );
    responses.insert(
        STATUS_FORBIDDEN.to_string(),
        error_response(
            "FORBIDDEN",
            "Insufficient scopes to invoke the requested operation.",
        ),
    );
    responses.insert(
        STATUS_NOT_FOUND.to_string(),
        error_response("NOT_FOUND", "Operation not registered (or is Internal)."),
    );
    responses.insert(
        STATUS_INTERNAL.to_string(),
        error_response("INTERNAL", "Internal error."),
    );
    responses.insert(
        STATUS_TIMEOUT.to_string(),
        error_response("TIMEOUT", "Request timed out."),
    );

    json!({
        "operationId": "subscribe",
        "summary": "Invoke a streaming operation. Body `{ operation, input }`; response is `text/event-stream`.",
        "requestBody": {
            "required": true,
            "content": {
                CONTENT_JSON: {
                    "schema": call_input_schema(),
                }
            }
        },
        "responses": Value::Object(responses),
    })
}

fn call_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "operation": {
                "type": "string",
                "description": "The fully-qualified operation name to invoke."
            },
            "input": {
                "type": "object",
                "description": "The JSON input object to pass to the operation."
            }
        },
        "required": ["operation"]
    })
}

fn batch_input_schema() -> Value {
    json!({
        "type": "array",
        "items": call_input_schema()
    })
}

fn search_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "operations": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "description": { "type": "string" }
                    }
                }
            }
        }
    })
}

fn schema_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "namespace": { "type": "string" },
            "op_type": { "type": "string" },
            "input_schema": {},
            "output_schema": {},
            "error_schemas": { "type": "array" },
            "access_control": {}
        }
    })
}

fn call_success_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "request_id": { "type": "string" },
            "result": { "type": "string", "enum": ["ok"] },
            "output": {}
        }
    })
}

fn batch_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "results": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "request_id": { "type": "string" },
                        "result": { "type": "string" },
                        "output": {},
                        "error": {}
                    }
                }
            }
        }
    })
}

fn json_response(schema: Value) -> Value {
    json!({
        "description": "",
        "content": {
            CONTENT_JSON: {
                "schema": schema,
            }
        }
    })
}

fn sse_response(schema: Value) -> Value {
    json!({
        "description": "",
        "content": {
            CONTENT_SSE: {
                "schema": schema,
            }
        }
    })
}

fn error_response(code: &str, message: &str) -> Value {
    json!({
        "description": message,
        "content": {
            CONTENT_JSON: {
                "schema": {
                    "type": "object",
                    "properties": {
                        "code": { "type": "string", "enum": [code] },
                        "message": { "type": "string" },
                        "retryable": { "type": "boolean" }
                    },
                    "required": ["code", "message", "retryable"]
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alknet_call::protocol::wire::ResponseEnvelope;
    use alknet_call::registry::registration::{
        make_handler, HandlerRegistration, OperationProvenance,
    };
    use alknet_call::registry::spec::{
        AccessControl, ErrorDefinition, OperationSpec, OperationType,
    };
    use alknet_core::types::Capabilities;

    fn echo_handler() -> alknet_call::registry::registration::Handler {
        make_handler(|input, ctx| async move { ResponseEnvelope::ok(ctx.request_id, input) })
    }

    fn register_op(registry: &mut OperationRegistry, spec: OperationSpec) {
        registry.register(HandlerRegistration::new(
            spec,
            echo_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
    }

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

    fn spec_with_errors(name: &str, errors: Vec<ErrorDefinition>) -> OperationSpec {
        OperationSpec::new(
            name,
            OperationType::Mutation,
            Visibility::External,
            json!({}),
            json!({}),
            errors,
            AccessControl::default(),
        )
    }

    fn err(code: &str, status: Option<u16>) -> ErrorDefinition {
        ErrorDefinition {
            code: code.to_string(),
            description: format!("{code} error"),
            schema: json!({ "type": "object", "properties": { "msg": { "type": "string" } } }),
            http_status: status,
        }
    }

    fn paths(spec: &OpenAPISpec) -> Vec<String> {
        spec.paths.keys().cloned().collect()
    }

    #[test]
    fn generated_doc_has_exactly_five_gateway_paths() {
        let registry = OperationRegistry::new();
        let spec = to_openapi(&registry);
        let mut p = paths(&spec);
        p.sort();
        assert_eq!(
            p,
            vec!["/batch", "/call", "/schema", "/search", "/subscribe"]
        );
    }

    #[test]
    fn generated_doc_does_not_leak_registry_operations_as_paths() {
        let mut registry = OperationRegistry::new();
        register_op(&mut registry, external_spec("fs/readFile"));
        register_op(&mut registry, external_spec("agent/chat"));
        let spec = to_openapi(&registry);
        let p = paths(&spec);
        assert!(!p.contains(&"/fs/readFile".to_string()));
        assert!(!p.contains(&"/agent/chat".to_string()));
        assert_eq!(p.len(), 5);
    }

    #[test]
    fn info_version_is_1_0_0() {
        let registry = OperationRegistry::new();
        let spec = to_openapi(&registry);
        assert_eq!(spec.info.version, "1.0.0");
    }

    #[test]
    fn call_request_schema_is_operation_and_input() {
        let registry = OperationRegistry::new();
        let spec = to_openapi(&registry);
        let call = &spec.paths["/call"].operations[0].1;
        let body = call.request_body.as_ref().expect("request body");
        let schema = body.content.get(CONTENT_JSON).expect("json content");
        let props = schema
            .get("properties")
            .and_then(Value::as_object)
            .expect("properties");
        assert!(props.contains_key("operation"));
        let input = props.get("input").expect("input");
        assert_eq!(input.get("type").and_then(Value::as_str), Some("object"));
        let required = schema
            .get("required")
            .and_then(Value::as_array)
            .expect("required");
        assert!(required.iter().any(|v| v == "operation"));
    }

    #[test]
    fn subscribe_response_content_type_is_text_event_stream() {
        let registry = OperationRegistry::new();
        let spec = to_openapi(&registry);
        let subscribe = &spec.paths["/subscribe"].operations[0].1;
        let resp = &subscribe.responses["200"];
        assert!(resp.content.contains_key(CONTENT_SSE));
        assert!(!resp.content.contains_key(CONTENT_JSON));
    }

    #[test]
    fn call_responses_include_all_protocol_level_error_statuses() {
        let registry = OperationRegistry::new();
        let spec = to_openapi(&registry);
        let call = &spec.paths["/call"].operations[0].1;
        for status in ["400", "401", "403", "404", "500", "504"] {
            assert!(
                call.responses.contains_key(status),
                "missing protocol-level response {status}"
            );
        }
    }

    #[test]
    fn call_responses_include_operation_level_errors_with_http_status() {
        let mut registry = OperationRegistry::new();
        register_op(
            &mut registry,
            spec_with_errors(
                "svc/op",
                vec![
                    err("RATE_LIMITED", Some(429)),
                    err("UNPROCESSABLE", Some(422)),
                ],
            ),
        );
        let spec = to_openapi(&registry);
        let call = &spec.paths["/call"].operations[0].1;
        assert!(call.responses.contains_key("429"));
        assert!(call.responses.contains_key("422"));
        let resp429 = &call.responses["429"];
        let schema = resp429
            .content
            .get(CONTENT_JSON)
            .and_then(|v| v.get("properties"))
            .and_then(|v| v.get("msg"))
            .expect("projected error schema");
        assert_eq!(schema.get("type").and_then(Value::as_str), Some("string"));
    }

    #[test]
    fn call_responses_project_http_404_error_code_as_404_response() {
        let mut registry = OperationRegistry::new();
        register_op(
            &mut registry,
            spec_with_errors("svc/op", vec![err("HTTP_404", Some(404))]),
        );
        let spec = to_openapi(&registry);
        let call = &spec.paths["/call"].operations[0].1;
        assert!(call.responses.contains_key("404"));
    }

    #[test]
    fn call_responses_do_not_duplicate_protocol_level_status_with_operation_error() {
        let mut registry = OperationRegistry::new();
        register_op(
            &mut registry,
            spec_with_errors("svc/op", vec![err("HTTP_500", Some(500))]),
        );
        let spec = to_openapi(&registry);
        let call = &spec.paths["/call"].operations[0].1;
        assert!(call.responses.contains_key("500"));
    }

    #[test]
    fn operation_errors_without_http_status_are_not_projected() {
        let mut registry = OperationRegistry::new();
        register_op(
            &mut registry,
            spec_with_errors("svc/op", vec![err("FILE_NOT_FOUND", None)]),
        );
        let spec = to_openapi(&registry);
        let call = &spec.paths["/call"].operations[0].1;
        assert!(!call.responses.contains_key("0"));
        assert!(call.responses.contains_key("500"));
    }

    #[test]
    fn to_openapi_is_a_pure_projection_and_not_an_operation_adapter() {
        fn assert_not_adapter<T>() {}
        assert_not_adapter::<fn(&OperationRegistry) -> OpenAPISpec>();
        let mut registry = OperationRegistry::new();
        register_op(&mut registry, external_spec("svc/op"));
        let before = registry.list_operations().len();
        let _ = to_openapi(&registry);
        assert_eq!(registry.list_operations().len(), before);
    }

    #[test]
    fn batch_request_schema_is_array_of_call_request() {
        let registry = OperationRegistry::new();
        let spec = to_openapi(&registry);
        let batch = &spec.paths["/batch"].operations[0].1;
        let body = batch.request_body.as_ref().expect("request body");
        let schema = body.content.get(CONTENT_JSON).expect("json content");
        assert_eq!(schema.get("type").and_then(Value::as_str), Some("array"));
    }

    #[test]
    fn subscribe_request_body_uses_call_input_schema() {
        let registry = OperationRegistry::new();
        let spec = to_openapi(&registry);
        let subscribe = &spec.paths["/subscribe"].operations[0].1;
        let body = subscribe.request_body.as_ref().expect("request body");
        let schema = body.content.get(CONTENT_JSON).expect("json content");
        assert!(schema
            .get("properties")
            .and_then(Value::as_object)
            .map(|m| m.contains_key("operation"))
            .unwrap_or(false));
    }

    #[test]
    fn search_and_schema_are_get_operations() {
        let registry = OperationRegistry::new();
        let spec = to_openapi(&registry);
        assert_eq!(spec.paths["/search"].operations[0].0, "get");
        assert_eq!(spec.paths["/schema"].operations[0].0, "get");
    }

    #[test]
    fn call_batch_subscribe_are_post_operations() {
        let registry = OperationRegistry::new();
        let spec = to_openapi(&registry);
        assert_eq!(spec.paths["/call"].operations[0].0, "post");
        assert_eq!(spec.paths["/batch"].operations[0].0, "post");
        assert_eq!(spec.paths["/subscribe"].operations[0].0, "post");
    }

    #[test]
    fn raw_doc_carries_openapi_3_0_and_gateway_version() {
        let registry = OperationRegistry::new();
        let spec = to_openapi(&registry);
        assert_eq!(
            spec.raw.get("openapi").and_then(Value::as_str),
            Some("3.0.0")
        );
        assert_eq!(
            spec.raw
                .get("info")
                .and_then(|i| i.get("version"))
                .and_then(Value::as_str),
            Some("1.0.0")
        );
    }
}
