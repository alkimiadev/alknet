//! `to_openapi`: gateway projection of the local operation registry into a
//! fixed 5-endpoint OpenAPI 3.0 document (ADR-042).
//!
//! `to_openapi` is a pure projection (ADR-017 §5): it consumes the registry
//! and produces a spec; it does not modify the registry, register
//! operations, or implement `OperationAdapter`. The generated doc describes
//! the 5 fixed gateway endpoints (`/search`, `/schema`, `/call`, `/batch`,
//! `/subscribe`) — the sole HTTP invoke path (ADR-047). The per-caller
//! operation surface is discovered at runtime through AccessControl-filtered
//! `/search`, not preloaded into the doc (ADR-042 §3).
//!
//! `info.version` is a semver constant tracking the **gateway endpoint
//! contract**, not the operation set — per-caller operation changes do not
//! bump the version (ADR-045). The initial version is `1.0.0`.
//!
//! Error fidelity (ADR-023): `/call`'s responses include the protocol-level
//! errors (400, 401, 403, 404, 500, 504) plus the operation-level errors
//! from the registry's `error_schemas`, mapped by `http_status`.
//! `HTTP_<status>`-prefixed codes project to their status without colliding
//! with the protocol-level codes.
//!
//! See `docs/architecture/crates/http/http-adapters.md` §"to_openapi" and
//! ADR-042/045/023.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use alknet_call::registry::registration::OperationRegistry;
use alknet_call::registry::spec::ErrorDefinition;

use super::from_openapi::OpenAPISpec;

const GATEWAY_VERSION: &str = "1.0.0";
const GATEWAY_TITLE: &str = "alknet gateway";
const OPENAPI_VERSION: &str = "3.0.0";

const PATH_SEARCH: &str = "/search";
const PATH_SCHEMA: &str = "/schema";
const PATH_CALL: &str = "/call";
const PATH_BATCH: &str = "/batch";
const PATH_SUBSCRIBE: &str = "/subscribe";

const STATUS_BAD_REQUEST: u16 = 400;
const STATUS_UNAUTHORIZED: u16 = 401;
const STATUS_FORBIDDEN: u16 = 403;
const STATUS_NOT_FOUND: u16 = 404;
const STATUS_INTERNAL: u16 = 500;
const STATUS_TIMEOUT: u16 = 504;

const CODE_INVALID_INPUT: &str = "INVALID_INPUT";
const CODE_FORBIDDEN: &str = "FORBIDDEN";
const CODE_NOT_FOUND: &str = "NOT_FOUND";
const CODE_INTERNAL: &str = "INTERNAL";
const CODE_TIMEOUT: &str = "TIMEOUT";

const HTTP_PREFIX: &str = "HTTP_";

pub fn to_openapi(registry: &OperationRegistry) -> OpenAPISpec {
    let operation_errors = collect_operation_errors(registry);
    let raw = build_doc(operation_errors);
    OpenAPISpec::from_value(raw).expect("to_openapi always emits a valid OpenAPI document")
}

fn build_doc(operation_errors: Vec<ErrorDefinition>) -> Value {
    let paths = json!({
        PATH_SEARCH: search_path_item(),
        PATH_SCHEMA: schema_path_item(),
        PATH_CALL: call_path_item(&operation_errors),
        PATH_BATCH: batch_path_item(),
        PATH_SUBSCRIBE: subscribe_path_item(),
    });

    json!({
        "openapi": OPENAPI_VERSION,
        "info": {
            "title": GATEWAY_TITLE,
            "version": GATEWAY_VERSION,
            "description": "alknet gateway: 5 fixed endpoints gating access to the operation registry. The per-caller operation surface is discovered via /search (AccessControl-filtered), not preloaded into this doc."
        },
        "paths": paths,
        "components": {
            "schemas": components_schemas()
        }
    })
}

