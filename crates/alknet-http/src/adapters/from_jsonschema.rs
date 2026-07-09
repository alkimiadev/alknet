//! `from_jsonschema` adapter: register a single HTTP-backed operation from a
//! caller-supplied [`OperationSpec`], path template, and HTTP method.
//!
//! The forwarding handler is the no-env-vars credential injection point
//! (ADR-014): it reads `OperationContext.capabilities`, never
//! `std::env::var`. Provenance is `FromJsonSchema` (leaf,
//! `composition_authority: None`, `scoped_env: None`, `Internal` by
//! default — ADR-015/022). Imported error codes are prefixed `HTTP_<status>`
//! to avoid collision with the protocol-level codes (ADR-023).
//!
//! See `docs/architecture/crates/http/http-adapters.md` §"from_jsonschema"
//! and ADR-066.

use std::sync::Arc;

use alknet_call::client::{AdapterError, OperationAdapter};
use alknet_call::registry::context::OperationContext;
use alknet_call::registry::registration::{
    make_handler, make_streaming_handler, HandlerKind, HandlerRegistration, OperationProvenance,
};
use alknet_call::registry::spec::{OperationSpec, OperationType};
use alknet_core::types::Capabilities;
use async_trait::async_trait;
use serde_json::Value;

use crate::adapters::from_openapi::{forward, forward_stream, HttpServiceConfig};
use crate::client::SharedHttpClient;

pub struct FromJsonSchema {
    spec: OperationSpec,
    config: HttpServiceConfig,
    path_template: String,
    method: String,
    http_client: Arc<SharedHttpClient>,
}

impl FromJsonSchema {
    pub fn new(
        spec: OperationSpec,
        config: HttpServiceConfig,
        path_template: String,
        method: String,
        http_client: Arc<SharedHttpClient>,
    ) -> Self {
        Self {
            spec,
            config,
            path_template,
            method,
            http_client,
        }
    }
}

