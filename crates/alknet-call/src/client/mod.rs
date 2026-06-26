//! Client adapters: turn external operation sources (JSON Schema, OpenAPI,
//! MCP, remote `from_call` peers) into `HandlerRegistration` bundles.
//!
//! See `docs/architecture/crates/call/client-and-adapters.md` for the
//! OperationAdapter trait and the Adapter Location Map, and
//! `docs/architecture/decisions/017-call-protocol-client-and-adapter-contract.md`
//! §5 for the trait contract.

mod call_client;
mod from_call;
mod from_jsonschema;

pub use call_client::{CallClient, CallCredentials, ClientError, RemoteIdentity};
pub use from_call::{from_call, FromCallConfig};
pub use from_jsonschema::{from_jsonschema, FromJsonSchema};

use crate::registry::registration::HandlerRegistration;

/// Errors produced by [`OperationAdapter::import`].
///
/// The variant set is the v1 default (two-way-door remainder, OQ-26);
/// `#[non_exhaustive]` lets downstream adapters (e.g. `alknet-http`'s
/// `from_openapi`/`from_mcp`) extend without breaking match arms. All
/// payloads are string messages — kept simple and `Send + Sync` by
/// construction.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AdapterError {
    /// `from_call` remote unreachable / `services/list` failed.
    #[error("discovery failed: {message}")]
    DiscoveryFailed { message: String },

    /// `from_openapi` / `from_jsonschema` couldn't parse the spec.
    #[error("schema parse error: {message}")]
    SchemaParse { message: String },

    /// Underlying transport error (QUIC for `from_call`, HTTP for adapters).
    #[error("transport error: {message}")]
    Transport { message: String },

    /// HTTP 401 for `from_openapi`/`from_mcp`, auth rejected for `from_call`.
    #[error("unauthorized: {message}")]
    Unauthorized { message: String },

    /// Namespace collision in `from_call` (DC-3); reused for other adapters.
    #[error("conflict: {message}")]
    Conflict { message: String },
}

/// Import a set of operations as `HandlerRegistration` bundles.
///
/// Async because `from_call` requires async discovery (`services/list` +
/// `services/schema` over a QUIC connection); sync adapters (e.g.
/// `from_jsonschema`, `from_openapi` reading a static spec) trivially satisfy
/// an async trait — their `import()` bodies contain no `.await` points.
///
/// See ADR-017 §5 (`docs/architecture/decisions/017-call-protocol-client-and-adapter-contract.md`)
/// and `docs/architecture/crates/call/client-and-adapters.md`.
#[async_trait::async_trait]
pub trait OperationAdapter: Send + Sync {
    async fn import(&self) -> Result<Vec<HandlerRegistration>, AdapterError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct OkAdapter;

    #[async_trait::async_trait]
    impl OperationAdapter for OkAdapter {
        async fn import(&self) -> Result<Vec<HandlerRegistration>, AdapterError> {
            Ok(vec![])
        }
    }

    struct ErrAdapter;

    #[async_trait::async_trait]
    impl OperationAdapter for ErrAdapter {
        async fn import(&self) -> Result<Vec<HandlerRegistration>, AdapterError> {
            Err(AdapterError::SchemaParse {
                message: "x".into(),
            })
        }
    }

    #[tokio::test]
    async fn ok_adapter_imports_empty() {
        let adapter = OkAdapter;
        match adapter.import().await {
            Ok(bundles) => assert!(bundles.is_empty()),
            Err(e) => panic!("expected Ok, got Err: {e}"),
        }
    }

    #[tokio::test]
    async fn err_adapter_returns_schema_parse() {
        let adapter = ErrAdapter;
        match adapter.import().await {
            Ok(_) => panic!("expected Err"),
            Err(AdapterError::SchemaParse { message }) => assert_eq!(message, "x"),
            Err(other) => panic!("expected SchemaParse, got {other}"),
        }
    }
}