fn search_path_item() -> Value {
    json!({
        "get": {
            "operationId": "gatewaySearch",
            "summary": "List/search operations (AccessControl-filtered). Returns names + descriptions.",
            "responses": {
                "200": json_response(schema_search_result()),
                "401": json_response(schema_unauthorized()),
                "403": json_response(schema_forbidden()),
                "500": json_response(schema_internal()),
                "504": json_response(schema_timeout())
            }
        }
    })
}

fn schema_path_item() -> Value {
    json!({
        "get": {
            "operationId": "gatewaySchema",
            "summary": "Get an operation's full OperationSpec (input/output JSON Schemas, error schemas).",
            "parameters": [
                {
                    "name": "name",
                    "in": "query",
                    "required": true,
                    "schema": { "type": "string" }
                }
            ],
            "responses": {
                "200": json_response(schema_schema_result()),
                "400": json_response(schema_invalid_input()),
                "401": json_response(schema_unauthorized()),
                "403": json_response(schema_forbidden()),
                "404": json_response(schema_not_found()),
                "500": json_response(schema_internal()),
                "504": json_response(schema_timeout())
            }
        }
    })
}

fn call_path_item(operation_errors: &[ErrorDefinition]) -> Value {
    let mut responses: Map<String, Value> = serde_json::Map::new();
    responses.insert("200".to_string(), json_response(schema_call_ok()));
    responses.insert(
        STATUS_BAD_REQUEST.to_string(),
        json_response(schema_protocol_error(CODE_INVALID_INPUT)),
    );
    responses.insert(
        STATUS_UNAUTHORIZED.to_string(),
        json_response(schema_unauthorized()),
    );
    responses.insert(
        STATUS_FORBIDDEN.to_string(),
        json_response(schema_protocol_error(CODE_FORBIDDEN)),
    );
    responses.insert(
        STATUS_NOT_FOUND.to_string(),
        json_response(schema_protocol_error(CODE_NOT_FOUND)),
    );
    responses.insert(
        STATUS_INTERNAL.to_string(),
        json_response(schema_protocol_error(CODE_INTERNAL)),
    );
    responses.insert(
        STATUS_TIMEOUT.to_string(),
        json_response(schema_protocol_error(CODE_TIMEOUT)),
    );

    let mut operation_errors_by_status: BTreeMap<u16, Vec<&ErrorDefinition>> = BTreeMap::new();
    for error in operation_errors {
        let status = match error.http_status {
            Some(status) => status,
            None => continue,
        };
        operation_errors_by_status
            .entry(status)
            .or_default()
            .push(error);
    }

    for (status, errors) in operation_errors_by_status {
        let key = status.to_string();
        let response = responses
            .entry(key)
            .or_insert_with(|| json_response(Value::Null));
        merge_operation_errors(response, &errors);
    }

    json!({
        "post": {
            "operationId": "gatewayCall",
            "summary": "Invoke an operation by name with a flat JSON input.",
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": schema_call_request()
                    }
                }
            },
            "responses": Value::Object(responses)
        }
    })
}

fn batch_path_item() -> Value {
    json!({
        "post": {
            "operationId": "gatewayBatch",
            "summary": "Invoke multiple operations in one request. Returns an array of results.",
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": schema_batch_request()
                    }
                }
            },
            "responses": {
                "200": json_response(schema_batch_result()),
                "400": json_response(schema_invalid_input()),
                "401": json_response(schema_unauthorized()),
                "403": json_response(schema_forbidden()),
                "500": json_response(schema_internal()),
                "504": json_response(schema_timeout())
            }
        }
    })
}

fn subscribe_path_item() -> Value {
    json!({
        "post": {
            "operationId": "gatewaySubscribe",
            "summary": "Invoke a streaming operation. Response is text/event-stream.",
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": schema_call_request()
                    }
                }
            },
            "responses": {
                "200": sse_response(),
                "400": json_response(schema_invalid_input()),
                "401": json_response(schema_unauthorized()),
                "403": json_response(schema_forbidden()),
                "404": json_response(schema_not_found()),
                "500": json_response(schema_internal()),
                "504": json_response(schema_timeout())
            }
        }
    })
}

