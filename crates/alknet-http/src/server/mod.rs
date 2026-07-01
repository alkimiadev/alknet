//! HTTP server: `HttpAdapter`, axum-over-QUIC, gateway routes, `/healthz`,
//! decoy, and custom routes.
//!
//! Implements `alknet_core::types::ProtocolHandler` for the standard HTTP
//! ALPNs (`h2`, `http/1.1`) with WebSocket upgrade for browser
//! bidirectional access (ADR-044). See
//! `docs/architecture/crates/http/http-server.md`.

pub mod adapter;

pub use adapter::{DecoyConfig, HttpAdapter};