//! WebSocket upgrade handler — the v1 browser bidirectional path (ADR-044).
//!
//! Carries the native `EventEnvelope` call-protocol session over WebSocket,
//! not the HTTP gateway shape (ADR-048). Bearer auth on the upgrade request
//! via the shared `bearer_auth_middleware` (same auth path as any HTTP
//! request). No token → `401`. The resolved identity is stored on the
//! `CallConnection` for observability + `AccessControl`.
//!
//! Framing: one `EventEnvelope` JSON object = one binary WS message, no
//! length prefix (ADR-044 Assumption 1 — the WS message boundary is the
//! delimiter, unlike QUIC's 4-byte prefix). Text WS messages are rejected
//! with a protocol-level close. The shared `Dispatcher` runs over the WS
//! message stream unchanged (ADR-012): `call.requested` →
//! `Dispatcher::dispatch_requested` (with `AccessControl::check` gating),
//! `call.aborted` → `Dispatcher::handle_abort`, `call.responded`/
//! `call.completed` correlated by `id` via `PendingRequestMap`.
//!
//! See `docs/architecture/crates/http/websocket.md`.

use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use tracing::{debug, warn};

use alknet_call::protocol::connection::CallConnection;
use alknet_call::protocol::dispatch::Dispatcher;
use alknet_call::protocol::wire::{
    CallError, EventEnvelope, ResponseEnvelope, EVENT_ABORTED, EVENT_COMPLETED, EVENT_ERROR,
    EVENT_REQUESTED, EVENT_RESPONDED,
};
use alknet_call::registry::registration::OperationRegistry;
use alknet_core::auth::{Identity, IdentityProvider};

use crate::server::ResolvedIdentity;

pub const WS_UPGRADE_PATH: &str = "/alknet/call";

const WS_CLOSE_PROTOCOL_ERROR: u16 = 1002;

#[async_trait]
trait WsStream: Send + 'static {
    async fn recv(&mut self) -> Option<Result<Message, axum::Error>>;
    async fn send(&mut self, msg: Message) -> Result<(), axum::Error>;
    async fn close(&mut self);
}

#[async_trait]
impl WsStream for WebSocket {
    async fn recv(&mut self) -> Option<Result<Message, axum::Error>> {
        WebSocket::recv(self).await
    }

    async fn send(&mut self, msg: Message) -> Result<(), axum::Error> {
        WebSocket::send(self, msg).await
    }

    async fn close(&mut self) {
        let _ = WebSocket::close(self).await;
    }
}

pub async fn ws_upgrade_handler(
    State(registry): State<Arc<OperationRegistry>>,
    State(identity_provider): State<Arc<dyn IdentityProvider>>,
    ResolvedIdentity(identity): ResolvedIdentity,
    ws_upgrade: WebSocketUpgrade,
) -> Response {
    ws_upgrade_handler_inner(registry, identity_provider, identity, Some(ws_upgrade)).await
}

async fn ws_upgrade_handler_inner(
    registry: Arc<OperationRegistry>,
    identity_provider: Arc<dyn IdentityProvider>,
    identity: Option<Identity>,
    ws_upgrade: Option<WebSocketUpgrade>,
) -> Response {
    let identity = match identity {
        Some(id) => id,
        None => return (StatusCode::UNAUTHORIZED, "401 Unauthorized").into_response(),
    };

    match ws_upgrade {
        Some(upgrade) => upgrade.on_upgrade(move |socket| {
            run_ws_session(socket, registry, identity_provider, identity)
        }),
        None => {
            let _ = registry;
            let _ = identity_provider;
            (StatusCode::SWITCHING_PROTOCOLS, "").into_response()
        }
    }
}

async fn run_ws_session(
    socket: WebSocket,
    registry: Arc<OperationRegistry>,
    identity_provider: Arc<dyn IdentityProvider>,
    identity: Identity,
) {
    let connection = Arc::new(CallConnection::new_overlay_only(identity));
    let dispatcher = Dispatcher::new(registry, identity_provider);
    drive_ws_session(socket, &dispatcher, &connection).await;
}

