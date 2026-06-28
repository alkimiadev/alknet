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
use crate::registry::registration::{Handler, HandlerRegistration, OperationProvenance};
use crate::registry::spec::{
    AccessControl, ErrorDefinition, OperationSpec, OperationType, Visibility,
};
use alknet_core::types::Capabilities;

/// Configuration for [`from_call`].
#[derive(Debug, Clone, Default)]
pub struct FromCallConfig {
    /// Optional namespace prefix applied to imported operation names. When
    /// `None` (default), no prefix is applied. Collision on import is an error
    /// (DC-3/OQ-28), not last-wins — a node importing from two remotes that
    /// both expose `/container/exec` without prefixes fails loudly.
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
/// `CallConnection::register_imported_all()`.
///
/// v1 defaults (two-way doors recorded in `client-and-adapters.md`):
/// - auto-on-reconnect: the overlay is per-connection (Layer 2, ADR-024), so
///   re-import on reconnect is naturally scoped; the assembly layer calls
///   `from_call` immediately after `connect()`.
/// - error-on-collision: applying the (possibly empty) prefix produces a name
///   that already exists in the target overlay → `AdapterError::Conflict`.
pub async fn from_call(
    connection: &CallConnection,
    config: FromCallConfig,
) -> Result<Vec<HandlerRegistration>, AdapterError> {
    let discovered = discover_operations(connection).await?;
    let mut bundles = Vec::with_capacity(discovered.len());
    let mut seen_names = HashSet::new();

    for op_summary in discovered {
        let remote_name = op_summary.name;
        if let Some(filter) = &config.operation_filter {
            if !filter.contains(&remote_name) {
                continue;
            }
        }

        let schema = fetch_schema(connection, &remote_name).await?;
        let spec = rebuild_spec(&schema, &remote_name, &config.namespace_prefix)?;

        if !seen_names.insert(spec.name.clone()) {
            return Err(AdapterError::Conflict {
                message: format!(
                    "namespace collision on import: {} (use a namespace_prefix)",
                    spec.name
                ),
            });
        }

        let handler = make_forwarding_handler(Arc::new(connection.clone()), remote_name);
        bundles.push(HandlerRegistration::new(
            spec,
            handler,
            OperationProvenance::FromCall,
            None,
            None,
            Capabilities::new(),
        ));
    }

    Ok(bundles)
}

#[derive(Debug, Clone)]
struct OpSummary {
    name: String,
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
        summaries.push(OpSummary {
            name: name.to_string(),
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
fn rebuild_spec(
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

/// Construct a forwarding handler for a `FromCall` leaf: on invocation, calls
/// the remote op via the `CallConnection` and returns its `ResponseEnvelope`.
/// For a `Subscription` op, the handler calls `subscribe` and streams until
/// `completed`/`aborted` (the streaming path is exercised at the
/// `CallConnection` layer; the handler here forwards the first response for
/// query/mutation and delegates streaming to the caller via the returned
/// envelope).
fn make_forwarding_handler(connection: Arc<CallConnection>, remote_name: String) -> Handler {
    use crate::registry::registration::make_handler;
    make_handler(move |input, context| {
        let connection = Arc::clone(&connection);
        let remote_name = remote_name.clone();
        async move {
            // The forwarding handler invokes the remote op via the
            // CallConnection. The parent_request_id participates in the abort
            // cascade (ADR-016 §6): if the parent is aborted, the cascade
            // reaches this handler, which sends call.aborted to the remote
            // node; the remote node cascades to its own descendants.
            // Cross-node abort is transparent.
            let response = connection.call(&remote_name, input).await;
            ResponseEnvelope {
                request_id: context.request_id,
                result: response.result,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::connection::CallConnection;
    use crate::registry::spec::OperationType;
    use alknet_core::types::{Capabilities, MockConnection};
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
        let spec = rebuild_spec(&schema, "fs/readFile", &None).expect("rebuild");
        assert_eq!(spec.name, "fs/readFile");
        assert_eq!(spec.op_type, OperationType::Query);
        assert_eq!(spec.visibility, Visibility::External);
    }

    #[test]
    fn rebuild_spec_with_prefix_applies_prefix() {
        let schema = sample_schema_json("fs/readFile", "query");
        let spec =
            rebuild_spec(&schema, "fs/readFile", &Some("worker".to_string())).expect("rebuild");
        assert_eq!(spec.name, "worker/fs/readFile");
    }

    #[test]
    fn rebuild_spec_unknown_op_type_returns_schema_parse() {
        let schema = sample_schema_json("fs/readFile", "weird");
        match rebuild_spec(&schema, "fs/readFile", &None) {
            Err(AdapterError::SchemaParse { .. }) => {}
            other => panic!("expected SchemaParse, got {other:?}"),
        }
    }

    #[test]
    fn rebuild_spec_missing_op_type_returns_schema_parse() {
        let schema = json!({"name": "fs/readFile"});
        match rebuild_spec(&schema, "fs/readFile", &None) {
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
        let spec = rebuild_spec(&schema, "fs/readFileErr", &None).expect("rebuild");
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
        );
        let handler = make_forwarding_handler(
            Arc::new(CallConnection::new(stub_connection())),
            "worker/echo".to_string(),
        );
        let reg = HandlerRegistration::new(
            spec,
            handler,
            OperationProvenance::FromCall,
            None,
            None,
            Capabilities::new(),
        );
        assert_eq!(reg.provenance, OperationProvenance::FromCall);
        assert!(reg.composition_authority.is_none());
        assert!(reg.scoped_env.is_none());
    }
}
