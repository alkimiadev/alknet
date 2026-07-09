//! HTTP-backed call-protocol adapters: `from_openapi`, `to_openapi`,
//! `from_mcp`, `to_mcp`.
//!
//! `from_openapi`/`from_mcp` are the no-env-vars credential injection point
//! (ADR-014); `to_openapi`/`to_mcp` are projections of the local registry
//! (ADR-017). `from_mcp`/`to_mcp` are feature-gated behind `mcp`
//! (streamable HTTP only — ADR-037). See
//! `docs/architecture/crates/http/http-adapters.md` and
//! `docs/architecture/crates/http/http-mcp.md`.

pub mod from_jsonschema;
pub mod from_openapi;

#[cfg(feature = "mcp")]
pub mod from_mcp;

#[cfg(feature = "mcp")]
pub mod to_mcp;

pub mod to_openapi;

pub use from_jsonschema::FromJsonSchema;
pub use from_openapi::{FromOpenAPI, HttpAuthScheme, HttpServiceConfig, OpenAPISpec};
pub use to_openapi::to_openapi;

#[cfg(feature = "mcp")]
pub use from_mcp::FromMCP;

#[cfg(feature = "mcp")]
pub use to_mcp::{to_mcp_service, ToMcpGateway, ToMcpService};
