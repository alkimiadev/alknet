//! Call protocol layer (Layer 3) of the three-layer model.
//!
//! See [ADR-024](docs/architecture/decisions/024-call-protocol.md) and
//! [ADR-033](docs/architecture/decisions/033-call-protocol-extensions.md).

pub mod context;
pub mod env;
pub mod envelope;
pub mod events;
pub mod frame;
pub mod pending;
pub mod registry;
pub mod response;
pub mod services;
pub mod spec;

pub use context::OperationContext;
pub use env::OperationEnv;
pub use envelope::EventEnvelope;
pub use events::{CALL_ABORTED, CALL_COMPLETED, CALL_ERROR, CALL_REQUESTED, CALL_RESPONDED};
pub use frame::{
    decode, decode_with_remainder, encode, FrameDecodeError, FrameFramedReader, FrameFramedWriter,
};
pub use pending::PendingRequestMap;
pub use registry::{Handler, OperationRegistry, OperationRegistryBuilder};
pub use response::{CallError, ResponseEnvelope};
pub use services::{register_default_operations, services_list_spec, services_schema_spec};
pub use spec::{AccessControl, OperationSpec, OperationType};
