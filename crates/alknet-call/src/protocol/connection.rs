//! `CallConnection`: an established `alknet/call` connection (either
//! direction — accepted or opened). Holds the connection's Layer 2 overlay
//! (imported ops).
//!
//! See `docs/architecture/crates/call/call-protocol.md` for the full
//! specification.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use alknet_core::types::Connection;
use futures::stream::Stream;
use parking_lot::{Mutex, RwLock};
use serde_json::Value;
use tokio::sync::mpsc;

use super::pending::PendingRequestMap;
use super::wire::{
    CallError, EventEnvelope, FrameFramedReader, FrameFramedWriter, EVENT_ABORTED, EVENT_COMPLETED,
    EVENT_ERROR, EVENT_RESPONDED,
};
use crate::protocol::wire::ResponseEnvelope;
use crate::registry::context::{
    generate_request_id, AbortPolicy, OperationContext, ScopedOperationEnv,
};
use crate::registry::env::OperationEnv;
use crate::registry::registration::{Handler, HandlerRegistration};

const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(30);

pub struct CallConnection {
    connection: Arc<Connection>,
    imported_operations: Arc<RwLock<HashMap<String, HandlerRegistration>>>,
    pending: Arc<Mutex<PendingRequestMap>>,
}

impl CallConnection {
    pub fn new(connection: Connection) -> Self {
        Self {
            connection: Arc::new(connection),
            imported_operations: Arc::new(RwLock::new(HashMap::new())),
            pending: Arc::new(Mutex::new(PendingRequestMap::new())),
        }
    }

    pub fn connection(&self) -> &Arc<Connection> {
        &self.connection
    }

    pub(crate) fn pending(&self) -> &Arc<Mutex<PendingRequestMap>> {
        &self.pending
    }

    pub fn register_imported(&self, registration: HandlerRegistration) {
        let name = registration.spec.name.clone();
        self.imported_operations.write().insert(name, registration);
    }

    pub fn register_imported_all(&self, registrations: Vec<HandlerRegistration>) {
        let mut overlay = self.imported_operations.write();
        for reg in registrations {
            overlay.insert(reg.spec.name.clone(), reg);
        }
    }

    pub fn overlay_env(&self) -> Arc<dyn OperationEnv + Send + Sync> {
        Arc::new(OverlayOperationEnv {
            overlay: Arc::clone(&self.imported_operations),
        })
    }

    pub async fn call(&self, operation_id: &str, input: Value) -> ResponseEnvelope {
        let request_id = generate_request_id();
        let payload = serde_json::json!({
            "operationId": operation_id,
            "input": input,
        });

        let (send, recv) = match self.connection.open_bi().await {
            Ok(pair) => pair,
            Err(err) => {
                let call_error = CallError::internal(format!("failed to open stream: {err}"));
                return ResponseEnvelope::error(request_id, call_error);
            }
        };

        let receiver = {
            let mut pending = self.pending.lock();
            pending.register_call(
                request_id.clone(),
                Instant::now() + DEFAULT_CALL_TIMEOUT,
                None,
            )
        };

        if let Err(err) = self.write_request(send, &request_id, payload).await {
            let call_error = CallError::internal(err);
            self.pending
                .lock()
                .handle_error(&request_id, call_error.clone());
            return ResponseEnvelope::error(request_id, call_error);
        }

        let pending = Arc::clone(&self.pending);
        tokio::spawn(async move {
            read_stream_until_closed(recv, &pending).await;
        });

        match receiver.await {
            Ok(Ok(value)) => ResponseEnvelope::ok(request_id, value),
            Ok(Err(error)) => ResponseEnvelope::error(request_id, error),
            Err(_) => ResponseEnvelope::error(request_id, CallError::internal("request cancelled")),
        }
    }

