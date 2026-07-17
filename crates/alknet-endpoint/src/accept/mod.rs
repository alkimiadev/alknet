//! Transport-specific accept loops.
//!
//! Each module contains a `run_accept_loop` function that accepts
//! connections on its transport, extracts ALPN + fingerprint, and
//! calls `crate::dispatch::dispatch_connection`.

#[cfg(feature = "quinn")]
pub(crate) mod quinn;

#[cfg(feature = "iroh")]
pub(crate) mod iroh;

#[cfg(feature = "tcp")]
pub(crate) mod tcp_tls;
