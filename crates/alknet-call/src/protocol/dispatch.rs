//! Shared dispatch loop for `alknet/call` connections.
//!
//! Both [`CallAdapter`]'s accept path and [`crate::client::CallClient`]'s
//! connect path produce a [`CallConnection`] and hand it to the same dispatch
//! loop here (ADR-017 §1): the loop reads `EventEnvelope` frames off accepted
//! bidirectional streams, dispatches `call.requested` events against the
//! operation registry (with optional peer-scoped filtering per ADR-028), and
//! writes the response back on the same stream. The connection-establishment
//! half differs (accept vs dial); the dispatch half is shared.
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
use crate::registry::context::{AbortPolicy, OperationContext, ScopedOperationEnv};
use crate::registry::env::{CompositeOperationEnv, LocalOperationEnv, OperationEnv};
use crate::registry::registration::OperationRegistry;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const SWEEPER_INTERVAL: Duration = Duration::from_secs(10);

/// Peer-scoped registry filter state (ADR-028). When `trusted_peer` is false
/// (the default-deny mode for a `CallClient`), incoming dispatch hides ops
/// whose `HandlerRegistration.remote_safe` is false, and `services/list` hides
/// them too. When `trusted_peer` is true (the explicit opt-in for trusted
/// peers), the filter is bypassed: all `External` ops dispatch and list.
///
/// For the `CallAdapter` (local accept path), `trusted_peer` is `true` by
/// convention — a direct QUIC client is not a filtered `CallClient` peer in
/// the ADR-028 sense; the accept path keeps listing all `External` ops.
#[derive(Clone, Copy)]
pub struct RemoteFilter {
    pub trusted_peer: bool,
}

impl RemoteFilter {
    /// Default-deny mode: only `remote_safe: true` ops dispatch/list.
    pub fn default_deny() -> Self {
        Self {
            trusted_peer: false,
        }
    }

    /// Trusted-peer mode: all `External` ops dispatch/list regardless of
    /// `remote_safe`.
    pub fn trusted() -> Self {
        Self { trusted_peer: true }
    }

    /// Returns whether `registration` is dispatchable to the remote peer.
    pub fn allows(&self, remote_safe: bool) -> bool {
        self.trusted_peer || remote_safe
    }
}

/// Shared dispatcher for an established `CallConnection`. Constructed by
/// both `CallAdapter` (accept path) and `CallClient` (connect path) and used
/// to run the dispatch loop. Holds no per-connection state; the
/// `CallConnection` is passed into `run_loop`.
pub struct Dispatcher {
    pub registry: Arc<OperationRegistry>,
    pub identity_provider: Arc<dyn IdentityProvider>,
    pub session_source: Option<Arc<dyn SessionOverlaySource + Send + Sync>>,
    pub default_timeout: Duration,
    pub remote_filter: RemoteFilter,
}

impl Dispatcher {
    pub fn new(
        registry: Arc<OperationRegistry>,
        identity_provider: Arc<dyn IdentityProvider>,
        remote_filter: RemoteFilter,
    ) -> Self {
        Self {
            registry,
            identity_provider,
            session_source: None,
            default_timeout: DEFAULT_TIMEOUT,
            remote_filter,
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

    pub(crate) fn compose_root_env(
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

    pub(crate) fn build_root_context(
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

    pub(crate) async fn dispatch_requested(
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

        // Peer-scoped default-deny filter (ADR-028). When the caller is a
        // remote peer (default-deny mode), an op marked `remote_safe: false`
        // is hidden from dispatch — return NOT_FOUND, same posture as
        // `Visibility::Internal` per ADR-015. Critically, this returns *before*
        // any capability material reaches the handler, so a non-remote-safe
        // op's `Capabilities` are never populated for a remote peer's call
        // (ADR-028 Context — the security argument for default-deny).
        if let Some(registration) = self.registry.registration(&operation_name) {
            if !self.remote_filter.allows(registration.remote_safe) {
                return ResponseEnvelope::not_found(request_id, &operation_name);
            }
        }

        let connection_identity = connection.connection().identity().cloned();
        let identity = self.resolve_identity(connection_identity, &payload);

        let input = payload.get("input").cloned().unwrap_or(Value::Null);

        let context =
            self.build_root_context(request_id.clone(), &operation_name, identity, connection);

        self.registry.invoke(&operation_name, input, context).await
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
                    let mut pending = connection.pending().lock();
                    let mut cascade = AbortCascade::new(&mut pending);
                    let aborted = cascade.cascade_abort(&request_id, AbortPolicy::AbortDependents);
                    pending.handle_aborted(&request_id);
                    if !aborted.is_empty() {
                        debug!(count = aborted.len(), "abort cascade evicted descendants");
                    }
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
            match connection.connection().accept_bi().await {
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
            remote_filter: self.remote_filter,
        }
    }
}
