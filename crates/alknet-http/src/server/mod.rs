//! HTTP server: `HttpAdapter`, axum-over-QUIC, gateway routes, `/healthz`,
//! decoy, and custom routes.
//!
//! Implements `alknet_core::types::ProtocolHandler` for the standard HTTP
//! ALPNs (`h2`, `http/1.1`) with WebSocket upgrade for browser
//! bidirectional access (ADR-044). See
//! `docs/architecture/crates/http/http-server.md`.

pub mod adapter;
pub mod decoy;
pub mod healthz;

pub use adapter::{DecoyConfig, HttpAdapter};
pub use decoy::decoy_fallback;
pub use healthz::healthz;