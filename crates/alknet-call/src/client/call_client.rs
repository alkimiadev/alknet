//! `CallClient`: the outbound connection opener (ADR-017 §1, ADR-028).
//!
//! Opens a QUIC connection to a remote node on ALPN `alknet/call`, performs
//! credential setup, and produces a [`CallConnection`] running the shared
//! dispatch loop (delegated to [`crate::protocol::dispatch::Dispatcher`]).
//! `CallClient` is the connection-establishment half; `CallAdapter`'s accept
//! path is the inbound half; both produce a `CallConnection` and hand it to
//! the same `Dispatcher::run_loop` (ADR-017 §1).
//!
//! After establishment the connection is symmetric (ADR-017 §2): both sides
//! can send and receive `call.requested`. The `CallClient` is both a caller
//! (initiates outgoing calls via `CallConnection::call()`/`subscribe()`/
//! `abort()`) and a callee (dispatches incoming calls against its
//! peer-scoped view of the registry).
//!
//! See `docs/architecture/crates/call/client-and-adapters.md` for the spec.

use std::net::SocketAddr;
use std::sync::Arc;

use alknet_core::auth::IdentityProvider;
use alknet_core::config::TlsIdentity;
use alknet_core::types::Connection;

use crate::protocol::connection::CallConnection;
use crate::protocol::dispatch::{Dispatcher, RemoteFilter};
use crate::registry::registration::OperationRegistry;

/// Expected identity of the remote node (ADR-017 §7). The concrete shape is
/// an implementation-detail two-way door; v1 carries a fingerprint string the
/// assembly layer derives from `Capabilities` (ADR-014). Verification is the
/// assembly layer's trust decision — `CallClient` surfaces the expected value
/// so the transport can pin it, but the v1 quinn client config does not enforce
/// a specific verifier (recorded as a two-way-door remainder).
#[derive(Debug, Clone)]
pub struct RemoteIdentity {
    pub fingerprint: String,
}

/// Credentials for an outbound `alknet/call` connection (ADR-017 §7). All
/// three dimensions come from `Capabilities` (ADR-014), never from environment
/// variables — see the No-Env-Vars Invariant in
/// `docs/architecture/crates/call/client-and-adapters.md`.
#[derive(Debug, Clone, Default)]
pub struct CallCredentials {
    /// The local node's TLS identity (RFC 7250 raw key or X.509), derived
    /// from the vault at startup.
    pub tls_identity: Option<TlsIdentity>,
    /// Opaque call-protocol-level auth token, decrypted from the vault.
    pub auth_token: Option<alknet_core::auth::AuthToken>,
    /// Expected fingerprint/cert of the remote node, stored as a capability.
    pub remote_identity: Option<RemoteIdentity>,
}

impl CallCredentials {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_tls_identity(mut self, tls_identity: TlsIdentity) -> Self {
        self.tls_identity = Some(tls_identity);
        self
    }

    pub fn with_auth_token(mut self, token: alknet_core::auth::AuthToken) -> Self {
        self.auth_token = Some(token);
        self
    }

    pub fn with_remote_identity(mut self, remote: RemoteIdentity) -> Self {
        self.remote_identity = Some(remote);
        self
    }
}

/// Errors produced by [`CallClient::connect`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ClientError {
    #[error("transport error: {message}")]
    Transport { message: String },
    #[error("tls setup error: {message}")]
    TlsSetup { message: String },
    #[error("connection closed")]
    ConnectionClosed,
}

/// Outbound `alknet/call` connection opener (the #1 gap, ADR-017 §1).
///
/// The peer-scoped registry view is a dispatch-time read over the single
/// Layer-0 registry (ADR-028 §5) — not a copy. In default mode
/// (`trusted_peer: false`) only registrations with `remote_safe: true`
/// dispatch to the remote peer, and `services/list` hides non-remote-safe
/// ops (ADR-028 Assumption 2). In trusted-peer mode (`trusted_peer: true`,
/// explicit opt-in per ADR-028 §3) all `External` ops dispatch and list.
pub struct CallClient {
    registry: Arc<OperationRegistry>,
    identity_provider: Arc<dyn IdentityProvider>,
    trusted_peer: bool,
}

impl CallClient {
    /// Default-deny mode: only `remote_safe: true` ops dispatch/list to the
    /// remote peer (ADR-028).
    pub fn new(
        registry: Arc<OperationRegistry>,
        identity_provider: Arc<dyn IdentityProvider>,
    ) -> Self {
        Self {
            registry,
            identity_provider,
            trusted_peer: false,
        }
    }

