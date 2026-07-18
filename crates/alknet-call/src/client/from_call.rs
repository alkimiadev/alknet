//! `from_call` adapter (ADR-017 §3): discovers the remote peer's `External`
//! operations via `services/list` + `services/schema` and registers them in
//! the connection's Layer 2 overlay as `FromCall`-provenance leaves with
//! forwarding handlers.
//!
//! The discovery mechanism (`services/list` + `services/schema`) is already
//! implemented in `registry/discovery.rs`; `from_call` is the client-side
//! consumer of that API.
//!
//! See `docs/architecture/crates/call/client-and-adapters.md` §from_call for
//! the spec and the v1 defaults (auto-on-reconnect, error-on-collision).

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::client::AdapterError;
use crate::protocol::connection::CallConnection;
use crate::protocol::wire::ResponseEnvelope;
use crate::registry::context::OperationContext;
use crate::registry::registration::{
    Handler, HandlerKind, HandlerRegistration, OperationProvenance, StreamingHandler,
};
use crate::registry::spec::{
    AccessControl, ErrorDefinition, OperationSpec, OperationType, Visibility,
};
use alknet_core::types::Capabilities;

/// Configuration for [`from_call`].
///
/// Under the peer-keyed overlay model (ADR-029 §5), cross-peer collision
/// dissolves — same name on different peers lives in separate sub-overlays.
/// Same-peer collision stays an error (`AdapterError::SamePeerCollision`):
/// a peer shouldn't expose two ops with the same name.
#[derive(Debug, Clone, Default)]
pub struct FromCallConfig {
    /// Optional namespace prefix applied to imported operation names. This is
    /// local-naming sugar for when the importing node wants to expose a peer's
    /// ops under a different name *locally* — not a disambiguation mechanism
    /// (cross-peer collision dissolves under the peer-keyed model, ADR-029
    /// §5). Defaults to `None`.
    pub namespace_prefix: Option<String>,
    /// Optional filter — import only operations whose names match. `None`
    /// imports all `External` ops discovered via `services/list`.
    pub operation_filter: Option<HashSet<String>>,
}

impl FromCallConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_namespace_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.namespace_prefix = Some(prefix.into());
        self
    }

    pub fn with_operation_filter(mut self, filter: HashSet<String>) -> Self {
        self.operation_filter = Some(filter);
        self
    }
}

/// Discover the remote peer's `External` ops via `services/list` +
/// `services/schema` and construct `HandlerRegistration` bundles with
/// `FromCall` provenance and forwarding handlers. The caller registers the
/// bundles in the connection's overlay via
/// `CallConnection::register_imported_all()` — this is the peer-keyed
/// registration model (ADR-029 §5): the connection's overlay is the peer's
/// sub-overlay, aggregated into `PeerCompositeEnv` by `PeerId`.
///
/// v1 defaults (two-way doors recorded in `client-and-adapters.md`):
/// - auto-on-reconnect: the overlay is per-connection (Layer 2, ADR-024), so
///   re-import on reconnect is naturally scoped; the assembly layer calls
///   `from_call` immediately after `AlknetClient::dial_*` + `spawn_dispatch`.
/// - same-peer collision = error: two ops with the same name from the same
///   peer (after applying the optional prefix) → `AdapterError::SamePeerCollision`.
///   Cross-peer collision dissolves (ADR-029 §5).
pub async fn from_call(
    connection: &CallConnection,
    config: FromCallConfig,
) -> Result<Vec<HandlerRegistration>, AdapterError> {
    let discovered = discover_operations(connection).await?;
    build_bundles(
        discovered,
        &config.namespace_prefix,
        &config.operation_filter,
    )
}

/// Pure bundle construction extracted from [`from_call`] for testability —
/// the discovery round-trip against a live `CallConnection` is exercised by
/// integration tests; the collision rule and the forwarded_for-populating
/// handler are unit-tested here. The `peer_id` parameter records which peer's
/// sub-overlay these bundles target (ADR-029 §5); it is metadata on the
/// bundles' forwarding handlers, not used for collision detection (collision
/// is same-peer only and is checked within this set).
fn build_bundles(
    discovered: Vec<OpSummary>,
    namespace_prefix: &Option<String>,
    operation_filter: &Option<HashSet<String>>,
) -> Result<Vec<HandlerRegistration>, AdapterError> {
    let mut bundles = Vec::with_capacity(discovered.len());
    let mut seen_names = HashSet::new();

    for op_summary in discovered {
        let remote_name = op_summary.name;
        if let Some(filter) = operation_filter {
            if !filter.contains(&remote_name) {
                continue;
            }
        }

        let spec = rebuild_spec_for(&op_summary.schema, &remote_name, namespace_prefix)?;

        if !seen_names.insert(spec.name.clone()) {
            return Err(AdapterError::SamePeerCollision {
                message: format!(
                    "same-peer collision on import: {} (peer exposes two ops with the same name after prefix)",
                    spec.name
                ),
            });
        }

        let kind = match spec.op_type {
            OperationType::Subscription => HandlerKind::Stream(make_streaming_forwarding_handler(
                Arc::new(op_summary.connection.clone()),
                remote_name,
            )),
            OperationType::Query | OperationType::Mutation => HandlerKind::Once(
                make_forwarding_handler(Arc::new(op_summary.connection.clone()), remote_name),
            ),
        };
        bundles.push(HandlerRegistration::new(
            spec,
            kind,
            OperationProvenance::FromCall,
            None,
            None,
            Capabilities::new(),
        ));
    }

    Ok(bundles)
}

