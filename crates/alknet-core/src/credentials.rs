//! Transport-level credential bundle for outbound connections (ADR-091).
//!
//! `ConnectionCredentials` carries the two dimensions the dial consumes:
//! the local node's TLS identity and the expected remote identity.
//! It is transport-agnostic — consumed by `alknet-tls` (TLS setup) and
//! `alknet-client` (dial).

use crate::config::TlsIdentity;

/// Expected identity of the remote node (ADR-017 §7, extended by ADR-034 §2).
///
/// Carries a fingerprint string the assembly layer derives from `Capabilities`
/// when the local node has a `PeerEntry` for the remote (the known-peer case →
/// fingerprint pin).
///
/// `remote_identity: None` is the **public X.509 endpoint** case: the local
/// node has no `PeerEntry` for the remote, so there is no fingerprint to pin.
/// Combined with an X.509 transport, `None` selects CA verification
/// (`WebPkiServerVerifier`) per the verifier-selection rule in ADR-034 §3.
/// Combined with an Ed25519 raw-key transport, `None` fails closed (raw-key
/// remotes are always known peers — no CA to fall back to).
///
/// The `Option` is therefore load-bearing, not cosmetic: `Some(fingerprint)`
/// means "pin this" (known peer), `None` means "trust the CA or fail"
/// (unknown remote). An implementer must not default `remote_identity` to a
/// placeholder value to "satisfy" the field — `None` is a real state that
/// drives verifier selection.
#[derive(Debug, Clone)]
pub struct RemoteIdentity {
    pub fingerprint: String,
}

/// Credentials for an outbound connection (ADR-091). All dimensions come from
/// `Capabilities` (ADR-014), never from environment variables — see the
/// No-Env-Vars Invariant in
/// `docs/architecture/crates/call/client-and-adapters.md`.
#[derive(Debug, Clone, Default)]
pub struct ConnectionCredentials {
    /// The local node's TLS identity (RFC 7250 raw key or X.509), derived
    /// from the vault at startup.
    pub tls_identity: Option<TlsIdentity>,
    /// Expected fingerprint/cert of the remote node, stored as a capability.
    /// `Some` → fingerprint pin (known peer with a `PeerEntry`); `None` → CA
    /// verification for X.509 remotes, fail-closed for Ed25519 raw-key remotes
    /// (ADR-034 §2/§3). `None` is the public-X.509-endpoint state, not a
    /// missing field — must not be defaulted to a placeholder.
    pub remote_identity: Option<RemoteIdentity>,
}

impl ConnectionCredentials {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_tls_identity(mut self, tls_identity: TlsIdentity) -> Self {
        self.tls_identity = Some(tls_identity);
        self
    }

    pub fn with_remote_identity(mut self, remote: RemoteIdentity) -> Self {
        self.remote_identity = Some(remote);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_credentials_builder_methods() {
        let creds = ConnectionCredentials::new().with_remote_identity(RemoteIdentity {
            fingerprint: "SHA256:abc".to_string(),
        });
        assert_eq!(
            creds.remote_identity.as_ref().unwrap().fingerprint,
            "SHA256:abc"
        );
        assert!(creds.tls_identity.is_none());
    }

    #[test]
    fn connection_credentials_none_is_load_bearing_not_defaulted() {
        let creds = ConnectionCredentials::new();
        assert!(
            creds.remote_identity.is_none(),
            "ConnectionCredentials::new() must keep remote_identity as None (the load-bearing \
             public-X.509-endpoint state), not default it to a placeholder"
        );
    }
}
