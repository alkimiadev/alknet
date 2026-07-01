//! HTTP server: `HttpAdapter`, axum-over-QUIC, gateway routes, `/healthz`,
//! decoy, custom routes, and shared Bearer auth middleware.
//!
//! Implements `alknet_core::types::ProtocolHandler` for the standard HTTP
//! ALPNs (`h2`, `http/1.1`) with WebSocket upgrade for browser
//! bidirectional access (ADR-044). See
//! `docs/architecture/crates/http/http-server.md`.

pub mod adapter;
pub mod auth;
pub mod decoy;
pub mod gateway_routes;
pub mod healthz;

pub use adapter::{DecoyConfig, HttpAdapter};
pub use auth::{bearer_auth_middleware, extract_bearer_identity, ResolvedIdentity};
pub use decoy::decoy_fallback;
pub use healthz::healthz;
