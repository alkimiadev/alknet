//! Shared dispatch loop for `alknet/call` connections.
//!
//! Both [`CallAdapter`]'s accept path and [`crate::client::CallClient`]'s
//! connect path produce a [`CallConnection`] and hand it to the same dispatch
//! loop here (ADR-017 §1): the loop reads `EventEnvelope` frames off accepted
//! bidirectional streams, dispatches `call.requested` events against the
//! operation registry, and writes the response back on the same stream. The
//! connection-establishment half differs (accept vs dial); the dispatch half
//! is shared.
//!
//! See `docs/architecture/crates/call/call-protocol.md` and
//! `docs/architecture/crates/call/client-and-adapters.md` for the spec.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alknet_core::auth::{AuthToken, Identity, IdentityProvider};
use alknet_core::types::StreamError;
use serde_json::Value;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use super::abort::AbortCascade;
use super::connection::CallConnection;
use super::wire::{
    CallError, EventEnvelope, FrameFramedReader, FrameFramedWriter, ResponseEnvelope,
    EVENT_ABORTED, EVENT_REQUESTED,
};
use crate::protocol::adapter::SessionOverlaySource;
use crate::registry::context::{AbortPolicy, OperationContext, ScopedPeerEnv};
use crate::registry::env::{LocalOperationEnv, OperationEnv, PeerCompositeEnv};
use crate::registry::registration::OperationRegistry;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const SWEEPER_INTERVAL: Duration = Duration::from_secs(10);

/// Shared dispatcher for an established `CallConnection`. Constructed by
/// both `CallAdapter` (accept path) and `CallClient` (connect path) and used
/// to run the dispatch loop. Holds no per-connection state; the
/// `CallConnection` is passed into `run_loop`.
pub struct Dispatcher {
    pub registry: Arc<OperationRegistry>,
    pub identity_provider: Arc<dyn IdentityProvider>,
    pub session_source: Option<Arc<dyn SessionOverlaySource + Send + Sync>>,
    pub default_timeout: Duration,
}

impl Dispatcher {
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

    fn strip_leading_slash(operation_id: &str) -> &str {
        operation_id.strip_prefix('/').unwrap_or(operation_id)
    }

