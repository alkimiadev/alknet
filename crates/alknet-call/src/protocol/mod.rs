//! Call protocol: wire format, streams, and the call adapter.
//!
//! Implements `ProtocolHandler` for ALPN `alknet/call` on top of the
//! operation registry. See `docs/architecture/crates/call/call-protocol.md`
//! for the full specification.

pub mod abort;
pub mod adapter;
pub mod connection;
pub mod dispatch;
pub mod pending;
pub mod wire;