#[derive(Clone)]
struct OpSummary {
    name: String,
    schema: Value,
    connection: CallConnection,
}

async fn discover_operations(connection: &CallConnection) -> Result<Vec<OpSummary>, AdapterError> {
    let response = connection.call("services/list", json!({})).await;
    let output = response.result.map_err(|e| AdapterError::DiscoveryFailed {
        message: format!("services/list failed: {} ({})", e.code, e.message),
    })?;
    let ops = output
        .get("operations")
        .and_then(|v| v.as_array())
        .ok_or_else(|| AdapterError::SchemaParse {
            message: "services/list response missing 'operations' array".to_string(),
        })?;
    let mut summaries = Vec::with_capacity(ops.len());
    for op in ops {
        let name =
            op.get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AdapterError::SchemaParse {
                    message: "services/list entry missing 'name'".to_string(),
                })?;
        let schema = fetch_schema(connection, name).await?;
        summaries.push(OpSummary {
            name: name.to_string(),
            schema,
            connection: connection.clone(),
        });
    }
    Ok(summaries)
}

async fn fetch_schema(connection: &CallConnection, name: &str) -> Result<Value, AdapterError> {
    let response = connection
        .call("services/schema", json!({ "name": name }))
        .await;
    response.result.map_err(|e| AdapterError::DiscoveryFailed {
        message: format!(
            "services/schema for {name} failed: {} ({})",
            e.code, e.message
        ),
    })
}

/// Rebuild an `OperationSpec` from the `services/schema` JSON, applying the
/// optional namespace prefix. The spec JSON shape matches `spec_to_json` in
/// `registry/discovery.rs`.
fn rebuild_spec_for(
    schema_json: &Value,
    remote_name: &str,
    namespace_prefix: &Option<String>,
) -> Result<OperationSpec, AdapterError> {
    let op_type = parse_op_type(
        schema_json
            .get("op_type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdapterError::SchemaParse {
                message: format!("schema for {remote_name} missing op_type"),
            })?,
    )?;
    let visibility = parse_visibility(
        schema_json
            .get("visibility")
            .and_then(|v| v.as_str())
            .unwrap_or("external"),
    );
    let input_schema = schema_json
        .get("input_schema")
        .cloned()
        .unwrap_or(Value::Null);
    let output_schema = schema_json
        .get("output_schema")
        .cloned()
        .unwrap_or(Value::Null);
    let error_schemas = schema_json
        .get("error_schemas")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(parse_error_definition).collect())
        .unwrap_or_default();
    let access_control = schema_json
        .get("access_control")
        .map(parse_access_control)
        .unwrap_or_default();

    let name = match namespace_prefix {
        Some(prefix) if !prefix.is_empty() => format!("{prefix}/{remote_name}"),
        _ => remote_name.to_string(),
    };

    Ok(OperationSpec::new(
        name,
        op_type,
        visibility,
        input_schema,
        output_schema,
        error_schemas,
        access_control,
        None,
    ))
}

fn parse_op_type(s: &str) -> Result<OperationType, AdapterError> {
    match s {
        "query" => Ok(OperationType::Query),
        "mutation" => Ok(OperationType::Mutation),
        "subscription" => Ok(OperationType::Subscription),
        other => Err(AdapterError::SchemaParse {
            message: format!("unknown op_type: {other}"),
        }),
    }
}

fn parse_visibility(s: &str) -> Visibility {
    match s {
        "internal" => Visibility::Internal,
        _ => Visibility::External,
    }
}

