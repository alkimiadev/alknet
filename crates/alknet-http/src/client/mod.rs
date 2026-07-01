//! Shared HTTP client (`ClientWithMiddleware`): reqwest + retry middleware
//! stack, used by `from_openapi`/`from_mcp` forwarding handlers.
//!
//! See `docs/architecture/crates/http/http-adapters.md` and OQ-40.

mod http_client;
mod retry_after;

pub use http_client::{ClientCertConfig, HttpClientBuildError, HttpClientConfig, SharedHttpClient};
pub use retry_after::RetryAfterMiddleware;