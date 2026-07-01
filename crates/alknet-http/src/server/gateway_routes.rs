//! The 5 fixed gateway endpoints (`/search`, `/schema`, `/call`, `/batch`,
//! `/subscribe`) — the sole HTTP invoke path (ADR-042, ADR-047).
//!
//! See `docs/architecture/crates/http/http-server.md` §"HTTP-to-call dispatch"
//! and `docs/architecture/crates/http/http-adapters.md` §"The gateway endpoint
//! set". Each endpoint delegates to `GatewayDispatch::invoke()` (the shared
//! dispatch spine); auth is the shared `bearer_auth_middleware`; error mapping
//! is `call_error_to_http_response`. There is no per-operation
//! `POST /{service}/{op}` direct-call surface (ADR-047).

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::{FromRef, Query, State};
use axum::http::StatusCode;
use axum::response::sse::Event;
use axum::response::{IntoResponse, Json, Response, Sse};
use axum::routing::{get, post};
use axum::Router;
use futures::stream::{self, BoxStream, Stream};
use serde::Deserialize;
use serde_json::{json, Value};

use alknet_call::protocol::wire::{CallError, ResponseEnvelope};
use alknet_call::registry::registration::OperationRegistry;
use alknet_call::registry::spec::{AccessResult, Visibility};
use alknet_core::auth::{Identity, IdentityProvider};

use super::adapter::RouterState;
use super::auth::ResolvedIdentity;
use crate::gateway::dispatch::GatewayDispatch;
use crate::gateway::error::{call_error_to_http_response, call_error_to_http_status_with_identity};

const SERVICES_LIST: &str = "services/list";
const SERVICES_SCHEMA: &str = "services/schema";

#[derive(Clone)]
pub(crate) struct GatewayState {
    registry: Arc<OperationRegistry>,
    identity_provider: Arc<dyn IdentityProvider>,
}

impl GatewayState {
    pub(crate) fn new(
        registry: Arc<OperationRegistry>,
        identity_provider: Arc<dyn IdentityProvider>,
    ) -> Self {
        Self {
            registry,
            identity_provider,
        }
    }

    fn dispatch(&self) -> GatewayDispatch {
        GatewayDispatch::new(
            Arc::clone(&self.registry),
            Arc::clone(&self.identity_provider),
        )
    }
}

impl FromRef<RouterState> for GatewayState {
    fn from_ref(state: &RouterState) -> Self {
        GatewayState::new(
            Arc::clone(&state.registry),
            Arc::clone(&state.identity_provider),
        )
    }
}

pub(crate) fn gateway_router() -> Router<RouterState> {
    Router::new()
        .route("/search", get(search_handler))
        .route("/schema", get(schema_handler))
        .route("/call", post(call_handler))
        .route("/batch", post(batch_handler))
        .route("/subscribe", post(subscribe_handler))
}

#[derive(Debug, Deserialize)]
pub struct CallRequest {
    pub operation: String,
    #[serde(default = "Value::default")]
    pub input: Value,
}

#[derive(Debug, Deserialize)]
pub struct SchemaQuery {
    pub name: String,
}

pub(crate) async fn call_handler(
    State(state): State<GatewayState>,
    ResolvedIdentity(identity): ResolvedIdentity,
    Json(request): Json<CallRequest>,
) -> Response {
    if is_internal_op(&state.registry, &request.operation) {
        return not_found_response(&request.operation);
    }
    let dispatch = state.dispatch();
    let envelope = dispatch
        .invoke(identity.clone(), &request.operation, request.input)
        .await;
    envelope_to_response(envelope, identity.as_ref())
}

pub(crate) async fn search_handler(
    State(state): State<GatewayState>,
    ResolvedIdentity(identity): ResolvedIdentity,
) -> Response {
    let dispatch = state.dispatch();
    let envelope = dispatch
        .invoke(identity.clone(), SERVICES_LIST, json!({}))
        .await;
    envelope_to_response(envelope, identity.as_ref())
}

