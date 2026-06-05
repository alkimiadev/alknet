//! SSH authentication (Ed25519 public key and OpenSSH certificate authority).
//!
//! Supports file-path and in-memory key sources. No password authentication.
//! See ADR-012 for the design rationale.

pub mod client_auth;
pub mod keys;
pub mod server_auth;

pub use client_auth::{ClientAuthConfig, ClientHandler};
pub use keys::{CertAuthorityEntry, KeySource, load_private_key, load_public_keys};
pub use server_auth::ServerAuthConfig;