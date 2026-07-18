//! `CallClient`: the outbound connection opener (ADR-017 §1).
//!
//! Runs the shared dispatch loop over a pre-established `Connection`
//! (delegated to [`crate::protocol::dispatch::Dispatcher`]).
//! `CallClient` is the connection-establishment half; `CallAdapter`'s accept
//! path is the inbound half; both produce a `CallConnection` and hand it to
//! the same `Dispatcher::run_loop` (ADR-017 §1).
//!
//! After establishment the connection is symmetric (ADR-017 §2): both sides
//! can send and receive `call.requested`. The `CallClient` is both a caller
//! (initiates outgoing calls via `CallConnection::call()`/`subscribe()`/
//! `abort()`) and a callee (dispatches incoming calls against its registry).
//!
//! Transport-level connection establishment (QUIC dial, TCP+TLS, iroh) is
//! handled by `alknet-client`; `CallClient::spawn_dispatch` takes a
//! pre-established `Connection` and runs the call protocol over it.
//!
//! See `docs/architecture/crates/call/client-and-adapters.md` for the spec.

use std::sync::Arc;

use alknet_core::auth::IdentityProvider;
use alknet_core::types::Connection;

use crate::protocol::connection::CallConnection;
use crate::protocol::dispatch::Dispatcher;
use crate::registry::registration::OperationRegistry;

/// Outbound `alknet/call` connection opener (the #1 gap, ADR-017 §1).
///
/// Peer authorization flows through the existing `AccessControl::check` gate
/// in `OperationRegistry::invoke` (ADR-029 §3) — no parallel `remote_safe`/
/// `trusted_peer` gate.
pub struct CallClient {
    registry: Arc<OperationRegistry>,
    identity_provider: Arc<dyn IdentityProvider>,
}

impl CallClient {
    pub fn new(
        registry: Arc<OperationRegistry>,
        identity_provider: Arc<dyn IdentityProvider>,
    ) -> Self {
        Self {
            registry,
            identity_provider,
        }
    }

    pub fn registry(&self) -> &Arc<OperationRegistry> {
        &self.registry
    }

    pub fn identity_provider(&self) -> &Arc<dyn IdentityProvider> {
        &self.identity_provider
    }

    /// Run the shared dispatch loop over a pre-established `Connection`. The
    /// `CallClient` spawns the dispatcher task and returns a live
    /// `CallConnection` the caller can use immediately. Used by the assembly
    /// layer after `AlknetClient::dial_*` + `spawn_dispatch` and by
    /// integration tests that wire a mock/loopback `Connection` directly.
    pub fn spawn_dispatch(&self, connection: Connection) -> CallConnection {
        let call_connection = Arc::new(CallConnection::new(connection));
        let dispatcher = Dispatcher::new(
            Arc::clone(&self.registry),
            Arc::clone(&self.identity_provider),
        );
        let run_conn = Arc::clone(&call_connection);
        tokio::spawn(async move {
            dispatcher.run_loop(run_conn).await;
        });
        (*call_connection).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::connection::CallConnection;
    use crate::protocol::wire::ResponseEnvelope;
    use crate::registry::registration::{
        make_handler, Handler, HandlerKind, HandlerRegistration, OperationProvenance,
    };
    use crate::registry::spec::{AccessControl, OperationSpec, OperationType, Visibility};
    use alknet_core::auth::Identity;
    use alknet_core::types::Capabilities;

    use crate::protocol::sink_empty_connection as stub_connection;

    fn external_spec(name: &str) -> OperationSpec {
        OperationSpec::new(
            name,
            OperationType::Query,
            Visibility::External,
            serde_json::json!({}),
            serde_json::json!({}),
            vec![],
            AccessControl::default(),
            None,
        )
    }

    fn caps_inspect_handler() -> Handler {
        make_handler(|_input, context| async move {
            let has_google = context.capabilities.get("google").is_some();
            ResponseEnvelope::ok(
                context.request_id,
                serde_json::json!({ "has_google_capability": has_google }),
            )
        })
    }

    struct NoopIdentityProvider;
    impl alknet_core::auth::IdentityProvider for NoopIdentityProvider {
        fn resolve_from_fingerprint(&self, _fp: &str) -> Option<Identity> {
            None
        }
        fn resolve_from_token(&self, _token: &alknet_core::auth::AuthToken) -> Option<Identity> {
            None
        }
    }

    fn registry_with_caps() -> Arc<OperationRegistry> {
        let mut registry = OperationRegistry::new();
        registry
            .register(HandlerRegistration::new(
                external_spec("pub/run"),
                HandlerKind::Once(caps_inspect_handler()),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new().with_api_key("google", "pub-key".to_string()),
            ))
            .unwrap();
        Arc::new(registry)
    }

    fn dispatcher(registry: &Arc<OperationRegistry>) -> Dispatcher {
        Dispatcher::new(Arc::clone(registry), Arc::new(NoopIdentityProvider))
    }

    async fn dispatch(d: &Dispatcher, conn: &Arc<CallConnection>, op: &str) -> ResponseEnvelope {
        d.dispatch_requested(
            conn,
            "req-test".to_string(),
            serde_json::json!({ "operationId": op, "input": {} }),
        )
        .await
    }

    #[tokio::test]
    async fn external_op_dispatches_and_populates_capabilities() {
        let registry = registry_with_caps();
        let d = dispatcher(&registry);
        let conn = Arc::new(CallConnection::new(stub_connection()));
        let response = dispatch(&d, &conn, "pub/run").await;
        let out = response.result.expect("ok");
        assert_eq!(
            out["has_google_capability"],
            serde_json::json!(true),
            "an External op's call must populate capabilities for the handler"
        );
    }

    #[tokio::test]
    async fn unknown_op_returns_not_found() {
        let registry = Arc::new(OperationRegistry::new());
        let d = dispatcher(&registry);
        let conn = Arc::new(CallConnection::new(stub_connection()));
        let response = dispatch(&d, &conn, "no/such").await;
        match response.result {
            Err(e) => assert_eq!(e.code, "NOT_FOUND"),
            other => panic!("expected NOT_FOUND, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn spawn_dispatch_returns_live_call_connection() {
        let registry = registry_with_caps();
        let client = CallClient::new(Arc::clone(&registry), Arc::new(NoopIdentityProvider));
        let conn = client.spawn_dispatch(stub_connection());
        assert_eq!(
            conn.connection()
                .expect("quic connection present")
                .remote_alpn(),
            b"alknet/call"
        );
        std::mem::drop(conn);
    }

    #[test]
    fn call_client_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CallClient>();
    }
}
