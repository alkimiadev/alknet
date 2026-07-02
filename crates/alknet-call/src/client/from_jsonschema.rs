//! Schema-only registration: produce a `HandlerRegistration` bundle with
//! `FromJsonSchema` provenance and no real handler. The caller fetches the
//! JSON Schema doc and passes it in; this adapter does no network I/O.
//!
//! See `docs/architecture/crates/call/client-and-adapters.md` (from_jsonschema
//! section) and ADR-017 §5.

use alknet_core::types::Capabilities;
use serde_json::Value;

use crate::client::{AdapterError, OperationAdapter};
use crate::protocol::wire::{CallError, ResponseEnvelope};
use crate::registry::context::OperationContext;
use crate::registry::registration::{
    make_handler, HandlerKind, HandlerRegistration, OperationProvenance,
};
use crate::registry::spec::OperationSpec;

/// Build a [`HandlerRegistration`] from a JSON Schema-described operation.
///
/// Schema-only: no real handler is attached — a placeholder returns a
/// `NOT_FOUND`-style error if ever invoked (schema-only ops are `Internal`,
/// so dispatch should never reach them; the placeholder fails loudly on
/// bugs). `provenance` is `FromJsonSchema`; `composition_authority` and
/// `scoped_env` are `None`; `capabilities` is empty.
pub fn from_jsonschema(spec: OperationSpec, _schema: Value) -> HandlerRegistration {
    let handler = make_handler(|_input: Value, context: OperationContext| async move {
        ResponseEnvelope::error(
            context.request_id,
            CallError::not_found("FromJsonSchema ops are schema-only and have no handler"),
        )
    });
    HandlerRegistration::new(
        spec,
        HandlerKind::Once(handler),
        OperationProvenance::FromJsonSchema,
        None,
        None,
        Capabilities::new(),
    )
}

/// A JSON-Schema-only [`OperationAdapter`].
///
/// Pure parse — no transport, no `.await` in `import()`. Returns
/// [`AdapterError::SchemaParse`] when the supplied schema is not a JSON
/// object.
pub struct FromJsonSchema {
    spec: OperationSpec,
    schema: Value,
}

impl FromJsonSchema {
    pub fn new(spec: OperationSpec, schema: Value) -> Self {
        Self { spec, schema }
    }
}

#[async_trait::async_trait]
impl OperationAdapter for FromJsonSchema {
    async fn import(&self) -> Result<Vec<HandlerRegistration>, AdapterError> {
        if !self.schema.is_object() {
            return Err(AdapterError::SchemaParse {
                message: "schema must be a JSON object".into(),
            });
        }
        Ok(vec![from_jsonschema(
            self.spec.clone(),
            self.schema.clone(),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::from_jsonschema as from_jsonschema_fn;
    use crate::registry::context::{AbortPolicy, ScopedPeerEnv};
    use crate::registry::env::OperationEnv;
    use crate::registry::spec::{AccessControl, OperationType, Visibility};
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

    struct NoopEnv;

    #[async_trait::async_trait]
    impl OperationEnv for NoopEnv {
        async fn invoke_with_policy(
            &self,
            _namespace: &str,
            _operation: &str,
            _input: Value,
            parent: &OperationContext,
            _policy: AbortPolicy,
        ) -> ResponseEnvelope {
            ResponseEnvelope::ok(parent.request_id.clone(), Value::Null)
        }
    }

    fn test_spec(name: &str) -> OperationSpec {
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

    fn test_context(request_id: &str) -> OperationContext {
        OperationContext {
            request_id: request_id.to_string(),
            parent_request_id: None,
            identity: None,
            handler_identity: None,
            forwarded_for: None,
            capabilities: Capabilities::new(),
            metadata: HashMap::new(),
            scoped_env: ScopedPeerEnv::empty(),
            env: Arc::new(NoopEnv),
            abort_policy: AbortPolicy::default(),
            deadline: Some(std::time::Instant::now() + Duration::from_secs(30)),
            internal: true,
        }
    }

    #[test]
    fn from_jsonschema_bundle_shape() {
        let bundle = from_jsonschema_fn::from_jsonschema(test_spec("ns/op"), serde_json::json!({}));
        assert_eq!(bundle.spec.name, "ns/op");
        assert_eq!(bundle.provenance, OperationProvenance::FromJsonSchema);
        assert!(bundle.composition_authority.is_none());
        assert!(bundle.scoped_env.is_none());
    }

    #[tokio::test]
    async fn placeholder_handler_returns_error_when_invoked() {
        let bundle = from_jsonschema_fn::from_jsonschema(test_spec("ns/op"), serde_json::json!({}));
        let ctx = test_context("req-1");
        let response = match &bundle.handler {
            HandlerKind::Once(h) => h(serde_json::json!({}), ctx).await,
            _ => panic!("expected Once handler"),
        };
        match response.result {
            Err(e) => {
                assert_eq!(e.code, "NOT_FOUND");
                assert!(e.message.contains("FromJsonSchema"));
            }
            other => panic!("expected NOT_FOUND, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn import_returns_ok_with_one_bundle() {
        let adapter =
            FromJsonSchema::new(test_spec("ns/op"), serde_json::json!({"type": "object"}));
        let bundles = match adapter.import().await {
            Ok(b) => b,
            Err(e) => panic!("expected Ok, got Err: {e}"),
        };
        assert_eq!(bundles.len(), 1);
        assert_eq!(bundles[0].provenance, OperationProvenance::FromJsonSchema);
    }

    #[tokio::test]
    async fn import_non_object_schema_returns_schema_parse() {
        let adapter = FromJsonSchema::new(test_spec("ns/op"), serde_json::json!(42));
        match adapter.import().await {
            Ok(_) => panic!("expected Err"),
            Err(AdapterError::SchemaParse { message }) => {
                assert!(message.contains("JSON object"));
            }
            Err(other) => panic!("expected SchemaParse, got {other}"),
        }
    }
}
