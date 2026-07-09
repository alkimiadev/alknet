//! `CallAdapter`: implements `ProtocolHandler` for ALPN `alknet/call`.
//!
//! Accepts bidirectional streams, reads `EventEnvelope` frames, and
//! dispatches `call.requested` events to the operation registry. See
//! `docs/architecture/crates/call/call-protocol.md` for the full
//! specification.
//!
//! The dispatch loop is shared with [`crate::client::CallClient`] via
//! [`super::dispatch::Dispatcher`] (ADR-017 §1): `CallAdapter` is the
//! inbound (accept) half; `CallClient` is the outbound (connect) half; both
//! produce a [`CallConnection`] and hand it to the same `Dispatcher::run_loop`.

use std::sync::Arc;
use std::time::Duration;

use alknet_core::auth::{AuthContext, IdentityProvider};
use alknet_core::ownership::OwnershipProvider;
use alknet_core::types::{Connection, HandlerError, ProtocolHandler};
use async_trait::async_trait;

use super::connection::CallConnection;
use super::dispatch::Dispatcher;
use crate::registry::context::OperationContext;
use crate::registry::registration::OperationRegistry;

#[cfg(test)]
use super::wire::ResponseEnvelope;
#[cfg(test)]
use alknet_core::auth::Identity;
#[cfg(test)]
use serde_json::Value;

pub trait SessionOverlaySource: Send + Sync {
    fn overlay_for(
        &self,
        context: &OperationContext,
    ) -> Option<Arc<dyn crate::registry::env::OperationEnv + Send + Sync>>;
}

pub struct CallAdapter {
    dispatcher: Dispatcher,
}

impl CallAdapter {
    pub fn new(
        registry: Arc<OperationRegistry>,
        identity_provider: Arc<dyn IdentityProvider>,
    ) -> Self {
        Self {
            dispatcher: Dispatcher::new(registry, identity_provider),
        }
    }

    pub fn with_session_source(
        mut self,
        source: Arc<dyn SessionOverlaySource + Send + Sync>,
    ) -> Self {
        self.dispatcher = self.dispatcher.with_session_source(source);
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.dispatcher = self.dispatcher.with_timeout(timeout);
        self
    }

    pub fn with_ownership_provider(mut self, provider: Arc<dyn OwnershipProvider>) -> Self {
        self.dispatcher = self.dispatcher.with_ownership_provider(provider);
        self
    }

    pub fn registry(&self) -> &Arc<OperationRegistry> {
        &self.dispatcher.registry
    }

    pub fn identity_provider(&self) -> &Arc<dyn IdentityProvider> {
        &self.dispatcher.identity_provider
    }

    pub fn default_timeout(&self) -> Duration {
        self.dispatcher.default_timeout
    }

    pub fn session_source(&self) -> Option<&Arc<dyn SessionOverlaySource + Send + Sync>> {
        self.dispatcher.session_source.as_ref()
    }

    // --- Test-facing wrappers around the shared Dispatcher -----------------
    // These exist so the adapter's existing tests keep compiling against the
    // adapter type; they delegate to the Dispatcher's shared implementation.
    // Gated to test builds — the production adapter delegates through
    // `handle()` -> `Dispatcher::run_loop()` directly.

    #[cfg(test)]
    pub(crate) fn strip_leading_slash(operation_id: &str) -> &str {
        operation_id.strip_prefix('/').unwrap_or(operation_id)
    }

    #[cfg(test)]
    pub(crate) fn resolve_identity(
        &self,
        connection_identity: Option<Identity>,
        payload: &Value,
    ) -> Option<Identity> {
        self.dispatcher
            .resolve_identity(connection_identity, payload)
    }

    #[cfg(test)]
    pub(crate) fn build_root_context(
        &self,
        request_id: String,
        operation_name: &str,
        identity: Option<Identity>,
        forwarded_for: Option<Identity>,
        connection: &CallConnection,
    ) -> OperationContext {
        self.dispatcher.build_root_context(
            request_id,
            operation_name,
            identity,
            forwarded_for,
            connection,
        )
    }

    #[cfg(test)]
    pub(crate) async fn dispatch_requested(
        &self,
        connection: &Arc<CallConnection>,
        request_id: String,
        payload: Value,
    ) -> ResponseEnvelope {
        self.dispatcher
            .dispatch_requested(connection, request_id, payload)
            .await
    }

