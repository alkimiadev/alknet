//! alknet-call: Structured RPC — operations, streaming, service discovery.
//!
//! Implements [`alknet_core::types::ProtocolHandler`] on ALPN `alknet/call`.
//!
//! The crate has two subsystems:
//! - [`registry`] — operation specs, context, dispatch, and the operation registry.
//! - [`protocol`] — wire format, streams, and the call adapter.

pub mod client;
pub mod protocol;
pub mod registry;