fn schema_call_request() -> Value {
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

fn schema_batch_request() -> Value {
    json!({
        "type": "array",
        "items": schema_call_request()
    })
}

fn schema_call_ok() -> Value {
    json!({
        "type": "object",
        "properties": {
            "request_id": { "type": "string" },
            "result": { "type": "string", "enum": ["ok"] },
            "output": { "type": "object", "description": "The operation's output." }
        },
        "required": ["request_id", "result", "output"]
    })
}

fn schema_search_result() -> Value {
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

fn schema_schema_result() -> Value {
    json!({
        "type": "object",
        "description": "The full OperationSpec for the requested operation.",
        "properties": {
            "name": { "type": "string" },
            "namespace": { "type": "string" },
            "op_type": { "type": "string", "enum": ["query", "mutation", "subscription"] },
            "input_schema": { "type": "object" },
            "output_schema": { "type": "object" },
            "error_schemas": { "type": "array" }
        }
    })
}

fn schema_batch_result() -> Value {
    json!({
        "type": "object",
        "properties": {
            "results": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "request_id": { "type": "string" },
                        "result": { "type": "string", "enum": ["ok", "error"] },
                        "output": { "type": "object" },
                        "error": { "type": "object" }
                    }
                }
            }
        }
    })
}

fn schema_invalid_input() -> Value {
    schema_protocol_error(CODE_INVALID_INPUT)
}

fn schema_unauthorized() -> Value {
    json!({
        "type": "object",
        "properties": {
            "code": { "type": "string", "enum": ["FORBIDDEN"] },
            "message": { "type": "string", "description": "Authentication required (no bearer token)." },
            "retryable": { "type": "boolean" }
        },
        "required": ["code", "message", "retryable"]
    })
}

fn schema_forbidden() -> Value {
    schema_protocol_error(CODE_FORBIDDEN)
}

fn schema_not_found() -> Value {
    schema_protocol_error(CODE_NOT_FOUND)
}

fn schema_internal() -> Value {
    schema_protocol_error(CODE_INTERNAL)
}

fn schema_timeout() -> Value {
    schema_protocol_error(CODE_TIMEOUT)
}

fn schema_protocol_error(code: &str) -> Value {
    json!({
        "type": "object",
        "properties": {
            "code": { "type": "string", "enum": [code] },
            "message": { "type": "string" },
            "retryable": { "type": "boolean" }
        },
        "required": ["code", "message", "retryable"]
    })
}

fn operation_error_schema(error: &ErrorDefinition) -> Value {
    let mut schema = if error.schema.is_object() {
        error.schema.clone()
    } else {
        json!({ "type": "object" })
    };
    let obj = schema.as_object_mut().expect("error schema is object");
    obj.entry("title")
        .or_insert(Value::String(error.code.clone()));
    obj.entry("description")
        .or_insert(Value::String(error.description.clone()));
    schema
}

fn json_response(schema: Value) -> Value {
    json!({
        "description": "",
        "content": {
            "application/json": {
                "schema": schema
            }
        }
    })
}

fn sse_response() -> Value {
    json!({
        "description": "Server-Sent Events stream. Each `data:` frame is a call.responded event; stream close is call.completed.",
        "content": {
            "text/event-stream": {
                "schema": {
                    "type": "string",
                    "description": "SSE frame: `data: <output>\\n\\n`."
                }
            }
        }
    })
}

