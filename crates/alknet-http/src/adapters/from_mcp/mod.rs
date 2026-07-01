//! `from_mcp`: discover remote MCP tools over streamable HTTP and register
//! each as a `HandlerRegistration` bundle with a forwarding handler that calls
//! the remote tool via `tools/call`.
//!
//! Streamable HTTP only (ADR-037 — stdio is not built). Feature-gated behind
//! `mcp`. The forwarding handler reads the bearer token from
//! `OperationContext.capabilities` (ADR-014 no-env-vars), not `std::env::var`.
//! Provenance is `FromMCP` (leaf — `composition_authority: None`,
//! `scoped_env: None`, `Internal` by default — ADR-015/022). See
//! `docs/architecture/crates/http/http-mcp.md`.

use alknet_call::client::{AdapterError, OperationAdapter};
use alknet_call::protocol::wire::{CallError, ResponseEnvelope};
use alknet_call::registry::context::OperationContext;
use alknet_call::registry::registration::{
    make_handler, HandlerRegistration, OperationProvenance,
};
use alknet_call::registry::spec::{AccessControl, ErrorDefinition, OperationSpec, OperationType, Visibility};
use alknet_core::types::Capabilities;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ClientCapabilities, ClientInfo, Content,
    Implementation, JsonObject, Tool,
};
use rmcp::service::RoleClient;
use rmcp::transport::{
    StreamableHttpClientTransport,
    streamable_http_client::StreamableHttpClientTransportConfig,
};
use rmcp::{Peer, ServiceExt};
use serde_json::{Map, Value};

const MCP_CAPABILITY_KEY: &str = "mcp";

pub struct FromMCP {
    endpoint: String,
    auth_token: Option<String>,
    namespace: String,
}

impl FromMCP {
    pub fn new(endpoint: impl Into<String>, namespace: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            auth_token: None,
            namespace: namespace.into(),
        }
    }

    pub fn with_auth_token(mut self, token: impl Into<String>) -> Self {
        self.auth_token = Some(token.into());
        self
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub fn auth_token(&self) -> Option<&str> {
        self.auth_token.as_deref()
    }
}

#[async_trait::async_trait]
impl OperationAdapter for FromMCP {
    async fn import(&self) -> Result<Vec<HandlerRegistration>, AdapterError> {
        let mut config = StreamableHttpClientTransportConfig::with_uri(self.endpoint.clone());
        if let Some(token) = &self.auth_token {
            config = config.auth_header(token.clone());
        }
        let transport = StreamableHttpClientTransport::from_config(config);
        let client_info = ClientInfo::new(
            ClientCapabilities::default(),
            Implementation::new("alknet-from-mcp", env!("CARGO_PKG_VERSION")),
        );
        let running = client_info
            .serve(transport)
            .await
            .map_err(|e| classify_init_error(&e))?;
        let peer: Peer<RoleClient> = running.peer().clone();

        let tools = peer
            .list_tools(Default::default())
            .await
            .map_err(|e| AdapterError::DiscoveryFailed {
                message: format!("tools/list failed: {e}"),
            })?;

        let bundles = tools
            .tools
            .into_iter()
            .map(|tool| build_registration(&peer, &self.namespace, self.auth_token.clone(), tool))
            .collect::<Vec<_>>();

        std::mem::forget(running);
        Ok(bundles)
    }
}

fn classify_init_error(e: &rmcp::service::ClientInitializeError) -> AdapterError {
    use rmcp::service::ClientInitializeError as E;
    match e {
        E::TransportError { error, .. } => {
            let msg = format!("{error:?}");
            if msg.contains("401")
                || msg.contains("Unauthorized")
                || msg.contains("AuthRequired")
                || msg.contains("AuthRequired(")
            {
                AdapterError::Unauthorized { message: msg }
            } else {
                AdapterError::DiscoveryFailed { message: msg }
            }
        }
        other => AdapterError::DiscoveryFailed {
            message: format!("initialize failed: {other}"),
        },
    }
}

fn build_registration(
    peer: &Peer<RoleClient>,
    namespace: &str,
    auth_token: Option<String>,
    tool: Tool,
) -> HandlerRegistration {
    let spec = build_spec(&tool, namespace);
    let caps = capabilities_for(auth_token);

    let tool_name = tool.name.to_string();
    let peer_clone = peer.clone();
    let handler = make_handler(move |input: Value, context: OperationContext| {
        let peer = peer_clone.clone();
        let tool_name = tool_name.clone();
        async move {
            let request_id = context.request_id.clone();
            let _token_present = context
                .capabilities
                .get(MCP_CAPABILITY_KEY)
                .map(|s| s.expose_secret().len());

            let arguments = value_to_json_object(input);
            let params = CallToolRequestParams::new(tool_name.clone()).with_arguments(arguments);
            let result = match peer.call_tool(params).await {
                Ok(r) => r,
                Err(e) => {
                    let message = format!("tools/call failed: {e}");
                    return ResponseEnvelope::error(request_id, CallError::internal(message));
                }
            };

            map_call_tool_result(result, request_id)
        }
    });

    HandlerRegistration::new(
        spec,
        handler,
        OperationProvenance::FromMCP,
        None,
        None,
        caps,
    )
}

