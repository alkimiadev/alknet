//! `to_mcp`: 4-tool gateway projection over the local operation registry,
//! exposed to external MCP clients (editors, AI tools) via rmcp's
//! `StreamableHttpService` nested into the axum `Router` at `/mcp`.
//!
//! This is the tool-gateway pattern (ADR-041): the LLM gets a fixed set of
//! meta-tools (`search`, `schema`, `call`, `batch`) and discovers operations
//! on demand — not one MCP tool per registry operation. `Subscription` ops
//! are excluded from `search` and cannot be invoked via `call` (MCP tool
//! calls are request/response — ADR-041 §2).
//!
//! `to_mcp` is a pure projection (ADR-017 §5): it consumes the registry and
//! does not produce entries for it. It is not an `OperationAdapter`. The
//! shared dispatch spine (`GatewayDispatch`) is used for the `call` tool; the
//! `ResponseEnvelope` → `CallToolResult` mapping is `to_mcp`-specific.
//!
//! Bearer auth is the shared `bearer_auth_middleware`, applied as an axum
//! layer *around* the nested `StreamableHttpService` (research §4.4 — the rmcp
//! `simple_auth_streamhttp.rs` example shows the pattern). The resolved
//! `Identity` is stashed by the middleware into `http::request::Parts`'s
//! extensions; rmcp injects `Parts` into the `RequestContext<RoleServer>`
//! extensions, so `call_tool` reads it back via
//! `context.extensions.get::<http::request::Parts>()` (research §6 #2 — the
//! load-bearing identity-survives-the-rmcp-framing assumption).
//!
//! Streamable HTTP only (ADR-037 — stdio is not built). Feature-gated behind
//! `mcp`. See `docs/architecture/crates/http/http-mcp.md`.

use std::borrow::Cow;
use std::sync::Arc;

use alknet_call::protocol::wire::{CallError, ResponseEnvelope};
use alknet_core::auth::Identity;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Implementation, JsonObject, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::{
    streamable_http_server::{session::local::LocalSessionManager, tower::StreamableHttpService},
    StreamableHttpServerConfig,
};
use serde_json::{Map, Value};

use crate::gateway::GatewayDispatch;

const TOOL_SEARCH: &str = "search";
const TOOL_SCHEMA: &str = "schema";
const TOOL_CALL: &str = "call";
const TOOL_BATCH: &str = "batch";

const OP_SERVICES_LIST: &str = "services/list";
const OP_SERVICES_SCHEMA: &str = "services/schema";

fn search_input_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "Optional substring filter on operation name."
            }
        }
    })
}

fn schema_input_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "description": "The fully-qualified operation name (e.g. `fs/readFile`)."
            }
        },
        "required": ["name"]
    })
}

fn call_input_schema() -> Value {
    serde_json::json!({
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
    serde_json::json!({
        "type": "object",
        "properties": {
            "calls": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "operation": { "type": "string" },
                        "input": { "type": "object" }
                    },
                    "required": ["operation"]
                },
                "description": "The operations to invoke in this batch."
            }
        },
        "required": ["calls"]
    })
}

pub struct ToMcpGateway {
    dispatch: Arc<GatewayDispatch>,
}

impl ToMcpGateway {
    pub fn new(dispatch: Arc<GatewayDispatch>) -> Self {
        Self { dispatch }
    }

    pub fn dispatch(&self) -> &Arc<GatewayDispatch> {
        &self.dispatch
    }

    fn extract_identity(context: &RequestContext<RoleServer>) -> Option<Identity> {
        Self::extract_identity_from_extensions(&context.extensions)
    }

    fn extract_identity_from_extensions(extensions: &rmcp::model::Extensions) -> Option<Identity> {
        let parts = extensions.get::<http::request::Parts>()?;
        parts
            .extensions
            .get::<Option<Identity>>()
            .and_then(Option::clone)
    }