    pub async fn subscribe(
        &self,
        operation_id: &str,
        input: Value,
    ) -> impl Stream<Item = ResponseEnvelope> {
        let request_id = generate_request_id();
        let payload = serde_json::json!({
            "operationId": operation_id,
            "input": input,
        });

        let (send, recv) = match self.connection.open_bi().await {
            Ok(pair) => pair,
            Err(err) => {
                let call_error = CallError::internal(format!("failed to open stream: {err}"));
                return SubscriptionStream::closed(request_id, call_error);
            }
        };

        let receiver = {
            let mut pending = self.pending.lock();
            pending.register_subscribe(request_id.clone(), None, None)
        };

        if let Err(err) = self.write_request(send, &request_id, payload).await {
            let call_error = CallError::internal(err);
            self.pending
                .lock()
                .handle_error(&request_id, call_error.clone());
            return SubscriptionStream::closed(request_id, call_error);
        }

        let pending = Arc::clone(&self.pending);
        tokio::spawn(async move {
            read_stream_until_closed(recv, &pending).await;
        });

        SubscriptionStream::new(request_id, receiver)
    }

    pub async fn abort(&self, request_id: &str) {
        let envelope = EventEnvelope::aborted(request_id);
        if let Err(err) = self.write_envelope(&envelope).await {
            tracing::warn!(error = %err, request_id, "failed to send call.aborted");
            return;
        }
        self.pending.lock().handle_aborted(request_id);
    }

    async fn write_request(
        &self,
        send: alknet_core::types::SendStream,
        request_id: &str,
        payload: Value,
    ) -> Result<(), String> {
        let envelope = EventEnvelope::requested(request_id, payload);
        let mut writer = FrameFramedWriter::new(send);
        writer
            .write_frame(&envelope)
            .await
            .map_err(|e| format!("failed to write frame: {e}"))
    }

    async fn write_envelope(&self, envelope: &EventEnvelope) -> Result<(), String> {
        let (send, _recv) = self
            .connection
            .open_bi()
            .await
            .map_err(|e| format!("failed to open stream: {e}"))?;
        let mut writer = FrameFramedWriter::new(send);
        writer
            .write_frame(envelope)
            .await
            .map_err(|e| format!("failed to write frame: {e}"))
    }
}

async fn read_stream_until_closed(
    recv: alknet_core::types::RecvStream,
    pending: &Arc<Mutex<PendingRequestMap>>,
) {
    let mut reader = FrameFramedReader::new(recv);
    while let Ok(envelope) = reader.read_frame().await {
        dispatch_envelope(pending, envelope);
    }
}

fn dispatch_envelope(pending: &Arc<Mutex<PendingRequestMap>>, envelope: EventEnvelope) {
    let request_id = envelope.id.clone();
    match envelope.r#type.as_str() {
        EVENT_RESPONDED => {
            let output = envelope
                .payload
                .get("output")
                .cloned()
                .unwrap_or(Value::Null);
            pending.lock().handle_responded(&request_id, output);
        }
        EVENT_COMPLETED => {
            pending.lock().handle_completed(&request_id);
        }
        EVENT_ABORTED => {
            pending.lock().handle_aborted(&request_id);
        }
        EVENT_ERROR => {
            if let Ok(error) = serde_json::from_value::<CallError>(envelope.payload) {
                pending.lock().handle_error(&request_id, error);
            }
        }
        _ => {}
    }
}

struct OverlayOperationEnv {
    overlay: Arc<RwLock<HashMap<String, HandlerRegistration>>>,
}

#[async_trait::async_trait]
impl OperationEnv for OverlayOperationEnv {
    async fn invoke_with_policy(
        &self,
        namespace: &str,
        operation: &str,
        input: Value,
        parent: &OperationContext,
        policy: AbortPolicy,
    ) -> ResponseEnvelope {
        let name = format!("{namespace}/{operation}");

        if !parent.scoped_env.allows(&name) {
            return ResponseEnvelope::not_found(parent.request_id.clone(), &name);
        }

        let handler: Handler;
        let composition_authority;
        let scoped_env;
        {
            let overlay = self.overlay.read();
            let Some(registration) = overlay.get(&name) else {
                return ResponseEnvelope::not_found(parent.request_id.clone(), &name);
            };
            handler = Arc::clone(&registration.handler);
            composition_authority = registration.composition_authority.clone();
            scoped_env = registration
                .scoped_env
                .clone()
                .unwrap_or_else(ScopedOperationEnv::empty);
        }

        let context = OperationContext {
            request_id: generate_request_id(),
            parent_request_id: Some(parent.request_id.clone()),
            identity: parent
                .handler_identity
                .as_ref()
                .and_then(|ca| ca.as_identity()),
            handler_identity: composition_authority,
            capabilities: parent.capabilities.clone(),
            metadata: HashMap::new(),
            abort_policy: policy,
            deadline: parent.deadline,
            scoped_env,
            env: parent.env.clone(),
            internal: true,
        };

        handler(input, context).await
    }