fn merge_operation_errors(response: &mut Value, errors: &[&ErrorDefinition]) {
    let obj = match response.as_object_mut() {
        Some(obj) => obj,
        None => return,
    };
    let content = obj
        .entry("content".to_string())
        .or_insert(json!({}))
        .as_object_mut();
    let content = match content {
        Some(c) => c,
        None => return,
    };
    let json_entry = content
        .entry("application/json".to_string())
        .or_insert(json!({}))
        .as_object_mut();
    let json_entry = match json_entry {
        Some(j) => j,
        None => return,
    };
    let existing_schema = json_entry.get("schema").cloned();
    let op_schemas: Vec<Value> = errors.iter().map(|e| operation_error_schema(e)).collect();
    let merged = match existing_schema {
        Some(existing) if !existing.is_null() => {
            let mut variants = vec![existing];
            for s in op_schemas {
                if !variant_already_present(&variants, &s) {
                    variants.push(s);
                }
            }
            if variants.len() == 1 {
                variants.into_iter().next().unwrap()
            } else {
                json!({ "oneOf": variants })
            }
        }
        _ => {
            if op_schemas.len() == 1 {
                op_schemas.into_iter().next().unwrap()
            } else {
                json!({ "oneOf": op_schemas })
            }
        }
    };
    json_entry.insert("schema".to_string(), merged);

    let description = errors
        .iter()
        .map(|e| format!("{}: {}", e.code, e.description))
        .collect::<Vec<_>>()
        .join("; ");
    obj.insert("description".to_string(), Value::String(description));
}

fn variant_already_present(variants: &[Value], candidate: &Value) -> bool {
    variants.iter().any(|v| {
        v.get("title").and_then(Value::as_str) == candidate.get("title").and_then(Value::as_str)
    })
}

fn components_schemas() -> Value {
    json!({
        "CallRequest": schema_call_request(),
        "CallOk": schema_call_ok(),
        "SearchResult": schema_search_result(),
        "SchemaResult": schema_schema_result(),
        "BatchResult": schema_batch_result()
    })
}

fn collect_operation_errors(registry: &OperationRegistry) -> Vec<ErrorDefinition> {
    let mut by_status: BTreeMap<u16, Vec<ErrorDefinition>> = BTreeMap::new();
    let mut seen_codes: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for spec in registry.list_operations() {
        for error in &spec.error_schemas {
            let status = match error.http_status {
                Some(status) => status,
                None => continue,
            };
            if is_protocol_status(status) && !is_http_prefixed_code(&error.code) {
                continue;
            }
            if !seen_codes.insert(error.code.clone()) {
                continue;
            }
            by_status.entry(status).or_default().push(error.clone());
        }
    }
    by_status.into_values().flatten().collect()
}

fn is_protocol_status(status: u16) -> bool {
    matches!(
        status,
        STATUS_BAD_REQUEST
            | STATUS_UNAUTHORIZED
            | STATUS_FORBIDDEN
            | STATUS_NOT_FOUND
            | STATUS_INTERNAL
            | STATUS_TIMEOUT
    )
}