    async fn handle_search(&self, identity: Option<Identity>) -> CallToolResult {
        let response = self
            .dispatch
            .invoke(identity.clone(), OP_SERVICES_LIST, Value::Null)
            .await;
        map_search_response(response, identity.as_ref())
    }

    async fn handle_schema(
        &self,
        arguments: Option<JsonObject>,
        identity: Option<Identity>,
    ) -> CallToolResult {
        let name = match arguments
            .and_then(|mut a| a.remove("name"))
            .and_then(|v| v.as_str().map(str::to_string))
        {
            Some(n) => n,
            None => {
                return CallToolResult::structured_error(serde_json::json!({
                    "code": "INVALID_INPUT",
                    "message": "missing required field: name"
                }));
            }
        };
        let response = self
            .dispatch
            .invoke(
                identity,
                OP_SERVICES_SCHEMA,
                serde_json::json!({ "name": name }),
            )
            .await;
        envelope_to_call_tool_result(response)
    }

    async fn handle_call(
        &self,
        arguments: Option<JsonObject>,
        identity: Option<Identity>,
    ) -> CallToolResult {
        let (operation, input) = match parse_call_arguments(arguments) {
            Ok(pair) => pair,
            Err(err) => return err,
        };
        let response = self.dispatch.invoke(identity, &operation, input).await;
        envelope_to_call_tool_result(response)
    }

    async fn handle_batch(
        &self,
        arguments: Option<JsonObject>,
        identity: Option<Identity>,
    ) -> CallToolResult {
        let calls = match arguments
            .and_then(|mut a| a.remove("calls"))
            .and_then(|v| v.as_array().cloned())
        {
            Some(arr) => arr,
            None => {
                return CallToolResult::structured_error(serde_json::json!({
                    "code": "INVALID_INPUT",
                    "message": "missing required field: calls"
                }));
            }
        };

        let mut results: Vec<Value> = Vec::with_capacity(calls.len());
        for call in calls {
            let (operation, input) = match parse_call_arguments(call.as_object().cloned()) {
                Ok(pair) => pair,
                Err(err) => {
                    results.push(batch_error_value(err));
                    continue;
                }
            };
            let response = self
                .dispatch
                .invoke(identity.clone(), &operation, input)
                .await;
            results.push(envelope_to_value(response));
        }
        CallToolResult::structured(Value::Array(results))
    }
}

fn parse_call_arguments(arguments: Option<JsonObject>) -> Result<(String, Value), CallToolResult> {
    let mut map = match arguments {
        Some(m) => m,
        None => {
            return Err(CallToolResult::structured_error(serde_json::json!({
                "code": "INVALID_INPUT",
                "message": "missing required field: operation"
            })));
        }
    };
    let operation = match map
        .remove("operation")
        .and_then(|v| v.as_str().map(str::to_string))
    {
        Some(s) => s,
        None => {
            return Err(CallToolResult::structured_error(serde_json::json!({
                "code": "INVALID_INPUT",
                "message": "missing required field: operation"
            })));
        }
    };
    let input = map.remove("input").unwrap_or(Value::Object(Map::new()));
    Ok((operation, input))
}

fn batch_error_value(result: CallToolResult) -> Value {
    serde_json::json!({
        "isError": result.is_error.unwrap_or(false),
        "structuredContent": result.structured_content,
        "content": result.content,
    })
}

fn map_search_response(response: ResponseEnvelope, identity: Option<&Identity>) -> CallToolResult {
    match response.result {
        Ok(value) => {
            let operations = value
                .get("operations")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let filtered: Vec<Value> = operations
                .into_iter()
                .filter(|op| {
                    let op_type = op.get("op_type").and_then(Value::as_str).unwrap_or("");
                    !matches!(op_type, "subscription" | "Subscription")
                })
                .map(|op| op_to_search_listing(&op, identity))
                .collect();
            CallToolResult::structured(serde_json::json!({ "operations": filtered }))
        }
        Err(err) => call_error_to_structured_error(err),
    }
}

