//! alknet-tty: Terminal session protocol handler for the `alknet/tty` ALPN.
//!
//! Two-carriage model (ADR-052): a JSON negotiation frame, then raw chunks
//! (`[stream_type: u8][length: u32 be][payload]`). Backend-agnostic via the
//! `TtyBackend` trait (ADR-053). Depends on alknet-core only (ADR-057).

pub mod adapter;
pub mod backend;
pub mod control;
pub mod negotiation;
pub mod wire;
