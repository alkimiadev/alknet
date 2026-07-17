//! Dial methods for `AlknetClient` — one per transport.
//!
//! Each dial method is feature-gated on the corresponding transport feature.
//! All three are unified on `&ConnectionCredentials` (ADR-091) and return a
//! `Connection` for protocol take-overs to consume.

#[cfg(feature = "quinn")]
pub mod quinn;
#[cfg(feature = "tcp")]
pub mod tcp_tls;
#[cfg(feature = "iroh")]
pub mod iroh;