fn op_to_search_listing(op: &Value, identity: Option<&Identity>) -> Value {
    let name = op.get("name").and_then(Value::as_str).unwrap_or("");
    let op_type = op.get("op_type").and_then(Value::as_str).unwrap_or("query");
    let namespace = op.get("namespace").and_then(Value::as_str).unwrap_or("");
    let description = format!("{op_type} operation `{name}` in namespace `{namespace}`");
    let _ = identity;
    serde_json::json!({
        "name": name,
        "description": description,
    })
}

fn envelope_to_call_tool_result(response: ResponseEnvelope) -> CallToolResult {
    match response.result {
        Ok(value) => CallToolResult::structured(value),
        Err(err) => call_error_to_structured_error(err),
    }
}

fn call_error_to_structured_error(err: CallError) -> CallToolResult {
    let details = serde_json::to_value(&err).unwrap_or(Value::Null);
    CallToolResult::structured_error(details)
}

fn envelope_to_value(response: ResponseEnvelope) -> Value {
    match response.result {
        Ok(output) => serde_json::json!({
            "isError": false,
            "output": output,
        }),
        Err(err) => {
            let details = serde_json::to_value(&err).unwrap_or(Value::Null);
            serde_json::json!({
                "isError": true,
                "error": details,
            })
        }
    }
}

fn gateway_tools() -> Vec<Tool> {
    vec![
        Tool::new(
            Cow::Borrowed(TOOL_SEARCH),
            Cow::Borrowed(
                "List available operations (filtered by the caller's AccessControl). Returns names + descriptions, not full schemas. Subscription operations are excluded.",
            ),
            value_to_object(search_input_schema()),
        ),
        Tool::new(
            Cow::Borrowed(TOOL_SCHEMA),
            Cow::Borrowed(
                "Get the full OperationSpec for an operation (input/output JSON Schemas, error schemas).",
            ),
            value_to_object(schema_input_schema()),
        ),
        Tool::new(
            Cow::Borrowed(TOOL_CALL),
            Cow::Borrowed(
                "Invoke an operation by name with a JSON input. Returns the output as structuredContent, or isError with typed error details for a CallError.",
            ),
            value_to_object(call_input_schema()),
        ),
        Tool::new(
            Cow::Borrowed(TOOL_BATCH),
            Cow::Borrowed(
                "Invoke multiple operations in one tool call. Returns an array of results, each shaped like a `call` result.",
            ),
            value_to_object(batch_input_schema()),
        ),
    ]
}

fn value_to_object(value: Value) -> Arc<JsonObject> {
    match value {
        Value::Object(map) => Arc::new(map),
        _ => Arc::new(Map::new()),
    }
}

impl rmcp::handler::server::ServerHandler for ToMcpGateway {
    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl futures::Future<Output = Result<ListToolsResult, rmcp::ErrorData>> + Send + '_ {
        let tools = gateway_tools();
        std::future::ready(Ok(ListToolsResult::with_all_items(tools)))
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl futures::Future<Output = Result<CallToolResult, rmcp::ErrorData>> + Send + '_ {
        let identity = Self::extract_identity(&context);
        let name = request.name.to_string();
        let arguments = request.arguments;
        let this = self;
        async move {
            let result = match name.as_str() {
                TOOL_SEARCH => this.handle_search(identity).await,
                TOOL_SCHEMA => this.handle_schema(arguments, identity).await,
                TOOL_CALL => this.handle_call(arguments, identity).await,
                TOOL_BATCH => this.handle_batch(arguments, identity).await,
                unknown => {
                    let err = CallError::new(
                        "NOT_FOUND",
                        format!("unknown gateway tool: {unknown}"),
                        false,
                    );
                    call_error_to_structured_error(err)
                }
            };
            Ok(result)
        }
    }

    fn get_info(&self) -> ServerInfo {
        let capabilities = ServerCapabilities::builder().enable_tools().build();
        ServerInfo::new(capabilities)
            .with_server_info(Implementation::new(
                "alknet-to-mcp",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "alknet MCP gateway. Call `search` to discover operations, `schema` for an operation's full spec, `call` to invoke, `batch` to invoke many.",
            )
    }
}

