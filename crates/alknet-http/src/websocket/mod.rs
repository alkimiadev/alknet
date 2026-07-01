//! WebSocket upgrade handler, framing, and dispatch handoff.
//!
//! WebSocket is the browser bidirectional path (ADR-044) and carries the
//! native `EventEnvelope` call-protocol session, not the gateway shape
//! (ADR-048). See `docs/architecture/crates/http/websocket.md`.

pub mod overlay;
pub mod upgrade;

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use alknet_call::protocol::connection::CallConnection;
    use alknet_call::protocol::dispatch::Dispatcher;
    use alknet_call::protocol::wire::{EventEnvelope, ResponseEnvelope, EVENT_RESPONDED};
    use alknet_call::registry::context::AbortPolicy;
    use alknet_call::registry::registration::{
        make_handler, HandlerRegistration, OperationProvenance,
    };
    use alknet_call::registry::spec::{AccessControl, OperationSpec, OperationType, Visibility};
    use alknet_core::auth::{Identity, IdentityProvider};
    use alknet_core::types::Capabilities;
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
        fn resolve_from_token(&self, token: &alknet_core::auth::AuthToken) -> Option<Identity> {
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

    fn external_spec(name: &str) -> OperationSpec {
        OperationSpec::new(
            name,
            OperationType::Query,
            Visibility::External,
            serde_json::json!({}),
            serde_json::json!({}),
            vec![],
            AccessControl::default(),
        )
    }

    fn echo_registry() -> Arc<alknet_call::registry::registration::OperationRegistry> {
        let mut registry = alknet_call::registry::registration::OperationRegistry::new();
        registry.register(HandlerRegistration::new(
            external_spec("echo/run"),
            make_handler(|input, ctx| async move { ResponseEnvelope::ok(ctx.request_id, input) }),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        Arc::new(registry)
    }

    #[tokio::test]
    async fn ws_dispatch_requested_via_pub_api_returns_response_envelope() {
        let registry = echo_registry();
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dispatcher = Dispatcher::new(registry, provider);
        let conn = Arc::new(CallConnection::new_overlay_only(identity("ws-peer")));

        let payload = serde_json::json!({
            "operationId": "/echo/run",
            "input": { "msg": "hi" },
        });
        let request_id = "ws-1".to_string();
        let response = dispatcher
            .dispatch_requested(&conn, request_id.clone(), payload)
            .await;
        assert!(response.result.is_ok());
        assert_eq!(response.request_id, "ws-1");
        assert_eq!(response.result.unwrap(), serde_json::json!({ "msg": "hi" }));
    }

    #[tokio::test]
    async fn ws_dispatch_round_trip_event_envelope_via_pub_api() {
        let registry = echo_registry();
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dispatcher = Dispatcher::new(registry, provider);
        let conn = Arc::new(CallConnection::new_overlay_only(identity("ws-peer")));

        let envelope = EventEnvelope::new(
            "call.requested",
            "ws-rt-1",
            serde_json::json!({
                "operationId": "/echo/run",
                "input": { "v": 7 },
            }),
        );
        let response = dispatcher
            .dispatch_requested(&conn, envelope.id.clone(), envelope.payload.clone())
            .await;
        let out: EventEnvelope = response.into();
        assert_eq!(out.r#type, EVENT_RESPONDED);
        assert_eq!(out.id, "ws-rt-1");
        assert_eq!(
            out.payload.get("output"),
            Some(&serde_json::json!({ "v": 7 }))
        );
    }

    #[tokio::test]
    async fn ws_handle_abort_via_pub_api_cascades_descendants() {
        let registry = echo_registry();
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dispatcher = Dispatcher::new(registry, provider);
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
        dispatcher.handle_abort(&conn, "ws-parent").await;
        assert!(!conn.pending().lock().contains("ws-parent"));
        assert!(!conn.pending().lock().contains("ws-child"));
    }

    #[tokio::test]
    async fn ws_overlay_only_connection_holds_overlay_and_pending() {
        let conn = CallConnection::new_overlay_only(identity("ws-peer"));
        assert!(conn.connection().is_none());
        assert_eq!(
            conn.identity().map(|i| i.id.clone()),
            Some("ws-peer".to_string())
        );
        assert!(conn.pending().lock().is_empty());

        let env = conn.overlay_env();
        assert!(!env.contains("worker/exec"));
        conn.register_imported(HandlerRegistration::new(
            external_spec("worker/exec"),
            make_handler(|input, ctx| async move { ResponseEnvelope::ok(ctx.request_id, input) }),
            OperationProvenance::FromCall,
            None,
            None,
            Capabilities::new(),
        ));
        assert!(env.contains("worker/exec"));
    }

    #[tokio::test]
    async fn ws_dispatch_with_auth_token_resolves_identity_via_provider() {
        let registry = echo_registry();
        let provider: Arc<dyn IdentityProvider> = Arc::new(
            StaticIdentityProvider::new().with_token("ws-token", identity("resolved-peer")),
        );
        let dispatcher = Dispatcher::new(registry, provider);
        let conn = Arc::new(CallConnection::new_overlay_only(identity("ws-peer")));

        let payload = serde_json::json!({
            "operationId": "/echo/run",
            "input": {},
            "auth_token": "ws-token",
        });
        let response = dispatcher
            .dispatch_requested(&conn, "ws-auth-1".to_string(), payload)
            .await;
        assert!(response.result.is_ok());
    }

    #[test]
    fn ws_abort_policy_default_is_abort_dependents() {
        assert_eq!(AbortPolicy::default(), AbortPolicy::AbortDependents);
    }
}
