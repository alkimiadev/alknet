//! alknet-http: HTTP interface for alknet — serves HTTP/1.1 + HTTP/2 on
//! standard ALPNs (with WebSocket upgrade for browser bidirectional access)
//! and hosts the HTTP-backed call-protocol adapters.
//!
//! Two roles in one crate (ADR-039): HTTP server (HttpAdapter, a
//! ProtocolHandler for h2/http1.1 + WS upgrade) and HTTP client host
//! (from_openapi/from_mcp forwarding, to_openapi/to_mcp projections).

pub mod adapters;
pub mod client;
pub mod gateway;
pub mod server;
pub mod websocket;