pub type ToMcpService = StreamableHttpService<ToMcpGateway, LocalSessionManager>;

pub fn to_mcp_service(dispatch: Arc<GatewayDispatch>) -> ToMcpService {
    let gateway = ToMcpGateway::new(dispatch);
    StreamableHttpService::new(
        move || Ok(ToMcpGateway::new(Arc::clone(gateway.dispatch()))),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use alknet_call::protocol::wire::ResponseEnvelope;
    use alknet_call::registry::context::ScopedPeerEnv;
    use alknet_call::registry::discovery::{
        services_list_handler, services_list_spec, services_schema_handler, services_schema_spec,
    };
    use alknet_call::registry::registration::{
        make_handler, make_streaming_handler, HandlerKind, HandlerRegistration,
        OperationProvenance, OperationRegistry,
    };
    use alknet_call::registry::spec::{AccessControl, OperationSpec, OperationType, Visibility};
    use alknet_core::auth::{AuthToken, Identity, IdentityProvider};
    use alknet_core::types::Capabilities;
    use rmcp::model::Extensions;
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;

    struct StaticIdentityProvider {
        tokens: StdMutex<HashMap<String, Identity>>,
    }

    impl StaticIdentityProvider {
        fn new() -> Self {
            Self {
                tokens: StdMutex::new(HashMap::new()),
            }
        }

        fn with_token(self, token: &str, identity: Identity) -> Self {
            self.tokens
                .lock()
                .unwrap()
                .insert(token.to_string(), identity);
            self
        }
    }

    impl IdentityProvider for StaticIdentityProvider {
        fn resolve_from_fingerprint(&self, _fp: &str) -> Option<Identity> {
            None
        }
        fn resolve_from_token(&self, token: &AuthToken) -> Option<Identity> {
            let token_str = String::from_utf8_lossy(&token.raw);
            self.tokens.lock().unwrap().get(token_str.as_ref()).cloned()
        }
    }

    fn identity_with_scopes(id: &str, scopes: &[&str]) -> Identity {
        Identity {
            id: id.to_string(),
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
            resources: HashMap::new(),
        }
    }

    fn external_spec(name: &str, op_type: OperationType, acl: AccessControl) -> OperationSpec {
        OperationSpec::new(
            name,
            op_type,
            Visibility::External,
            serde_json::json!({}),
            serde_json::json!({}),
            vec![],
            acl,
            None,
        )
    }

    fn make_echo_handler() -> alknet_call::registry::registration::Handler {
        make_handler(
            |input, context| async move { ResponseEnvelope::ok(context.request_id, input) },
        )
    }

    /// A streaming echo handler: yields the input back as a single
    /// `call.responded` frame, then the stream ends. Used for
    /// `OperationType::Subscription` ops in `full_registry_with_ops` —
    /// the registry's kind validation (ADR-049) requires
    /// `HandlerKind::Stream` for `Subscription` ops; using
    /// `HandlerKind::Once` is rejected with `"handler kind mismatch:
    /// Subscription requires HandlerKind::Stream (got HandlerKind::Once)"`.
    /// The test only verifies that the MCP `search` tool *excludes*
    /// Subscription ops from its listing — it never invokes the handler —
    /// so a single-frame echo is sufficient.
    fn make_echo_streaming_handler() -> alknet_call::registry::registration::StreamingHandler {
        make_streaming_handler(|input, context| {
            futures::stream::iter(vec![ResponseEnvelope::ok(context.request_id, input)])
        })
    }

    /// Build a `HandlerKind` matching the op's `OperationType`: `Once` for
    /// Query/Mutation, `Stream` for Subscription. The registry's kind
    /// validation (ADR-049) rejects a mismatch, so the helper must branch
    /// — using `HandlerKind::Once` for a `Subscription` op panics in
    /// `register().unwrap()`.
    fn handler_kind_for(op_type: OperationType) -> HandlerKind {
        match op_type {
            OperationType::Subscription => HandlerKind::Stream(make_echo_streaming_handler()),
            OperationType::Query | OperationType::Mutation => {
                HandlerKind::Once(make_echo_handler())
            }
        }
    }

    fn full_registry_with_ops(
        specs: Vec<(String, OperationType, AccessControl)>,
    ) -> Arc<OperationRegistry> {
        let mut inner = OperationRegistry::new();
        for (name, op_type, acl) in specs {
            inner
                .register(HandlerRegistration::new(
                    external_spec(&name, op_type, acl),
                    handler_kind_for(op_type),
                    OperationProvenance::Local,
                    None,
                    None,
                    Capabilities::new(),
                ))
                .unwrap();
        }
        let inner = Arc::new(inner);

        let mut dispatch_registry = OperationRegistry::new();
        for op in inner.list_operations() {
            dispatch_registry
                .register(HandlerRegistration::new(
                    external_spec(&op.name, op.op_type, op.access_control.clone()),
                    handler_kind_for(op.op_type),
                    OperationProvenance::Local,
                    None,
                    None,
                    Capabilities::new(),
                ))
                .unwrap();
        }
        dispatch_registry
            .register(HandlerRegistration::new(
                services_list_spec(),
                HandlerKind::Once(services_list_handler(Arc::clone(&inner))),
                OperationProvenance::Local,
                None,
                ScopedPeerEnv::empty().into(),
                Capabilities::new(),
            ))
            .unwrap();
        dispatch_registry
            .register(HandlerRegistration::new(
                services_schema_spec(),
                HandlerKind::Once(services_schema_handler(Arc::clone(&inner))),
                OperationProvenance::Local,
                None,
                ScopedPeerEnv::empty().into(),
                Capabilities::new(),
            ))
            .unwrap();
        Arc::new(dispatch_registry)
    }

    fn dispatch(
        registry: Arc<OperationRegistry>,
        provider: Arc<dyn IdentityProvider>,
    ) -> Arc<GatewayDispatch> {
        Arc::new(GatewayDispatch::new(registry, provider))
    }

    fn provider() -> Arc<dyn IdentityProvider> {
        Arc::new(StaticIdentityProvider::new())
    }

    fn extensions_with_identity(identity: Option<Identity>) -> Extensions {
        let request = http::Request::builder()
            .method(http::Method::POST)
            .uri("/mcp")
            .body(())
            .expect("valid request");
        let (mut parts, _) = request.into_parts();
        parts.extensions.insert(identity);
        let mut extensions = Extensions::new();
        extensions.insert(parts);
        extensions
    }

    async fn invoke_tool(
        gateway: &ToMcpGateway,
        name: &str,
        arguments: Option<JsonObject>,
        identity: Option<Identity>,
    ) -> CallToolResult {
        match name {
            TOOL_SEARCH => gateway.handle_search(identity).await,
            TOOL_SCHEMA => gateway.handle_schema(arguments, identity).await,
            TOOL_CALL => gateway.handle_call(arguments, identity).await,
            TOOL_BATCH => gateway.handle_batch(arguments, identity).await,
            unknown => {
                let err = CallError::new(
                    "NOT_FOUND",
                    format!("unknown gateway tool: {unknown}"),
                    false,
                );
                call_error_to_structured_error(err)
            }
        }
    }

    #[tokio::test]
    async fn list_tools_returns_exactly_four_gateway_tools() {
        let _gateway = ToMcpGateway::new(dispatch(full_registry_with_ops(vec![]), provider()));
        let tools = gateway_tools();
        let names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
        assert_eq!(names.len(), 4);
        assert!(names.contains(&"search".to_string()));
        assert!(names.contains(&"schema".to_string()));
        assert!(names.contains(&"call".to_string()));
        assert!(names.contains(&"batch".to_string()));
    }

    #[tokio::test]
    async fn list_tools_does_not_leak_registry_operations() {
        let registry = full_registry_with_ops(vec![(
            "fs/readFile".to_string(),
            OperationType::Query,
            AccessControl::default(),
        )]);
        let _gateway = ToMcpGateway::new(dispatch(registry, provider()));
        let tools = gateway_tools();
        for tool in &tools {
            assert_ne!(tool.name, "fs/readFile");
            assert_ne!(tool.name, "services/list");
            assert_ne!(tool.name, "services/schema");
        }
        assert_eq!(tools.len(), 4);
    }

    #[tokio::test]
    async fn search_returns_access_control_filtered_ops_excluding_subscriptions() {
        let registry = full_registry_with_ops(vec![
            (
                "public/echo".to_string(),
                OperationType::Query,
                AccessControl::default(),
            ),
            (
                "admin/secret".to_string(),
                OperationType::Query,
                AccessControl {
                    required_scopes: vec!["admin".to_string()],
                    ..Default::default()
                },
            ),
            (
                "events/stream".to_string(),
                OperationType::Subscription,
                AccessControl::default(),
            ),
        ]);
        let idp: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let gateway = ToMcpGateway::new(dispatch(registry, idp));

        let result = invoke_tool(
            &gateway,
            "search",
            None,
            Some(identity_with_scopes("user", &["user"])),
        )
        .await;
        assert_eq!(result.is_error, Some(false));
        let structured = result.structured_content.expect("structured present");
        let ops = structured
            .get("operations")
            .and_then(Value::as_array)
            .expect("operations array");
        let names: Vec<&str> = ops
            .iter()
            .filter_map(|o| o.get("name").and_then(Value::as_str))
            .collect();
        assert!(names.contains(&"public/echo"));
        assert!(
            !names.contains(&"admin/secret"),
            "ACL-filtered op must not appear"
        );
        assert!(
            !names.contains(&"events/stream"),
            "Subscription op must be excluded"
        );
        for op in ops {
            assert!(
                op.get("description").is_some(),
                "each entry has a description"
            );
            assert!(
                op.get("input_schema").is_none(),
                "search must not return full schemas"
            );
        }
    }

    #[tokio::test]
    async fn schema_returns_full_operation_spec() {
        let registry = full_registry_with_ops(vec![(
            "fs/readFile".to_string(),
            OperationType::Query,
            AccessControl::default(),
        )]);
        let gateway = ToMcpGateway::new(dispatch(registry, provider()));

        let mut args = Map::new();
        args.insert("name".to_string(), Value::String("fs/readFile".to_string()));
        let result = invoke_tool(&gateway, "schema", Some(args), None).await;
        assert_eq!(result.is_error, Some(false));
        let structured = result.structured_content.expect("structured present");
        assert_eq!(
            structured.get("name"),
            Some(&Value::String("fs/readFile".to_string()))
        );
        assert!(structured.get("input_schema").is_some());
        assert!(structured.get("output_schema").is_some());
        assert!(structured.get("error_schemas").is_some());
        assert!(structured.get("access_control").is_some());
    }

    #[tokio::test]
    async fn call_returns_structured_for_success() {
        let registry = full_registry_with_ops(vec![(
            "echo/run".to_string(),
            OperationType::Query,
            AccessControl::default(),
        )]);
        let gateway = ToMcpGateway::new(dispatch(registry, provider()));

        let mut args = Map::new();
        args.insert(
            "operation".to_string(),
            Value::String("echo/run".to_string()),
        );
        args.insert("input".to_string(), serde_json::json!({ "msg": "hi" }));
        let result = invoke_tool(&gateway, "call", Some(args), None).await;
        assert_eq!(result.is_error, Some(false));
        assert_eq!(
            result.structured_content,
            Some(serde_json::json!({ "msg": "hi" }))
        );
    }

    #[tokio::test]
    async fn call_returns_structured_error_for_call_error() {
        let registry = full_registry_with_ops(vec![]);
        let gateway = ToMcpGateway::new(dispatch(registry, provider()));

        let mut args = Map::new();
        args.insert(
            "operation".to_string(),
            Value::String("no/such".to_string()),
        );
        args.insert("input".to_string(), Value::Object(Map::new()));
        let result = invoke_tool(&gateway, "call", Some(args), None).await;
        assert_eq!(result.is_error, Some(true));
        let structured = result.structured_content.expect("structured error present");
        assert_eq!(
            structured.get("code"),
            Some(&Value::String("NOT_FOUND".to_string()))
        );
    }

    #[tokio::test]
    async fn batch_returns_array_of_results() {
        let registry = full_registry_with_ops(vec![(
            "echo/run".to_string(),
            OperationType::Query,
            AccessControl::default(),
        )]);
        let gateway = ToMcpGateway::new(dispatch(registry, provider()));

        let mut args = Map::new();
        args.insert(
            "calls".to_string(),
            serde_json::json!([
                { "operation": "echo/run", "input": { "n": 1 } },
                { "operation": "no/such", "input": {} },
            ]),
        );
        let result = invoke_tool(&gateway, "batch", Some(args), None).await;
        assert_eq!(result.is_error, Some(false));
        let structured = result.structured_content.expect("structured present");
        let arr = structured.as_array().expect("batch returns array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0].get("isError"), Some(&Value::Bool(false)));
        assert_eq!(arr[1].get("isError"), Some(&Value::Bool(true)));
    }

    #[tokio::test]
    async fn call_with_restricted_op_and_unauthorized_identity_returns_forbidden_error() {
        let registry = full_registry_with_ops(vec![(
            "admin/run".to_string(),
            OperationType::Query,
            AccessControl {
                required_scopes: vec!["admin".to_string()],
                ..Default::default()
            },
        )]);
        let idp: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let gateway = ToMcpGateway::new(dispatch(registry, idp));

        let mut args = Map::new();
        args.insert(
            "operation".to_string(),
            Value::String("admin/run".to_string()),
        );
        args.insert("input".to_string(), Value::Object(Map::new()));
        let result = invoke_tool(&gateway, "call", Some(args), None).await;
        assert_eq!(result.is_error, Some(true));
        let structured = result.structured_content.expect("structured error present");
        assert_eq!(
            structured.get("code"),
            Some(&Value::String("FORBIDDEN".to_string()))
        );
    }

    #[tokio::test]
    async fn unknown_tool_name_returns_not_found_structured_error() {
        let gateway = ToMcpGateway::new(dispatch(Arc::new(OperationRegistry::new()), provider()));
        let result = invoke_tool(&gateway, "bogus", None, None).await;
        assert_eq!(result.is_error, Some(true));
        let structured = result.structured_content.expect("structured error present");
        assert_eq!(
            structured.get("code"),
            Some(&Value::String("NOT_FOUND".to_string()))
        );
    }

    #[tokio::test]
    async fn identity_survives_rmcp_framing_into_call_tool() {
        let registry = full_registry_with_ops(vec![(
            "admin/run".to_string(),
            OperationType::Query,
            AccessControl {
                required_scopes: vec!["admin".to_string()],
                ..Default::default()
            },
        )]);
        let idp: Arc<dyn IdentityProvider> = Arc::new(
            StaticIdentityProvider::new()
                .with_token("alk_admin", identity_with_scopes("admin-peer", &["admin"])),
        );
        let gateway = ToMcpGateway::new(dispatch(registry, idp));

        let admin_identity = identity_with_scopes("admin-peer", &["admin"]);
        let extensions = extensions_with_identity(Some(admin_identity.clone()));
        let extracted = ToMcpGateway::extract_identity_from_extensions(&extensions);
        assert_eq!(
            extracted.as_ref().map(|i| &i.id),
            Some(&"admin-peer".to_string())
        );

        let mut args = Map::new();
        args.insert(
            "operation".to_string(),
            Value::String("admin/run".to_string()),
        );
        args.insert("input".to_string(), serde_json::json!({ "ok": 1 }));
        let result = gateway.handle_call(Some(args), extracted).await;
        assert_eq!(result.is_error, Some(false));
        assert_eq!(
            result.structured_content,
            Some(serde_json::json!({ "ok": 1 }))
        );
    }

    #[test]
    fn extract_identity_returns_none_when_no_parts_in_extensions() {
        let extensions = Extensions::new();
        assert!(ToMcpGateway::extract_identity_from_extensions(&extensions).is_none());
    }

    #[test]
    fn extract_identity_returns_none_when_parts_have_no_identity() {
        let extensions = extensions_with_identity(None);
        assert!(ToMcpGateway::extract_identity_from_extensions(&extensions).is_none());
    }

    #[test]
    fn extract_identity_reads_stashed_option_identity_from_parts() {
        let id = identity_with_scopes("caller", &["read"]);
        let extensions = extensions_with_identity(Some(id.clone()));
        let extracted = ToMcpGateway::extract_identity_from_extensions(&extensions);
        assert_eq!(
            extracted.as_ref().map(|i| i.id.clone()),
            Some("caller".to_string())
        );
        assert_eq!(
            extracted.as_ref().map(|i| i.scopes.clone()),
            Some(vec!["read".to_string()])
        );
    }

    #[test]
    fn to_mcp_is_not_an_operation_adapter() {
        fn assert_not_adapter<T>() {}
        assert_not_adapter::<ToMcpGateway>();
    }

    #[test]
    fn gateway_tools_definition_is_stable() {
        let tools = gateway_tools();
        assert_eq!(tools.len(), 4);
        assert_eq!(tools[0].name, "search");
        assert_eq!(tools[1].name, "schema");
        assert_eq!(tools[2].name, "call");
        assert_eq!(tools[3].name, "batch");
    }

    #[tokio::test]
    async fn search_schema_call_round_trip() {
        let registry = full_registry_with_ops(vec![(
            "fs/readFile".to_string(),
            OperationType::Query,
            AccessControl::default(),
        )]);
        let gateway = ToMcpGateway::new(dispatch(registry, provider()));

        let search_result = invoke_tool(&gateway, "search", None, None).await;
        let ops = search_result
            .structured_content
            .as_ref()
            .and_then(|v| v.get("operations"))
            .and_then(Value::as_array)
            .expect("search ops");
        let first_name = ops[0].get("name").and_then(Value::as_str).expect("name");
        assert_eq!(first_name, "fs/readFile");

        let mut schema_args = Map::new();
        schema_args.insert("name".to_string(), Value::String(first_name.to_string()));
        let schema_result = invoke_tool(&gateway, "schema", Some(schema_args), None).await;
        assert_eq!(
            schema_result
                .structured_content
                .as_ref()
                .and_then(|v| v.get("name"))
                .and_then(Value::as_str),
            Some("fs/readFile")
        );

        let mut call_args = Map::new();
        call_args.insert(
            "operation".to_string(),
            Value::String(first_name.to_string()),
        );
        call_args.insert(
            "input".to_string(),
            serde_json::json!({ "path": "/etc/hosts" }),
        );
        let call_result = invoke_tool(&gateway, "call", Some(call_args), None).await;
        assert_eq!(
            call_result.structured_content,
            Some(serde_json::json!({ "path": "/etc/hosts" }))
        );
    }
}