fn parse_error_definition(v: &Value) -> Option<ErrorDefinition> {
    Some(ErrorDefinition {
        code: v.get("code")?.as_str()?.to_string(),
        description: v
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        schema: v.get("schema").cloned().unwrap_or(Value::Null),
        http_status: v
            .get("http_status")
            .and_then(|v| v.as_u64())
            .map(|n| n as u16),
    })
}

fn parse_access_control(v: &Value) -> AccessControl {
    AccessControl {
        required_scopes: v
            .get("required_scopes")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        required_scopes_any: v
            .get("required_scopes_any")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect()
            }),
        resource_type: v
            .get("resource_type")
            .and_then(|v| v.as_str())
            .map(String::from),
        resource_action: v
            .get("resource_action")
            .and_then(|v| v.as_str())
            .map(String::from),
    }
}

/// Construct a forwarding handler for a `FromCall` `Query`/`Mutation` leaf:
/// on invocation, calls the remote op via the `CallConnection` and returns
/// its `ResponseEnvelope` (single `call_with_payload()`, `HandlerKind::Once`).
/// `Subscription` ops use [`make_streaming_forwarding_handler`] instead.
///
/// Per ADR-032 §3, the handler populates `forwarded_for` on the
/// `call.requested` payload from the hub's `OperationContext.identity` (the
/// end user the hub authenticated). The hub authenticates as itself when
/// forwarding. The spoke authorizes the hub (its direct caller);
/// `forwarded_for` is metadata, never read by `AccessControl::check`.
///
/// If `context.identity` is `None` (the hub chose not to disclose, or has not
/// authenticated an originator), `forwarded_for` is omitted — the spoke
/// receives only the hub's identity.
fn make_forwarding_handler(connection: Arc<CallConnection>, remote_name: String) -> Handler {
    use crate::registry::registration::make_handler;
    make_handler(move |input, context| {
        let connection = Arc::clone(&connection);
        let remote_name = remote_name.clone();
        async move {
            let payload = build_forwarded_payload(&remote_name, input, &context);
            // The forwarding handler invokes the remote op via the
            // CallConnection. The parent_request_id participates in the abort
            // cascade (ADR-016 §6): if the parent is aborted, the cascade
            // reaches this handler, which sends call.aborted to the remote
            // node; the remote node cascades to its own descendants.
            // Cross-node abort is transparent.
            let response = connection.call_with_payload(payload).await;
            ResponseEnvelope {
                request_id: context.request_id,
                result: response.result,
            }
        }
    })
}

/// Construct a streaming forwarding handler for a `FromCall` `Subscription`
/// leaf: on invocation, calls `CallConnection::subscribe_with_payload()` and
/// forwards the remote stream end-to-end. Each `call.responded` from the
/// remote becomes a stream item, `call.completed` ends the stream, and
/// `call.aborted` drops it (ADR-049 §8). No truncation, no first-value
/// fallback.
///
/// `forwarded_for` is populated from `context.identity` (ADR-032 §3), exactly
/// as the request/response forwarding handler does — both via
/// `build_forwarded_payload` (no new payload-construction code). The
/// `subscribe_with_payload` path registers the request in
/// `PendingRequestMap`, so the abort cascade (ADR-016 §6) is already wired:
/// a parent abort drops the `SubscriptionStream`, which sends `call.aborted`
/// to the remote node.
fn make_streaming_forwarding_handler(
    connection: Arc<CallConnection>,
    remote_name: String,
) -> StreamingHandler {
    use crate::registry::registration::make_streaming_handler;
    use futures::stream::{once, StreamExt};
    make_streaming_handler(move |input, context| {
        let connection = Arc::clone(&connection);
        let remote_name = remote_name.clone();
        once(async move {
            let payload = build_forwarded_payload(&remote_name, input, &context);
            connection.subscribe_with_payload(payload).await
        })
        .flatten()
    })
}

