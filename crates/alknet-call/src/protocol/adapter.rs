//! `CallAdapter`: implements `ProtocolHandler` for ALPN `alknet/call`.
//!
//! Accepts bidirectional streams, reads `EventEnvelope` frames, and
//! dispatches `call.requested` events to the operation registry. See
//! `docs/architecture/crates/call/call-protocol.md` for the full
//! specification.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alknet_core::auth::{AuthContext, AuthToken, Identity, IdentityProvider};
use alknet_core::types::{Connection, HandlerError, ProtocolHandler, StreamError};
use async_trait::async_trait;
use serde_json::Value;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use super::connection::CallConnection;
use super::wire::{
    CallError, EventEnvelope, FrameFramedReader, FrameFramedWriter, ResponseEnvelope,
    EVENT_REQUESTED,
};
use crate::registry::context::{AbortPolicy, OperationContext, ScopedOperationEnv};
use crate::registry::env::{CompositeOperationEnv, LocalOperationEnv, OperationEnv};
use crate::registry::registration::OperationRegistry;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const SWEEPER_INTERVAL: Duration = Duration::from_secs(10);

pub struct CallAdapter {
    registry: Arc<OperationRegistry>,
    identity_provider: Arc<dyn IdentityProvider>,
    session_source: Option<Arc<dyn SessionOverlaySource + Send + Sync>>,
    default_timeout: Duration,
}

pub trait SessionOverlaySource: Send + Sync {
    fn overlay_for(
        &self,
        context: &OperationContext,
    ) -> Option<Arc<dyn OperationEnv + Send + Sync>>;
}

impl CallAdapter {
    pub fn new(
        registry: Arc<OperationRegistry>,
        identity_provider: Arc<dyn IdentityProvider>,
    ) -> Self {
        Self {
            registry,
            identity_provider,
            session_source: None,
            default_timeout: DEFAULT_TIMEOUT,
        }
    }

