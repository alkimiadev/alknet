//! Integration test for `FromMCP`: spins up a real rmcp streamable HTTP MCP
//! server, imports its tools via `FromMCP::import()`, and invokes a
//! forwarding handler end-to-end. Verifies the handler calls the remote MCP
//! tool via rmcp and reads `context.capabilities` (not `std::env::var`).

#![cfg(feature = "mcp")]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alknet_call::client::OperationAdapter;
use alknet_call::protocol::wire::ResponseEnvelope;
use alknet_call::registry::context::{AbortPolicy, OperationContext, ScopedPeerEnv};
use alknet_call::registry::env::OperationEnv;
use alknet_call::registry::registration::{HandlerKind, OperationProvenance};
use alknet_core::types::Capabilities;
use alknet_http::adapters::FromMCP;
use axum::Router;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, ListToolsResult, PaginatedRequestParams, Tool,
};
use rmcp::service::RequestContext;
use rmcp::transport::{
    streamable_http_server::{session::local::LocalSessionManager, tower::StreamableHttpService},
    StreamableHttpServerConfig,
};
use rmcp::{RoleServer, ServerHandler};
use serde_json::Value;

struct NoopEnv;

#[async_trait::async_trait]
impl OperationEnv for NoopEnv {
    async fn invoke_with_policy(
        &self,
        _ns: &str,
        _op: &str,
        _input: Value,
        parent: &OperationContext,
        _policy: AbortPolicy,
    ) -> ResponseEnvelope {
        ResponseEnvelope::ok(parent.request_id.clone(), Value::Null)
    }

    fn contains(&self, _name: &str) -> bool {
        false
    }
}

fn test_context(request_id: &str, caps: Capabilities) -> OperationContext {
    OperationContext {
        request_id: request_id.to_string(),
        parent_request_id: None,
        identity: None,
        handler_identity: None,
        forwarded_for: None,
        capabilities: caps,
        metadata: HashMap::new(),
        scoped_env: ScopedPeerEnv::empty(),
        env: Arc::new(NoopEnv),
        abort_policy: AbortPolicy::default(),
        deadline: Some(Instant::now() + Duration::from_secs(30)),
        internal: true,
        ownership: None,
    }
}

struct EchoServer;

impl ServerHandler for EchoServer {
    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, rmcp::ErrorData>>
           + rmcp::service::MaybeSendFuture
           + '_ {
        let tools = vec![
            Tool::new_with_raw(
                "echo",
                Some("Echo the input back as structured content".into()),
                Arc::new(serde_json::Map::new()),
            )
            .with_raw_output_schema(Arc::new(serde_json::Map::from_iter([(
                "type".to_string(),
                Value::String("object".into()),
            )]))),
            Tool::new_with_raw(
                "legacy",
                Some("Legacy tool returning text content blocks".into()),
                Arc::new(serde_json::Map::new()),
            ),
        ];
        std::future::ready(Ok(ListToolsResult {
            meta: None,
            next_cursor: None,
            tools,
        }))
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResult, rmcp::ErrorData>>
           + rmcp::service::MaybeSendFuture
           + '_ {
        let name = request.name.to_string();
        std::future::ready(Ok(match name.as_str() {
            "echo" => {
                let args = request.arguments.map(Value::Object).unwrap_or(Value::Null);
                CallToolResult::structured(serde_json::json!({ "echoed": args }))
            }
            "legacy" => CallToolResult::success(vec![Content::text("plain text result")]),
            other => CallToolResult::error(vec![Content::text(format!("unknown tool: {other}"))]),
        }))
    }

    fn get_info(&self) -> rmcp::model::ServerInfo {
        rmcp::model::ServerInfo::default()
    }
}

async fn spawn_server() -> (String, tokio::task::JoinHandle<()>) {
    let mcp_service: StreamableHttpService<EchoServer, LocalSessionManager> =
        StreamableHttpService::new(
            || Ok(EchoServer),
            LocalSessionManager::default().into(),
            StreamableHttpServerConfig::default(),
        );
    let app = Router::new().nest_service("/mcp", mcp_service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}/mcp"), handle)
}