    /// Trusted-peer mode: expose all `External` ops to the remote peer,
    /// ignoring the `remote_safe` marking. Explicit opt-in per ADR-028 §3.
    pub fn trusted_peer(
        registry: Arc<OperationRegistry>,
        identity_provider: Arc<dyn IdentityProvider>,
    ) -> Self {
        Self {
            registry,
            identity_provider,
            trusted_peer: true,
        }
    }

    pub fn registry(&self) -> &Arc<OperationRegistry> {
        &self.registry
    }

    pub fn identity_provider(&self) -> &Arc<dyn IdentityProvider> {
        &self.identity_provider
    }

    pub fn is_trusted_peer(&self) -> bool {
        self.trusted_peer
    }

    /// Open a QUIC connection to `addr` on ALPN `alknet/call`, perform
    /// credential handshake, and return a `CallConnection` running the shared
    /// dispatch loop. Credentials come from `Capabilities` (ADR-014), not env
    /// vars — the no-env-vars invariant.
    ///
    /// The dispatch loop runs on a spawned task; the returned `CallConnection`
    /// is live until the remote closes the connection or the caller drops it.
    /// The caller can immediately use `call()`/`subscribe()`/`abort()` on the
    /// returned connection, and the remote peer can call back into this
    /// `CallClient`'s peer-scoped registry view (connection symmetry,
    /// ADR-017 §2).
    #[cfg(feature = "quinn")]
    pub async fn connect(
        &self,
        addr: SocketAddr,
        credentials: CallCredentials,
    ) -> Result<CallConnection, ClientError> {
        let alpn = b"alknet/call".to_vec();
        let client_config = build_quinn_client_config(&credentials, &alpn)
            .map_err(|e| ClientError::TlsSetup { message: e })?;

        let bind_addr: SocketAddr = "0.0.0.0:0".parse().expect("valid bind addr");
        let endpoint = quinn::Endpoint::client(bind_addr).map_err(|e| ClientError::Transport {
            message: e.to_string(),
        })?;

        let connection = endpoint
            .connect_with(client_config, addr, "alknet")
            .map_err(|e| ClientError::Transport {
                message: e.to_string(),
            })?
            .await
            .map_err(|e| ClientError::Transport {
                message: e.to_string(),
            })?;

        let connection = Connection::from_quinn_with_alpn(connection, alpn);
        Ok(self.spawn_dispatch(connection))
    }

    /// Run the shared dispatch loop over a pre-established `Connection`. The
    /// `CallClient` spawns the dispatcher task and returns a live
    /// `CallConnection` the caller can use immediately. Used by `connect()`
    /// (after the QUIC dial completes) and by integration tests that wire a
    /// mock/loopback `Connection` directly.
    pub fn spawn_dispatch(&self, connection: Connection) -> CallConnection {
        let call_connection = Arc::new(CallConnection::new(connection));
        let dispatcher = Dispatcher::new(
            Arc::clone(&self.registry),
            Arc::clone(&self.identity_provider),
            if self.trusted_peer {
                RemoteFilter::trusted()
            } else {
                RemoteFilter::default_deny()
            },
        );
        let run_conn = Arc::clone(&call_connection);
        tokio::spawn(async move {
            dispatcher.run_loop(run_conn).await;
        });
        (*call_connection).clone()
    }
}

#[cfg(feature = "quinn")]
fn build_quinn_client_config(
    _credentials: &CallCredentials,
    alpn: &[u8],
) -> Result<quinn::ClientConfig, String> {
    // TODO(OQ-29): connects without client-auth TLS identity. The server-side
    // `AcceptAnyCertVerifier` (in alknet-core::endpoint) requests but does not
    // verify client certs, so a client cert is not needed to establish a
    // connection. However, without a client cert, the server cannot extract a
    // fingerprint, so `IdentityProvider::resolve_from_fingerprint` returns
    // None and the peer gets no stable `PeerEntry.peer_id` (ADR-030). This is
    // load-bearing on ADR-030's peer-identity model — see OQ-29 for the
    // decision needed before the ADR-029 migration lands.
    //
    // The `credentials.tls_identity` field is carried through `CallCredentials`
    // so the assembly layer can populate it; wiring it into the rustls client
    // config is the missing piece. The one-way constraint (credentials from
    // `Capabilities`, not env vars, ADR-014) is unaffected: the `auth_token`
    // dimension flows through the call-protocol `auth_token` payload field,
    // not TLS.
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyServerCertVerifier))
        .with_no_client_auth();
    config.alpn_protocols = vec![alpn.to_vec()];
    config.enable_early_data = true;

    Ok(quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(config).map_err(|e| e.to_string())?,
    )))
}

#[cfg(feature = "quinn")]
struct AcceptAnyServerCertVerifier;