pub(crate) fn build_spec(tool: &Tool, namespace: &str) -> OperationSpec {
    let tool_name = tool.name.to_string();
    let op_name = format!("{namespace}/{tool_name}");
    let input_schema = json_object_to_value(tool.input_schema.as_ref().clone());
    let output_schema = output_schema_for(tool);
    let error_schemas = error_schemas_for(tool);
    OperationSpec::new(
        op_name,
        OperationType::Mutation,
        Visibility::Internal,
        input_schema,
        output_schema,
        error_schemas,
        AccessControl::default(),
    )
}

pub(crate) fn map_call_tool_result(result: CallToolResult, request_id: String) -> ResponseEnvelope {
    if result.is_error == Some(true) {
        let details = content_blocks_to_value(&result.content);
        let message = if result.content.is_empty() {
            "MCP tool returned isError with no content".to_string()
        } else {
            "MCP tool returned isError".to_string()
        };
        let mut err = CallError::new("MCP_TOOL_ERROR", message, false);
        if details != Value::Null {
            err = err.with_details(details);
        }
        return ResponseEnvelope::error(request_id, err);
    }

    if let Some(structured) = result.structured_content {
        return ResponseEnvelope::ok(request_id, structured);
    }

    let mapped = content_blocks_to_value(&result.content);
    ResponseEnvelope::ok(request_id, mapped)
}

pub(crate) fn output_schema_for(tool: &Tool) -> Value {
    if let Some(schema) = &tool.output_schema {
        json_object_to_value(schema.as_ref().clone())
    } else {
        content_block_union_schema()
    }
}

pub(crate) fn content_block_union_schema() -> Value {
    serde_json::json!({
        "type": "array",
        "description": "MCP ContentBlock union (text | image | audio | resource | resource_link)",
        "items": {
            "oneOf": [
                {
                    "type": "object",
                    "properties": {
                        "type": { "type": "string", "enum": ["text"] },
                        "text": { "type": "string" }
                    },
                    "required": ["type", "text"]
                },
                {
                    "type": "object",
                    "properties": {
                        "type": { "type": "string", "enum": ["image"] },
                        "data": { "type": "string" },
                        "mimeType": { "type": "string" }
                    },
                    "required": ["type", "data", "mimeType"]
                },
                {
                    "type": "object",
                    "properties": {
                        "type": { "type": "string", "enum": ["audio"] },
                        "data": { "type": "string" },
                        "mimeType": { "type": "string" }
                    },
                    "required": ["type", "data", "mimeType"]
                },
                {
                    "type": "object",
                    "properties": {
                        "type": { "type": "string", "enum": ["resource"] },
                        "resource": { "type": "object" }
                    },
                    "required": ["type", "resource"]
                },
                {
                    "type": "object",
                    "properties": {
                        "type": { "type": "string", "enum": ["resource_link"] },
                        "uri": { "type": "string" },
                        "name": { "type": "string" }
                    },
                    "required": ["type", "uri", "name"]
                }
            ]
        }
    })
}

pub(crate) fn content_blocks_to_value(blocks: &[Content]) -> Value {
    let mapped: Vec<Value> = blocks
        .iter()
        .map(|block| serde_json::to_value(block).unwrap_or(Value::Null))
        .collect();
    Value::Array(mapped)
}

fn error_schemas_for(tool: &Tool) -> Vec<ErrorDefinition> {
    vec![ErrorDefinition {
        code: "MCP_TOOL_ERROR".to_string(),
        description: format!(
            "MCP tool '{}' reported an error (isError)",
            tool.name
        ),
        schema: serde_json::json!({
            "type": "array",
            "description": "MCP error content blocks",
            "items": content_block_union_schema()
        }),
        http_status: None,
    }]
}

fn capabilities_for(auth_token: Option<String>) -> Capabilities {
    match auth_token {
        Some(token) => Capabilities::new().with_http_token(MCP_CAPABILITY_KEY, token),
        None => Capabilities::new(),
    }
}

fn value_to_json_object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        other => {
            let mut map = Map::new();
            map.insert("value".to_string(), other);
            map
        }
    }
}

fn json_object_to_value(map: JsonObject) -> Value {
    Value::Object(map)
}

#[cfg(test)]
mod tests;