fn is_http_prefixed_code(code: &str) -> bool {
    code.starts_with(HTTP_PREFIX) && code[HTTP_PREFIX.len()..].parse::<u16>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alknet_call::protocol::wire::ResponseEnvelope;
    use alknet_call::registry::registration::{
        make_handler, HandlerRegistration, OperationProvenance,
    };
    use alknet_call::registry::spec::{AccessControl, OperationSpec, OperationType, Visibility};
    use alknet_core::types::Capabilities;
    use serde_json::{json, Map};

    fn noop_handler() -> alknet_call::registry::registration::Handler {
        make_handler(|_input, ctx| async move { ResponseEnvelope::ok(ctx.request_id, Value::Null) })
    }

    fn register(registry: &mut OperationRegistry, spec: OperationSpec) {
        registry.register(HandlerRegistration::new(
            spec,
            noop_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
    }

    fn external_spec(name: &str, errors: Vec<ErrorDefinition>) -> OperationSpec {
        OperationSpec::new(
            name,
            OperationType::Query,
            Visibility::External,
            json!({}),
            json!({}),
            errors,
            AccessControl::default(),
        )
    }

    fn error(code: &str, http_status: Option<u16>) -> ErrorDefinition {
        ErrorDefinition {
            code: code.to_string(),
            description: format!("error {code}"),
            schema: json!({ "type": "object" }),
            http_status,
        }
    }

    fn paths_object(spec: &OpenAPISpec) -> &Map<String, Value> {
        spec.raw
            .get("paths")
            .and_then(Value::as_object)
            .expect("paths object present")
    }

    fn path<'a>(spec: &'a OpenAPISpec, name: &str) -> &'a Map<String, Value> {
        paths_object(spec)
            .get(name)
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("path {name} present"))
    }

    fn operation<'a>(spec: &'a OpenAPISpec, name: &str, method: &str) -> &'a Map<String, Value> {
        path(spec, name)
            .get(method)
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("operation {method} {name} present"))
    }

    fn responses<'a>(spec: &'a OpenAPISpec, name: &str, method: &str) -> &'a Map<String, Value> {
        operation(spec, name, method)
            .get("responses")
            .and_then(Value::as_object)
            .expect("responses present")
    }

    #[test]
    fn empty_registry_produces_five_gateway_paths() {
        let registry = OperationRegistry::new();
        let spec = to_openapi(&registry);
        let paths = paths_object(&spec);
        assert_eq!(paths.len(), 5);
        assert!(paths.contains_key(PATH_SEARCH));
        assert!(paths.contains_key(PATH_SCHEMA));
        assert!(paths.contains_key(PATH_CALL));
        assert!(paths.contains_key(PATH_BATCH));
        assert!(paths.contains_key(PATH_SUBSCRIBE));
    }

    #[test]
    fn registry_with_operations_does_not_add_per_operation_paths() {
        let mut registry = OperationRegistry::new();
        register(&mut registry, external_spec("fs/readFile", vec![]));
        register(&mut registry, external_spec("agent/chat", vec![]));
        let spec = to_openapi(&registry);
        let paths = paths_object(&spec);
        assert_eq!(paths.len(), 5);
        assert!(!paths.contains_key("/fs/readFile"));
        assert!(!paths.contains_key("/agent/chat"));
    }

    #[test]
    fn info_version_is_1_0_0() {
        let registry = OperationRegistry::new();
        let spec = to_openapi(&registry);
        let version = spec
            .raw
            .get("info")
            .and_then(|i: &Value| i.get("version"))
            .and_then(Value::as_str)
            .unwrap();
        assert_eq!(version, GATEWAY_VERSION);
        assert_eq!(version, "1.0.0");
    }

    #[test]
    fn info_title_present() {
        let registry = OperationRegistry::new();
        let spec = to_openapi(&registry);
        let title = spec
            .raw
            .get("info")
            .and_then(|i: &Value| i.get("title"))
            .and_then(Value::as_str)
            .unwrap();
        assert_eq!(title, GATEWAY_TITLE);
    }

    #[test]
    fn openapi_field_is_3_0_0() {
        let registry = OperationRegistry::new();
        let spec = to_openapi(&registry);
        let openapi = spec.raw.get("openapi").and_then(Value::as_str).unwrap();
        assert_eq!(openapi, OPENAPI_VERSION);
    }

    #[test]
    fn call_request_body_is_flat_operation_input() {
        let registry = OperationRegistry::new();
        let spec = to_openapi(&registry);
        let request_schema = operation(&spec, PATH_CALL, "post")
            .get("requestBody")
            .and_then(|rb| rb.get("content"))
            .and_then(|c: &Value| c.get("application/json"))
            .and_then(|c: &Value| c.get("schema"))
            .unwrap();
        let props = request_schema
            .get("properties")
            .and_then(Value::as_object)
            .unwrap();
        assert!(props.contains_key("operation"));
        assert!(props.contains_key("input"));
        let operation_prop = props.get("operation").unwrap();
        assert_eq!(
            operation_prop.get("type").and_then(Value::as_str),
            Some("string")
        );
        let input_prop = props.get("input").unwrap();
        assert_eq!(
            input_prop.get("type").and_then(Value::as_str),
            Some("object")
        );
        let required = request_schema
            .get("required")
            .and_then(Value::as_array)
            .unwrap();
        assert!(required.iter().any(|v| v == "operation"));
    }

    #[test]
    fn call_includes_all_protocol_level_error_statuses() {
        let registry = OperationRegistry::new();
        let spec = to_openapi(&registry);
        let responses = responses(&spec, PATH_CALL, "post");
        for status in [
            STATUS_BAD_REQUEST,
            STATUS_UNAUTHORIZED,
            STATUS_FORBIDDEN,
            STATUS_NOT_FOUND,
            STATUS_INTERNAL,
            STATUS_TIMEOUT,
        ] {
            assert!(
                responses.contains_key(&status.to_string()),
                "protocol status {status} present on /call"
            );
        }
        assert!(responses.contains_key("200"));
    }

    #[test]
    fn call_protocol_error_status_codes_have_protocol_codes() {
        let registry = OperationRegistry::new();
        let spec = to_openapi(&registry);
        let responses = responses(&spec, PATH_CALL, "post");

        let invalid_input_schema = responses
            .get(&STATUS_BAD_REQUEST.to_string())
            .and_then(|r: &Value| r.get("content"))
            .and_then(|c: &Value| c.get("application/json"))
            .and_then(|c: &Value| c.get("schema"))
            .and_then(|s: &Value| s.get("properties"))
            .and_then(|p: &Value| p.get("code"))
            .and_then(|c: &Value| c.get("enum"))
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(invalid_input_schema[0], CODE_INVALID_INPUT);

        let forbidden_schema = responses
            .get(&STATUS_FORBIDDEN.to_string())
            .and_then(|r: &Value| r.get("content"))
            .and_then(|c: &Value| c.get("application/json"))
            .and_then(|c: &Value| c.get("schema"))
            .and_then(|s: &Value| s.get("properties"))
            .and_then(|p: &Value| p.get("code"))
            .and_then(|c: &Value| c.get("enum"))
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(forbidden_schema[0], CODE_FORBIDDEN);

        let timeout_schema = responses
            .get(&STATUS_TIMEOUT.to_string())
            .and_then(|r: &Value| r.get("content"))
            .and_then(|c: &Value| c.get("application/json"))
            .and_then(|c: &Value| c.get("schema"))
            .and_then(|s: &Value| s.get("properties"))
            .and_then(|p: &Value| p.get("code"))
            .and_then(|c: &Value| c.get("enum"))
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(timeout_schema[0], CODE_TIMEOUT);
    }

    #[test]
    fn operation_errors_projected_onto_call() {
        let mut registry = OperationRegistry::new();
        register(
            &mut registry,
            external_spec(
                "fs/readFile",
                vec![
                    error("FILE_NOT_FOUND", Some(404)),
                    error("RATE_LIMITED", Some(429)),
                ],
            ),
        );
        let spec = to_openapi(&registry);
        let responses = responses(&spec, PATH_CALL, "post");
        assert!(
            responses.contains_key("429"),
            "operation-level 429 projected onto /call"
        );
        let response_429 = responses.get("429").unwrap();
        let schema = response_429
            .get("content")
            .and_then(|c: &Value| c.get("application/json"))
            .and_then(|c: &Value| c.get("schema"))
            .unwrap();
        let title = schema.get("title").and_then(Value::as_str).unwrap();
        assert_eq!(title, "RATE_LIMITED");
        let description = response_429
            .get("description")
            .and_then(Value::as_str)
            .unwrap();
        assert!(description.contains("RATE_LIMITED"));
    }

    #[test]
    fn http_prefixed_error_code_projects_to_status() {
        let mut registry = OperationRegistry::new();
        register(
            &mut registry,
            external_spec("svc/op", vec![error("HTTP_404", Some(404))]),
        );
        let spec = to_openapi(&registry);
        let responses = responses(&spec, PATH_CALL, "post");
        let response_404 = responses.get("404").unwrap();
        let schema = response_404
            .get("content")
            .and_then(|c: &Value| c.get("application/json"))
            .and_then(|c: &Value| c.get("schema"))
            .unwrap();
        let one_of = schema.get("oneOf").and_then(Value::as_array);
        let titles: Vec<&str> = match one_of {
            Some(arr) => arr
                .iter()
                .filter_map(|v: &Value| v.get("title").and_then(Value::as_str))
                .collect(),
            None => vec![schema.get("title").and_then(Value::as_str).unwrap_or("")],
        };
        assert!(
            titles.contains(&"HTTP_404"),
            "HTTP_404 operation error must be projected on /call 404, got titles: {titles:?}"
        );
    }

    #[test]
    fn http_prefixed_code_does_not_collide_with_protocol_code() {
        let mut registry = OperationRegistry::new();
        register(
            &mut registry,
            external_spec("svc/op", vec![error("HTTP_404", Some(404))]),
        );
        let spec = to_openapi(&registry);
        let responses = responses(&spec, PATH_CALL, "post");
        let response_404 = responses.get("404").unwrap();
        let schema = response_404
            .get("content")
            .and_then(|c: &Value| c.get("application/json"))
            .and_then(|c: &Value| c.get("schema"))
            .unwrap();
        let one_of = schema.get("oneOf").and_then(Value::as_array);
        let variants: Vec<&Value> = match one_of {
            Some(arr) => arr.iter().collect(),
            None => vec![schema],
        };
        let http_404_variant = variants
            .iter()
            .find(|v| v.get("title").and_then(Value::as_str) == Some("HTTP_404"))
            .expect("HTTP_404 variant present");
        let http_enum = http_404_variant
            .get("properties")
            .and_then(|p: &Value| p.get("code"))
            .and_then(|c: &Value| c.get("enum"))
            .and_then(Value::as_array);
        assert!(
            http_enum.is_none(),
            "HTTP_404 variant is not constrained to a protocol code enum"
        );
        let titles: Vec<&str> = variants
            .iter()
            .filter_map(|v| v.get("title").and_then(Value::as_str))
            .collect();
        assert!(
            titles.contains(&"HTTP_404"),
            "HTTP_404 operation error projected alongside protocol 404, got titles: {titles:?}"
        );
    }

    #[test]
    fn operation_error_without_http_status_not_projected() {
        let mut registry = OperationRegistry::new();
        register(
            &mut registry,
            external_spec("svc/op", vec![error("DOMAIN_ERROR", None)]),
        );
        let spec = to_openapi(&registry);
        let responses = responses(&spec, PATH_CALL, "post");
        assert!(!responses.contains_key("0"));
        assert_eq!(
            responses.len(),
            7,
            "only protocol-level statuses + 200 present"
        );
    }

    #[test]
    fn subscribe_response_is_text_event_stream() {
        let registry = OperationRegistry::new();
        let spec = to_openapi(&registry);
        let responses = responses(&spec, PATH_SUBSCRIBE, "post");
        let ok = responses.get("200").unwrap();
        let content = ok.get("content").and_then(Value::as_object).unwrap();
        assert!(content.contains_key("text/event-stream"));
        assert!(!content.contains_key("application/json"));
    }

    #[test]
    fn subscribe_request_body_is_flat_operation_input() {
        let registry = OperationRegistry::new();
        let spec = to_openapi(&registry);
        let request_schema = operation(&spec, PATH_SUBSCRIBE, "post")
            .get("requestBody")
            .and_then(|rb| rb.get("content"))
            .and_then(|c: &Value| c.get("application/json"))
            .and_then(|c: &Value| c.get("schema"))
            .unwrap();
        let props = request_schema
            .get("properties")
            .and_then(Value::as_object)
            .unwrap();
        assert!(props.contains_key("operation"));
        assert!(props.contains_key("input"));
    }

    #[test]
    fn batch_request_body_is_array_of_call_requests() {
        let registry = OperationRegistry::new();
        let spec = to_openapi(&registry);
        let request_schema = operation(&spec, PATH_BATCH, "post")
            .get("requestBody")
            .and_then(|rb| rb.get("content"))
            .and_then(|c: &Value| c.get("application/json"))
            .and_then(|c: &Value| c.get("schema"))
            .unwrap();
        assert_eq!(
            request_schema.get("type").and_then(Value::as_str),
            Some("array")
        );
        let items = request_schema
            .get("items")
            .and_then(Value::as_object)
            .unwrap();
        assert!(items.contains_key("properties"));
    }

    #[test]
    fn search_has_get_method() {
        let registry = OperationRegistry::new();
        let spec = to_openapi(&registry);
        assert!(path(&spec, PATH_SEARCH).contains_key("get"));
        assert!(!path(&spec, PATH_SEARCH).contains_key("post"));
    }

    #[test]
    fn schema_has_get_method_with_name_query_param() {
        let registry = OperationRegistry::new();
        let spec = to_openapi(&registry);
        let params = operation(&spec, PATH_SCHEMA, "get")
            .get("parameters")
            .and_then(Value::as_array)
            .unwrap();
        let name_param = &params[0];
        assert_eq!(name_param.get("name").and_then(Value::as_str), Some("name"));
        assert_eq!(name_param.get("in").and_then(Value::as_str), Some("query"));
        assert_eq!(
            name_param.get("required").and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn call_has_post_method() {
        let registry = OperationRegistry::new();
        let spec = to_openapi(&registry);
        assert!(path(&spec, PATH_CALL).contains_key("post"));
        assert!(!path(&spec, PATH_CALL).contains_key("get"));
    }

    #[test]
    fn to_openapi_is_pure_projection_does_not_modify_registry() {
        let mut registry = OperationRegistry::new();
        register(&mut registry, external_spec("fs/readFile", vec![]));
        let before_count = registry.list_operations().len();
        let _ = to_openapi(&registry);
        assert_eq!(registry.list_operations().len(), before_count);
        assert!(registry.registration("fs/readFile").is_some());
    }

    #[test]
    fn duplicate_error_status_surfaces_all_distinct_codes() {
        let mut registry = OperationRegistry::new();
        register(
            &mut registry,
            external_spec("svc/a", vec![error("RATE_LIMITED", Some(429))]),
        );
        register(
            &mut registry,
            external_spec("svc/b", vec![error("TOO_MANY_REQUESTS", Some(429))]),
        );
        let spec = to_openapi(&registry);
        let responses = responses(&spec, PATH_CALL, "post");
        assert!(responses.contains_key("429"));
        let schema = responses
            .get("429")
            .and_then(|r: &Value| r.get("content"))
            .and_then(|c: &Value| c.get("application/json"))
            .and_then(|c: &Value| c.get("schema"))
            .unwrap();
        let one_of = schema.get("oneOf").and_then(Value::as_array).unwrap();
        let titles: Vec<&str> = one_of
            .iter()
            .filter_map(|v| v.get("title").and_then(Value::as_str))
            .collect();
        assert!(titles.contains(&"RATE_LIMITED"));
        assert!(titles.contains(&"TOO_MANY_REQUESTS"));
    }

    #[test]
    fn internal_operations_excluded_from_error_projection() {
        let mut registry = OperationRegistry::new();
        registry.register(HandlerRegistration::new(
            OperationSpec::new(
                "internal/op",
                OperationType::Query,
                Visibility::Internal,
                json!({}),
                json!({}),
                vec![error("INTERNAL_ERROR", Some(418))],
                AccessControl::default(),
            ),
            noop_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        let spec = to_openapi(&registry);
        let responses = responses(&spec, PATH_CALL, "post");
        assert!(
            !responses.contains_key("418"),
            "internal op errors not projected"
        );
    }

    #[test]
    fn operation_error_with_protocol_status_but_http_prefix_is_projected() {
        let mut registry = OperationRegistry::new();
        register(
            &mut registry,
            external_spec("svc/op", vec![error("HTTP_500", Some(500))]),
        );
        let spec = to_openapi(&registry);
        let responses = responses(&spec, PATH_CALL, "post");
        let schema = responses
            .get("500")
            .and_then(|r: &Value| r.get("content"))
            .and_then(|c: &Value| c.get("application/json"))
            .and_then(|c: &Value| c.get("schema"))
            .unwrap();
        let one_of = schema.get("oneOf").and_then(Value::as_array).unwrap();
        let titles: Vec<&str> = one_of
            .iter()
            .filter_map(|v| v.get("title").and_then(Value::as_str))
            .collect();
        assert!(titles.contains(&"HTTP_500"));
    }
}