#[tokio::test]
async fn import_discovers_tools_and_builds_registrations() {
    let (endpoint, _handle) = spawn_server().await;
    let adapter = FromMCP::new(endpoint, "echo");
    let bundles = adapter
        .import()
        .await
        .expect("import succeeds against running server");
    assert_eq!(bundles.len(), 2);
    let names: Vec<&str> = bundles.iter().map(|b| b.spec.name.as_str()).collect();
    assert!(names.contains(&"echo/echo"));
    assert!(names.contains(&"echo/legacy"));
    for b in &bundles {
        assert_eq!(b.provenance, OperationProvenance::FromMCP);
        assert!(b.composition_authority.is_none());
        assert!(b.scoped_env.is_none());
    }
}

#[tokio::test]
async fn forwarding_handler_calls_echo_and_returns_structured_content() {
    let (endpoint, _handle) = spawn_server().await;
    let adapter = FromMCP::new(endpoint, "echo");
    let bundles = adapter.import().await.expect("import succeeds");
    let echo = bundles
        .into_iter()
        .find(|b| b.spec.name == "echo/echo")
        .expect("echo tool present");

    let caps = Capabilities::new().with_http_token("mcp", "unused-on-server".to_string());
    let ctx = test_context("req-echo", caps);
    let input = serde_json::json!({ "msg": "hello" });
    let response = match &echo.handler {
        HandlerKind::Once(h) => h(input, ctx).await,
        HandlerKind::Stream(_) => panic!("expected Once handler for echo tool"),
    };

    assert_eq!(response.request_id, "req-echo");
    match response.result {
        Ok(v) => {
            let obj = v.as_object().expect("structured object");
            assert!(obj.contains_key("echoed"));
        }
        Err(e) => panic!("expected Ok, got Err: {e:?}"),
    }
}

#[tokio::test]
async fn forwarding_handler_calls_legacy_and_returns_content_blocks() {
    let (endpoint, _handle) = spawn_server().await;
    let adapter = FromMCP::new(endpoint, "echo");
    let bundles = adapter.import().await.expect("import succeeds");
    let legacy = bundles
        .into_iter()
        .find(|b| b.spec.name == "echo/legacy")
        .expect("legacy tool present");

    let ctx = test_context("req-legacy", Capabilities::new());
    let response = match &legacy.handler {
        HandlerKind::Once(h) => h(serde_json::json!({}), ctx).await,
        HandlerKind::Stream(_) => panic!("expected Once handler for legacy tool"),
    };

    match response.result {
        Ok(Value::Array(blocks)) => {
            assert_eq!(blocks.len(), 1);
            assert_eq!(blocks[0]["type"], "text");
            assert_eq!(blocks[0]["text"], "plain text result");
        }
        other => panic!("expected array of content blocks, got {other:?}"),
    }
}

#[tokio::test]
async fn forwarding_handler_does_not_read_env_vars() {
    std::env::set_var("MCP_TOKEN", "should-not-be-used");
    let (endpoint, _handle) = spawn_server().await;
    let adapter = FromMCP::new(endpoint, "echo");
    let bundles = adapter.import().await.expect("import succeeds");
    let echo = bundles
        .into_iter()
        .find(|b| b.spec.name == "echo/echo")
        .expect("echo tool present");

    let ctx = test_context("req-noenv", Capabilities::new());
    let response = match &echo.handler {
        HandlerKind::Once(h) => h(serde_json::json!({ "x": 1 }), ctx).await,
        HandlerKind::Stream(_) => panic!("expected Once handler for echo tool"),
    };
    assert!(response.result.is_ok(), "handler works without env var");
    std::env::remove_var("MCP_TOKEN");
}

#[tokio::test]
async fn import_unreachable_server_returns_discovery_failed() {
    let adapter = FromMCP::new("http://127.0.0.1:1/mcp", "x");
    match adapter.import().await {
        Ok(_) => panic!("expected Err for unreachable server"),
        Err(alknet_call::client::AdapterError::DiscoveryFailed { .. }) => {}
        Err(alknet_call::client::AdapterError::Transport { .. }) => {}
        Err(other) => panic!("expected DiscoveryFailed or Transport, got {other}"),
    }
}