pub(crate) async fn schema_handler(
    State(state): State<GatewayState>,
    ResolvedIdentity(identity): ResolvedIdentity,
    Query(query): Query<SchemaQuery>,
) -> Response {
    if let Some(forbidden) = access_check_for_op(&state.registry, &query.name, identity.as_ref()) {
        return forbidden_response(forbidden, identity.as_ref());
    }
    let dispatch = state.dispatch();
    let envelope = dispatch
        .invoke(
            identity.clone(),
            SERVICES_SCHEMA,
            json!({ "name": query.name }),
        )
        .await;
    envelope_to_response(envelope, identity.as_ref())
}

pub(crate) async fn batch_handler(
    State(state): State<GatewayState>,
    ResolvedIdentity(identity): ResolvedIdentity,
    Json(requests): Json<Vec<CallRequest>>,
) -> Response {
    let dispatch = state.dispatch();
    let mut results: Vec<Value> = Vec::with_capacity(requests.len());
    for request in requests {
        if is_internal_op(&state.registry, &request.operation) {
            results.push(not_found_envelope_json(&request.operation));
            continue;
        }
        let envelope = dispatch
            .invoke(identity.clone(), &request.operation, request.input)
            .await;
        results.push(envelope_to_json(envelope));
    }
    Json(json!({ "results": results })).into_response()
}

pub(crate) async fn subscribe_handler(
    State(state): State<GatewayState>,
    ResolvedIdentity(identity): ResolvedIdentity,
    Json(request): Json<CallRequest>,
) -> Sse<SubscribeStream> {
    let stream = if is_internal_op(&state.registry, &request.operation) {
        subscribe_stream_internal_error(request.operation)
    } else {
        let dispatch = state.dispatch();
        let envelope = dispatch
            .invoke(identity, &request.operation, request.input)
            .await;
        subscribe_stream_from_envelope(envelope)
    };
    Sse::new(stream)
}

pub type SubscribeStream = BoxStream<'static, Result<Event, Infallible>>;

fn subscribe_stream_from_envelope(envelope: ResponseEnvelope) -> SubscribeStream {
    Box::pin(envelope_to_sse_stream(envelope))
}

fn subscribe_stream_internal_error(operation: String) -> SubscribeStream {
    Box::pin(stream::once(async move { error_event(&operation) }))
}

