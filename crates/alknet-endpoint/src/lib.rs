//! alknet-endpoint: Server-side multi-transport accept-loop runner.
//!
//! `AlknetEndpoint` takes pre-built transports (quinn, iroh, TCP+TLS) via
//! builder methods, runs their accept loops inside `run()`, and dispatches
//! each accepted connection to the registered `ProtocolHandler` by ALPN.
//!
//! The endpoint does not build transports and does not depend on
//! `alknet-tls` — transport construction is the assembly layer's concern.

pub mod accept;
pub mod dispatch;
pub mod endpoint;
pub mod registry;

pub use endpoint::AlknetEndpoint;
pub use registry::HandlerRegistry;