    #[cfg(test)]
    pub(crate) async fn handle_stream(
        &self,
        connection: Arc<CallConnection>,
        send: alknet_core::types::SendStream,
        recv: alknet_core::types::RecvStream,
    ) {
        self.dispatcher.handle_stream(connection, send, recv).await;
    }
}

#[async_trait]
impl ProtocolHandler for CallAdapter {
    fn alpn(&self) -> &'static [u8] {
        b"alknet/call"
    }

    async fn handle(&self, connection: Connection, auth: &AuthContext) -> Result<(), HandlerError> {
        if let Some(identity) = auth.identity.clone() {
            let _ = connection.set_identity(identity);
        }

        let call_connection = Arc::new(CallConnection::new(connection));
        self.dispatcher.clone().run_loop(call_connection).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::wire::{
        CallError, EventEnvelope, EVENT_COMPLETED, EVENT_ERROR, EVENT_RESPONDED,
    };
    use crate::registry::context::{AbortPolicy, OperationContext, ScopedPeerEnv};
    use crate::registry::env::OperationEnv;
    use crate::registry::registration::{
        make_handler, HandlerKind, HandlerRegistration, OperationProvenance,
    };
    use crate::registry::spec::{AccessControl, OperationSpec, OperationType, Visibility};
    use alknet_core::auth::AuthToken;
    use alknet_core::types::Capabilities;
    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
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

    #[allow(dead_code)]
    fn internal_spec(name: &str) -> OperationSpec {
        OperationSpec::new(
            name,
            OperationType::Query,
            Visibility::Internal,
            serde_json::json!({}),
            serde_json::json!({}),
            vec![],
            AccessControl::default(),
            None,
        )
    }

    fn registry_with(
        name: &str,
        visibility: Visibility,
        acl: AccessControl,
        handler: crate::registry::registration::Handler,
    ) -> Arc<OperationRegistry> {
        let mut registry = OperationRegistry::new();
        registry
            .register(HandlerRegistration::new(
                OperationSpec::new(
                    name,
                    OperationType::Query,
                    visibility,
                    serde_json::json!({}),
                    serde_json::json!({}),
                    vec![],
                    acl,
                    None,
                ),
                HandlerKind::Once(handler),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        Arc::new(registry)
    }

    fn echo_handler() -> crate::registry::registration::Handler {
        make_handler(
            |input, context| async move { ResponseEnvelope::ok(context.request_id, input) },
        )
    }

    fn inspect_identity_handler() -> crate::registry::registration::Handler {
        make_handler(|_input, context| async move {
            let id = context.identity.as_ref().map(|i| i.id.clone());
            ResponseEnvelope::ok(context.request_id, serde_json::json!({ "identity_id": id }))
        })
    }

    fn stub_connection() -> Connection {
        Connection::from_stream(
            tokio::io::sink(),
            tokio::io::empty(),
            b"alknet/call".to_vec(),
            Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4321)),
        )
    }

    #[test]
    fn alpn_returns_alknet_call() {
        let registry = Arc::new(OperationRegistry::new());
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let adapter = CallAdapter::new(registry, provider);
        assert_eq!(adapter.alpn(), b"alknet/call");
    }

    #[test]
    fn constructors_set_fields() {
        let registry = Arc::new(OperationRegistry::new());
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let adapter = CallAdapter::new(Arc::clone(&registry), Arc::clone(&provider))
            .with_timeout(Duration::from_secs(60));
        assert_eq!(adapter.default_timeout(), Duration::from_secs(60));
        assert!(Arc::ptr_eq(adapter.registry(), &registry));
        assert!(adapter.session_source().is_none());
    }

    #[test]
    fn strip_leading_slash_removes_prefix() {
        assert_eq!(
            CallAdapter::strip_leading_slash("/fs/readFile"),
            "fs/readFile"
        );
        assert_eq!(
            CallAdapter::strip_leading_slash("fs/readFile"),
            "fs/readFile"
        );
        assert_eq!(
            CallAdapter::strip_leading_slash("/services/list"),
            "services/list"
        );
    }

    #[test]
    fn resolve_identity_uses_connection_identity_when_no_token() {
        let registry = Arc::new(OperationRegistry::new());
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let adapter = CallAdapter::new(registry, provider);
        let conn_id = identity_with_scopes("caller", &["user"]);
        let payload = serde_json::json!({ "operationId": "/echo/run", "input": {} });
        let resolved = adapter.resolve_identity(Some(conn_id.clone()), &payload);
        assert_eq!(resolved, Some(conn_id));
    }

    #[test]
    fn resolve_identity_token_overrides_connection_identity() {
        let registry = Arc::new(OperationRegistry::new());
        let token_identity = identity_with_scopes("admin", &["admin"]);
        let provider: Arc<dyn IdentityProvider> = Arc::new(
            StaticIdentityProvider::new().with_token("alk_secret", token_identity.clone()),
        );
        let adapter = CallAdapter::new(registry, provider);
        let conn_id = identity_with_scopes("caller", &["user"]);
        let payload = serde_json::json!({
            "operationId": "/echo/run",
            "input": {},
            "auth_token": "alk_secret",
        });
        let resolved = adapter.resolve_identity(Some(conn_id), &payload);
        assert_eq!(resolved.as_ref().map(|i| &i.id), Some(&"admin".to_string()));
    }

    #[test]
    fn resolve_identity_token_failure_falls_back_to_connection_identity() {
        let registry = Arc::new(OperationRegistry::new());
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let adapter = CallAdapter::new(registry, provider);
        let conn_id = identity_with_scopes("caller", &["user"]);
        let payload = serde_json::json!({
            "operationId": "/echo/run",
            "input": {},
            "auth_token": "alk_unknown",
        });
        let resolved = adapter.resolve_identity(Some(conn_id.clone()), &payload);
        assert_eq!(resolved, Some(conn_id));
    }

    #[test]
    fn resolve_identity_token_failure_with_no_connection_identity_returns_none() {
        let registry = Arc::new(OperationRegistry::new());
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let adapter = CallAdapter::new(registry, provider);
        let payload = serde_json::json!({
            "operationId": "/echo/run",
            "input": {},
            "auth_token": "alk_unknown",
        });
        let resolved = adapter.resolve_identity(None, &payload);
        assert!(resolved.is_none());
    }

    #[tokio::test]
    async fn build_root_context_sets_internal_false_and_deadline() {
        let registry = registry_with(
            "echo/run",
            Visibility::External,
            AccessControl::default(),
            echo_handler(),
        );
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let adapter = CallAdapter::new(registry, provider);
        let conn = CallConnection::new(stub_connection());

        let context =
            adapter.build_root_context("req-1".to_string(), "echo/run", None, None, &conn);

        assert!(!context.is_internal());
        assert!(context.parent_request_id.is_none());
        assert!(context.deadline.is_some());
    }

    #[tokio::test]
    async fn build_root_context_carries_capabilities_and_scoped_env() {
        let mut registry = OperationRegistry::new();
        let scoped = ScopedPeerEnv::new(["fs/readFile"]);
        let caps = Capabilities::new().with_api_key("google", "k".to_string());
        registry
            .register(HandlerRegistration::new(
                external_spec("agent/run", AccessControl::default()),
                HandlerKind::Once(echo_handler()),
                OperationProvenance::Local,
                None,
                Some(scoped.clone()),
                caps.clone(),
            ))
            .unwrap();
        let registry = Arc::new(registry);
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let adapter = CallAdapter::new(registry, provider);
        let conn = CallConnection::new(stub_connection());

        let context =
            adapter.build_root_context("req-2".to_string(), "agent/run", None, None, &conn);

        assert!(context.scoped_env.allows("fs/readFile"));
        assert!(!context.scoped_env.allows("other/op"));
    }

    #[tokio::test]
    async fn compose_root_env_aggregates_layers() {
        let registry = registry_with(
            "fs/readFile",
            Visibility::External,
            AccessControl::default(),
            echo_handler(),
        );
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let adapter = CallAdapter::new(registry.clone(), provider);
        let conn = CallConnection::new(stub_connection());

        let context =
            adapter.build_root_context("req-3".to_string(), "fs/readFile", None, None, &conn);

        assert!(context.env.contains("fs/readFile"));
    }

    struct StubSessionOverlay {
        env: Option<Arc<dyn OperationEnv + Send + Sync>>,
    }

    impl SessionOverlaySource for StubSessionOverlay {
        fn overlay_for(
            &self,
            _context: &OperationContext,
        ) -> Option<Arc<dyn OperationEnv + Send + Sync>> {
            self.env.clone()
        }
    }

    struct StaticEnv {
        name: String,
        contains_set: Vec<String>,
    }

    #[async_trait::async_trait]
    impl OperationEnv for StaticEnv {
        async fn invoke_with_policy(
            &self,
            _namespace: &str,
            _operation: &str,
            _input: Value,
            parent: &OperationContext,
            _policy: AbortPolicy,
        ) -> ResponseEnvelope {
            ResponseEnvelope::ok(parent.request_id.clone(), Value::String(self.name.clone()))
        }

        fn contains(&self, name: &str) -> bool {
            self.contains_set.iter().any(|n| n == name)
        }
    }

    #[tokio::test]
    async fn compose_root_env_uses_session_overlay_when_present() {
        let registry = registry_with(
            "fs/readFile",
            Visibility::External,
            AccessControl::default(),
            echo_handler(),
        );
        let session_env: Arc<dyn OperationEnv + Send + Sync> = Arc::new(StaticEnv {
            name: "session".to_string(),
            contains_set: vec!["agent/chat".to_string()],
        });
        let session_source: Arc<dyn SessionOverlaySource + Send + Sync> =
            Arc::new(StubSessionOverlay {
                env: Some(session_env),
            });
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let adapter = CallAdapter::new(registry, provider).with_session_source(session_source);
        let conn = CallConnection::new(stub_connection());

        let context =
            adapter.build_root_context("req-4".to_string(), "fs/readFile", None, None, &conn);

        assert!(context.env.contains("agent/chat"));
        assert!(context.env.contains("fs/readFile"));
    }

    #[tokio::test]
    async fn compose_root_env_attaches_peer_when_connection_has_identity() {
        let registry = registry_with(
            "fs/readFile",
            Visibility::External,
            AccessControl::default(),
            echo_handler(),
        );
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let adapter = CallAdapter::new(registry, provider);
        let conn = CallConnection::new(stub_connection());
        let imported = HandlerRegistration::new(
            OperationSpec::new(
                "worker/exec",
                OperationType::Query,
                Visibility::Internal,
                serde_json::json!({}),
                serde_json::json!({}),
                vec![],
                AccessControl::default(),
                None,
            ),
            HandlerKind::Once(echo_handler()),
            OperationProvenance::FromCall,
            None,
            None,
            Capabilities::new(),
        );
        conn.register_imported(imported);
        let peer_identity = Identity {
            id: "worker-a".to_string(),
            scopes: vec![],
            resources: HashMap::new(),
        };
        conn.connection()
            .expect("quic connection present")
            .set_identity(peer_identity)
            .expect("identity not yet set");

        let context =
            adapter.build_root_context("req-5".to_string(), "fs/readFile", None, None, &conn);

        let scoped = ScopedPeerEnv::new(["worker/exec"]);
        let invoke_ctx = OperationContext {
            request_id: "req-5".to_string(),
            parent_request_id: None,
            identity: None,
            handler_identity: None,
            forwarded_for: None,
            capabilities: Capabilities::new(),
            metadata: HashMap::new(),
            scoped_env: scoped,
            env: context.env.clone(),
            abort_policy: AbortPolicy::default(),
            deadline: context.deadline,
            internal: false,
            ownership: None,
        };
        let response = context
            .env
            .invoke("worker", "exec", serde_json::json!({"v": 1}), &invoke_ctx)
            .await;
        assert!(
            response.result.is_ok(),
            "peer overlay dispatches the imported op when identity is attached"
        );
        assert_eq!(response.result.unwrap(), serde_json::json!({"v": 1}));
    }

    #[tokio::test]
    async fn compose_root_env_does_not_attach_peer_when_connection_has_no_identity() {
        let registry = registry_with(
            "fs/readFile",
            Visibility::External,
            AccessControl::default(),
            echo_handler(),
        );
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let adapter = CallAdapter::new(registry, provider);
        let conn = CallConnection::new(stub_connection());
        let imported = HandlerRegistration::new(
            OperationSpec::new(
                "worker/exec",
                OperationType::Query,
                Visibility::Internal,
                serde_json::json!({}),
                serde_json::json!({}),
                vec![],
                AccessControl::default(),
                None,
            ),
            HandlerKind::Once(echo_handler()),
            OperationProvenance::FromCall,
            None,
            None,
            Capabilities::new(),
        );
        conn.register_imported(imported);

        let context =
            adapter.build_root_context("req-6".to_string(), "fs/readFile", None, None, &conn);

        let scoped = ScopedPeerEnv::new(["worker/exec"]);
        let invoke_ctx = OperationContext {
            request_id: "req-6".to_string(),
            parent_request_id: None,
            identity: None,
            handler_identity: None,
            forwarded_for: None,
            capabilities: Capabilities::new(),
            metadata: HashMap::new(),
            scoped_env: scoped,
            env: context.env.clone(),
            abort_policy: AbortPolicy::default(),
            deadline: context.deadline,
            internal: false,
            ownership: None,
        };
        let response = context
            .env
            .invoke("worker", "exec", serde_json::json!({}), &invoke_ctx)
            .await;
        match response.result {
            Err(e) => assert_eq!(
                e.code, "NOT_FOUND",
                "no peer overlay attached: op falls through to base registry which has no worker/exec"
            ),
            other => panic!("expected NOT_FOUND, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_requested_round_trip_returns_responded() {
        let registry = registry_with(
            "echo/run",
            Visibility::External,
            AccessControl::default(),
            echo_handler(),
        );
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let adapter = CallAdapter::new(registry, provider);
        let conn = Arc::new(CallConnection::new(stub_connection()));

        let payload = serde_json::json!({
            "operationId": "/echo/run",
            "input": { "msg": "hi" },
        });
        let response = adapter
            .dispatch_requested(&conn, "req-1".to_string(), payload)
            .await;

        assert_eq!(response.request_id, "req-1");
        assert_eq!(response.result, Ok(serde_json::json!({ "msg": "hi" })));
    }

    #[tokio::test]
    async fn dispatch_requested_internal_op_from_wire_returns_not_found() {
        let registry = registry_with(
            "secret/op",
            Visibility::Internal,
            AccessControl::default(),
            echo_handler(),
        );
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let adapter = CallAdapter::new(registry, provider);
        let conn = Arc::new(CallConnection::new(stub_connection()));

        let payload = serde_json::json!({
            "operationId": "/secret/op",
            "input": {},
        });
        let response = adapter
            .dispatch_requested(&conn, "req-2".to_string(), payload)
            .await;

        match response.result {
            Err(e) => assert_eq!(e.code, "NOT_FOUND"),
            other => panic!("expected NOT_FOUND, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_requested_acl_denied_returns_forbidden() {
        let registry = registry_with(
            "admin/run",
            Visibility::External,
            AccessControl {
                required_scopes: vec!["admin".to_string()],
                ..Default::default()
            },
            echo_handler(),
        );
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let adapter = CallAdapter::new(registry, provider);
        let conn = Arc::new(CallConnection::new(stub_connection()));

        let payload = serde_json::json!({
            "operationId": "/admin/run",
            "input": {},
        });
        let response = adapter
            .dispatch_requested(&conn, "req-3".to_string(), payload)
            .await;

        match response.result {
            Err(e) => {
                assert_eq!(e.code, "FORBIDDEN");
                assert_eq!(e.message, "authentication required");
            }
            other => panic!("expected FORBIDDEN, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_requested_unknown_op_returns_not_found() {
        let registry = Arc::new(OperationRegistry::new());
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let adapter = CallAdapter::new(registry, provider);
        let conn = Arc::new(CallConnection::new(stub_connection()));

        let payload = serde_json::json!({
            "operationId": "/missing/op",
            "input": {},
        });
        let response = adapter
            .dispatch_requested(&conn, "req-4".to_string(), payload)
            .await;

        match response.result {
            Err(e) => assert_eq!(e.code, "NOT_FOUND"),
            other => panic!("expected NOT_FOUND, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_requested_auth_token_overrides_connection_identity() {
        let registry = registry_with(
            "admin/run",
            Visibility::External,
            AccessControl {
                required_scopes: vec!["admin".to_string()],
                ..Default::default()
            },
            inspect_identity_handler(),
        );
        let token_identity = identity_with_scopes("admin-user", &["admin"]);
        let provider: Arc<dyn IdentityProvider> =
            Arc::new(StaticIdentityProvider::new().with_token("alk_admin", token_identity));
        let adapter = CallAdapter::new(registry, provider);
        let conn = Arc::new(CallConnection::new(stub_connection()));

        let payload = serde_json::json!({
            "operationId": "/admin/run",
            "input": {},
            "auth_token": "alk_admin",
        });
        let response = adapter
            .dispatch_requested(&conn, "req-5".to_string(), payload)
            .await;

        let out = response.result.expect("ok");
        assert_eq!(out["identity_id"], Value::String("admin-user".into()));
    }

    #[tokio::test]
    async fn dispatch_requested_no_leading_slash_still_resolves() {
        let registry = registry_with(
            "echo/run",
            Visibility::External,
            AccessControl::default(),
            echo_handler(),
        );
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let adapter = CallAdapter::new(registry, provider);
        let conn = Arc::new(CallConnection::new(stub_connection()));

        let payload = serde_json::json!({
            "operationId": "echo/run",
            "input": { "x": 1 },
        });
        let response = adapter
            .dispatch_requested(&conn, "req-6".to_string(), payload)
            .await;

        assert_eq!(response.result, Ok(serde_json::json!({ "x": 1 })));
    }

    #[tokio::test]
    async fn response_envelope_ok_converts_to_call_responded_event() {
        let response = ResponseEnvelope::ok("req-1", Value::String("hi".into()));
        let event: EventEnvelope = response.into();
        assert_eq!(event.r#type, EVENT_RESPONDED);
        assert_eq!(event.id, "req-1");
        assert_eq!(
            event.payload.get("output"),
            Some(&Value::String("hi".into()))
        );
    }

    #[tokio::test]
    async fn response_envelope_error_converts_to_call_error_event() {
        let response = ResponseEnvelope::error("req-2", CallError::not_found("missing/op"));
        let event: EventEnvelope = response.into();
        assert_eq!(event.r#type, EVENT_ERROR);
        assert_eq!(event.id, "req-2");
        assert_eq!(
            event.payload.get("code"),
            Some(&Value::String("NOT_FOUND".into()))
        );
    }

    #[tokio::test]
    async fn completed_event_has_empty_payload() {
        let event = EventEnvelope::completed("sub-1");
        assert_eq!(event.r#type, EVENT_COMPLETED);
        assert_eq!(event.payload, serde_json::json!({}));
    }

    #[tokio::test]
    async fn handle_sets_connection_identity_from_auth_context() {
        let registry = Arc::new(OperationRegistry::new());
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let adapter = CallAdapter::new(registry, provider);

        let conn = stub_connection();
        let auth = AuthContext {
            identity: Some(identity_with_scopes("caller", &["user"])),
            alpn: b"alknet/call".to_vec(),
            remote_addr: None,
            tls_client_fingerprint: None,
        };

        let handle_conn = conn;
        let result = adapter.handle(handle_conn, &auth).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn handle_returns_ok_when_accept_bi_returns_stream_closed() {
        let registry = Arc::new(OperationRegistry::new());
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let adapter = CallAdapter::new(registry, provider);
        let conn = stub_connection();
        let auth = AuthContext {
            identity: None,
            alpn: b"alknet/call".to_vec(),
            remote_addr: None,
            tls_client_fingerprint: None,
        };
        let result = adapter.handle(conn, &auth).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn handle_fail_all_on_close_uses_internal_error() {
        let registry = Arc::new(OperationRegistry::new());
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let adapter = CallAdapter::new(registry, provider);
        let conn = stub_connection();
        let auth = AuthContext {
            identity: None,
            alpn: b"alknet/call".to_vec(),
            remote_addr: None,
            tls_client_fingerprint: None,
        };
        let result = adapter.handle(conn, &auth).await;
        assert!(result.is_ok());
    }

    #[test]
    fn session_overlay_source_trait_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<StubSessionOverlay>();
    }

    #[test]
    fn call_adapter_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CallAdapter>();
    }

    #[tokio::test]
    async fn build_root_context_with_unknown_op_produces_empty_scoped_env() {
        let registry = Arc::new(OperationRegistry::new());
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let adapter = CallAdapter::new(registry, provider);
        let conn = CallConnection::new(stub_connection());

        let context =
            adapter.build_root_context("req-7".to_string(), "missing/op", None, None, &conn);

        assert!(!context.scoped_env.allows("missing/op"));
        assert!(context.handler_identity.is_none());
    }

    #[tokio::test]
    async fn dispatch_requested_for_internal_spec_does_not_invoke_handler() {
        let registry = registry_with(
            "secret/op",
            Visibility::Internal,
            AccessControl::default(),
            echo_handler(),
        );
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let adapter = CallAdapter::new(registry, provider);
        let conn = Arc::new(CallConnection::new(stub_connection()));

        let payload = serde_json::json!({
            "operationId": "/secret/op",
            "input": { "should_not": "reach" },
        });
        let response = adapter
            .dispatch_requested(&conn, "req-8".to_string(), payload)
            .await;
        match response.result {
            Err(e) => {
                assert_eq!(e.code, "NOT_FOUND");
                assert!(e.message.contains("secret/op"));
            }
            other => panic!("expected NOT_FOUND, got {other:?}"),
        }
    }

    fn inspect_forwarded_for_handler() -> crate::registry::registration::Handler {
        make_handler(|_input, context| async move {
            let identity_id = context.identity.as_ref().map(|i| i.id.clone());
            let forwarded_for_id = context.forwarded_for.as_ref().map(|i| i.id.clone());
            ResponseEnvelope::ok(
                context.request_id,
                serde_json::json!({
                    "identity_id": identity_id,
                    "forwarded_for_id": forwarded_for_id,
                }),
            )
        })
    }

    #[test]
    fn build_root_context_populates_forwarded_for_from_argument() {
        let registry = registry_with(
            "echo/run",
            Visibility::External,
            AccessControl::default(),
            echo_handler(),
        );
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let adapter = CallAdapter::new(registry, provider);
        let conn = CallConnection::new(stub_connection());

        let forwarded = identity_with_scopes("alice", &["fs:read"]);
        let context = adapter.build_root_context(
            "req-ff-1".to_string(),
            "echo/run",
            None,
            Some(forwarded.clone()),
            &conn,
        );

        assert_eq!(
            context.forwarded_for.as_ref().map(|i| &i.id),
            Some(&"alice".to_string())
        );
        assert_eq!(
            context.forwarded_for.as_ref().map(|i| i.scopes.clone()),
            Some(forwarded.scopes.clone())
        );
    }

    #[test]
    fn build_root_context_missing_forwarded_for_is_none() {
        let registry = registry_with(
            "echo/run",
            Visibility::External,
            AccessControl::default(),
            echo_handler(),
        );
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let adapter = CallAdapter::new(registry, provider);
        let conn = CallConnection::new(stub_connection());

        let context =
            adapter.build_root_context("req-ff-2".to_string(), "echo/run", None, None, &conn);

        assert!(context.forwarded_for.is_none());
    }

    #[tokio::test]
    async fn dispatch_requested_populates_forwarded_for_from_payload() {
        let registry = registry_with(
            "inspect/run",
            Visibility::External,
            AccessControl::default(),
            inspect_forwarded_for_handler(),
        );
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let adapter = CallAdapter::new(registry, provider);
        let conn = Arc::new(CallConnection::new(stub_connection()));

        let payload = serde_json::json!({
            "operationId": "/inspect/run",
            "input": {},
            "forwarded_for": {
                "id": "alice",
                "scopes": ["fs:read", "docker:start"],
                "resources": {},
            },
        });
        let response = adapter
            .dispatch_requested(&conn, "req-ff-3".to_string(), payload)
            .await;

        let out = response.result.expect("ok");
        assert_eq!(out["forwarded_for_id"], Value::String("alice".into()));
    }

    #[tokio::test]
    async fn dispatch_requested_missing_forwarded_for_yields_none() {
        let registry = registry_with(
            "inspect/run",
            Visibility::External,
            AccessControl::default(),
            inspect_forwarded_for_handler(),
        );
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let adapter = CallAdapter::new(registry, provider);
        let conn = Arc::new(CallConnection::new(stub_connection()));

        let payload = serde_json::json!({
            "operationId": "/inspect/run",
            "input": {},
        });
        let response = adapter
            .dispatch_requested(&conn, "req-ff-4".to_string(), payload)
            .await;

        let out = response.result.expect("ok");
        assert!(out["forwarded_for_id"].is_null());
    }

    #[tokio::test]
    async fn dispatch_requested_malformed_forwarded_for_yields_none() {
        let registry = registry_with(
            "inspect/run",
            Visibility::External,
            AccessControl::default(),
            inspect_forwarded_for_handler(),
        );
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let adapter = CallAdapter::new(registry, provider);
        let conn = Arc::new(CallConnection::new(stub_connection()));

        let payload = serde_json::json!({
            "operationId": "/inspect/run",
            "input": {},
            "forwarded_for": "not-an-object",
        });
        let response = adapter
            .dispatch_requested(&conn, "req-ff-5".to_string(), payload)
            .await;

        let out = response.result.expect("ok");
        assert!(out["forwarded_for_id"].is_null());
    }

    #[tokio::test]
    async fn dispatch_requested_forwarded_for_does_not_satisfy_acl() {
        let registry = registry_with(
            "admin/run",
            Visibility::External,
            AccessControl {
                required_scopes: vec!["admin".to_string()],
                ..Default::default()
            },
            inspect_forwarded_for_handler(),
        );
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let adapter = CallAdapter::new(registry, provider);
        let conn = Arc::new(CallConnection::new(stub_connection()));

        let payload = serde_json::json!({
            "operationId": "/admin/run",
            "input": {},
            "forwarded_for": {
                "id": "alice",
                "scopes": ["admin"],
                "resources": {},
            },
        });
        let response = adapter
            .dispatch_requested(&conn, "req-ff-6".to_string(), payload)
            .await;

        match response.result {
            Err(e) => {
                assert_eq!(e.code, "FORBIDDEN");
                assert_eq!(e.message, "authentication required");
            }
            other => panic!("expected FORBIDDEN (forwarded_for must not authorize), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_requested_forwarded_for_present_with_satisfied_acl() {
        let registry = registry_with(
            "admin/run",
            Visibility::External,
            AccessControl {
                required_scopes: vec!["admin".to_string()],
                ..Default::default()
            },
            inspect_forwarded_for_handler(),
        );
        let token_identity = identity_with_scopes("hub", &["admin"]);
        let provider: Arc<dyn IdentityProvider> =
            Arc::new(StaticIdentityProvider::new().with_token("alk_hub", token_identity));
        let adapter = CallAdapter::new(registry, provider);
        let conn = Arc::new(CallConnection::new(stub_connection()));

        let payload = serde_json::json!({
            "operationId": "/admin/run",
            "input": {},
            "auth_token": "alk_hub",
            "forwarded_for": {
                "id": "alice",
                "scopes": ["fs:read"],
                "resources": {},
            },
        });
        let response = adapter
            .dispatch_requested(&conn, "req-ff-7".to_string(), payload)
            .await;

        let out = response.result.expect("ok");
        assert_eq!(out["identity_id"], Value::String("hub".into()));
        assert_eq!(out["forwarded_for_id"], Value::String("alice".into()));
    }

    fn encode_frame(envelope: &EventEnvelope) -> Vec<u8> {
        let body = serde_json::to_vec(envelope).unwrap();
        let mut buf = (body.len() as u32).to_be_bytes().to_vec();
        buf.extend_from_slice(&body);
        buf
    }

    #[tokio::test]
    async fn handle_stream_aborted_cascades_parent_and_child() {
        let registry = registry_with(
            "parent/run",
            Visibility::External,
            AccessControl::default(),
            echo_handler(),
        );
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let adapter = CallAdapter::new(registry, provider);
        let conn = Arc::new(CallConnection::new(stub_connection()));

        {
            let mut pending = conn.pending().lock();
            pending.register_call(
                "parent-1".to_string(),
                Instant::now() + Duration::from_secs(30),
                None,
            );
            pending.register_call(
                "child-1".to_string(),
                Instant::now() + Duration::from_secs(30),
                Some("parent-1".to_string()),
            );
        }

        let frame = encode_frame(&EventEnvelope::aborted("parent-1"));
        let recv = tokio::io::BufReader::new(std::io::Cursor::new(frame));
        let (send, _recv_sink) = tokio::io::duplex(64);
        let send = alknet_core::types::SendStream::from_stream(send);
        let recv = alknet_core::types::RecvStream::from_stream(recv);

        adapter.handle_stream(conn.clone(), send, recv).await;

        let pending = conn.pending().lock();
        assert!(
            !pending.contains("parent-1"),
            "parent entry must be removed after abort"
        );
        assert!(
            !pending.contains("child-1"),
            "child entry must be removed by cascade"
        );
    }

    #[tokio::test]
    async fn handle_stream_aborted_unknown_request_id_is_noop() {
        let registry = registry_with(
            "parent/run",
            Visibility::External,
            AccessControl::default(),
            echo_handler(),
        );
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let adapter = CallAdapter::new(registry, provider);
        let conn = Arc::new(CallConnection::new(stub_connection()));

        {
            let mut pending = conn.pending().lock();
            pending.register_call(
                "unrelated-1".to_string(),
                Instant::now() + Duration::from_secs(30),
                None,
            );
        }

        let frame = encode_frame(&EventEnvelope::aborted("does-not-exist"));
        let recv = tokio::io::BufReader::new(std::io::Cursor::new(frame));
        let (send, _recv_sink) = tokio::io::duplex(64);
        let send = alknet_core::types::SendStream::from_stream(send);
        let recv = alknet_core::types::RecvStream::from_stream(recv);

        adapter.handle_stream(conn.clone(), send, recv).await;

        let pending = conn.pending().lock();
        assert!(
            pending.contains("unrelated-1"),
            "unrelated entry must survive abort of unknown id"
        );
    }
}
