//! Authentication: `AuthContext`, `Identity`, `IdentityProvider`, `AuthToken`,
//! `ConfigIdentityProvider`.
//!
//! See `docs/architecture/crates/core/auth.md` for the full specification.

use std::collections::HashMap;
use std::net::SocketAddr;

#[derive(Debug, Clone, PartialEq)]
pub struct Identity {
    pub id: String,
    pub scopes: Vec<String>,
    pub resources: HashMap<String, Vec<String>>,
}

#[derive(Clone)]
pub struct AuthContext {
    pub identity: Option<Identity>,
    pub alpn: Vec<u8>,
    pub remote_addr: Option<SocketAddr>,
    pub tls_client_fingerprint: Option<String>,
}
