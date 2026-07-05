//! Connection-local Layer 2 overlay for browser-registered ops
//! (ADR-024, ADR-034 §4, ADR-044 §5).
//!
//! A browser over WebSocket has no `PeerId`, does not enter
//! `PeerCompositeEnv`, and any ops it registers land in a per-
//! `CallConnection` overlay that dies when the connection drops. The hub
//! reaches browser ops through the live `CallConnection` handle's
//! `overlay_env()`, not through `PeerRef::Specific` (the browser is not a
//! peer). `AccessControl` on browser-registered ops gates the hub's
//! calls. WS close drops the overlay and aborts in-flight calls (ADR-016).
//!
//! This module is verification + integration tests for the overlay
//! mechanism the upgrade handler (`upgrade.rs`) and `CallConnection`
//! (`alknet-call`) already provide. See
//! `docs/architecture/crates/http/websocket.md`.

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use alknet_call::protocol::connection::CallConnection;
    use alknet_call::protocol::dispatch::Dispatcher;
    use alknet_call::protocol::wire::{
        EventEnvelope, ResponseEnvelope, EVENT_ERROR, EVENT_RESPONDED,
    };
    use alknet_call::registry::context::{
        AbortPolicy, CompositionAuthority, OperationContext, ScopedPeerEnv,
    };
    use alknet_call::registry::env::{OperationEnv, PeerRef};
    use alknet_call::registry::registration::{
        make_handler, HandlerKind, HandlerRegistration, OperationProvenance, OperationRegistry,
    };
    use alknet_call::registry::spec::{AccessControl, OperationSpec, OperationType, Visibility};
    use alknet_core::auth::{Identity, IdentityProvider};
    use alknet_core::types::Capabilities;

    struct StaticIdentityProvider {
        tokens: std::sync::Mutex<HashMap<String, Identity>>,
    }

    impl StaticIdentityProvider {
        fn new() -> Self {
            Self {
                tokens: std::sync::Mutex::new(HashMap::new()),
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
            None,
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
            None,
        )
    }

    fn browser_registration(
        name: &str,
        acl: AccessControl,
        composition_authority: Option<CompositionAuthority>,
    ) -> HandlerRegistration {
        HandlerRegistration::new(
            external_spec(name, acl),
            HandlerKind::Once(make_handler(|input, ctx| async move {
                ResponseEnvelope::ok(ctx.request_id, input)
            })),
            OperationProvenance::FromCall,
            composition_authority,
            None,
            Capabilities::new(),
        )
    }

    fn echo_registry() -> Arc<OperationRegistry> {
        let mut registry = OperationRegistry::new();
        registry
            .register(HandlerRegistration::new(
                external_spec("echo/run", AccessControl::default()),
                HandlerKind::Once(make_handler(|input, ctx| async move {
                    ResponseEnvelope::ok(ctx.request_id, input)
                })),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        Arc::new(registry)
    }

    fn empty_provider() -> Arc<dyn IdentityProvider> {
        Arc::new(StaticIdentityProvider::new())
    }

    fn dispatcher(
        registry: Arc<OperationRegistry>,
        provider: Arc<dyn IdentityProvider>,
    ) -> Dispatcher {
        Dispatcher::new(registry, provider)
    }

    fn hub_root_context(
        request_id: &str,
        allowed: &[&str],
        hub_identity: Option<CompositionAuthority>,
        env: Arc<dyn OperationEnv + Send + Sync>,
    ) -> OperationContext {
        OperationContext {
            request_id: request_id.to_string(),
            parent_request_id: None,
            identity: None,
            handler_identity: hub_identity,
            forwarded_for: None,
            capabilities: Capabilities::new(),
            metadata: HashMap::new(),
            scoped_env: ScopedPeerEnv::new(allowed.iter().copied()),
            env,
            abort_policy: AbortPolicy::default(),
            deadline: Some(Instant::now() + Duration::from_secs(30)),
            internal: true,
            ownership: None,
        }
    }

    #[tokio::test]
    async fn browser_registered_op_lands_in_overlay_not_peer_composite_env() {
        let conn = Arc::new(CallConnection::new_overlay_only(identity("browser")));
        let overlay_env = conn.overlay_env();
        assert!(!overlay_env.contains("ui/dragged"));

        conn.register_imported(browser_registration(
            "ui/dragged",
            AccessControl::default(),
            None,
        ));
        assert!(overlay_env.contains("ui/dragged"));
        assert!(
            overlay_env.peer_ids().is_empty(),
            "browser overlay env exposes no PeerIds (browser is not a peer)"
        );
    }

    #[tokio::test]
    async fn browser_connection_has_no_peer_entry_and_no_peerid() {
        let conn = CallConnection::new_overlay_only(identity("browser"));
        assert!(conn.connection().is_none());
        assert_eq!(conn.identity().unwrap().id, "browser");
        let env = conn.overlay_env();
        assert!(
            env.peer_ids().is_empty(),
            "overlay-only connection has no PeerIds — no PeerCompositeEnv entry"
        );
    }

    #[tokio::test]
    async fn register_imported_and_register_imported_all_both_populate_overlay() {
        let conn = Arc::new(CallConnection::new_overlay_only(identity("browser")));
        conn.register_imported(browser_registration(
            "ui/click",
            AccessControl::default(),
            None,
        ));
        conn.register_imported_all(vec![
            browser_registration("ui/focus", AccessControl::default(), None),
            browser_registration("ui/scroll", AccessControl::default(), None),
        ]);

        let env = conn.overlay_env();
        assert!(env.contains("ui/click"));
        assert!(env.contains("ui/focus"));
        assert!(env.contains("ui/scroll"));
        assert!(!env.contains("ui/missing"));
    }

    #[tokio::test]
    async fn hub_outgoing_call_routes_through_overlay_env_not_peerref_specific() {
        let registry = echo_registry();
        let dp = dispatcher(registry, empty_provider());
        let conn = Arc::new(CallConnection::new_overlay_only(identity("browser")));

        conn.register_imported(browser_registration(
            "ui/dragged",
            AccessControl::default(),
            None,
        ));

        let composed_env = dp.compose_root_env(
            &conn,
            &hub_root_context(
                "hub-call-1",
                &["ui/dragged"],
                CompositionAuthority::new("hub", vec![]).into(),
                conn.overlay_env(),
            ),
        );

        let ctx = hub_root_context(
            "hub-call-1",
            &["ui/dragged"],
            CompositionAuthority::new("hub", vec![]).into(),
            composed_env.clone(),
        );

        let response = composed_env
            .invoke("ui", "dragged", serde_json::json!({ "x": 5 }), &ctx)
            .await;
        assert!(response.result.is_ok());
        assert_eq!(response.result.unwrap(), serde_json::json!({ "x": 5 }));
    }

    #[tokio::test]
    async fn peerref_specific_browser_x_routes_to_nothing_no_peer_entry() {
        let registry = echo_registry();
        let dp = dispatcher(registry, empty_provider());
        let conn = Arc::new(CallConnection::new_overlay_only(identity("browser")));

        conn.register_imported(browser_registration(
            "ui/dragged",
            AccessControl::default(),
            None,
        ));

        let composed_env = dp.compose_root_env(
            &conn,
            &hub_root_context(
                "hub-peer-1",
                &["ui/dragged"],
                CompositionAuthority::new("hub", vec![]).into(),
                conn.overlay_env(),
            ),
        );

        let ctx = hub_root_context(
            "hub-peer-1",
            &["ui/dragged"],
            CompositionAuthority::new("hub", vec![]).into(),
            composed_env.clone(),
        );

        let response = composed_env
            .invoke_peer(
                &PeerRef::Specific("browser-X".to_string()),
                "ui",
                "dragged",
                serde_json::json!({}),
                &ctx,
                AbortPolicy::default(),
            )
            .await;

        match response.result {
            Err(e) => assert_eq!(e.code, "NOT_FOUND"),
            other => panic!("expected NOT_FOUND for PeerRef::Specific(browser-X), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn access_control_on_browser_op_gates_hub_call_allowed() {
        let conn = Arc::new(CallConnection::new_overlay_only(identity("browser")));
        conn.register_imported(browser_registration(
            "ui/dragged",
            AccessControl {
                required_scopes: vec!["ui:write".to_string()],
                ..Default::default()
            },
            None,
        ));

        let env = conn.overlay_env();
        let ctx = hub_root_context(
            "hub-acl-ok",
            &["ui/dragged"],
            Some(CompositionAuthority::new(
                "hub",
                vec!["ui:write".to_string()],
            )),
            env.clone(),
        );

        let response = env
            .invoke("ui", "dragged", serde_json::json!({ "v": 1 }), &ctx)
            .await;
        assert!(response.result.is_ok());
        assert_eq!(response.result.unwrap(), serde_json::json!({ "v": 1 }));
    }

    #[tokio::test]
    async fn access_control_on_browser_op_gates_hub_call_forbidden() {
        let conn = Arc::new(CallConnection::new_overlay_only(identity("browser")));
        conn.register_imported(browser_registration(
            "ui/dragged",
            AccessControl {
                required_scopes: vec!["ui:write".to_string()],
                ..Default::default()
            },
            None,
        ));

        let env = conn.overlay_env();
        let ctx = hub_root_context(
            "hub-acl-deny",
            &["ui/dragged"],
            Some(CompositionAuthority::new(
                "hub",
                vec!["ui:read".to_string()],
            )),
            env.clone(),
        );

        let response = env
            .invoke("ui", "dragged", serde_json::json!({}), &ctx)
            .await;
        match response.result {
            Err(e) => assert_eq!(e.code, "FORBIDDEN"),
            other => panic!("expected FORBIDDEN, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn access_control_default_on_browser_op_allows_hub_without_scopes() {
        let conn = Arc::new(CallConnection::new_overlay_only(identity("browser")));
        conn.register_imported(browser_registration(
            "ui/dragged",
            AccessControl::default(),
            None,
        ));

        let env = conn.overlay_env();
        let ctx = hub_root_context(
            "hub-acl-default",
            &["ui/dragged"],
            Some(CompositionAuthority::new("hub", vec![])),
            env.clone(),
        );

        let response = env
            .invoke("ui", "dragged", serde_json::json!({ "ok": true }), &ctx)
            .await;
        assert!(response.result.is_ok());
    }

    #[tokio::test]
    async fn overlay_dropped_on_ws_close_op_no_longer_reachable() {
        let conn1 = CallConnection::new_overlay_only(identity("browser-1"));
        conn1.register_imported(browser_registration(
            "ui/dragged",
            AccessControl::default(),
            None,
        ));
        assert!(conn1.overlay_env().contains("ui/dragged"));
        drop(conn1);

        let conn2 = CallConnection::new_overlay_only(identity("browser-2"));
        assert!(
            !conn2.overlay_env().contains("ui/dragged"),
            "a fresh connection's overlay is empty — the dropped connection's overlay did not leak into global state"
        );
    }

    #[tokio::test]
    async fn overlay_isolation_between_connections() {
        let conn_a = CallConnection::new_overlay_only(identity("browser-a"));
        let conn_b = CallConnection::new_overlay_only(identity("browser-b"));

        conn_a.register_imported(browser_registration(
            "ui/dragged",
            AccessControl::default(),
            None,
        ));
        conn_b.register_imported(browser_registration(
            "ui/click",
            AccessControl::default(),
            None,
        ));

        assert!(conn_a.overlay_env().contains("ui/dragged"));
        assert!(!conn_a.overlay_env().contains("ui/click"));
        assert!(conn_b.overlay_env().contains("ui/click"));
        assert!(!conn_b.overlay_env().contains("ui/dragged"));
    }

    #[tokio::test]
    async fn browser_with_no_registered_ops_has_unused_server_to_client_direction() {
        let conn = Arc::new(CallConnection::new_overlay_only(identity("browser")));
        let env = conn.overlay_env();
        assert!(!env.contains("anything"));
        assert!(!env.contains("ui/dragged"));

        let ctx = hub_root_context(
            "no-ops",
            &["ui/dragged"],
            Some(CompositionAuthority::new("hub", vec![])),
            env.clone(),
        );
        let response = env
            .invoke("ui", "dragged", serde_json::json!({}), &ctx)
            .await;
        match response.result {
            Err(e) => assert_eq!(e.code, "NOT_FOUND"),
            other => panic!("expected NOT_FOUND when browser registered no ops, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn bidirectionality_hub_calls_browser_op_via_overlay() {
        let registry = echo_registry();
        let dp = dispatcher(registry, empty_provider());
        let conn = Arc::new(CallConnection::new_overlay_only(identity("browser")));

        conn.register_imported(HandlerRegistration::new(
            external_spec("ui/dragged", AccessControl::default()),
            HandlerKind::Once(make_handler(|input, ctx| async move {
                ResponseEnvelope::ok(ctx.request_id, serde_json::json!({ "echoed": input }))
            })),
            OperationProvenance::FromCall,
            None,
            None,
            Capabilities::new(),
        ));

        let composed_env = dp.compose_root_env(
            &conn,
            &hub_root_context(
                "bidir-1",
                &["ui/dragged"],
                CompositionAuthority::new("hub", vec![]).into(),
                conn.overlay_env(),
            ),
        );
        let ctx = hub_root_context(
            "bidir-1",
            &["ui/dragged"],
            CompositionAuthority::new("hub", vec![]).into(),
            composed_env.clone(),
        );

        let response = composed_env
            .invoke("ui", "dragged", serde_json::json!({ "dx": 10 }), &ctx)
            .await;
        assert!(response.result.is_ok());
        assert_eq!(
            response.result.unwrap(),
            serde_json::json!({ "echoed": { "dx": 10 } })
        );
    }

    #[tokio::test]
    async fn ws_close_aborts_in_flight_subscription_and_cascades_descendants() {
        let conn = Arc::new(CallConnection::new_overlay_only(identity("browser")));

        {
            let mut pending = conn.pending().lock();
            pending.register_subscribe("ws-sub-root".to_string(), None, None);
            pending.register_call(
                "ws-sub-child".to_string(),
                Instant::now() + Duration::from_secs(30),
                Some("ws-sub-root".to_string()),
            );
        }
        assert!(conn.pending().lock().contains("ws-sub-root"));
        assert!(conn.pending().lock().contains("ws-sub-child"));

        let failed =
            conn.pending()
                .lock()
                .fail_all(alknet_call::protocol::wire::CallError::internal(
                    "connection closed",
                ));
        assert!(failed.contains(&"ws-sub-root".to_string()));
        assert!(failed.contains(&"ws-sub-child".to_string()));
        assert!(conn.pending().lock().is_empty());
    }

    #[tokio::test]
    async fn ws_close_mid_call_to_browser_op_aborts_call_error_cascade() {
        let conn = Arc::new(CallConnection::new_overlay_only(identity("browser")));
        conn.register_imported(browser_registration(
            "ui/dragged",
            AccessControl::default(),
            None,
        ));

        let rx = {
            let mut pending = conn.pending().lock();
            pending.register_call(
                "hub-call-inflight".to_string(),
                Instant::now() + Duration::from_secs(30),
                None,
            )
        };

        let failed =
            conn.pending()
                .lock()
                .fail_all(alknet_call::protocol::wire::CallError::internal(
                    "connection closed",
                ));
        assert!(failed.contains(&"hub-call-inflight".to_string()));

        let result = tokio::time::timeout(Duration::from_millis(100), rx).await;
        match result {
            Ok(Ok(Err(e))) => assert_eq!(e.code, "INTERNAL"),
            other => panic!("expected Err(INTERNAL) from aborted call, got {other:?}"),
        }

        assert!(
            conn.pending().lock().is_empty(),
            "in-flight call aborted from pending map on ws close"
        );
        assert!(conn.overlay_env().contains("ui/dragged"));
    }

    #[tokio::test]
    async fn overlay_env_invoke_event_envelope_round_trip_for_browser_op() {
        let conn = Arc::new(CallConnection::new_overlay_only(identity("browser")));
        conn.register_imported(browser_registration(
            "ui/dragged",
            AccessControl::default(),
            None,
        ));

        let env = conn.overlay_env();
        let ctx = hub_root_context(
            "env-rt-1",
            &["ui/dragged"],
            Some(CompositionAuthority::new("hub", vec![])),
            env.clone(),
        );
        let response = env
            .invoke("ui", "dragged", serde_json::json!({ "v": 9 }), &ctx)
            .await;
        let envelope: EventEnvelope = response.into();
        assert_eq!(envelope.r#type, EVENT_RESPONDED);
        assert_eq!(
            envelope.payload.get("output"),
            Some(&serde_json::json!({ "v": 9 }))
        );
    }

    #[tokio::test]
    async fn overlay_env_invoke_forbidden_emits_call_error_envelope() {
        let conn = Arc::new(CallConnection::new_overlay_only(identity("browser")));
        conn.register_imported(browser_registration(
            "ui/dragged",
            AccessControl {
                required_scopes: vec!["ui:write".to_string()],
                ..Default::default()
            },
            None,
        ));

        let env = conn.overlay_env();
        let ctx = hub_root_context(
            "env-rt-forbid",
            &["ui/dragged"],
            Some(CompositionAuthority::new("hub", vec![])),
            env.clone(),
        );
        let response = env
            .invoke("ui", "dragged", serde_json::json!({}), &ctx)
            .await;
        let envelope: EventEnvelope = response.into();
        assert_eq!(envelope.r#type, EVENT_ERROR);
        assert_eq!(
            envelope.payload.get("code"),
            Some(&serde_json::json!("FORBIDDEN"))
        );
    }

    #[tokio::test]
    async fn overlay_reachability_gate_returns_not_found_for_disallowed_op() {
        let conn = Arc::new(CallConnection::new_overlay_only(identity("browser")));
        conn.register_imported(browser_registration(
            "ui/dragged",
            AccessControl::default(),
            None,
        ));

        let env = conn.overlay_env();
        let ctx = hub_root_context(
            "reach-deny",
            &[],
            Some(CompositionAuthority::new("hub", vec![])),
            env.clone(),
        );
        let response = env
            .invoke("ui", "dragged", serde_json::json!({}), &ctx)
            .await;
        match response.result {
            Err(e) => assert_eq!(e.code, "NOT_FOUND"),
            other => panic!("expected NOT_FOUND (not in scoped_env), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn overlay_subscription_spec_round_trips_via_overlay_env() {
        let conn = Arc::new(CallConnection::new_overlay_only(identity("browser")));
        let counter = std::sync::Arc::new(std::sync::Mutex::new(0u32));
        let handler = {
            let counter = std::sync::Arc::clone(&counter);
            make_handler(move |_input, ctx| {
                let counter = std::sync::Arc::clone(&counter);
                async move {
                    let mut c = counter.lock().unwrap();
                    *c += 1;
                    ResponseEnvelope::ok(ctx.request_id, serde_json::json!({ "n": *c }))
                }
            })
        };
        conn.register_imported(HandlerRegistration::new(
            subscription_spec("events/stream"),
            HandlerKind::Once(handler),
            OperationProvenance::FromCall,
            None,
            None,
            Capabilities::new(),
        ));

        let env = conn.overlay_env();
        assert!(env.contains("events/stream"));

        for i in 0..3 {
            let ctx = hub_root_context(
                &format!("sub-{i}"),
                &["events/stream"],
                Some(CompositionAuthority::new("hub", vec![])),
                env.clone(),
            );
            let response = env
                .invoke("events", "stream", serde_json::json!({}), &ctx)
                .await;
            assert!(response.result.is_ok());
        }
    }

    #[tokio::test]
    async fn browser_identity_resolved_at_upgrade_is_stored_on_connection() {
        let provider = Arc::new(StaticIdentityProvider::new().with_token(
            "browser-token",
            identity_with_scopes("browser-user", &["ui:read"]),
        ));
        let registry = echo_registry();
        let dp = dispatcher(registry, Arc::clone(&provider) as Arc<dyn IdentityProvider>);

        let conn = Arc::new(CallConnection::new_overlay_only(identity_with_scopes(
            "browser-user",
            &["ui:read"],
        )));
        assert_eq!(conn.identity().unwrap().id, "browser-user");
        assert_eq!(conn.identity().unwrap().scopes, vec!["ui:read".to_string()]);

        let composed_env = dp.compose_root_env(
            &conn,
            &hub_root_context(
                "id-check",
                &["echo/run"],
                CompositionAuthority::new("hub", vec![]).into(),
                conn.overlay_env(),
            ),
        );
        let peer_ids = composed_env.peer_ids();
        assert_eq!(peer_ids, vec!["browser-user".to_string()]);
    }
}