async fn drive_ws_session<S: WsStream>(
    mut socket: S,
    dispatcher: &Dispatcher,
    connection: &Arc<CallConnection>,
) {
    loop {
        match socket.recv().await {
            Some(Ok(Message::Binary(bytes))) => {
                let envelope: EventEnvelope = match serde_json::from_slice(&bytes) {
                    Ok(env) => env,
                    Err(err) => {
                        warn!(error = %err, "ws binary message is not a valid EventEnvelope; closing");
                        let _ = socket
                            .send(Message::Close(Some(CloseFrame {
                                code: WS_CLOSE_PROTOCOL_ERROR,
                                reason: "invalid EventEnvelope".into(),
                            })))
                            .await;
                        break;
                    }
                };
                let response = handle_inbound_envelope(dispatcher, connection, envelope).await;
                if let Some(out_envelope) = response {
                    match serialize_envelope(&out_envelope) {
                        Ok(out_bytes) => {
                            if let Err(err) = socket.send(Message::Binary(out_bytes.into())).await {
                                warn!(error = %err, "ws write failed; closing session");
                                break;
                            }
                        }
                        Err(err) => {
                            warn!(error = %err, "failed to serialize outbound EventEnvelope");
                            break;
                        }
                    }
                }
            }
            Some(Ok(Message::Text(_))) => {
                warn!("ws text message received; protocol-level close");
                let _ = socket
                    .send(Message::Close(Some(CloseFrame {
                        code: WS_CLOSE_PROTOCOL_ERROR,
                        reason: "text messages not supported".into(),
                    })))
                    .await;
                break;
            }
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
            Some(Ok(Message::Close(_))) => break,
            Some(Err(err)) => {
                warn!(error = %err, "ws read error; closing session");
                break;
            }
            None => break,
        }
    }

    fail_all_pending(dispatcher, connection).await;
    socket.close().await;
}

async fn handle_inbound_envelope(
    dispatcher: &Dispatcher,
    connection: &Arc<CallConnection>,
    envelope: EventEnvelope,
) -> Option<EventEnvelope> {
    match envelope.r#type.as_str() {
        EVENT_REQUESTED => {
            let request_id = envelope.id.clone();
            let payload = envelope.payload.clone();
            let response = dispatcher
                .dispatch_requested(connection, request_id.clone(), payload)
                .await;
            Some(response_into_envelope(response))
        }
        EVENT_ABORTED => {
            dispatcher.handle_abort(connection, &envelope.id).await;
            None
        }
        EVENT_RESPONDED | EVENT_COMPLETED | EVENT_ERROR => {
            dispatch_envelope_to_pending(connection, &envelope);
            None
        }
        other => {
            debug!(event_type = %other, id = %envelope.id, "ignoring unknown event type on ws session");
            None
        }
    }
}

fn dispatch_envelope_to_pending(connection: &Arc<CallConnection>, envelope: &EventEnvelope) {
    let request_id = envelope.id.clone();
    let mut pending = connection.pending().lock();
    match envelope.r#type.as_str() {
        EVENT_RESPONDED => {
            let output = envelope
                .payload
                .get("output")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            pending.handle_responded(&request_id, output);
        }
        EVENT_COMPLETED => {
            pending.handle_completed(&request_id);
        }
        EVENT_ERROR => {
            if let Ok(error) = serde_json::from_value::<CallError>(envelope.payload.clone()) {
                pending.handle_error(&request_id, error);
            }
        }
        _ => {}
    }
}

async fn fail_all_pending(_dispatcher: &Dispatcher, connection: &Arc<CallConnection>) {
    let pending = Arc::clone(connection.pending());
    let failed = pending
        .lock()
        .fail_all(CallError::internal("connection closed"));
    if !failed.is_empty() {
        debug!(count = failed.len(), "failed pending requests on ws close");
    }
}

fn response_into_envelope(response: ResponseEnvelope) -> EventEnvelope {
    response.into_event()
}