#[async_trait]
impl OperationAdapter for FromJsonSchema {
    async fn import(&self) -> Result<Vec<HandlerRegistration>, AdapterError> {
        let path_template = self.path_template.clone();
        let method_upper = self.method.to_ascii_uppercase();
        let auth_scheme = self.config.auth.clone();
        let default_headers = self.config.default_headers.clone();
        let base_url = self.config.base_url.clone();
        let namespace = self.config.namespace.clone();
        let http_client = Arc::clone(&self.http_client);
        let op_type = self.spec.op_type;

        let error_status_codes: Vec<(u16, String)> = self
            .spec
            .error_schemas
            .iter()
            .map(|e| (e.http_status.unwrap_or(0), e.code.clone()))
            .collect();

        let handler = if op_type == OperationType::Subscription {
            let stream_handler =
                make_streaming_handler(move |input: Value, context: OperationContext| {
                    let path_template = path_template.clone();
                    let method_upper = method_upper.clone();
                    let auth_scheme = auth_scheme.clone();
                    let default_headers = default_headers.clone();
                    let base_url = base_url.clone();
                    let namespace = namespace.clone();
                    let http_client = Arc::clone(&http_client);
                    let error_status_codes = error_status_codes.clone();
                    forward_stream(
                        &http_client,
                        &base_url,
                        &path_template,
                        &method_upper,
                        &auth_scheme,
                        &default_headers,
                        &namespace,
                        &error_status_codes,
                        input,
                        context,
                    )
                });
            HandlerKind::Stream(stream_handler)
        } else {
            let once_handler = make_handler(move |input: Value, context: OperationContext| {
                let path_template = path_template.clone();
                let method_upper = method_upper.clone();
                let auth_scheme = auth_scheme.clone();
                let default_headers = default_headers.clone();
                let base_url = base_url.clone();
                let namespace = namespace.clone();
                let http_client = Arc::clone(&http_client);
                let error_status_codes = error_status_codes.clone();
                let op_type = op_type;
                async move {
                    forward(
                        &http_client,
                        &base_url,
                        &path_template,
                        &method_upper,
                        &auth_scheme,
                        &default_headers,
                        &namespace,
                        &error_status_codes,
                        op_type,
                        input,
                        context,
                    )
                    .await
                }
            });
            HandlerKind::Once(once_handler)
        };

        let capabilities = Capabilities::new();
        Ok(vec![HandlerRegistration::new(
            self.spec.clone(),
            handler,
            OperationProvenance::FromJsonSchema,
            None,
            None,
            capabilities,
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::from_openapi::{build_request, HttpAuthScheme};
    use crate::client::HttpClientConfig;
    use alknet_call::registry::spec::{AccessControl, ErrorDefinition, Visibility};
    use reqwest::header::AUTHORIZATION;
    use reqwest::Method;
    use std::collections::HashMap;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn noop_context(request_id: &str, capabilities: Capabilities) -> OperationContext {
        struct NoopEnv;
        #[async_trait]
        impl alknet_call::registry::env::OperationEnv for NoopEnv {
            async fn invoke_with_policy(
                &self,
                _ns: &str,
                _op: &str,
                _input: Value,
                parent: &OperationContext,
                _policy: alknet_call::registry::context::AbortPolicy,
            ) -> alknet_call::protocol::wire::ResponseEnvelope {
                alknet_call::protocol::wire::ResponseEnvelope::ok(
                    parent.request_id.clone(),
                    Value::Null,
                )
            }
            fn contains(&self, _name: &str) -> bool {
                false
            }
        }
        OperationContext {
            request_id: request_id.to_string(),
            parent_request_id: None,
            identity: None,
            handler_identity: None,
            forwarded_for: None,
            capabilities,
            metadata: HashMap::new(),
            scoped_env: alknet_call::registry::context::ScopedPeerEnv::empty(),
            env: Arc::new(NoopEnv),
            abort_policy: alknet_call::registry::context::AbortPolicy::default(),
            deadline: Some(std::time::Instant::now() + Duration::from_secs(30)),
            internal: true,
            ownership: None,
        }
    }

    fn test_spec(name: &str, op_type: OperationType) -> OperationSpec {
        OperationSpec::new(
            name,
            op_type,
            Visibility::Internal,
            serde_json::json!({"type":"object","properties":{"id":{"type":"string"}}}),
            serde_json::json!({"type":"object"}),
            vec![],
            AccessControl::default(),
            None,
        )
    }

    fn test_spec_with_errors(
        name: &str,
        op_type: OperationType,
        errors: Vec<ErrorDefinition>,
    ) -> OperationSpec {
        OperationSpec::new(
            name,
            op_type,
            Visibility::Internal,
            serde_json::json!({"type":"object"}),
            serde_json::json!({"type":"object"}),
            errors,
            AccessControl::default(),
            None,
        )
    }

    fn test_config(namespace: &str, base_url: &str) -> HttpServiceConfig {
        HttpServiceConfig {
            namespace: namespace.to_string(),
            base_url: base_url.to_string(),
            auth: None,
            default_headers: HashMap::new(),
        }
    }

    fn test_http_client() -> Arc<SharedHttpClient> {
        Arc::new(SharedHttpClient::new(HttpClientConfig::default()).unwrap())
    }

    #[tokio::test]
    async fn import_produces_one_handler_registration() {
        let adapter = FromJsonSchema::new(
            test_spec("svc/getWidget", OperationType::Query),
            test_config("svc", "https://api.example.com"),
            "/widgets".to_string(),
            "GET".to_string(),
            test_http_client(),
        );
        let bundles = adapter.import().await.unwrap();
        assert_eq!(bundles.len(), 1);
        assert_eq!(bundles[0].spec.name, "svc/getWidget");
        assert_eq!(bundles[0].provenance, OperationProvenance::FromJsonSchema);
        assert!(bundles[0].composition_authority.is_none());
        assert!(bundles[0].scoped_env.is_none());
    }

    #[tokio::test]
    async fn query_op_registration_is_handler_kind_once() {
        let adapter = FromJsonSchema::new(
            test_spec("svc/getWidget", OperationType::Query),
            test_config("svc", "https://api.example.com"),
            "/widgets".to_string(),
            "GET".to_string(),
            test_http_client(),
        );
        let bundles = adapter.import().await.unwrap();
        assert!(matches!(bundles[0].handler, HandlerKind::Once(_)));
    }

    #[tokio::test]
    async fn subscription_op_registration_is_handler_kind_stream() {
        let adapter = FromJsonSchema::new(
            test_spec("svc/stream", OperationType::Subscription),
            test_config("svc", "https://api.example.com"),
            "/stream".to_string(),
            "POST".to_string(),
            test_http_client(),
        );
        let bundles = adapter.import().await.unwrap();
        assert!(matches!(bundles[0].handler, HandlerKind::Stream(_)));
    }

    #[tokio::test]
    async fn mutation_op_registration_is_handler_kind_once() {
        let adapter = FromJsonSchema::new(
            test_spec("svc/createWidget", OperationType::Mutation),
            test_config("svc", "https://api.example.com"),
            "/widgets".to_string(),
            "POST".to_string(),
            test_http_client(),
        );
        let bundles = adapter.import().await.unwrap();
        assert!(matches!(bundles[0].handler, HandlerKind::Once(_)));
    }

    #[tokio::test]
    async fn build_request_injects_bearer_from_capabilities() {
        let caps = Capabilities::new().with_http_token("github", "tok-123".to_string());
        let ctx = noop_context("req-1", caps);
        let (method, url, _body, headers) = build_request(
            "https://api.github.com",
            "/repos/{owner}/{repo}/issues",
            "GET",
            &Some(HttpAuthScheme::Bearer),
            &HashMap::new(),
            "github",
            &serde_json::json!({"owner":"a","repo":"b"}),
            &ctx,
        )
        .unwrap();
        assert_eq!(method, Method::GET);
        assert_eq!(url.path(), "/repos/a/b/issues");
        assert_eq!(url.host_str(), Some("api.github.com"));
        let auth = headers.get(AUTHORIZATION).unwrap();
        assert_eq!(auth.to_str().unwrap(), "Bearer tok-123");
    }

    #[tokio::test]
    async fn build_request_path_and_query_split() {
        let ctx = noop_context("req-2", Capabilities::new());
        let (_, url, _, _) = build_request(
            "https://api.example.com",
            "/widgets/{id}",
            "GET",
            &None,
            &HashMap::new(),
            "svc",
            &serde_json::json!({"id":42,"filter":"active"}),
            &ctx,
        )
        .unwrap();
        assert_eq!(url.path(), "/widgets/42");
        assert_eq!(url.query().unwrap(), "filter=active");
    }

    async fn spawn_echo_server(
        status: u16,
        body: &'static str,
        content_type: &'static str,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(_) => break,
                };
                let status_line = match status {
                    200 => "200 OK",
                    201 => "201 Created",
                    404 => "404 Not Found",
                    500 => "500 Internal Server Error",
                    _ => "200 OK",
                };
                let body_bytes = body.as_bytes();
                let response = format!(
                    "HTTP/1.1 {status_line}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body_bytes.len(),
                    body
                );
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                sock.write_all(response.as_bytes()).await.unwrap();
                sock.flush().await.unwrap();
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn integration_forwarding_handler_calls_external_endpoint() {
        let base = spawn_echo_server(200, r#"{"ok":true}"#, "application/json").await;
        let adapter = FromJsonSchema::new(
            test_spec("svc/data", OperationType::Query),
            test_config("svc", &base),
            "/data".to_string(),
            "GET".to_string(),
            test_http_client(),
        );
        let bundles = adapter.import().await.unwrap();
        let registration = &bundles[0];
        let ctx = noop_context("req-10", Capabilities::new());
        let response = match &registration.handler {
            HandlerKind::Once(h) => h(serde_json::json!({}), ctx).await,
            _ => panic!("expected Once handler"),
        };
        assert_eq!(response.request_id, "req-10");
        match response.result {
            Ok(v) => assert_eq!(v, serde_json::json!({"ok":true})),
            Err(e) => panic!("expected Ok, got {e:?}"),
        }
    }

    #[tokio::test]
    async fn integration_non_2xx_returns_declared_error() {
        let base = spawn_echo_server(404, r#"{"error":"missing"}"#, "application/json").await;
        let errors = vec![ErrorDefinition {
            code: "HTTP_404".to_string(),
            description: "Not found".to_string(),
            schema: serde_json::json!({"type":"object"}),
            http_status: Some(404),
        }];
        let adapter = FromJsonSchema::new(
            test_spec_with_errors("svc/missing", OperationType::Query, errors),
            test_config("svc", &base),
            "/missing".to_string(),
            "GET".to_string(),
            test_http_client(),
        );
        let bundles = adapter.import().await.unwrap();
        let registration = &bundles[0];
        let ctx = noop_context("req-11", Capabilities::new());
        let response = match &registration.handler {
            HandlerKind::Once(h) => h(serde_json::json!({}), ctx).await,
            _ => panic!("expected Once handler"),
        };
        match response.result {
            Err(e) => {
                assert_eq!(e.code, "HTTP_404");
                assert!(!e.retryable);
            }
            other => panic!("expected HTTP_404 error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn integration_undeclared_error_status_returns_http_status_code() {
        let base = spawn_echo_server(500, "boom", "text/plain").await;
        let adapter = FromJsonSchema::new(
            test_spec("svc/x", OperationType::Query),
            test_config("svc", &base),
            "/x".to_string(),
            "GET".to_string(),
            test_http_client(),
        );
        let bundles = adapter.import().await.unwrap();
        let registration = &bundles[0];
        let ctx = noop_context("req-12", Capabilities::new());
        let response = match &registration.handler {
            HandlerKind::Once(h) => h(serde_json::json!({}), ctx).await,
            _ => panic!("expected Once handler"),
        };
        match response.result {
            Err(e) => assert_eq!(e.code, "HTTP_500"),
            other => panic!("expected HTTP_500, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn integration_sse_subscription_streams_responded_events() {
        let sse_body = "data: {\"n\":1}\n\ndata: {\"n\":2}\n\n";
        let base = spawn_echo_server(200, sse_body, "text/event-stream").await;
        let adapter = FromJsonSchema::new(
            test_spec("svc/stream", OperationType::Subscription),
            test_config("svc", &base),
            "/stream".to_string(),
            "POST".to_string(),
            test_http_client(),
        );
        let bundles = adapter.import().await.unwrap();
        let registration = &bundles[0];
        let ctx = noop_context("req-13", Capabilities::new());
        let stream = match &registration.handler {
            HandlerKind::Stream(h) => h(serde_json::json!({}), ctx),
            _ => panic!("expected Stream handler"),
        };
        use futures::StreamExt;
        let collected: Vec<alknet_call::protocol::wire::ResponseEnvelope> = stream.collect().await;
        assert_eq!(collected.len(), 2);
        assert_eq!(collected[0].result, Ok(serde_json::json!({"n":1})));
        assert_eq!(collected[1].result, Ok(serde_json::json!({"n":2})));
        assert_eq!(collected[0].request_id, "req-13");
        assert_eq!(collected[1].request_id, "req-13");
    }

    #[test]
    fn no_env_vars_read_in_build_request() {
        std::env::set_var("OPENAI_API_KEY", "should-not-be-used");
        let ctx = noop_context("req-14", Capabilities::new());
        let (_, _, _, headers) = build_request(
            "https://api.openai.com",
            "/v1/chat",
            "POST",
            &Some(HttpAuthScheme::Bearer),
            &HashMap::new(),
            "openai",
            &serde_json::json!({"body":{"prompt":"hi"}}),
            &ctx,
        )
        .unwrap();
        assert!(
            headers.get(AUTHORIZATION).is_none(),
            "no auth header when capabilities absent"
        );
        std::env::remove_var("OPENAI_API_KEY");
    }
}
