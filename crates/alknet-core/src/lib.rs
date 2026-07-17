//! alknet-core: Core library for ALPN-based protocol dispatch.
//!
//! Every handler crate depends on this crate. It provides the
//! [`ProtocolHandler`][crate::types::ProtocolHandler] trait, the
//! [`Connection`][crate::types::Connection] wrapper, auth primitives,
//! hot-reloadable configuration, and the [`AlknetEndpoint`][crate::endpoint::AlknetEndpoint]
//! that dispatches incoming QUIC connections by ALPN string.

pub mod auth;
pub mod config;
pub mod credentials;
pub mod endpoint;
pub mod fingerprint;
pub mod ownership;
pub mod store;
pub mod types;

pub use auth::{IdentityProvider, IdentityStore};
pub use credentials::{ConnectionCredentials, RemoteIdentity};
pub use ownership::{InMemoryOwnershipStore, OwnershipError, OwnershipProvider, OwnershipStore};
pub use store::{CredentialStore, EncryptedData, InMemoryCredentialStore, StoreError};