fn serialize_envelope(envelope: &EventEnvelope) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(envelope)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alknet_call::registry::context::{
        AbortPolicy, CompositionAuthority, OperationContext, ScopedPeerEnv,
    };
    use alknet_call::registry::discovery::{
        services_list_handler, services_list_spec, services_schema_handler, services_schema_spec,
    };
    use alknet_call::registry::env::OperationEnv;
    use alknet_call::registry::registration::{
        make_handler, HandlerRegistration, OperationProvenance,
    };
    use alknet_call::registry::spec::{AccessControl, OperationSpec, OperationType, Visibility};
    use alknet_core::auth::{AuthToken, Identity};
    use alknet_core::types::Capabilities;
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;
    use std::time::{Duration, Instant};

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

    fn identity(id: &str) -> Identity {
        Identity {
            id: id.to_string(),
            scopes: vec![],
            resources: HashMap::new(),
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
            serde_json::json!({}),
            serde_json::json!({}),
            vec![],
            acl,
        )
    }

    fn subscription_spec(name: &str) -> OperationSpec {
        OperationSpec::new(
            name,
            OperationType::Subscription,
            Visibility::External,
            serde_json::json!({}),
            serde_json::json!({}),
            vec![],
            AccessControl::default(),
        )
    }

    fn echo_registry() -> Arc<OperationRegistry> {
        let mut registry = OperationRegistry::new();
        registry.register(HandlerRegistration::new(
            external_spec("echo/run", AccessControl::default()),
            make_handler(|input, ctx| async move { ResponseEnvelope::ok(ctx.request_id, input) }),
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
            make_handler(|input, ctx| async move { ResponseEnvelope::ok(ctx.request_id, input) }),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        Arc::new(registry)
    }

    fn registry_with_subscription() -> Arc<OperationRegistry> {
        let mut registry = OperationRegistry::new();
        let count = Arc::new(StdMutex::new(0u32));
        let handler = make_handler(move |_input, ctx| {
            let counter = Arc::clone(&count);
            async move {
                let mut c = counter.lock().unwrap();
                *c += 1;
                let value = *c;
                ResponseEnvelope::ok(ctx.request_id, serde_json::json!({ "n": value }))
            }
        });
        registry.register(HandlerRegistration::new(
            subscription_spec("events/stream"),
            handler,
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        Arc::new(registry)
    }

    fn registry_with_discovery(inner: Arc<OperationRegistry>) -> Arc<OperationRegistry> {
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
        Arc::new(registry)
    }

    fn dispatcher(
        registry: Arc<OperationRegistry>,
        provider: Arc<dyn IdentityProvider>,
    ) -> Dispatcher {
        Dispatcher::new(registry, provider)
    }

    #[test]
    fn upgrade_path_is_alknet_call() {
        assert_eq!(WS_UPGRADE_PATH, "/alknet/call");
    }

    #[test]
    fn upgrade_path_namespaces_away_from_reserved_paths() {
        assert_ne!(WS_UPGRADE_PATH, "/healthz");
        assert_ne!(WS_UPGRADE_PATH, "/openapi.json");
        assert_ne!(WS_UPGRADE_PATH, "/mcp");
        assert_ne!(WS_UPGRADE_PATH, "/search");
        assert_ne!(WS_UPGRADE_PATH, "/schema");
        assert_ne!(WS_UPGRADE_PATH, "/call");
        assert_ne!(WS_UPGRADE_PATH, "/batch");
        assert_ne!(WS_UPGRADE_PATH, "/subscribe");
    }

    #[test]
    fn response_into_envelope_maps_ok_to_responded() {
        let response = ResponseEnvelope::ok("req-1", serde_json::json!({ "v": 1 }));
        let envelope = response_into_envelope(response);
        assert_eq!(envelope.r#type, EVENT_RESPONDED);
        assert_eq!(envelope.id, "req-1");
    }

    #[test]
    fn response_into_envelope_maps_error_to_call_error() {
        let response = ResponseEnvelope::forbidden("req-2", "no scopes");
        let envelope = response_into_envelope(response);
        assert_eq!(envelope.r#type, EVENT_ERROR);
        assert_eq!(envelope.id, "req-2");
        assert_eq!(
            envelope.payload.get("code"),
            Some(&serde_json::json!("FORBIDDEN"))
        );
    }

    #[test]
    fn serialize_envelope_round_trips() {
        let envelope = EventEnvelope::requested(
            "req-x",
            serde_json::json!({ "operationId": "/echo/run", "input": { "v": 7 } }),
        );
        let bytes = serialize_envelope(&envelope).unwrap();
        let parsed: EventEnvelope = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed, envelope);
    }

    #[test]
    fn serialize_envelope_produces_no_length_prefix() {
        let envelope = EventEnvelope::responded("req-1", serde_json::json!({ "v": 1 }));
        let bytes = serialize_envelope(&envelope).unwrap();
        assert!(
            serde_json::from_slice::<EventEnvelope>(&bytes).is_ok(),
            "ws path emits a raw JSON EventEnvelope, no length prefix"
        );
    }

    #[tokio::test]
    async fn dispatch_requested_via_pub_api_returns_response_envelope() {
        let registry = echo_registry();
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatcher(registry, provider);
        let conn = Arc::new(CallConnection::new_overlay_only(identity("ws-peer")));

        let payload = serde_json::json!({
            "operationId": "/echo/run",
            "input": { "msg": "hi" },
        });
        let response = dp
            .dispatch_requested(&conn, "ws-1".to_string(), payload)
            .await;
        assert!(response.result.is_ok());
        assert_eq!(response.result.unwrap(), serde_json::json!({ "msg": "hi" }));
    }

    #[tokio::test]
    async fn handle_inbound_envelope_requested_writes_call_responded() {
        let registry = echo_registry();
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatcher(registry, provider);
        let conn = Arc::new(CallConnection::new_overlay_only(identity("ws-peer")));

        let envelope = EventEnvelope::new(
            EVENT_REQUESTED,
            "req-rt-1",
            serde_json::json!({
                "operationId": "/echo/run",
                "input": { "v": 7 },
            }),
        );
        let out = handle_inbound_envelope(&dp, &conn, envelope)
            .await
            .expect("response envelope");
        assert_eq!(out.r#type, EVENT_RESPONDED);
        assert_eq!(out.id, "req-rt-1");
        assert_eq!(
            out.payload.get("output"),
            Some(&serde_json::json!({ "v": 7 }))
        );
    }

    #[tokio::test]
    async fn handle_inbound_envelope_forbidden_yields_call_error() {
        let registry = registry_with_restricted_op();
        let provider: Arc<dyn IdentityProvider> =
            Arc::new(StaticIdentityProvider::new().with_token("none", identity("unpriv")));
        let dp = dispatcher(registry, provider);
        let conn = Arc::new(CallConnection::new_overlay_only(identity("unpriv")));

        let envelope = EventEnvelope::new(
            EVENT_REQUESTED,
            "req-forbid-1",
            serde_json::json!({
                "operationId": "/admin/run",
                "input": {},
            }),
        );
        let out = handle_inbound_envelope(&dp, &conn, envelope)
            .await
            .expect("error envelope");
        assert_eq!(out.r#type, EVENT_ERROR);
        assert_eq!(out.id, "req-forbid-1");
        assert_eq!(
            out.payload.get("code"),
            Some(&serde_json::json!("FORBIDDEN"))
        );
    }

    #[tokio::test]
    async fn handle_inbound_envelope_internal_op_yields_not_found() {
        let mut registry = OperationRegistry::new();
        registry.register(HandlerRegistration::new(
            OperationSpec::new(
                "secret/op",
                OperationType::Query,
                Visibility::Internal,
                serde_json::json!({}),
                serde_json::json!({}),
                vec![],
                AccessControl::default(),
            ),
            make_handler(|input, ctx| async move { ResponseEnvelope::ok(ctx.request_id, input) }),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        let registry = Arc::new(registry);
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatcher(registry, provider);
        let conn = Arc::new(CallConnection::new_overlay_only(identity("ws-peer")));

        let envelope = EventEnvelope::new(
            EVENT_REQUESTED,
            "req-secret",
            serde_json::json!({ "operationId": "/secret/op", "input": {} }),
        );
        let out = handle_inbound_envelope(&dp, &conn, envelope)
            .await
            .expect("error envelope");
        assert_eq!(out.r#type, EVENT_ERROR);
        assert_eq!(out.id, "req-secret");
        assert_eq!(
            out.payload.get("code"),
            Some(&serde_json::json!("NOT_FOUND"))
        );
    }

    #[tokio::test]
    async fn handle_inbound_envelope_aborted_invokes_handle_abort_cascade() {
        let registry = echo_registry();
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatcher(registry, provider);
        let conn = Arc::new(CallConnection::new_overlay_only(identity("ws-peer")));

        {
            let mut pending = conn.pending().lock();
            pending.register_call(
                "ws-parent".to_string(),
                Instant::now() + Duration::from_secs(30),
                None,
            );
            pending.register_call(
                "ws-child".to_string(),
                Instant::now() + Duration::from_secs(30),
                Some("ws-parent".to_string()),
            );
        }

        let envelope = EventEnvelope::new(EVENT_ABORTED, "ws-parent", serde_json::json!({}));
        let out = handle_inbound_envelope(&dp, &conn, envelope).await;
        assert!(out.is_none(), "call.aborted produces no outbound envelope");

        assert!(!conn.pending().lock().contains("ws-parent"));
        assert!(!conn.pending().lock().contains("ws-child"));
    }

    #[tokio::test]
    async fn handle_inbound_envelope_responded_correlates_via_pending_map() {
        let registry = echo_registry();
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatcher(registry, provider);
        let conn = Arc::new(CallConnection::new_overlay_only(identity("ws-peer")));

        let rx = {
            let mut pending = conn.pending().lock();
            pending.register_call(
                "ws-rt-resp".to_string(),
                Instant::now() + Duration::from_secs(30),
                None,
            )
        };

        let envelope = EventEnvelope::responded("ws-rt-resp", serde_json::json!({ "v": 99 }));
        let out = handle_inbound_envelope(&dp, &conn, envelope).await;
        assert!(out.is_none());

        let result = tokio::time::timeout(Duration::from_millis(100), rx).await;
        match result {
            Ok(Ok(Ok(value))) => assert_eq!(value, serde_json::json!({ "v": 99 })),
            other => panic!("expected Ok value, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_inbound_envelope_completed_removes_pending_entry() {
        let registry = echo_registry();
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatcher(registry, provider);
        let conn = Arc::new(CallConnection::new_overlay_only(identity("ws-peer")));

        {
            let mut pending = conn.pending().lock();
            pending.register_subscribe("ws-sub-1".to_string(), None, None);
        }
        assert!(conn.pending().lock().contains("ws-sub-1"));

        let envelope = EventEnvelope::completed("ws-sub-1");
        handle_inbound_envelope(&dp, &conn, envelope).await;
        assert!(!conn.pending().lock().contains("ws-sub-1"));
    }

    #[tokio::test]
    async fn handle_inbound_envelope_unknown_event_returns_none() {
        let registry = echo_registry();
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatcher(registry, provider);
        let conn = Arc::new(CallConnection::new_overlay_only(identity("ws-peer")));

        let envelope = EventEnvelope::new("call.mystery", "ws-unknown", serde_json::json!({}));
        let out = handle_inbound_envelope(&dp, &conn, envelope).await;
        assert!(out.is_none());
    }

    #[tokio::test]
    async fn fail_all_pending_aborts_all_pending_on_disconnect() {
        let registry = echo_registry();
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatcher(registry, provider);
        let conn = Arc::new(CallConnection::new_overlay_only(identity("ws-peer")));

        {
            let mut pending = conn.pending().lock();
            pending.register_call(
                "ws-a".to_string(),
                Instant::now() + Duration::from_secs(30),
                None,
            );
            pending.register_call(
                "ws-b".to_string(),
                Instant::now() + Duration::from_secs(30),
                None,
            );
        }

        fail_all_pending(&dp, &conn).await;
        assert!(
            conn.pending().lock().is_empty(),
            "all pending failed on disconnect"
        );
    }

    #[tokio::test]
    async fn fail_all_pending_aborts_subscription_and_cascades_descendants() {
        let registry = echo_registry();
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatcher(registry, provider);
        let conn = Arc::new(CallConnection::new_overlay_only(identity("ws-peer")));

        {
            let mut pending = conn.pending().lock();
            pending.register_call(
                "ws-parent".to_string(),
                Instant::now() + Duration::from_secs(30),
                None,
            );
            pending.register_call(
                "ws-child".to_string(),
                Instant::now() + Duration::from_secs(30),
                Some("ws-parent".to_string()),
            );
        }

        fail_all_pending(&dp, &conn).await;
        assert!(!conn.pending().lock().contains("ws-parent"));
        assert!(!conn.pending().lock().contains("ws-child"));
    }

    #[tokio::test]
    async fn round_trip_call_requested_to_call_responded_over_ws_message_stream() {
        let registry = echo_registry();
        let provider: Arc<dyn IdentityProvider> =
            Arc::new(StaticIdentityProvider::new().with_token("ws-token", identity("ws-peer")));
        let dp = dispatcher(Arc::clone(&registry), Arc::clone(&provider));
        let conn = Arc::new(CallConnection::new_overlay_only(identity("ws-peer")));

        let request = EventEnvelope::requested(
            "rt-1",
            serde_json::json!({ "operationId": "/echo/run", "input": { "v": 42 } }),
        );
        let out = handle_inbound_envelope(&dp, &conn, request)
            .await
            .expect("response");
        let out_bytes = serialize_envelope(&out).unwrap();
        let parsed: EventEnvelope = serde_json::from_slice(&out_bytes).unwrap();
        assert_eq!(parsed.r#type, EVENT_RESPONDED);
        assert_eq!(parsed.id, "rt-1");
        assert_eq!(
            parsed.payload.get("output"),
            Some(&serde_json::json!({ "v": 42 }))
        );
    }

    #[tokio::test]
    async fn subscription_streams_multiple_call_responded_events() {
        let registry = registry_with_subscription();
        let provider: Arc<dyn IdentityProvider> =
            Arc::new(StaticIdentityProvider::new().with_token("ws-token", identity("ws-peer")));
        let dp = dispatcher(registry, provider);
        let conn = Arc::new(CallConnection::new_overlay_only(identity("ws-peer")));

        let mut received = Vec::new();
        for i in 0..3 {
            let request = EventEnvelope::requested(
                format!("sub-{i}"),
                serde_json::json!({ "operationId": "/events/stream", "input": {} }),
            );
            let out = handle_inbound_envelope(&dp, &conn, request)
                .await
                .expect("response");
            assert_eq!(out.r#type, EVENT_RESPONDED);
            received.push(out.id);
        }
        assert_eq!(received.len(), 3);
    }

    #[tokio::test]
    async fn access_control_denied_returns_forbidden_before_handler() {
        let registry = registry_with_restricted_op();
        let provider: Arc<dyn IdentityProvider> = Arc::new(
            StaticIdentityProvider::new()
                .with_token("no-admin", identity_with_scopes("user", &["user"])),
        );
        let dp = dispatcher(registry, provider);
        let conn = Arc::new(CallConnection::new_overlay_only(identity_with_scopes(
            "user",
            &["user"],
        )));

        let request = EventEnvelope::requested(
            "req-admin",
            serde_json::json!({ "operationId": "/admin/run", "input": {} }),
        );
        let out = handle_inbound_envelope(&dp, &conn, request)
            .await
            .expect("error envelope");
        assert_eq!(out.r#type, EVENT_ERROR);
        assert_eq!(
            out.payload.get("code"),
            Some(&serde_json::json!("FORBIDDEN"))
        );
    }

    #[tokio::test]
    async fn services_list_dispatches_as_call_protocol_op() {
        let inner = echo_registry();
        let registry = registry_with_discovery(Arc::clone(&inner));
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatcher(registry, provider);
        let conn = Arc::new(CallConnection::new_overlay_only(identity("ws-peer")));

        let request = EventEnvelope::requested(
            "ws-list",
            serde_json::json!({ "operationId": "/services/list", "input": {} }),
        );
        let out = handle_inbound_envelope(&dp, &conn, request)
            .await
            .expect("list response");
        assert_eq!(out.r#type, EVENT_RESPONDED);
        assert_eq!(out.id, "ws-list");
        let ops = out
            .payload
            .get("output")
            .and_then(|v| v.get("operations"))
            .and_then(|v| v.as_array())
            .expect("operations array");
        let names: Vec<&str> = ops
            .iter()
            .filter_map(|o| o.get("name").and_then(|n| n.as_str()))
            .collect();
        assert!(names.contains(&"echo/run"));
    }

    #[tokio::test]
    async fn authorized_identity_dispatches_restricted_op() {
        let registry = registry_with_restricted_op();
        let provider: Arc<dyn IdentityProvider> = Arc::new(
            StaticIdentityProvider::new()
                .with_token("alk_admin", identity_with_scopes("admin-peer", &["admin"])),
        );
        let dp = dispatcher(registry, provider);
        let conn = Arc::new(CallConnection::new_overlay_only(identity_with_scopes(
            "admin-peer",
            &["admin"],
        )));

        let request = EventEnvelope::requested(
            "req-admin-ok",
            serde_json::json!({ "operationId": "/admin/run", "input": { "ok": 1 } }),
        );
        let out = handle_inbound_envelope(&dp, &conn, request)
            .await
            .expect("response");
        assert_eq!(out.r#type, EVENT_RESPONDED);
        assert_eq!(out.id, "req-admin-ok");
        assert_eq!(
            out.payload.get("output"),
            Some(&serde_json::json!({ "ok": 1 }))
        );
    }

    #[tokio::test]
    async fn connection_holds_resolved_identity_for_access_control() {
        let conn = CallConnection::new_overlay_only(identity("ws-peer"));
        assert_eq!(conn.identity().unwrap().id, "ws-peer");
    }

    #[tokio::test]
    async fn bidirectional_overlay_allows_hub_to_call_browser_registered_op() {
        let registry = echo_registry();
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatcher(Arc::clone(&registry), Arc::clone(&provider));
        let conn = Arc::new(CallConnection::new_overlay_only(identity("browser")));

        conn.register_imported(HandlerRegistration::new(
            external_spec("ui/dragged", AccessControl::default()),
            make_handler(|input, ctx| async move { ResponseEnvelope::ok(ctx.request_id, input) }),
            OperationProvenance::FromCall,
            None,
            None,
            Capabilities::new(),
        ));

        let overlay_env = conn.overlay_env();
        assert!(overlay_env.contains("ui/dragged"));

        let composed_env: Arc<dyn OperationEnv + Send + Sync> = dp.compose_root_env(
            &conn,
            &root_context_for_compose("hub-call-1", overlay_env.clone()),
        );
        let ctx = root_context_with_env("hub-call-1", composed_env);
        let response = overlay_env
            .invoke("ui", "dragged", serde_json::json!({ "x": 5 }), &ctx)
            .await;
        assert!(response.result.is_ok());
        assert_eq!(response.result.unwrap(), serde_json::json!({ "x": 5 }));
    }

    fn root_context_for_compose(
        request_id: &str,
        env: Arc<dyn alknet_call::registry::env::OperationEnv + Send + Sync>,
    ) -> OperationContext {
        OperationContext {
            request_id: request_id.to_string(),
            parent_request_id: None,
            identity: None,
            handler_identity: Some(CompositionAuthority::new("hub", vec![])),
            forwarded_for: None,
            capabilities: Capabilities::new(),
            metadata: HashMap::new(),
            scoped_env: ScopedPeerEnv::new(["ui/dragged"]),
            env,
            abort_policy: AbortPolicy::default(),
            deadline: Some(Instant::now() + Duration::from_secs(30)),
            internal: false,
        }
    }

    fn root_context_with_env(
        request_id: &str,
        env: Arc<dyn alknet_call::registry::env::OperationEnv + Send + Sync>,
    ) -> OperationContext {
        OperationContext {
            request_id: request_id.to_string(),
            parent_request_id: None,
            identity: None,
            handler_identity: Some(CompositionAuthority::new("hub", vec![])),
            forwarded_for: None,
            capabilities: Capabilities::new(),
            metadata: HashMap::new(),
            scoped_env: ScopedPeerEnv::new(["ui/dragged"]),
            env,
            abort_policy: AbortPolicy::default(),
            deadline: Some(Instant::now() + Duration::from_secs(30)),
            internal: true,
        }
    }

    #[tokio::test]
    async fn drive_ws_session_round_trips_binary_call_requested_to_call_responded() {
        let registry = echo_registry();
        let provider: Arc<dyn IdentityProvider> =
            Arc::new(StaticIdentityProvider::new().with_token("ws-token", identity("ws-peer")));
        let dp = dispatcher(Arc::clone(&registry), Arc::clone(&provider));
        let conn = Arc::new(CallConnection::new_overlay_only(identity("ws-peer")));

        let (socket, mut client) = MockWsStream::pair(8);
        let server_handle = tokio::spawn(async move {
            drive_ws_session(socket, &dp, &conn).await;
        });

        let request = EventEnvelope::requested(
            "ws-socket-1",
            serde_json::json!({ "operationId": "/echo/run", "input": { "v": 7 } }),
        );
        client
            .send_binary(serialize_envelope(&request).unwrap())
            .await;

        let msg = client.recv_timeout(Duration::from_secs(5)).await;
        match msg {
            MockMsg::Binary(bytes) => {
                let env: EventEnvelope = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(env.r#type, EVENT_RESPONDED);
                assert_eq!(env.id, "ws-socket-1");
                assert_eq!(
                    env.payload.get("output"),
                    Some(&serde_json::json!({ "v": 7 }))
                );
            }
            other => panic!("expected binary, got {other:?}"),
        }

        client.close().await;
        server_handle.await.ok();
    }

    #[tokio::test]
    async fn drive_ws_session_rejects_text_with_protocol_close() {
        let registry = echo_registry();
        let provider: Arc<dyn IdentityProvider> =
            Arc::new(StaticIdentityProvider::new().with_token("ws-token", identity("ws-peer")));
        let dp = dispatcher(Arc::clone(&registry), Arc::clone(&provider));
        let conn = Arc::new(CallConnection::new_overlay_only(identity("ws-peer")));

        let (socket, mut client) = MockWsStream::pair(8);
        let server_handle = tokio::spawn(async move {
            drive_ws_session(socket, &dp, &conn).await;
        });

        client.send_text("hi".to_string()).await;

        let msg = client.recv_timeout(Duration::from_secs(5)).await;
        match msg {
            MockMsg::Close(code) => {
                assert_eq!(code, WS_CLOSE_PROTOCOL_ERROR);
            }
            other => panic!("expected close frame, got {other:?}"),
        }

        server_handle.await.ok();
    }

    #[tokio::test]
    async fn drive_ws_session_disconnect_aborts_in_flight_pending() {
        let registry = echo_registry();
        let provider: Arc<dyn IdentityProvider> =
            Arc::new(StaticIdentityProvider::new().with_token("ws-token", identity("ws-peer")));
        let dp = dispatcher(Arc::clone(&registry), Arc::clone(&provider));
        let conn = Arc::new(CallConnection::new_overlay_only(identity("ws-peer")));

        {
            let mut pending = conn.pending().lock();
            pending.register_call(
                "ws-inflight".to_string(),
                Instant::now() + Duration::from_secs(30),
                None,
            );
        }

        let (socket, client) = MockWsStream::pair(8);
        let conn_for_server = Arc::clone(&conn);
        let server_handle = tokio::spawn(async move {
            drive_ws_session(socket, &dp, &conn_for_server).await;
        });

        drop(client);

        tokio::time::timeout(Duration::from_secs(5), server_handle)
            .await
            .expect("server did not terminate")
            .ok();

        assert!(
            !conn.pending().lock().contains("ws-inflight"),
            "in-flight pending must be aborted on ws disconnect"
        );
    }

    #[tokio::test]
    async fn drive_ws_session_subscription_streams_call_responded_events() {
        let registry = registry_with_subscription();
        let provider: Arc<dyn IdentityProvider> =
            Arc::new(StaticIdentityProvider::new().with_token("ws-token", identity("ws-peer")));
        let dp = dispatcher(Arc::clone(&registry), Arc::clone(&provider));
        let conn = Arc::new(CallConnection::new_overlay_only(identity("ws-peer")));

        let (socket, mut client) = MockWsStream::pair(16);
        let server_handle = tokio::spawn(async move {
            drive_ws_session(socket, &dp, &conn).await;
        });

        let mut got = Vec::new();
        for i in 0..3 {
            let request = EventEnvelope::requested(
                format!("sub-ws-{i}"),
                serde_json::json!({ "operationId": "/events/stream", "input": {} }),
            );
            client
                .send_binary(serialize_envelope(&request).unwrap())
                .await;

            let msg = client.recv_timeout(Duration::from_secs(5)).await;
            match msg {
                MockMsg::Binary(bytes) => {
                    let env: EventEnvelope = serde_json::from_slice(&bytes).unwrap();
                    assert_eq!(env.id, format!("sub-ws-{i}"));
                    assert_eq!(env.r#type, EVENT_RESPONDED);
                    got.push(env.id);
                }
                other => panic!("expected binary, got {other:?}"),
            }
        }
        assert_eq!(got.len(), 3);

        client.close().await;
        server_handle.await.ok();
    }

    #[tokio::test]
    async fn drive_ws_session_invalid_binary_closes_with_protocol_error() {
        let registry = echo_registry();
        let provider: Arc<dyn IdentityProvider> =
            Arc::new(StaticIdentityProvider::new().with_token("ws-token", identity("ws-peer")));
        let dp = dispatcher(Arc::clone(&registry), Arc::clone(&provider));
        let conn = Arc::new(CallConnection::new_overlay_only(identity("ws-peer")));

        let (socket, mut client) = MockWsStream::pair(8);
        let server_handle = tokio::spawn(async move {
            drive_ws_session(socket, &dp, &conn).await;
        });

        client.send_binary(b"not-json".to_vec()).await;

        let msg = client.recv_timeout(Duration::from_secs(5)).await;
        match msg {
            MockMsg::Close(code) => assert_eq!(code, WS_CLOSE_PROTOCOL_ERROR),
            other => panic!("expected close frame, got {other:?}"),
        }

        server_handle.await.ok();
    }

    #[tokio::test]
    async fn drive_ws_session_client_close_terminates_server() {
        let registry = echo_registry();
        let provider: Arc<dyn IdentityProvider> =
            Arc::new(StaticIdentityProvider::new().with_token("ws-token", identity("ws-peer")));
        let dp = dispatcher(Arc::clone(&registry), Arc::clone(&provider));
        let conn = Arc::new(CallConnection::new_overlay_only(identity("ws-peer")));

        let (socket, mut client) = MockWsStream::pair(8);
        let server_handle = tokio::spawn(async move {
            drive_ws_session(socket, &dp, &conn).await;
        });

        client.send_close().await;

        tokio::time::timeout(Duration::from_secs(5), server_handle)
            .await
            .expect("server did not terminate on client close")
            .ok();
    }

    #[derive(Debug)]
    enum MockMsg {
        Binary(Vec<u8>),
        Text,
        Close(u16),
        Ended,
    }

    struct MockWsStream {
        inbound_rx: tokio::sync::mpsc::Receiver<Message>,
        outbound_tx: tokio::sync::mpsc::Sender<Message>,
    }

    #[async_trait]
    impl WsStream for MockWsStream {
        async fn recv(&mut self) -> Option<Result<Message, axum::Error>> {
            self.inbound_rx.recv().await.map(Ok)
        }

        async fn send(&mut self, msg: Message) -> Result<(), axum::Error> {
            self.outbound_tx.send(msg).await.ok();
            Ok(())
        }

        async fn close(&mut self) {
            let _ = self.outbound_tx.send(Message::Close(None)).await;
        }
    }

    struct MockWsClient {
        inbound_rx: tokio::sync::mpsc::Receiver<Message>,
        outbound_tx: tokio::sync::mpsc::Sender<Message>,
    }

    impl MockWsClient {
        async fn send_binary(&mut self, bytes: Vec<u8>) {
            self.outbound_tx
                .send(Message::Binary(bytes.into()))
                .await
                .ok();
        }

        async fn send_text(&mut self, text: String) {
            self.outbound_tx.send(Message::Text(text.into())).await.ok();
        }

        async fn send_close(&mut self) {
            self.outbound_tx.send(Message::Close(None)).await.ok();
        }

        async fn close(&mut self) {
            let _ = self.outbound_tx.send(Message::Close(None)).await;
        }

        async fn recv_timeout(&mut self, dur: Duration) -> MockMsg {
            let msg = tokio::time::timeout(dur, self.inbound_rx.recv())
                .await
                .expect("timed out waiting for ws message");
            match msg {
                Some(Message::Binary(b)) => MockMsg::Binary(b.to_vec()),
                Some(Message::Text(_)) => MockMsg::Text,
                Some(Message::Close(Some(frame))) => MockMsg::Close(frame.code),
                Some(Message::Close(None)) => MockMsg::Close(0),
                Some(Message::Ping(_) | Message::Pong(_)) => MockMsg::Ended,
                None => MockMsg::Ended,
            }
        }
    }

    impl MockWsStream {
        fn pair(capacity: usize) -> (MockWsStream, MockWsClient) {
            let (server_inbound_tx, server_inbound_rx) = tokio::sync::mpsc::channel(capacity);
            let (server_outbound_tx, server_outbound_rx) = tokio::sync::mpsc::channel(capacity);
            let socket = MockWsStream {
                inbound_rx: server_inbound_rx,
                outbound_tx: server_outbound_tx,
            };
            let client = MockWsClient {
                inbound_rx: server_outbound_rx,
                outbound_tx: server_inbound_tx,
            };
            (socket, client)
        }
    }

    #[tokio::test]
    async fn ws_upgrade_handler_returns_401_when_identity_is_none() {
        let registry = echo_registry();
        let provider: Arc<dyn IdentityProvider> =
            Arc::new(StaticIdentityProvider::new().with_token("ws-token", identity("ws-peer")));
        let identity: Option<Identity> = None;

        let response = ws_upgrade_handler_inner(registry, provider, identity, None).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn ws_upgrade_handler_does_not_reject_when_identity_present() {
        let registry = echo_registry();
        let provider: Arc<dyn IdentityProvider> =
            Arc::new(StaticIdentityProvider::new().with_token("ws-token", identity("ws-peer")));
        let identity = identity("ws-peer");

        let response = ws_upgrade_handler_inner(registry, provider, Some(identity), None).await;
        assert_ne!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
