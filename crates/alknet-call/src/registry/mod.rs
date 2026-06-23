//! Operation registry: specs, handlers, access control, service discovery.
//!
//! Maps operation names to specs and handlers, enforces access control, and
//! dispatches `call.requested` events to local handlers. The registry is
//! layered by trust boundary (ADR-024): a curated layer (immutable after
//! startup) plus dynamic session and connection overlays.

pub mod context;
pub mod discovery;
pub mod env;
pub mod registration;
pub mod spec;
