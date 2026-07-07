//! alknet-tty: Terminal session protocol handler for the `alknet/tty` ALPN.
//!
//! Two-carriage model (ADR-052): a JSON negotiation frame, then raw chunks
//! (`[stream_type: u8][length: u32 be][payload]`). Backend-agnostic via the
//! `TtyBackend` trait (ADR-053). Depends on alknet-core only (ADR-057).
//!
//! # Local backend
//!
//! The local backend (`LocalTtyBackend`) lives in the sibling crate
//! `alknet-tty-local` (ADR-054). ADR-054 prescribes a `local` feature gate
//! that re-exports `alknet_tty_local::LocalTtyBackend` as
//! `alknet_tty::local::LocalTtyBackend`, but cargo rejects the cyclic
//! dependency (`alknet-tty → alknet-tty-local → alknet-tty`) even with an
//! optional dependency declaration. The re-export is therefore handled at
//! the assembly layer: a consumer that wants the local backend depends on
//! `alknet-tty-local` directly and registers `LocalTtyBackend` in the
//! `TtyAdapter` backend map.
//!
//! # Assembly pattern
//!
//! ```ignore
//! let mut backends = std::collections::HashMap::new();
//! backends.insert(
//!     "local".into(),
//!     std::sync::Arc::new(alknet_tty_local::LocalTtyBackend::new())
//!         as std::sync::Arc<dyn alknet_tty::TtyBackend>,
//! );
//! let tty_adapter = alknet_tty::adapter::TtyAdapter::new(backends);
//! ```

pub mod adapter;
pub mod backend;
pub mod control;
pub mod negotiation;
pub mod wire;