    fn contains(&self, name: &str) -> bool {
        self.overlay.read().contains_key(name)
    }
}

pub struct SubscriptionStream {
    request_id: String,
    receiver: mpsc::Receiver<Result<Value, CallError>>,
    done: bool,
}

impl SubscriptionStream {
    fn new(request_id: String, receiver: mpsc::Receiver<Result<Value, CallError>>) -> Self {
        Self {
            request_id,
            receiver,
            done: false,
        }
    }

    fn closed(request_id: String, error: CallError) -> Self {
        let (tx, rx) = mpsc::channel(1);
        let _ = tx.try_send(Err(error));
        Self {
            request_id,
            receiver: rx,
            done: false,
        }
    }
}

impl Stream for SubscriptionStream {
    type Item = ResponseEnvelope;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.done {
            return Poll::Ready(None);
        }
        let this = self.get_mut();
        match this.receiver.poll_recv(cx) {
            Poll::Ready(None) => {
                this.done = true;
                Poll::Ready(None)
            }
            Poll::Ready(Some(Ok(value))) => {
                Poll::Ready(Some(ResponseEnvelope::ok(this.request_id.clone(), value)))
            }
            Poll::Ready(Some(Err(error))) => {
                this.done = true;
                Poll::Ready(Some(ResponseEnvelope::error(
                    this.request_id.clone(),
                    error,
                )))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::context::CompositionAuthority;
    use crate::registry::registration::{make_handler, OperationProvenance};
    use crate::registry::spec::{AccessControl, OperationSpec, OperationType, Visibility};
    use alknet_core::types::{Capabilities, MockConnection};
    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::Mutex as StdMutex;
    use std::time::{Duration, Instant};

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

    fn stub_connection() -> Connection {
        Connection::from_mock(Arc::new(StubConnection {
            alpn: b"alknet/call",
            addr: Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4321)),
            closed: StdMutex::new(None),
        }))
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

    fn echo_handler() -> Handler {
        make_handler(
            |input, context| async move { ResponseEnvelope::ok(context.request_id, input) },
        )
    }

    fn imported_registration(name: &str) -> HandlerRegistration {
        HandlerRegistration::new(
            external_spec(name),
            echo_handler(),
            OperationProvenance::FromCall,
            None,
            None,
            Capabilities::new(),
        )
    }

    fn root_context(
        request_id: &str,
        scoped_env: ScopedOperationEnv,
        env: Arc<dyn OperationEnv + Send + Sync>,
    ) -> OperationContext {
        OperationContext {
            request_id: request_id.to_string(),
            parent_request_id: None,
            identity: None,
            handler_identity: Some(CompositionAuthority::new("agent", ["fs:read".to_string()])),
            capabilities: Capabilities::new(),
            metadata: HashMap::new(),
            scoped_env,
            env,
            abort_policy: AbortPolicy::default(),
            deadline: Some(Instant::now() + Duration::from_secs(30)),
            internal: true,
        }
    }

    #[test]
    fn register_imported_adds_to_overlay_and_contains_returns_true() {
        let conn = CallConnection::new(stub_connection());
        let env = conn.overlay_env();

        assert!(!env.contains("worker/exec"));

        conn.register_imported(imported_registration("worker/exec"));

        assert!(env.contains("worker/exec"));
        assert!(!env.contains("worker/missing"));
    }

    #[test]
    fn register_imported_all_bulk_adds_to_overlay() {
        let conn = CallConnection::new(stub_connection());
        let env = conn.overlay_env();

        conn.register_imported_all(vec![
            imported_registration("worker/exec"),
            imported_registration("worker/status"),
            imported_registration("fs/readFile"),
        ]);

        assert!(env.contains("worker/exec"));
        assert!(env.contains("worker/status"));
        assert!(env.contains("fs/readFile"));
        assert!(!env.contains("worker/missing"));
    }

    #[tokio::test]
    async fn overlay_env_dispatches_to_imported_op() {
        let conn = CallConnection::new(stub_connection());
        conn.register_imported(imported_registration("worker/exec"));
        let env = conn.overlay_env();

        let scoped = ScopedOperationEnv::new(["worker/exec"]);
        let ctx = root_context("root-1", scoped, env.clone());

        let response = env
            .invoke("worker", "exec", serde_json::json!({"hi": 1}), &ctx)
            .await;

        assert!(response.result.is_ok());
        assert_eq!(response.result.unwrap(), serde_json::json!({"hi": 1}));
    }

    #[tokio::test]
    async fn overlay_env_contains_returns_false_for_non_imported_op() {
        let conn = CallConnection::new(stub_connection());
        conn.register_imported(imported_registration("worker/exec"));
        let env = conn.overlay_env();

        assert!(!env.contains("worker/missing"));

        let scoped = ScopedOperationEnv::new(["worker/missing"]);
        let ctx = root_context("root-2", scoped, env.clone());

        let response = env
            .invoke("worker", "missing", serde_json::json!({}), &ctx)
            .await;

        match response.result {
            Err(e) => assert_eq!(e.code, "NOT_FOUND"),
            other => panic!("expected NOT_FOUND, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn overlay_env_reachability_check_returns_not_found_for_disallowed_op() {
        let conn = CallConnection::new(stub_connection());
        conn.register_imported(imported_registration("worker/exec"));
        let env = conn.overlay_env();

        let scoped = ScopedOperationEnv::empty();
        let ctx = root_context("root-3", scoped, env.clone());

        let response = env
            .invoke("worker", "exec", serde_json::json!({}), &ctx)
            .await;

        match response.result {
            Err(e) => assert_eq!(e.code, "NOT_FOUND"),
            other => panic!("expected NOT_FOUND, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn overlay_env_dispatched_child_has_internal_true_and_parent_set() {
        let conn = CallConnection::new(stub_connection());
        let inspect_handler = make_handler(|_input, context| async move {
            let internal = context.is_internal();
            let parent_set = context.parent_request_id.is_some();
            ResponseEnvelope::ok(
                context.request_id,
                serde_json::json!({
                    "internal": internal,
                    "parent_set": parent_set,
                }),
            )
        });
        conn.register_imported(HandlerRegistration::new(
            external_spec("worker/exec"),
            inspect_handler,
            OperationProvenance::FromCall,
            None,
            None,
            Capabilities::new(),
        ));
        let env = conn.overlay_env();

        let scoped = ScopedOperationEnv::new(["worker/exec"]);
        let ctx = root_context("root-4", scoped, env.clone());

        let response = env
            .invoke("worker", "exec", serde_json::json!({}), &ctx)
            .await;
        let out = response.result.expect("ok");
        assert_eq!(out["internal"], Value::Bool(true));
        assert_eq!(out["parent_set"], Value::Bool(true));
    }

    #[test]
    fn connection_accessor_returns_underlying_connection() {
        let conn = CallConnection::new(stub_connection());
        assert_eq!(conn.connection().remote_alpn(), b"alknet/call");
    }

    #[test]
    fn empty_overlay_contains_nothing() {
        let conn = CallConnection::new(stub_connection());
        let env = conn.overlay_env();
        assert!(!env.contains("anything"));
        assert!(!env.contains(""));
    }

    #[test]
    fn overlay_drops_with_connection() {
        let captured: Arc<RwLock<HashMap<String, HandlerRegistration>>> =
            Arc::new(RwLock::new(HashMap::new()));
        {
            let conn = CallConnection::new(stub_connection());
            conn.register_imported(imported_registration("worker/exec"));
            assert!(conn.overlay_env().contains("worker/exec"));
            std::mem::swap(
                &mut *captured.write(),
                &mut *conn.imported_operations.write(),
            );
        }
        assert!(captured.read().contains_key("worker/exec"));
    }

    // --- dispatch_envelope -------------------------------------------------

    fn empty_pending() -> Arc<Mutex<PendingRequestMap>> {
        Arc::new(Mutex::new(PendingRequestMap::new()))
    }

    #[tokio::test]
    async fn dispatch_envelope_responded_resolves_call_receiver() {
        let pending = empty_pending();
        let rx = pending
            .lock()
            .register_call("req-1".to_string(), Instant::now() + Duration::from_secs(30), None);
        let envelope = EventEnvelope::responded("req-1", serde_json::json!({"v": 42}));
        dispatch_envelope(&pending, envelope);
        assert!(!pending.lock().contains("req-1"));
        let result = tokio::time::timeout(Duration::from_millis(100), rx).await;
        match result {
            Ok(Ok(Ok(value))) => assert_eq!(value, serde_json::json!({"v": 42})),
            other => panic!("expected Ok({{v:42}}), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_envelope_responded_pushes_to_subscribe_channel() {
        let pending = empty_pending();
        let mut rx = pending
            .lock()
            .register_subscribe("sub-1".to_string(), None, None);
        dispatch_envelope(&pending, EventEnvelope::responded("sub-1", serde_json::json!("first")));
        dispatch_envelope(&pending, EventEnvelope::responded("sub-1", serde_json::json!("second")));
        assert!(pending.lock().contains("sub-1"));
        let a = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await;
        let b = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await;
        match (a, b) {
            (Ok(Some(Ok(x))), Ok(Some(Ok(y)))) => {
                assert_eq!(x, serde_json::json!("first"));
                assert_eq!(y, serde_json::json!("second"));
            }
            other => panic!("expected two Ok values, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_envelope_completed_removes_entry() {
        let pending = empty_pending();
        let _rx = pending
            .lock()
            .register_subscribe("sub-2".to_string(), None, None);
        assert!(pending.lock().contains("sub-2"));
        dispatch_envelope(&pending, EventEnvelope::completed("sub-2"));
        assert!(!pending.lock().contains("sub-2"));
    }

    #[tokio::test]
    async fn dispatch_envelope_aborted_removes_entry() {
        let pending = empty_pending();
        let _rx = pending.lock().register_call(
            "req-2".to_string(),
            Instant::now() + Duration::from_secs(30),
            None,
        );
        assert!(pending.lock().contains("req-2"));
        dispatch_envelope(&pending, EventEnvelope::aborted("req-2"));
        assert!(!pending.lock().contains("req-2"));
    }

    #[tokio::test]
    async fn dispatch_envelope_error_resolves_call_with_error() {
        let pending = empty_pending();
        let rx = pending.lock().register_call(
            "req-3".to_string(),
            Instant::now() + Duration::from_secs(30),
            None,
        );
        let err = CallError::new("FILE_NOT_FOUND", "missing", false);
        dispatch_envelope(&pending, EventEnvelope::error("req-3", &err));
        assert!(!pending.lock().contains("req-3"));
        let result = tokio::time::timeout(Duration::from_millis(100), rx).await;
        match result {
            Ok(Ok(Err(e))) => {
                assert_eq!(e.code, "FILE_NOT_FOUND");
                assert!(!e.retryable);
            }
            other => panic!("expected Err(FILE_NOT_FOUND), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_envelope_error_pushes_error_to_subscribe_channel() {
        let pending = empty_pending();
        let mut rx = pending
            .lock()
            .register_subscribe("sub-3".to_string(), None, None);
        let err = CallError::new("RATE_LIMITED", "slow down", true);
        dispatch_envelope(&pending, EventEnvelope::error("sub-3", &err));
        assert!(!pending.lock().contains("sub-3"));
        let result = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await;
        match result {
            Ok(Some(Err(e))) => {
                assert_eq!(e.code, "RATE_LIMITED");
                assert!(e.retryable);
            }
            other => panic!("expected Err(RATE_LIMITED), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_envelope_error_with_invalid_payload_is_no_op() {
        let pending = empty_pending();
        let _rx = pending.lock().register_call(
            "req-4".to_string(),
            Instant::now() + Duration::from_secs(30),
            None,
        );
        let malformed = EventEnvelope::new(EVENT_ERROR, "req-4", serde_json::json!("not-an-object"));
        dispatch_envelope(&pending, malformed);
        assert!(pending.lock().contains("req-4"));
    }

    #[tokio::test]
    async fn dispatch_envelope_unknown_event_type_is_no_op() {
        let pending = empty_pending();
        let _rx = pending.lock().register_call(
            "req-5".to_string(),
            Instant::now() + Duration::from_secs(30),
            None,
        );
        let unknown = EventEnvelope::new("call.mystery", "req-5", serde_json::json!({}));
        dispatch_envelope(&pending, unknown);
        assert!(pending.lock().contains("req-5"));
    }

    #[tokio::test]
    async fn dispatch_envelope_unknown_request_id_is_no_op() {
        let pending = empty_pending();
        dispatch_envelope(&pending, EventEnvelope::responded("ghost", serde_json::json!(1)));
        dispatch_envelope(&pending, EventEnvelope::completed("ghost"));
        dispatch_envelope(&pending, EventEnvelope::aborted("ghost"));
        assert!(pending.lock().is_empty());
    }

    // --- SubscriptionStream ------------------------------------------------

    #[tokio::test]
    async fn subscription_stream_closed_yields_one_error_then_ends() {
        use futures::stream::StreamExt;
        let err = CallError::internal("stream closed before send");
        let mut stream = SubscriptionStream::closed("req-x".to_string(), err);
        let first = stream.next().await;
        match first {
            Some(env) => {
                assert_eq!(env.request_id, "req-x");
                assert!(env.result.is_err());
                assert_eq!(env.result.unwrap_err().code, "INTERNAL");
            }
            other => panic!("expected one error envelope, got {other:?}"),
        }
        let second = stream.next().await;
        assert!(second.is_none(), "stream must terminate after the error");
    }

    #[tokio::test]
    async fn subscription_stream_emits_ok_values_then_completes() {
        use futures::stream::StreamExt;
        let (tx, rx) = mpsc::channel(8);
        let mut stream = SubscriptionStream::new("req-y".to_string(), rx);
        tx.try_send(Ok(serde_json::json!(1))).unwrap();
        tx.try_send(Ok(serde_json::json!(2))).unwrap();
        drop(tx);

        let a = stream.next().await.unwrap();
        assert_eq!(a.request_id, "req-y");
        assert_eq!(a.result.unwrap(), serde_json::json!(1));
        let b = stream.next().await.unwrap();
        assert_eq!(b.result.unwrap(), serde_json::json!(2));
        assert!(stream.next().await.is_none(), "stream ends after channel closes");
    }

    #[tokio::test]
    async fn subscription_stream_emits_error_then_terminates() {
        use futures::stream::StreamExt;
        let (tx, rx) = mpsc::channel(8);
        let mut stream = SubscriptionStream::new("req-z".to_string(), rx);
        tx.try_send(Ok(serde_json::json!("ok"))).unwrap();
        tx.try_send(Err(CallError::timeout("timed out"))).unwrap();
        drop(tx);

        let first = stream.next().await.unwrap();
        assert_eq!(first.result.unwrap(), serde_json::json!("ok"));
        let second = stream.next().await.unwrap();
        assert_eq!(second.request_id, "req-z");
        assert_eq!(second.result.unwrap_err().code, "TIMEOUT");
        assert!(stream.next().await.is_none(), "stream terminates after error");
    }
}