#[cfg(feature = "quinn")]
impl std::fmt::Debug for AcceptAnyServerCertVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcceptAnyServerCertVerifier").finish()
    }
}

#[cfg(feature = "quinn")]
impl rustls::client::danger::ServerCertVerifier for AcceptAnyServerCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::connection::CallConnection;
    use crate::protocol::dispatch::{Dispatcher, RemoteFilter};
    use crate::protocol::wire::ResponseEnvelope;
    use crate::registry::registration::{
        make_handler, Handler, HandlerRegistration, OperationProvenance,
    };
    use crate::registry::spec::{AccessControl, OperationSpec, OperationType, Visibility};
    use alknet_core::auth::Identity;
    use alknet_core::types::{Capabilities, MockConnection};
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::Mutex as StdMutex;

    struct StubConnection {
        alpn: &'static [u8],
        addr: Option<SocketAddr>,
        closed: StdMutex<Option<(u32, String)>>,
    }

    impl MockConnection for StubConnection {
        fn remote_alpn(&self) -> &[u8] {
            self.alpn
        }
        fn remote_addr(&self) -> Option<SocketAddr> {
            self.addr
        }
        fn close(&self, code: u32, reason: &str) {
            *self.closed.lock().unwrap() = Some((code, reason.to_string()));
        }
    }

    fn stub_connection() -> Connection {
        Connection::from_mock(Arc::new(StubConnection {
            alpn: b"alknet/call",
            addr: Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4321)),
            closed: StdMutex::new(None),
        }))
    }

    fn external_spec(name: &str) -> OperationSpec {
        OperationSpec::new(
            name,
            OperationType::Query,
            Visibility::External,
            serde_json::json!({}),
            serde_json::json!({}),
            vec![],
            AccessControl::default(),
        )
    }

    fn caps_inspect_handler() -> Handler {
        make_handler(|_input, context| async move {
            let has_google = context.capabilities.get("google").is_some();
            ResponseEnvelope::ok(
                context.request_id,
                serde_json::json!({ "has_google_capability": has_google }),
            )
        })
    }

    struct NoopIdentityProvider;
    impl alknet_core::auth::IdentityProvider for NoopIdentityProvider {
        fn resolve_from_fingerprint(&self, _fp: &str) -> Option<Identity> {
            None
        }
        fn resolve_from_token(&self, _token: &alknet_core::auth::AuthToken) -> Option<Identity> {
            None
        }
    }

    fn registry_with_remote_safe_and_caps() -> Arc<OperationRegistry> {
        let mut registry = OperationRegistry::new();
        // remote_safe: false, carries a google api-key capability
        registry.register(HandlerRegistration::new(
            external_spec("secret/run"),
            caps_inspect_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new().with_api_key("google", "secret-key".to_string()),
        ));
        // remote_safe: true, carries a google api-key capability
        registry.register(
            HandlerRegistration::new(
                external_spec("pub/run"),
                caps_inspect_handler(),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new().with_api_key("google", "pub-key".to_string()),
            )
            .remote_safe(true),
        );
        Arc::new(registry)
    }

    fn dispatcher(registry: &Arc<OperationRegistry>, trusted_peer: bool) -> Dispatcher {
        Dispatcher::new(
            Arc::clone(registry),
            Arc::new(NoopIdentityProvider),
            if trusted_peer {
                RemoteFilter::trusted()
            } else {
                RemoteFilter::default_deny()
            },
        )
    }

    async fn dispatch(d: &Dispatcher, conn: &Arc<CallConnection>, op: &str) -> ResponseEnvelope {
        d.dispatch_requested(
            conn,
            "req-test".to_string(),
            serde_json::json!({ "operationId": op, "input": {} }),
        )
        .await
    }

    #[test]
    fn call_client_new_is_default_deny() {
        let registry = Arc::new(OperationRegistry::new());
        let client = CallClient::new(Arc::clone(&registry), Arc::new(NoopIdentityProvider));
        assert!(!client.is_trusted_peer(), "new() is default-deny");
    }

    #[test]
    fn call_client_trusted_peer_is_trusted() {
        let registry = Arc::new(OperationRegistry::new());
        let client =
            CallClient::trusted_peer(Arc::clone(&registry), Arc::new(NoopIdentityProvider));
        assert!(
            client.is_trusted_peer(),
            "trusted_peer() is trusted-peer mode"
        );
    }

    #[test]
    fn call_credentials_builder_methods() {
        let creds = CallCredentials::new().with_remote_identity(RemoteIdentity {
            fingerprint: "SHA256:abc".to_string(),
        });
        assert_eq!(
            creds.remote_identity.as_ref().unwrap().fingerprint,
            "SHA256:abc"
        );
        assert!(creds.tls_identity.is_none());
        assert!(creds.auth_token.is_none());
    }

    #[tokio::test]
    async fn default_deny_non_remote_safe_op_returns_not_found() {
        let registry = registry_with_remote_safe_and_caps();
        let d = dispatcher(&registry, false);
        let conn = Arc::new(CallConnection::new(stub_connection()));
        let response = dispatch(&d, &conn, "secret/run").await;
        match response.result {
            Err(e) => assert_eq!(e.code, "NOT_FOUND"),
            other => panic!("expected NOT_FOUND for non-remote-safe op, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn default_deny_remote_safe_op_dispatches() {
        let registry = registry_with_remote_safe_and_caps();
        let d = dispatcher(&registry, false);
        let conn = Arc::new(CallConnection::new(stub_connection()));
        let response = dispatch(&d, &conn, "pub/run").await;
        assert!(
            response.result.is_ok(),
            "remote_safe op must dispatch in default-deny mode"
        );
    }

    #[tokio::test]
    async fn trusted_peer_dispatches_non_remote_safe_op() {
        let registry = registry_with_remote_safe_and_caps();
        let d = dispatcher(&registry, true);
        let conn = Arc::new(CallConnection::new(stub_connection()));
        let response = dispatch(&d, &conn, "secret/run").await;
        assert!(
            response.result.is_ok(),
            "trusted-peer mode dispatches non-remote-safe ops"
        );
    }

    /// The load-bearing security invariant (ADR-028 Context): a remote
    /// peer's call to a non-remote-safe op must NOT populate
    /// `OperationContext.capabilities` from the local registration bundle.
    /// This test asserts the handler is never reached for non-remote-safe
    /// ops in default-deny mode (NOT_FOUND before dispatch), so capabilities
    /// are never populated — verified by the handler not running.
    #[tokio::test]
    async fn default_deny_non_remote_safe_does_not_populate_capabilities() {
        let registry = registry_with_remote_safe_and_caps();
        let d = dispatcher(&registry, false);
        let conn = Arc::new(CallConnection::new(stub_connection()));
        let response = dispatch(&d, &conn, "secret/run").await;
        match response.result {
            Err(e) => assert_eq!(e.code, "NOT_FOUND"),
            Ok(_) => panic!("non-remote-safe op must not dispatch (would populate capabilities)"),
        }
    }

    /// A remote-safe op's call DOES populate capabilities (the security
    /// argument is about *non-remote-safe* ops, not all ops). The handler
    /// inspects capabilities and reports whether the google key was injected.
    #[tokio::test]
    async fn remote_safe_op_populates_capabilities_for_handler() {
        let registry = registry_with_remote_safe_and_caps();
        let d = dispatcher(&registry, false);
        let conn = Arc::new(CallConnection::new(stub_connection()));
        let response = dispatch(&d, &conn, "pub/run").await;
        let out = response.result.expect("ok");
        assert_eq!(
            out["has_google_capability"],
            serde_json::json!(true),
            "remote_safe op must have its capabilities populated"
        );
    }

    #[tokio::test]
    async fn trusted_peer_populates_capabilities_for_non_remote_safe() {
        let registry = registry_with_remote_safe_and_caps();
        let d = dispatcher(&registry, true);
        let conn = Arc::new(CallConnection::new(stub_connection()));
        let response = dispatch(&d, &conn, "secret/run").await;
        let out = response.result.expect("ok");
        assert_eq!(
            out["has_google_capability"],
            serde_json::json!(true),
            "trusted-peer mode populates capabilities for all External ops"
        );
    }

    #[tokio::test]
    async fn default_deny_unknown_op_returns_not_found() {
        let registry = Arc::new(OperationRegistry::new());
        let d = dispatcher(&registry, false);
        let conn = Arc::new(CallConnection::new(stub_connection()));
        let response = dispatch(&d, &conn, "no/such").await;
        match response.result {
            Err(e) => assert_eq!(e.code, "NOT_FOUND"),
            other => panic!("expected NOT_FOUND, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn spawn_dispatch_returns_live_call_connection() {
        let registry = registry_with_remote_safe_and_caps();
        let client = CallClient::new(Arc::clone(&registry), Arc::new(NoopIdentityProvider));
        let conn = client.spawn_dispatch(stub_connection());
        // The returned CallConnection is usable: it has an empty overlay and
        // the underlying connection reports the alknet/call ALPN.
        assert_eq!(conn.connection().remote_alpn(), b"alknet/call");
        // The dispatch task is spawned; dropping the connection closes it.
        std::mem::drop(conn);
    }

    #[test]
    fn call_client_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CallClient>();
        assert_send_sync::<CallCredentials>();
        assert_send_sync::<RemoteIdentity>();
    }
}