fn envelope_to_response(envelope: ResponseEnvelope, identity: Option<&Identity>) -> Response {
    match envelope.result {
        Ok(output) => {
            let body = envelope_to_ok_json(&envelope.request_id, &output);
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(error) => {
            let status_code = call_error_to_http_status_with_identity(&error, identity);
            let status =
                StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let body = serde_json::to_value(&error).unwrap_or(Value::Null);
            (status, Json(body)).into_response()
        }
    }
}

fn envelope_to_json(envelope: ResponseEnvelope) -> Value {
    match envelope.result {
        Ok(output) => envelope_to_ok_json(&envelope.request_id, &output),
        Err(error) => envelope_to_error_json(&envelope.request_id, &error),
    }
}

fn envelope_to_ok_json(request_id: &str, output: &Value) -> Value {
    json!({
        "request_id": request_id,
        "result": "ok",
        "output": output,
    })
}

fn envelope_to_error_json(request_id: &str, error: &CallError) -> Value {
    json!({
        "request_id": request_id,
        "result": "error",
        "error": serde_json::to_value(error).unwrap_or(Value::Null),
    })
}

fn not_found_envelope_json(operation: &str) -> Value {
    let error = CallError::not_found(operation);
    json!({
        "request_id": Value::Null,
        "result": "error",
        "error": serde_json::to_value(&error).unwrap_or(Value::Null),
    })
}

fn not_found_response(operation: &str) -> Response {
    let error = CallError::not_found(operation);
    call_error_to_http_response(&error)
}

fn forbidden_response(message: String, identity: Option<&Identity>) -> Response {
    let error = CallError::forbidden(message);
    let status_code = call_error_to_http_status_with_identity(&error, identity);
    let status = StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let body = serde_json::to_value(&error).unwrap_or(Value::Null);
    (status, Json(body)).into_response()
}

fn access_check_for_op(
    registry: &OperationRegistry,
    operation: &str,
    identity: Option<&Identity>,
) -> Option<String> {
    let name = operation.strip_prefix('/').unwrap_or(operation);
    let reg = registry.registration(name)?;
    if let AccessResult::Forbidden(message) = reg.spec.access_control.check(identity) {
        return Some(message);
    }
    None
}

fn is_internal_op(registry: &OperationRegistry, operation: &str) -> bool {
    let name = operation.strip_prefix('/').unwrap_or(operation);
    match registry.registration(name) {
        Some(reg) => reg.spec.visibility == Visibility::Internal,
        None => false,
    }
}

fn envelope_to_sse_stream(
    envelope: ResponseEnvelope,
) -> impl Stream<Item = Result<Event, Infallible>> {
    stream::once(async move {
        match envelope.result {
            Ok(output) => {
                let data = serde_json::to_string(&output).unwrap_or_else(|_| "null".to_string());
                Ok(Event::default().data(data))
            }
            Err(error) => {
                let payload = serde_json::to_value(&error).unwrap_or(Value::Null);
                let data = serde_json::to_string(&payload).unwrap_or_else(|_| "null".to_string());
                Ok(Event::default().event("error").data(data))
            }
        }
    })
}

fn error_event(operation: &str) -> Result<Event, Infallible> {
    let error = CallError::not_found(operation);
    let payload = serde_json::to_value(&error).unwrap_or(Value::Null);
    let data = serde_json::to_string(&payload).unwrap_or_else(|_| "null".to_string());
    Ok(Event::default().event("error").data(data))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alknet_call::registry::discovery::{
        services_list_handler, services_list_spec, services_schema_handler, services_schema_spec,
    };
    use alknet_call::registry::registration::{
        make_handler, HandlerRegistration, OperationProvenance,
    };
    use alknet_call::registry::spec::{AccessControl, OperationSpec, OperationType};
    use alknet_core::auth::{AuthToken, Identity};
    use alknet_core::types::Capabilities;
    use axum::body::Body;
    use axum::http::Request;
    use axum::middleware::from_fn_with_state;
    use http_body_util::BodyExt;
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;
    use tower::ServiceExt;

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

    fn external_spec(name: &str, acl: AccessControl) -> OperationSpec {
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

    fn internal_spec(name: &str) -> OperationSpec {
        OperationSpec::new(
            name,
            OperationType::Query,
            Visibility::Internal,
            json!({}),
            json!({}),
            vec![],
            AccessControl::default(),
        )
    }

    fn echo_handler() -> alknet_call::registry::registration::Handler {
        make_handler(|input, ctx| async move { ResponseEnvelope::ok(ctx.request_id, input) })
    }

    fn registry_with_echo() -> Arc<OperationRegistry> {
        let mut registry = OperationRegistry::new();
        registry.register(HandlerRegistration::new(
            external_spec("echo/run", AccessControl::default()),
            echo_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        Arc::new(registry)
    }

    fn registry_with_restricted_op() -> Arc<OperationRegistry> {
        let mut registry = OperationRegistry::new();
        registry.register(HandlerRegistration::new(
            external_spec(
                "admin/run",
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
        Arc::new(registry)
    }

    fn registry_with_internal_op() -> Arc<OperationRegistry> {
        let mut registry = OperationRegistry::new();
        registry.register(HandlerRegistration::new(
            internal_spec("secret/op"),
            echo_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        Arc::new(registry)
    }

    fn registry_with_discovery_and_ops(
        inner_ops: Vec<HandlerRegistration>,
    ) -> Arc<OperationRegistry> {
        let mut inner = OperationRegistry::new();
        for op in inner_ops {
            inner.register(op);
        }
        let inner = Arc::new(inner);
        let mut registry = OperationRegistry::new();
        registry.register(HandlerRegistration::new(
            services_list_spec(),
            services_list_handler(Arc::clone(&inner)),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        registry.register(HandlerRegistration::new(
            services_schema_spec(),
            services_schema_handler(Arc::clone(&inner)),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        for spec in inner.list_operations() {
            let name = spec.name.clone();
            let reg = inner.registration(&name).unwrap();
            registry.register(HandlerRegistration::new(
                reg.spec.clone(),
                Arc::clone(&reg.handler),
                reg.provenance,
                reg.composition_authority.clone(),
                reg.scoped_env.clone(),
                reg.capabilities.clone(),
            ));
        }
        Arc::new(registry)
    }

    fn unused_provider() -> Arc<dyn IdentityProvider> {
        Arc::new(StaticIdentityProvider::new())
    }

    fn build_router(
        registry: Arc<OperationRegistry>,
        provider: Arc<dyn IdentityProvider>,
    ) -> Router {
        let state = RouterState {
            registry: Arc::clone(&registry),
            identity_provider: Arc::clone(&provider),
            decoy: crate::server::DecoyConfig::NotFound,
        };
        let auth_state = Arc::clone(&provider);
        gateway_router()
            .route_layer(from_fn_with_state(
                auth_state,
                super::super::auth::bearer_auth_middleware,
            ))
            .with_state(state)
    }

    fn auth_header(token: &str) -> (&'static str, String) {
        ("authorization", format!("Bearer {token}"))
    }

    async fn send(router: Router, req: Request<Body>) -> (StatusCode, Value) {
        let resp = router.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, body)
    }

    #[tokio::test]
    async fn call_round_trip_external_op_returns_200_with_json_body() {
        let router = build_router(registry_with_echo(), unused_provider());
        let req = Request::builder()
            .method("POST")
            .uri("/call")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({ "operation": "echo/run", "input": { "msg": "hi" } }))
                    .unwrap(),
            ))
            .unwrap();
        let (status, body) = send(router, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.get("result"), Some(&json!("ok")));
        assert_eq!(body.get("output"), Some(&json!({ "msg": "hi" })));
    }

    #[tokio::test]
    async fn call_internal_op_returns_404() {
        let router = build_router(registry_with_internal_op(), unused_provider());
        let req = Request::builder()
            .method("POST")
            .uri("/call")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({ "operation": "secret/op", "input": {} })).unwrap(),
            ))
            .unwrap();
        let (status, body) = send(router, req).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body.get("code"), Some(&json!("NOT_FOUND")));
    }

    #[tokio::test]
    async fn call_unauthorized_restricted_op_returns_403() {
        let provider: Arc<dyn IdentityProvider> = Arc::new(
            StaticIdentityProvider::new()
                .with_token("user-tok", identity_with_scopes("user", &["user"])),
        );
        let router = build_router(registry_with_restricted_op(), provider);
        let (k, v) = auth_header("user-tok");
        let req = Request::builder()
            .method("POST")
            .uri("/call")
            .header("content-type", "application/json")
            .header(k, v)
            .body(Body::from(
                serde_json::to_vec(&json!({ "operation": "admin/run", "input": {} })).unwrap(),
            ))
            .unwrap();
        let (status, _body) = send(router, req).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn call_unauthenticated_restricted_op_returns_401() {
        let router = build_router(registry_with_restricted_op(), unused_provider());
        let req = Request::builder()
            .method("POST")
            .uri("/call")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({ "operation": "admin/run", "input": {} })).unwrap(),
            ))
            .unwrap();
        let (status, _body) = send(router, req).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn search_returns_only_access_control_allowed_ops() {
        let ops = vec![
            HandlerRegistration::new(
                external_spec("public/echo", AccessControl::default()),
                echo_handler(),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ),
            HandlerRegistration::new(
                external_spec(
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
            ),
        ];
        let discovery = registry_with_discovery_and_ops(ops);
        let provider: Arc<dyn IdentityProvider> = Arc::new(
            StaticIdentityProvider::new()
                .with_token("user-tok", identity_with_scopes("regular", &["user"])),
        );
        let router = build_router(discovery, provider);
        let (k, v) = auth_header("user-tok");
        let req = Request::builder()
            .method("GET")
            .uri("/search")
            .header(k, v)
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(router, req).await;
        assert_eq!(status, StatusCode::OK);
        let ops = body
            .get("output")
            .and_then(|o| o.get("operations"))
            .and_then(|o| o.as_array())
            .expect("operations array");
        let names: Vec<&str> = ops
            .iter()
            .filter_map(|o| o.get("name").and_then(|n| n.as_str()))
            .collect();
        assert!(names.contains(&"public/echo"));
        assert!(!names.contains(&"admin/secret"));
    }

    #[tokio::test]
    async fn schema_returns_full_spec_for_authorized_op() {
        let ops = vec![HandlerRegistration::new(
            external_spec("echo/run", AccessControl::default()),
            echo_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        )];
        let discovery = registry_with_discovery_and_ops(ops);
        let router = build_router(discovery, unused_provider());
        let req = Request::builder()
            .method("GET")
            .uri("/schema?name=echo%2Frun")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(router, req).await;
        assert_eq!(status, StatusCode::OK);
        let output = body.get("output").expect("output");
        assert_eq!(output.get("name"), Some(&json!("echo/run")));
        assert_eq!(output.get("namespace"), Some(&json!("echo")));
        assert!(output.get("input_schema").is_some());
        assert!(output.get("output_schema").is_some());
    }

    #[tokio::test]
    async fn schema_for_unauthorized_op_returns_403() {
        let ops = vec![HandlerRegistration::new(
            external_spec(
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
        )];
        let discovery = registry_with_discovery_and_ops(ops);
        let provider: Arc<dyn IdentityProvider> = Arc::new(
            StaticIdentityProvider::new()
                .with_token("user-tok", identity_with_scopes("regular", &["user"])),
        );
        let router = build_router(discovery, provider);
        let (k, v) = auth_header("user-tok");
        let req = Request::builder()
            .method("GET")
            .uri("/schema?name=admin%2Fsecret")
            .header(k, v)
            .body(Body::empty())
            .unwrap();
        let (status, _body) = send(router, req).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn batch_returns_array_of_results_in_order() {
        let router = build_router(registry_with_echo(), unused_provider());
        let req = Request::builder()
            .method("POST")
            .uri("/batch")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!([
                    { "operation": "echo/run", "input": { "n": 1 } },
                    { "operation": "echo/run", "input": { "n": 2 } },
                ]))
                .unwrap(),
            ))
            .unwrap();
        let (status, body) = send(router, req).await;
        assert_eq!(status, StatusCode::OK);
        let results = body
            .get("results")
            .and_then(|r| r.as_array())
            .expect("results array");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].get("output"), Some(&json!({ "n": 1 })));
        assert_eq!(results[1].get("output"), Some(&json!({ "n": 2 })));
    }

    #[tokio::test]
    async fn batch_internal_op_returns_not_found_in_array() {
        let mut registry = OperationRegistry::new();
        registry.register(HandlerRegistration::new(
            internal_spec("secret/op"),
            echo_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        registry.register(HandlerRegistration::new(
            external_spec("echo/run", AccessControl::default()),
            echo_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        let registry = Arc::new(registry);
        let router = build_router(registry, unused_provider());
        let req = Request::builder()
            .method("POST")
            .uri("/batch")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!([
                    { "operation": "echo/run", "input": {} },
                    { "operation": "secret/op", "input": {} },
                ]))
                .unwrap(),
            ))
            .unwrap();
        let (status, body) = send(router, req).await;
        assert_eq!(status, StatusCode::OK);
        let results = body
            .get("results")
            .and_then(|r| r.as_array())
            .expect("results array");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].get("result"), Some(&json!("ok")));
        assert_eq!(results[1].get("result"), Some(&json!("error")));
        assert_eq!(
            results[1].get("error").and_then(|e| e.get("code")),
            Some(&json!("NOT_FOUND"))
        );
    }

    #[tokio::test]
    async fn subscribe_streams_sse_data_event_until_completed() {
        let router = build_router(registry_with_echo(), unused_provider());
        let req = Request::builder()
            .method("POST")
            .uri("/subscribe")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({ "operation": "echo/run", "input": { "v": 9 } }))
                    .unwrap(),
            ))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ctype = resp
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .map(|v| v.to_str().unwrap().to_string());
        assert!(
            ctype
                .as_deref()
                .unwrap_or("")
                .starts_with("text/event-stream"),
            "expected text/event-stream, got {ctype:?}"
        );
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&bytes);
        assert!(body.contains("data:"), "expected a data frame, got: {body}");
        assert!(
            body.contains("\"v\":9"),
            "expected output payload, got: {body}"
        );
    }

    #[tokio::test]
    async fn subscribe_internal_op_emits_error_event() {
        let router = build_router(registry_with_internal_op(), unused_provider());
        let req = Request::builder()
            .method("POST")
            .uri("/subscribe")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({ "operation": "secret/op", "input": {} })).unwrap(),
            ))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&bytes);
        assert!(
            body.contains("event:error") || body.contains("event: error"),
            "expected error event, got: {body}"
        );
        assert!(
            body.contains("NOT_FOUND"),
            "expected NOT_FOUND, got: {body}"
        );
    }

    #[test]
    fn is_internal_op_returns_false_for_unknown() {
        let registry = OperationRegistry::new();
        assert!(!is_internal_op(&registry, "no/such"));
        assert!(!is_internal_op(&registry, "/no/such"));
    }

    #[test]
    fn is_internal_op_detects_registered_internal_op() {
        let mut registry = OperationRegistry::new();
        registry.register(HandlerRegistration::new(
            internal_spec("secret/op"),
            echo_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        assert!(is_internal_op(&registry, "secret/op"));
        assert!(is_internal_op(&registry, "/secret/op"));
    }

    #[test]
    fn is_internal_op_false_for_external_op() {
        let mut registry = OperationRegistry::new();
        registry.register(HandlerRegistration::new(
            external_spec("echo/run", AccessControl::default()),
            echo_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        assert!(!is_internal_op(&registry, "echo/run"));
    }

    #[test]
    fn envelope_to_ok_json_shape() {
        let env = ResponseEnvelope::ok("req-1", json!({ "v": 1 }));
        let v = envelope_to_json(env);
        assert_eq!(v.get("request_id"), Some(&json!("req-1")));
        assert_eq!(v.get("result"), Some(&json!("ok")));
        assert_eq!(v.get("output"), Some(&json!({ "v": 1 })));
    }

    #[test]
    fn envelope_to_error_json_shape() {
        let env = ResponseEnvelope::not_found("req-2", "no/such");
        let v = envelope_to_json(env);
        assert_eq!(v.get("result"), Some(&json!("error")));
        assert_eq!(
            v.get("error").and_then(|e| e.get("code")),
            Some(&json!("NOT_FOUND"))
        );
    }

    #[tokio::test]
    async fn call_with_leading_slash_in_operation_dispatches() {
        let router = build_router(registry_with_echo(), unused_provider());
        let req = Request::builder()
            .method("POST")
            .uri("/call")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({ "operation": "/echo/run", "input": {} })).unwrap(),
            ))
            .unwrap();
        let (status, body) = send(router, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.get("result"), Some(&json!("ok")));
    }

    #[tokio::test]
    async fn call_unknown_op_returns_404() {
        let router = build_router(registry_with_echo(), unused_provider());
        let req = Request::builder()
            .method("POST")
            .uri("/call")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({ "operation": "no/such", "input": {} })).unwrap(),
            ))
            .unwrap();
        let (status, body) = send(router, req).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body.get("code"), Some(&json!("NOT_FOUND")));
    }

    #[tokio::test]
    async fn search_unauthenticated_lists_default_acl_ops_only() {
        let ops = vec![
            HandlerRegistration::new(
                external_spec("public/echo", AccessControl::default()),
                echo_handler(),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ),
            HandlerRegistration::new(
                external_spec(
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
            ),
        ];
        let discovery = registry_with_discovery_and_ops(ops);
        let router = build_router(discovery, unused_provider());
        let req = Request::builder()
            .method("GET")
            .uri("/search")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(router, req).await;
        assert_eq!(status, StatusCode::OK);
        let ops = body
            .get("output")
            .and_then(|o| o.get("operations"))
            .and_then(|o| o.as_array())
            .expect("operations array");
        let names: Vec<&str> = ops
            .iter()
            .filter_map(|o| o.get("name").and_then(|n| n.as_str()))
            .collect();
        assert!(names.contains(&"public/echo"));
        assert!(!names.contains(&"admin/secret"));
    }

    #[tokio::test]
    async fn schema_unknown_op_returns_404() {
        let ops = vec![HandlerRegistration::new(
            external_spec("echo/run", AccessControl::default()),
            echo_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        )];
        let discovery = registry_with_discovery_and_ops(ops);
        let router = build_router(discovery, unused_provider());
        let req = Request::builder()
            .method("GET")
            .uri("/schema?name=no%2Fsuch")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(router, req).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body.get("code"), Some(&json!("NOT_FOUND")));
    }
}