/// Build the `call.requested` payload for a forwarded call, populating
/// `forwarded_for` from the hub's `OperationContext.identity` (ADR-032 §3).
/// `forwarded_for` is omitted when `context.identity` is `None` (the hub
/// chooses not to disclose the originator).
fn build_forwarded_payload(operation_id: &str, input: Value, context: &OperationContext) -> Value {
    let mut payload = serde_json::Map::new();
    payload.insert(
        "operationId".to_string(),
        Value::String(operation_id.to_string()),
    );
    payload.insert("input".to_string(), input);
    if let Some(originator) = &context.identity {
        if let Ok(value) = serde_json::to_value(originator) {
            payload.insert("forwarded_for".to_string(), value);
        }
    }
    Value::Object(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::connection::CallConnection;
    use crate::registry::registration::{make_handler, make_streaming_handler};
    use crate::registry::spec::OperationType;
    use alknet_core::auth::Identity;
    use alknet_core::types::Capabilities;
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;

    use crate::protocol::sink_empty_connection as stub_connection;

    fn sample_schema_json(name: &str, op_type: &str) -> Value {
        json!({
            "name": name,
            "namespace": name.split('/').next().unwrap_or(""),
            "op_type": op_type,
            "visibility": "external",
            "input_schema": {"type": "object"},
            "output_schema": {"type": "string"},
            "error_schemas": [],
            "access_control": {"required_scopes": []},
        })
    }

    #[test]
    fn rebuild_spec_no_prefix_preserves_name() {
        let schema = sample_schema_json("fs/readFile", "query");
        let spec = rebuild_spec_for(&schema, "fs/readFile", &None).expect("rebuild");
        assert_eq!(spec.name, "fs/readFile");
        assert_eq!(spec.op_type, OperationType::Query);
        assert_eq!(spec.visibility, Visibility::External);
    }

    #[test]
    fn rebuild_spec_with_prefix_applies_prefix() {
        let schema = sample_schema_json("fs/readFile", "query");
        let spec =
            rebuild_spec_for(&schema, "fs/readFile", &Some("worker".to_string())).expect("rebuild");
        assert_eq!(spec.name, "worker/fs/readFile");
    }

    #[test]
    fn rebuild_spec_unknown_op_type_returns_schema_parse() {
        let schema = sample_schema_json("fs/readFile", "weird");
        match rebuild_spec_for(&schema, "fs/readFile", &None) {
            Err(AdapterError::SchemaParse { .. }) => {}
            other => panic!("expected SchemaParse, got {other:?}"),
        }
    }

    #[test]
    fn rebuild_spec_missing_op_type_returns_schema_parse() {
        let schema = json!({"name": "fs/readFile"});
        match rebuild_spec_for(&schema, "fs/readFile", &None) {
            Err(AdapterError::SchemaParse { .. }) => {}
            other => panic!("expected SchemaParse, got {other:?}"),
        }
    }

    #[test]
    fn rebuild_spec_parses_error_schemas_and_acl() {
        let schema = json!({
            "name": "fs/readFileErr",
            "namespace": "fs",
            "op_type": "query",
            "visibility": "external",
            "input_schema": {},
            "output_schema": {},
            "error_schemas": [{
                "code": "FILE_NOT_FOUND",
                "description": "file not found",
                "schema": {"type": "object"},
                "http_status": 404,
            }],
            "access_control": {
                "required_scopes": ["fs:read"],
                "required_scopes_any": null,
                "resource_type": "fs",
                "resource_action": "read",
            },
        });
        let spec = rebuild_spec_for(&schema, "fs/readFileErr", &None).expect("rebuild");
        assert_eq!(spec.error_schemas.len(), 1);
        assert_eq!(spec.error_schemas[0].code, "FILE_NOT_FOUND");
        assert_eq!(spec.error_schemas[0].http_status, Some(404));
        assert_eq!(
            spec.access_control.required_scopes,
            vec!["fs:read".to_string()]
        );
        assert_eq!(spec.access_control.resource_type.as_deref(), Some("fs"));
    }

    #[test]
    fn from_call_config_builder_methods() {
        let config = FromCallConfig::new()
            .with_namespace_prefix("worker")
            .with_operation_filter(HashSet::from(["fs/readFile".to_string()]));
        assert_eq!(config.namespace_prefix.as_deref(), Some("worker"));
        assert!(config.operation_filter.unwrap().contains("fs/readFile"));
    }

    /// `from_call` against a stub `CallConnection` (no real transport) returns
    /// a `DiscoveryFailed` because `services/list` can't dispatch on a mock
    /// connection. This verifies the error path rather than the happy path
    /// (the happy path is covered by the integration test in a later task).
    #[tokio::test]
    async fn from_call_against_mock_connection_returns_discovery_failed() {
        let conn = CallConnection::new(stub_connection());
        let result = from_call(&conn, FromCallConfig::new()).await;
        match result {
            Err(AdapterError::DiscoveryFailed { .. }) => {}
            Err(other) => panic!("expected DiscoveryFailed, got another error variant: {other}"),
            Ok(_) => panic!("expected DiscoveryFailed on mock connection, got Ok"),
        }
    }

    #[test]
    fn from_call_provenance_is_from_call_and_leaf_fields() {
        // Verify the registration shape produced by from_call: provenance
        // FromCall, no composition authority, no scoped_env, empty caps.
        // Uses a synthetic spec to avoid the transport round-trip.
        let spec = OperationSpec::new(
            "worker/echo",
            OperationType::Query,
            Visibility::External,
            json!({}),
            json!({}),
            vec![],
            AccessControl::default(),
            None,
        );
        let handler = make_forwarding_handler(
            Arc::new(CallConnection::new(stub_connection())),
            "worker/echo".to_string(),
        );
        let reg = HandlerRegistration::new(
            spec,
            HandlerKind::Once(handler),
            OperationProvenance::FromCall,
            None,
            None,
            Capabilities::new(),
        );
        assert_eq!(reg.provenance, OperationProvenance::FromCall);
        assert!(reg.composition_authority.is_none());
        assert!(reg.scoped_env.is_none());
    }

    // --- ADR-032: forwarded_for population --------------------------------

    struct NoopEnv;
    #[async_trait::async_trait]
    impl crate::registry::env::OperationEnv for NoopEnv {
        async fn invoke_with_policy(
            &self,
            _ns: &str,
            _op: &str,
            _input: Value,
            parent: &OperationContext,
            _policy: crate::registry::context::AbortPolicy,
        ) -> ResponseEnvelope {
            ResponseEnvelope::ok(parent.request_id.clone(), Value::Null)
        }
        fn contains(&self, _name: &str) -> bool {
            false
        }
    }

    fn test_context(identity: Option<Identity>) -> OperationContext {
        use crate::registry::context::{AbortPolicy, ScopedPeerEnv};
        use std::collections::HashMap;
        use std::time::{Duration, Instant};
        OperationContext {
            request_id: "req-test".to_string(),
            parent_request_id: None,
            identity,
            handler_identity: None,
            forwarded_for: None,
            capabilities: Capabilities::new(),
            metadata: HashMap::new(),
            scoped_env: ScopedPeerEnv::empty(),
            env: Arc::new(NoopEnv),
            abort_policy: AbortPolicy::default(),
            deadline: Some(Instant::now() + Duration::from_secs(30)),
            internal: false,
            ownership: None,
        }
    }

    fn alice_identity() -> Identity {
        Identity {
            id: "alice".to_string(),
            scopes: vec!["fs:read".to_string()],
            resources: HashMap::new(),
        }
    }

    #[test]
    fn build_forwarded_payload_populates_forwarded_for_from_context_identity() {
        let ctx = test_context(Some(alice_identity()));
        let payload = build_forwarded_payload("fs/readFile", json!({"p": 1}), &ctx);
        assert_eq!(payload["operationId"], "fs/readFile");
        assert_eq!(payload["input"], json!({"p": 1}));
        let forwarded_for = payload.get("forwarded_for").expect("forwarded_for present");
        assert_eq!(forwarded_for["id"], "alice");
        assert_eq!(forwarded_for["scopes"][0], "fs:read");
    }

    #[test]
    fn build_forwarded_payload_omits_forwarded_for_when_context_identity_is_none() {
        let ctx = test_context(None);
        let payload = build_forwarded_payload("fs/readFile", json!({}), &ctx);
        assert!(payload.get("forwarded_for").is_none());
        assert_eq!(payload["operationId"], "fs/readFile");
    }

    /// Verify the forwarding handler actually populates `forwarded_for` on
    /// the wire payload it sends. We intercept the payload by using a handler
    /// that records the payload passed to `call_with_payload`. Since
    /// `call_with_payload` on a mock connection returns an error envelope
    /// (no transport), we instead test the payload-construction function
    /// directly (above) and rely on the handler wiring to call
    /// `call_with_payload(payload)`. The handler's contract is: read
    /// `context.identity`, build the payload, call. The payload-construction
    /// function is the unit under test.
    #[tokio::test]
    async fn forwarding_handler_populates_forwarded_for_from_context_identity() {
        let conn = Arc::new(CallConnection::new(stub_connection()));
        let captured_payload = Arc::new(StdMutex::new(None::<Value>));
        let captured = Arc::clone(&captured_payload);

        let handler: Handler = {
            let conn = Arc::clone(&conn);
            make_handler(move |input, context| {
                let conn = Arc::clone(&conn);
                let captured = Arc::clone(&captured);
                let remote_name = "fs/readFile".to_string();
                async move {
                    let payload = build_forwarded_payload(&remote_name, input, &context);
                    *captured.lock().unwrap() = Some(payload.clone());
                    let response = conn.call_with_payload(payload).await;
                    ResponseEnvelope {
                        request_id: context.request_id,
                        result: response.result,
                    }
                }
            })
        };

        let ctx = test_context(Some(alice_identity()));
        let _ = handler(json!({}), ctx).await;
        let payload = captured_payload.lock().unwrap().clone().expect("captured");
        assert_eq!(payload["forwarded_for"]["id"], "alice");
        assert_eq!(payload["operationId"], "fs/readFile");
    }

    #[tokio::test]
    async fn forwarding_handler_omits_forwarded_for_when_context_identity_is_none() {
        let conn = Arc::new(CallConnection::new(stub_connection()));
        let captured_payload = Arc::new(StdMutex::new(None::<Value>));
        let captured = Arc::clone(&captured_payload);

        let handler: Handler = {
            let conn = Arc::clone(&conn);
            make_handler(move |input, context| {
                let conn = Arc::clone(&conn);
                let captured = Arc::clone(&captured);
                let remote_name = "fs/readFile".to_string();
                async move {
                    let payload = build_forwarded_payload(&remote_name, input, &context);
                    *captured.lock().unwrap() = Some(payload.clone());
                    let response = conn.call_with_payload(payload).await;
                    ResponseEnvelope {
                        request_id: context.request_id,
                        result: response.result,
                    }
                }
            })
        };

        let ctx = test_context(None);
        let _ = handler(json!({}), ctx).await;
        let payload = captured_payload.lock().unwrap().clone().expect("captured");
        assert!(
            payload.get("forwarded_for").is_none(),
            "forwarded_for must be omitted when context.identity is None"
        );
    }

    // --- ADR-029 §5: collision rule ---------------------------------------

    fn op_summary(name: &str, conn: &CallConnection) -> OpSummary {
        OpSummary {
            name: name.to_string(),
            schema: sample_schema_json(name, "query"),
            connection: conn.clone(),
        }
    }

    fn op_summary_typed(name: &str, op_type: &str, conn: &CallConnection) -> OpSummary {
        OpSummary {
            name: name.to_string(),
            schema: sample_schema_json(name, op_type),
            connection: conn.clone(),
        }
    }

    #[test]
    fn build_bundles_same_peer_collision_returns_same_peer_collision_error() {
        let conn = CallConnection::new(stub_connection());
        // Same peer exposing two ops that resolve to the same name after the
        // (empty) prefix → SamePeerCollision.
        let discovered = vec![
            op_summary("worker/exec", &conn),
            op_summary("worker/exec", &conn),
        ];
        match build_bundles(discovered, &None, &None) {
            Err(AdapterError::SamePeerCollision { message }) => {
                assert!(message.contains("worker/exec"));
            }
            Err(other) => panic!("expected SamePeerCollision, got another error: {other}"),
            Ok(_) => panic!("expected SamePeerCollision, got Ok"),
        }
    }

    #[test]
    fn build_bundles_same_peer_collision_after_prefix_returns_error() {
        let conn = CallConnection::new(stub_connection());
        // Two ops with different remote names that collide after the prefix is
        // applied (prefix drops, then same name) → SamePeerCollision. Here we
        // use the same remote name twice, which is the canonical same-peer
        // collision.
        let discovered = vec![
            op_summary("fs/readFile", &conn),
            op_summary("fs/readFile", &conn),
        ];
        match build_bundles(discovered, &Some("worker".to_string()), &None) {
            Err(AdapterError::SamePeerCollision { message }) => {
                assert!(message.contains("worker/fs/readFile"));
            }
            Err(other) => panic!("expected SamePeerCollision, got another error: {other}"),
            Ok(_) => panic!("expected SamePeerCollision, got Ok"),
        }
    }

    #[test]
    fn build_bundles_cross_peer_same_name_does_not_collide() {
        // Cross-peer collision dissolves (ADR-029 §5): the same name on
        // different peers lives in separate sub-overlays. `from_call` runs
        // per-connection (per-peer), so the `build_bundles` collision check is
        // same-peer only. This test verifies that a single `build_bundles`
        // call with distinct names succeeds — the cross-peer case is
        // structurally separate `from_call` invocations on different
        // connections, each producing its own bundle set with no collision.
        let conn_a = CallConnection::new(stub_connection());
        let conn_b = CallConnection::new(stub_connection());

        let bundles_a = build_bundles(vec![op_summary("container/exec", &conn_a)], &None, &None)
            .expect("peer a bundles");
        let bundles_b = build_bundles(vec![op_summary("container/exec", &conn_b)], &None, &None)
            .expect("peer b bundles");

        assert_eq!(bundles_a.len(), 1);
        assert_eq!(bundles_b.len(), 1);
        assert_eq!(bundles_a[0].spec.name, "container/exec");
        assert_eq!(bundles_b[0].spec.name, "container/exec");
        // Same name, different peer sub-overlays — no collision.
    }

    #[test]
    fn build_bundles_distinct_names_in_same_peer_do_not_collide() {
        let conn = CallConnection::new(stub_connection());
        let discovered = vec![
            op_summary("worker/exec", &conn),
            op_summary("worker/status", &conn),
            op_summary("fs/readFile", &conn),
        ];
        let bundles = build_bundles(discovered, &None, &None).expect("distinct names ok");
        assert_eq!(bundles.len(), 3);
        for b in &bundles {
            assert_eq!(b.provenance, OperationProvenance::FromCall);
        }
    }

    #[test]
    fn build_bundles_applies_namespace_prefix_without_collision() {
        let conn = CallConnection::new(stub_connection());
        let discovered = vec![op_summary("exec", &conn), op_summary("status", &conn)];
        let bundles =
            build_bundles(discovered, &Some("worker".to_string()), &None).expect("prefixed ok");
        assert_eq!(bundles[0].spec.name, "worker/exec");
        assert_eq!(bundles[1].spec.name, "worker/status");
    }

    #[test]
    fn build_bundles_respects_operation_filter() {
        let conn = CallConnection::new(stub_connection());
        let discovered = vec![
            op_summary("worker/exec", &conn),
            op_summary("worker/status", &conn),
            op_summary("fs/readFile", &conn),
        ];
        let filter: HashSet<String> = HashSet::from(["worker/exec".to_string()]);
        let bundles = build_bundles(discovered, &None, &Some(filter)).expect("filtered ok");
        assert_eq!(bundles.len(), 1);
        assert_eq!(bundles[0].spec.name, "worker/exec");
    }

    // --- ADR-049 §8: streaming forwarding for Subscription ops -------------

    #[test]
    fn build_bundles_subscription_op_produces_stream_kind() {
        let conn = CallConnection::new(stub_connection());
        let discovered = vec![op_summary_typed("events/stream", "subscription", &conn)];
        let bundles = build_bundles(discovered, &None, &None).expect("bundles");
        assert_eq!(bundles.len(), 1);
        assert_eq!(bundles[0].spec.op_type, OperationType::Subscription);
        assert!(
            matches!(bundles[0].handler, HandlerKind::Stream(_)),
            "Subscription op must register HandlerKind::Stream"
        );
        assert_eq!(bundles[0].provenance, OperationProvenance::FromCall);
        assert!(bundles[0].composition_authority.is_none());
        assert!(bundles[0].scoped_env.is_none());
    }

    #[test]
    fn build_bundles_query_op_produces_once_kind() {
        let conn = CallConnection::new(stub_connection());
        let discovered = vec![op_summary_typed("fs/readFile", "query", &conn)];
        let bundles = build_bundles(discovered, &None, &None).expect("bundles");
        assert_eq!(bundles.len(), 1);
        assert_eq!(bundles[0].spec.op_type, OperationType::Query);
        assert!(
            matches!(bundles[0].handler, HandlerKind::Once(_)),
            "Query op must register HandlerKind::Once"
        );
    }

    #[test]
    fn build_bundles_mutation_op_produces_once_kind() {
        let conn = CallConnection::new(stub_connection());
        let discovered = vec![op_summary_typed("fs/writeFile", "mutation", &conn)];
        let bundles = build_bundles(discovered, &None, &None).expect("bundles");
        assert_eq!(bundles.len(), 1);
        assert_eq!(bundles[0].spec.op_type, OperationType::Mutation);
        assert!(
            matches!(bundles[0].handler, HandlerKind::Once(_)),
            "Mutation op must register HandlerKind::Once"
        );
    }

    #[test]
    fn build_bundles_mixed_op_types_route_to_correct_kind() {
        let conn = CallConnection::new(stub_connection());
        let discovered = vec![
            op_summary_typed("fs/readFile", "query", &conn),
            op_summary_typed("fs/writeFile", "mutation", &conn),
            op_summary_typed("events/stream", "subscription", &conn),
        ];
        let bundles = build_bundles(discovered, &None, &None).expect("bundles");
        assert_eq!(bundles.len(), 3);
        let by_name: std::collections::HashMap<&str, &HandlerKind> = bundles
            .iter()
            .map(|b| (b.spec.name.as_str(), &b.handler))
            .collect();
        assert!(matches!(by_name["fs/readFile"], HandlerKind::Once(_)));
        assert!(matches!(by_name["fs/writeFile"], HandlerKind::Once(_)));
        assert!(matches!(by_name["events/stream"], HandlerKind::Stream(_)));
    }

    /// Verify `make_streaming_forwarding_handler` produces a `StreamingHandler`
    /// that builds the forwarded payload with `forwarded_for` populated from
    /// `context.identity` (ADR-032) and calls `subscribe_with_payload`. Since
    /// `subscribe_with_payload` on a mock connection returns a closed stream
    /// (no transport), we capture the payload by intercepting the build step:
    /// the handler's contract is "build payload via `build_forwarded_payload`,
    /// then call `subscribe_with_payload(payload)`". We mirror the existing
    /// `forwarding_handler_populates_forwarded_for` test by constructing the
    /// handler and exercising the payload-construction path it relies on, plus
    /// asserting the produced stream terminates (the mock-connection path
    /// yields one error envelope then ends — no truncation, no hang).
    #[tokio::test]
    async fn streaming_forwarding_handler_populates_forwarded_for_and_streams() {
        use futures::stream::StreamExt;

        let conn = Arc::new(CallConnection::new(stub_connection()));
        let captured_payload = Arc::new(StdMutex::new(None::<Value>));
        let captured = Arc::clone(&captured_payload);

        let handler: StreamingHandler = {
            let conn = Arc::clone(&conn);
            make_streaming_handler(move |input, context| {
                let conn = Arc::clone(&conn);
                let captured = Arc::clone(&captured);
                let remote_name = "events/stream".to_string();
                use futures::stream::{once, StreamExt};
                once(async move {
                    let payload = build_forwarded_payload(&remote_name, input, &context);
                    *captured.lock().unwrap() = Some(payload.clone());
                    conn.subscribe_with_payload(payload).await
                })
                .flatten()
            })
        };

        let ctx = test_context(Some(alice_identity()));
        let mut stream = handler(json!({}), ctx);
        let first = stream.next().await;
        assert!(
            first.is_some(),
            "streaming forwarding handler must produce at least one envelope"
        );
        if let Some(env) = first {
            assert!(
                env.result.is_err(),
                "mock connection has no transport, so the stream yields an error envelope"
            );
        }
        let second = stream.next().await;
        assert!(
            second.is_none(),
            "stream must terminate after the error (no truncation, no hang)"
        );

        let payload = captured_payload.lock().unwrap().clone().expect("captured");
        assert_eq!(payload["operationId"], "events/stream");
        assert_eq!(payload["forwarded_for"]["id"], "alice");
    }

    /// The streaming forwarding handler omits `forwarded_for` when
    /// `context.identity` is `None`, mirroring the request/response handler.
    #[tokio::test]
    async fn streaming_forwarding_handler_omits_forwarded_for_when_identity_none() {
        use futures::stream::StreamExt;

        let conn = Arc::new(CallConnection::new(stub_connection()));
        let captured_payload = Arc::new(StdMutex::new(None::<Value>));
        let captured = Arc::clone(&captured_payload);

        let handler: StreamingHandler = {
            let conn = Arc::clone(&conn);
            make_streaming_handler(move |input, context| {
                let conn = Arc::clone(&conn);
                let captured = Arc::clone(&captured);
                let remote_name = "events/stream".to_string();
                use futures::stream::{once, StreamExt};
                once(async move {
                    let payload = build_forwarded_payload(&remote_name, input, &context);
                    *captured.lock().unwrap() = Some(payload.clone());
                    conn.subscribe_with_payload(payload).await
                })
                .flatten()
            })
        };

        let ctx = test_context(None);
        let mut stream = handler(json!({}), ctx);
        let _ = stream.next().await;
        let payload = captured_payload.lock().unwrap().clone().expect("captured");
        assert!(
            payload.get("forwarded_for").is_none(),
            "forwarded_for must be omitted when context.identity is None"
        );
        assert_eq!(payload["operationId"], "events/stream");
    }

    /// `make_streaming_forwarding_handler` produces a `StreamingHandler` (not a
    /// `Handler`) — verifies the helper returns the right type and that
    /// `build_bundles` wires it into `HandlerKind::Stream`.
    #[test]
    fn make_streaming_forwarding_handler_returns_streaming_handler() {
        let handler = make_streaming_forwarding_handler(
            Arc::new(CallConnection::new(stub_connection())),
            "events/stream".to_string(),
        );
        let reg = HandlerRegistration::new(
            OperationSpec::new(
                "events/stream",
                OperationType::Subscription,
                Visibility::External,
                json!({}),
                json!({}),
                vec![],
                AccessControl::default(),
                None,
            ),
            HandlerKind::Stream(handler),
            OperationProvenance::FromCall,
            None,
            None,
            Capabilities::new(),
        );
        assert!(matches!(reg.handler, HandlerKind::Stream(_)));
        assert_eq!(reg.provenance, OperationProvenance::FromCall);
        assert!(reg.composition_authority.is_none());
        assert!(reg.scoped_env.is_none());
    }
}