    pub fn with_session_source(
        mut self,
        source: Arc<dyn SessionOverlaySource + Send + Sync>,
    ) -> Self {
        self.session_source = Some(source);
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    pub fn registry(&self) -> &Arc<OperationRegistry> {
        &self.registry
    }

    pub fn identity_provider(&self) -> &Arc<dyn IdentityProvider> {
        &self.identity_provider
    }

    pub fn default_timeout(&self) -> Duration {
        self.default_timeout
    }

    fn strip_leading_slash(operation_id: &str) -> &str {
        operation_id.strip_prefix('/').unwrap_or(operation_id)
    }

    fn resolve_identity(
        &self,
        connection_identity: Option<Identity>,
        payload: &Value,
    ) -> Option<Identity> {
        let auth_token = payload.get("auth_token").and_then(|v| v.as_str());
        match auth_token {
            Some(token_str) => {
                let token = AuthToken {
                    raw: token_str.as_bytes().to_vec(),
                };
                match self.identity_provider.resolve_from_token(&token) {
                    Some(identity) => Some(identity),
                    None => connection_identity,
                }
            }
            None => connection_identity,
        }
    }

    fn compose_root_env(
        &self,
        connection: &CallConnection,
        context: &OperationContext,
    ) -> Arc<dyn OperationEnv + Send + Sync> {
        let base: Arc<dyn OperationEnv + Send + Sync> =
            Arc::new(LocalOperationEnv::new(Arc::clone(&self.registry)));
        let session = self
            .session_source
            .as_ref()
            .and_then(|s| s.overlay_for(context));
        let connection_overlay = connection.overlay_env();
        Arc::new(CompositeOperationEnv::new(
            base,
            Some(connection_overlay),
            session,
        ))
    }

    fn build_root_context(
        &self,
        request_id: String,
        operation_name: &str,
        identity: Option<Identity>,
        connection: &CallConnection,
    ) -> OperationContext {
        let registration = self.registry.registration(operation_name);
        let (composition_authority, capabilities, scoped_env) = match registration {
            Some(r) => (
                r.composition_authority.clone(),
                r.capabilities.clone(),
                r.scoped_env
                    .clone()
                    .unwrap_or_else(ScopedOperationEnv::empty),
            ),
            None => (
                None,
                alknet_core::types::Capabilities::new(),
                ScopedOperationEnv::empty(),
            ),
        };

        let stub_env: Arc<dyn OperationEnv + Send + Sync> =
            Arc::new(LocalOperationEnv::new(Arc::clone(&self.registry)));
        let mut context = OperationContext {
            request_id,
            parent_request_id: None,
            identity: identity.clone(),
            handler_identity: composition_authority,
            capabilities,
            metadata: HashMap::new(),
            deadline: Some(Instant::now() + self.default_timeout),
            scoped_env,
            env: stub_env,
            abort_policy: AbortPolicy::default(),
            internal: false,
        };
        context.env = self.compose_root_env(connection, &context);
        context
    }

    async fn dispatch_requested(
        &self,
        connection: &Arc<CallConnection>,
        request_id: String,
        payload: Value,
    ) -> ResponseEnvelope {
        let operation_id = payload
            .get("operationId")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let operation_name = Self::strip_leading_slash(operation_id).to_string();

        let connection_identity = connection.connection().identity().cloned();
        let identity = self.resolve_identity(connection_identity, &payload);

        let input = payload.get("input").cloned().unwrap_or(Value::Null);

        let context =
            self.build_root_context(request_id.clone(), &operation_name, identity, connection);

        self.registry.invoke(&operation_name, input, context).await
    }

    async fn handle_stream(
        &self,
        connection: Arc<CallConnection>,
        send: alknet_core::types::SendStream,
        recv: alknet_core::types::RecvStream,
    ) {
        let mut reader = FrameFramedReader::new(recv);
        let mut writer = FrameFramedWriter::new(send);

        loop {
            let envelope = match reader.read_frame().await {
                Ok(env) => env,
                Err(super::wire::FrameError::ConnectionClosed) => break,
                Err(err) => {
                    warn!(error = %err, "stream frame read error; closing stream");
                    break;
                }
            };

            if envelope.r#type != EVENT_REQUESTED {
                debug!(event_type = %envelope.r#type, id = %envelope.id, "ignoring non-requested event on inbound stream");
                continue;
            }

            let request_id = envelope.id.clone();
            let payload = envelope.payload.clone();

            let response = self
                .dispatch_requested(&connection, request_id.clone(), payload)
                .await;

            let event: EventEnvelope = response.into();
            if let Err(err) = writer.write_frame(&event).await {
                warn!(error = %err, "failed to write response frame; closing stream");
                break;
            }
        }
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
        let pending = Arc::clone(call_connection.pending());

        let sweeper_pending = Arc::clone(&pending);
        let sweeper_handle: JoinHandle<()> = tokio::spawn(async move {
            let mut interval = tokio::time::interval(SWEEPER_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let evicted = sweeper_pending.lock().evict_expired();
                if !evicted.is_empty() {
                    debug!(
                        count = evicted.len(),
                        "sweeper evicted expired pending entries"
                    );
                }
            }
        });

        loop {
            match call_connection.connection().accept_bi().await {
                Ok((send, recv)) => {
                    let conn = Arc::clone(&call_connection);
                    let adapter_registry = Arc::clone(&self.registry);
                    let adapter_identity = Arc::clone(&self.identity_provider);
                    let adapter_session = self.session_source.clone();
                    let adapter_timeout = self.default_timeout;
                    tokio::spawn(async move {
                        let adapter = CallAdapter {
                            registry: adapter_registry,
                            identity_provider: adapter_identity,
                            session_source: adapter_session,
                            default_timeout: adapter_timeout,
                        };
                        adapter.handle_stream(conn, send, recv).await;
                    });
                }
                Err(StreamError::ConnectionClosed) => break,
                Err(StreamError::StreamClosed) => break,
                Err(StreamError::Timeout) => break,
                Err(err) => {
                    warn!(error = %err, "accept_bi error; stopping accept loop");
                    break;
                }
            }
        }

        let failed = pending
            .lock()
            .fail_all(CallError::internal("connection closed"));
        if !failed.is_empty() {
            debug!(
                count = failed.len(),
                "failed pending requests on connection close"
            );
        }

        sweeper_handle.abort();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::wire::{EVENT_COMPLETED, EVENT_ERROR, EVENT_RESPONDED};
    use crate::registry::registration::{make_handler, HandlerRegistration, OperationProvenance};
    use crate::registry::spec::{AccessControl, OperationSpec, OperationType, Visibility};
    use alknet_core::auth::AuthToken;
    use alknet_core::types::Capabilities;
    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
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

    fn internal_spec(name: &str) -> OperationSpec {
        OperationSpec::new(
            name,
            OperationType::Query,
            Visibility::Internal,
            serde_json::json!({}),
            serde_json::json!({}),
            vec![],
            AccessControl::default(),
        )
    }

    fn registry_with(
        name: &str,
        visibility: Visibility,
        acl: AccessControl,
        handler: crate::registry::registration::Handler,
    ) -> Arc<OperationRegistry> {
        let mut registry = OperationRegistry::new();
        registry.register(HandlerRegistration::new(
            OperationSpec::new(
                name,
                OperationType::Query,
                visibility,
                serde_json::json!({}),
                serde_json::json!({}),
                vec![],
                acl,
            ),
            handler,
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
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

    struct StubConnection {
        alpn: &'static [u8],
        addr: Option<SocketAddr>,
        closed: StdMutex<Option<(u32, String)>>,
    }

    impl alknet_core::types::MockConnection for StubConnection {
        fn remote_alpn(&self) -> &[u8] {
            self.alpn
        }
        fn remote_addr(&self) -> Option<SocketAddr> {
            self.addr
        }
        fn close(&self, code: u32, reason: &str) {
            *self.closed.lock().unwrap() = Some((code, reason.to_string()));
        }
    }

    fn stub_connection() -> Connection {
        Connection::from_mock(Arc::new(StubConnection {
            alpn: b"alknet/call",
            addr: Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4321)),
            closed: StdMutex::new(None),
        }))
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
        assert!(adapter.session_source.is_none());
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

        let context = adapter.build_root_context("req-1".to_string(), "echo/run", None, &conn);

        assert!(!context.is_internal());
        assert!(context.parent_request_id.is_none());
        assert!(context.deadline.is_some());
    }

    #[tokio::test]
    async fn build_root_context_carries_capabilities_and_scoped_env() {
        let mut registry = OperationRegistry::new();
        let scoped = ScopedOperationEnv::new(["fs/readFile"]);
        let caps = Capabilities::new().with_api_key("google", "k".to_string());
        registry.register(HandlerRegistration::new(
            external_spec("agent/run", AccessControl::default()),
            echo_handler(),
            OperationProvenance::Local,
            None,
            Some(scoped.clone()),
            caps.clone(),
        ));
        let registry = Arc::new(registry);
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let adapter = CallAdapter::new(registry, provider);
        let conn = CallConnection::new(stub_connection());

        let context = adapter.build_root_context("req-2".to_string(), "agent/run", None, &conn);

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

        let context = adapter.build_root_context("req-3".to_string(), "fs/readFile", None, &conn);

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

        let context = adapter.build_root_context("req-4".to_string(), "fs/readFile", None, &conn);

        assert!(context.env.contains("agent/chat"));
        assert!(context.env.contains("fs/readFile"));
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

        let context = adapter.build_root_context("req-7".to_string(), "missing/op", None, &conn);

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
}