    pub(crate) fn resolve_identity(
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

    pub fn compose_root_env(
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

        let mut env = PeerCompositeEnv::new(base);
        if let Some(session) = session {
            env = env.with_session(session);
        }
        if let Some(peer_id) = connection.identity().map(|identity| identity.id.clone()) {
            env.attach_peer(peer_id, connection.overlay_env());
        }
        Arc::new(env)
    }

    pub(crate) fn build_root_context(
        &self,
        request_id: String,
        operation_name: &str,
        identity: Option<Identity>,
        forwarded_for: Option<Identity>,
        connection: &CallConnection,
    ) -> OperationContext {
        let registration = self.registry.registration(operation_name);
        let (composition_authority, capabilities, scoped_env) = match registration {
            Some(r) => (
                r.composition_authority.clone(),
                r.capabilities.clone(),
                r.scoped_env.clone().unwrap_or_else(ScopedPeerEnv::empty),
            ),
            None => (
                None,
                alknet_core::types::Capabilities::new(),
                ScopedPeerEnv::empty(),
            ),
        };

        let stub_env: Arc<dyn OperationEnv + Send + Sync> =
            Arc::new(LocalOperationEnv::new(Arc::clone(&self.registry)));
        let mut context = OperationContext {
            request_id,
            parent_request_id: None,
            identity: identity.clone(),
            handler_identity: composition_authority,
            forwarded_for,
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

    pub async fn dispatch_requested(
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

        let connection_identity = connection.identity().cloned();
        let identity = self.resolve_identity(connection_identity, &payload);
        let forwarded_for = payload
            .get("forwarded_for")
            .and_then(|v| serde_json::from_value::<Identity>(v.clone()).ok());

        let input = payload.get("input").cloned().unwrap_or(Value::Null);

        let context = self.build_root_context(
            request_id.clone(),
            &operation_name,
            identity,
            forwarded_for,
            connection,
        );

        self.registry.invoke(&operation_name, input, context).await
    }

    pub async fn handle_abort(&self, connection: &Arc<CallConnection>, request_id: &str) {
        let mut pending = connection.pending().lock();
        let mut cascade = AbortCascade::new(&mut pending);
        let aborted = cascade.cascade_abort(request_id, AbortPolicy::AbortDependents);
        pending.handle_aborted(request_id);
        if !aborted.is_empty() {
            debug!(count = aborted.len(), "abort cascade evicted descendants");
        }
    }

    pub(crate) async fn handle_stream(
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

            match envelope.r#type.as_str() {
                EVENT_REQUESTED => {
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
                EVENT_ABORTED => {
                    let request_id = envelope.id.clone();
                    self.handle_abort(&connection, &request_id).await;
                }
                other => {
                    debug!(event_type = %other, id = %envelope.id, "ignoring non-requested/non-aborted event on inbound stream");
                }
            }
        }
    }

    /// Run the shared dispatch loop over an established `CallConnection`:
    /// spawn the pending-entry sweeper, accept bidirectional streams until the
    /// connection closes, dispatch each stream via `handle_stream`, and fail
    /// outstanding pending requests on close. Returns when the connection is
    /// closed (accept loop yields `ConnectionClosed`/`StreamClosed`/`Timeout`).
    pub async fn run_loop(self, connection: Arc<CallConnection>) {
        let pending = Arc::clone(connection.pending());

        let quic = match connection.connection() {
            Some(c) => Arc::clone(c),
            None => {
                warn!("run_loop called with an overlay-only CallConnection; returning");
                return;
            }
        };

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
            match quic.accept_bi().await {
                Ok((send, recv)) => {
                    let conn = Arc::clone(&connection);
                    let dispatcher = self.clone();
                    tokio::spawn(async move {
                        dispatcher.handle_stream(conn, send, recv).await;
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
    }
}

impl Clone for Dispatcher {
    fn clone(&self) -> Self {
        Self {
            registry: Arc::clone(&self.registry),
            identity_provider: Arc::clone(&self.identity_provider),
            session_source: self.session_source.clone(),
            default_timeout: self.default_timeout,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::wire::EVENT_RESPONDED;
    use crate::registry::registration::{make_handler, HandlerRegistration, OperationProvenance};
    use crate::registry::spec::{AccessControl, OperationSpec, OperationType, Visibility};
    use alknet_core::auth::{AuthToken, Identity, IdentityProvider};
    use alknet_core::types::{Capabilities, MockConnection};
    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::Mutex as StdMutex;

    struct StubConnection {
        alpn: &'static [u8],
        addr: Option<SocketAddr>,
        closed: StdMutex<Option<(u32, String)>>,
    }

    impl MockConnection for StubConnection {
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

    fn stub_connection() -> alknet_core::types::Connection {
        alknet_core::types::Connection::from_mock(Arc::new(StubConnection {
            alpn: b"alknet/call",
            addr: Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4321)),
            closed: StdMutex::new(None),
        }))
    }

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

    fn registry_with(name: &str, visibility: Visibility, acl: AccessControl) -> OperationRegistry {
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
            make_handler(|input, context| async move {
                ResponseEnvelope::ok(context.request_id, input)
            }),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        registry
    }

    fn dispatcher() -> Dispatcher {
        Dispatcher::new(
            Arc::new(OperationRegistry::new()),
            Arc::new(StaticIdentityProvider::new()),
        )
    }

    #[tokio::test]
    async fn dispatch_authorized_peer_dispatches_and_populates_capabilities() {
        let caps = Capabilities::new().with_api_key("google", "k".to_string());
        let mut registry = OperationRegistry::new();
        let handler = make_handler(|_input, context| async move {
            let has_google = context.capabilities.get("google").is_some();
            ResponseEnvelope::ok(
                context.request_id,
                serde_json::json!({ "has_google": has_google }),
            )
        });
        registry.register(HandlerRegistration::new(
            external_spec("admin/run", AccessControl::default()),
            handler,
            OperationProvenance::Local,
            None,
            None,
            caps,
        ));
        let registry = Arc::new(registry);
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = Dispatcher::new(registry, provider);
        let conn = Arc::new(CallConnection::new(stub_connection()));

        let payload = serde_json::json!({
            "operationId": "/admin/run",
            "input": {},
        });
        let response = dp
            .dispatch_requested(&conn, "req-1".to_string(), payload)
            .await;
        let out = response.result.expect("dispatch ok");
        assert_eq!(out["has_google"], Value::Bool(true));
    }

    #[tokio::test]
    async fn dispatch_unauthorized_peer_returns_forbidden_capabilities_never_populated() {
        let caps = Capabilities::new().with_api_key("google", "k".to_string());
        let mut registry = OperationRegistry::new();
        let handler = make_handler(|_input, context| async move {
            let has_google = context.capabilities.get("google").is_some();
            ResponseEnvelope::ok(
                context.request_id,
                serde_json::json!({ "has_google": has_google }),
            )
        });
        registry.register(HandlerRegistration::new(
            external_spec(
                "admin/run",
                AccessControl {
                    required_scopes: vec!["admin".to_string()],
                    ..Default::default()
                },
            ),
            handler,
            OperationProvenance::Local,
            None,
            None,
            caps,
        ));
        let registry = Arc::new(registry);
        let provider: Arc<dyn IdentityProvider> = Arc::new(
            StaticIdentityProvider::new()
                .with_token("alk_user", identity_with_scopes("regular-user", &["user"])),
        );
        let dp = Dispatcher::new(registry, provider);
        let conn = Arc::new(CallConnection::new(stub_connection()));

        let payload = serde_json::json!({
            "operationId": "/admin/run",
            "input": {},
            "auth_token": "alk_user",
        });
        let response = dp
            .dispatch_requested(&conn, "req-2".to_string(), payload)
            .await;
        match response.result {
            Err(e) => {
                assert_eq!(e.code, "FORBIDDEN");
                assert!(e.message.contains("admin"));
            }
            other => panic!("expected FORBIDDEN, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_internal_op_from_wire_returns_not_found_before_acl() {
        let registry = Arc::new(registry_with(
            "secret/op",
            Visibility::Internal,
            AccessControl::default(),
        ));
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = Dispatcher::new(registry, provider);
        let conn = Arc::new(CallConnection::new(stub_connection()));

        let payload = serde_json::json!({
            "operationId": "/secret/op",
            "input": {},
        });
        let response = dp
            .dispatch_requested(&conn, "req-3".to_string(), payload)
            .await;
        match response.result {
            Err(e) => {
                assert_eq!(e.code, "NOT_FOUND");
                assert!(e.message.contains("secret/op"));
            }
            other => panic!("expected NOT_FOUND, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_connection_with_no_identity_produces_no_peer_id_in_env() {
        let registry = Arc::new(registry_with(
            "fs/readFile",
            Visibility::External,
            AccessControl::default(),
        ));
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = Dispatcher::new(registry, provider);
        let conn = CallConnection::new(stub_connection());

        let context = dp.build_root_context("req-4".to_string(), "fs/readFile", None, None, &conn);

        assert!(
            context.identity.is_none(),
            "no connection identity → context.identity is None"
        );
        assert!(
            context.env.peer_ids().is_empty(),
            "no peer overlay attached when connection has no identity"
        );
    }

    #[tokio::test]
    async fn dispatch_connection_with_identity_attaches_peer_overlay_keyed_by_identity_id() {
        let registry = Arc::new(registry_with(
            "fs/readFile",
            Visibility::External,
            AccessControl::default(),
        ));
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = Dispatcher::new(registry, provider);
        let conn = CallConnection::new(stub_connection());
        conn.connection()
            .expect("quic connection present")
            .set_identity(identity_with_scopes("worker-a", &[]))
            .expect("identity not yet set");

        let context = dp.build_root_context("req-5".to_string(), "fs/readFile", None, None, &conn);

        assert_eq!(
            context.env.peer_ids(),
            vec!["worker-a".to_string()],
            "PeerId for connection comes from connection.identity().id"
        );
    }

    #[tokio::test]
    async fn dispatch_extract_forwarded_for_from_payload_into_context() {
        let mut registry = OperationRegistry::new();
        let handler = make_handler(|_input, context| async move {
            let forwarded_id = context.forwarded_for.as_ref().map(|i| i.id.clone());
            ResponseEnvelope::ok(
                context.request_id,
                serde_json::json!({ "forwarded_for_id": forwarded_id }),
            )
        });
        registry.register(HandlerRegistration::new(
            external_spec("fs/readFile", AccessControl::default()),
            handler,
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        let registry = Arc::new(registry);
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = Dispatcher::new(registry, provider);
        let conn = Arc::new(CallConnection::new(stub_connection()));

        let payload = serde_json::json!({
            "operationId": "/fs/readFile",
            "input": {},
            "forwarded_for": {
                "id": "alice",
                "scopes": ["fs:read"],
                "resources": {}
            },
        });
        let response = dp
            .dispatch_requested(&conn, "req-6".to_string(), payload)
            .await;
        let out = response.result.expect("ok");
        assert_eq!(out["forwarded_for_id"], Value::String("alice".into()));
    }

    #[tokio::test]
    async fn dispatch_without_forwarded_for_field_is_none() {
        let mut registry = OperationRegistry::new();
        let handler = make_handler(|_input, context| async move {
            let present = context.forwarded_for.is_some();
            ResponseEnvelope::ok(
                context.request_id,
                serde_json::json!({ "present": present }),
            )
        });
        registry.register(HandlerRegistration::new(
            external_spec("fs/readFile", AccessControl::default()),
            handler,
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        let registry = Arc::new(registry);
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = Dispatcher::new(registry, provider);
        let conn = Arc::new(CallConnection::new(stub_connection()));

        let payload = serde_json::json!({
            "operationId": "/fs/readFile",
            "input": {},
        });
        let response = dp
            .dispatch_requested(&conn, "req-7".to_string(), payload)
            .await;
        let out = response.result.expect("ok");
        assert_eq!(out["present"], Value::Bool(false));
    }

    #[tokio::test]
    async fn dispatch_default_access_control_dispatches_to_any_peer() {
        let registry = Arc::new(registry_with(
            "echo/run",
            Visibility::External,
            AccessControl::default(),
        ));
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = Dispatcher::new(registry, provider);
        let conn = Arc::new(CallConnection::new(stub_connection()));

        let payload = serde_json::json!({
            "operationId": "/echo/run",
            "input": { "msg": "hi" },
        });
        let response = dp
            .dispatch_requested(&conn, "req-8".to_string(), payload)
            .await;
        assert_eq!(response.result, Ok(serde_json::json!({ "msg": "hi" })));
    }

    #[test]
    fn dispatcher_helper_compiles_with_full_signature() {
        let _dp = dispatcher();
    }

    // --- non-QUIC (overlay-only) dispatch path ----------------------------

    fn overlay_only_connection(identity: Identity) -> Arc<CallConnection> {
        Arc::new(CallConnection::new_overlay_only(identity))
    }

    #[tokio::test]
    async fn dispatch_requested_works_with_overlay_only_connection() {
        let registry = Arc::new(registry_with(
            "echo/run",
            Visibility::External,
            AccessControl::default(),
        ));
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = Dispatcher::new(registry, provider);
        let conn = overlay_only_connection(identity_with_scopes("ws-peer", &[]));

        let payload = serde_json::json!({
            "operationId": "/echo/run",
            "input": { "msg": "hello" },
        });
        let response = dp
            .dispatch_requested(&conn, "ws-req-1".to_string(), payload)
            .await;
        assert_eq!(response.request_id, "ws-req-1");
        assert_eq!(response.result, Ok(serde_json::json!({ "msg": "hello" })));
    }

    #[tokio::test]
    async fn dispatch_requested_overlay_only_attaches_peer_keyed_by_stored_identity() {
        let mut registry = OperationRegistry::new();
        let handler = make_handler(|_input, context| async move {
            let peer_ids = context.env.peer_ids();
            ResponseEnvelope::ok(
                context.request_id,
                serde_json::json!({ "peer_ids": peer_ids }),
            )
        });
        registry.register(HandlerRegistration::new(
            external_spec("fs/readFile", AccessControl::default()),
            handler,
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        let registry = Arc::new(registry);
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = Dispatcher::new(registry, provider);
        let conn = overlay_only_connection(identity_with_scopes("ws-peer", &[]));

        let payload = serde_json::json!({
            "operationId": "/fs/readFile",
            "input": {},
        });
        let response = dp
            .dispatch_requested(&conn, "ws-req-2".to_string(), payload)
            .await;
        let out = response.result.expect("ok");
        assert_eq!(out["peer_ids"], serde_json::json!(["ws-peer"]));
    }

    #[tokio::test]
    async fn dispatch_requested_overlay_only_unknown_op_returns_not_found() {
        let registry = Arc::new(OperationRegistry::new());
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = Dispatcher::new(registry, provider);
        let conn = overlay_only_connection(identity_with_scopes("ws-peer", &[]));

        let payload = serde_json::json!({
            "operationId": "/no/such/op",
            "input": {},
        });
        let response = dp
            .dispatch_requested(&conn, "ws-req-3".to_string(), payload)
            .await;
        match response.result {
            Err(e) => assert_eq!(e.code, "NOT_FOUND"),
            other => panic!("expected NOT_FOUND, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_abort_works_with_overlay_only_connection() {
        let registry = Arc::new(registry_with(
            "echo/run",
            Visibility::External,
            AccessControl::default(),
        ));
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = Dispatcher::new(registry, provider);
        let conn = overlay_only_connection(identity_with_scopes("ws-peer", &[]));

        let parent_id = "ws-abort-root".to_string();
        let child_id = "ws-abort-child".to_string();
        {
            let mut pending = conn.pending().lock();
            pending.register_call(parent_id.clone(), Instant::now() + Duration::from_secs(30), None);
            pending.register_call(
                child_id.clone(),
                Instant::now() + Duration::from_secs(30),
                Some(parent_id.clone()),
            );
        }
        assert!(conn.pending().lock().contains(&parent_id));
        assert!(conn.pending().lock().contains(&child_id));

        dp.handle_abort(&conn, &parent_id).await;

        assert!(
            !conn.pending().lock().contains(&parent_id),
            "parent entry removed after abort"
        );
        assert!(
            !conn.pending().lock().contains(&child_id),
            "child aborted by cascade"
        );
    }

    #[tokio::test]
    async fn handle_abort_unknown_request_id_is_noop_for_overlay_only() {
        let registry = Arc::new(OperationRegistry::new());
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = Dispatcher::new(registry, provider);
        let conn = overlay_only_connection(identity_with_scopes("ws-peer", &[]));

        dp.handle_abort(&conn, "totally-unknown").await;
        assert!(conn.pending().lock().is_empty());
    }

    #[tokio::test]
    async fn overlay_only_full_dispatch_round_trip_returns_response_envelope() {
        let registry = Arc::new(registry_with(
            "echo/run",
            Visibility::External,
            AccessControl::default(),
        ));
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = Dispatcher::new(registry, provider);
        let conn = overlay_only_connection(identity_with_scopes("ws-peer", &[]));

        let payload = serde_json::json!({
            "operationId": "/echo/run",
            "input": { "v": 42 },
        });
        let request_id = "ws-roundtrip-1".to_string();
        let response = dp.dispatch_requested(&conn, request_id.clone(), payload).await;
        assert!(response.result.is_ok());
        let envelope: EventEnvelope = response.into();
        assert_eq!(envelope.r#type, EVENT_RESPONDED);
        assert_eq!(envelope.id, "ws-roundtrip-1");
        assert_eq!(envelope.payload.get("output"), Some(&serde_json::json!({ "v": 42 })));
    }
}
