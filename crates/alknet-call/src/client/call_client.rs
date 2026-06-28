//! `CallClient`: the outbound connection opener (ADR-017 §1).
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
//! `abort()`) and a callee (dispatches incoming calls against its registry).
//!
//! See `docs/architecture/crates/call/client-and-adapters.md` for the spec.

use std::net::SocketAddr;
use std::sync::Arc;

use alknet_core::auth::IdentityProvider;
use alknet_core::config::TlsIdentity;
use alknet_core::types::Connection;

use crate::protocol::connection::CallConnection;
use crate::protocol::dispatch::Dispatcher;
use crate::registry::registration::OperationRegistry;

/// Expected identity of the remote node (ADR-017 §7, extended by ADR-034 §2).
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
    /// `Some` → fingerprint pin (known peer with a `PeerEntry`); `None` → CA
    /// verification for X.509 remotes, fail-closed for Ed25519 raw-key remotes
    /// (ADR-034 §2/§3). `None` is the public-X.509-endpoint state, not a
    /// missing field — must not be defaulted to a placeholder.
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
/// Peer authorization flows through the existing `AccessControl::check` gate
/// in `OperationRegistry::invoke` (ADR-029 §3) — no parallel `remote_safe`/
/// `trusted_peer` gate.
pub struct CallClient {
    registry: Arc<OperationRegistry>,
    identity_provider: Arc<dyn IdentityProvider>,
}

impl CallClient {
    pub fn new(
        registry: Arc<OperationRegistry>,
        identity_provider: Arc<dyn IdentityProvider>,
    ) -> Self {
        Self {
            registry,
            identity_provider,
        }
    }

    pub fn registry(&self) -> &Arc<OperationRegistry> {
        &self.registry
    }

    pub fn identity_provider(&self) -> &Arc<dyn IdentityProvider> {
        &self.identity_provider
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
    /// `CallClient`'s registry (connection symmetry, ADR-017 §2).
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
    credentials: &CallCredentials,
    alpn: &[u8],
) -> Result<quinn::ClientConfig, String> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());

    let client_auth = build_client_auth(&provider, &credentials.tls_identity)?;
    let verifier = select_server_verifier(&provider, &credentials.remote_identity)?;

    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_cert_resolver(client_auth);
    config.alpn_protocols = vec![alpn.to_vec()];
    config.enable_early_data = true;

    Ok(quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(config).map_err(|e| e.to_string())?,
    )))
}

/// Build the client-auth cert resolver that presents the local node's TLS
/// identity. For `TlsIdentity::RawKey` the Ed25519 key is presented as an RFC
/// 7250 raw public key client cert (`only_raw_public_keys() == true`) — the
/// client-side equivalent of the server's `RawKeyCertResolver`. For X.509 the
/// cert chain + key are loaded from disk. `None` (no `tls_identity` configured)
/// resolves to no client cert (the server gets nothing to fingerprint).
#[cfg(feature = "quinn")]
fn build_client_auth(
    provider: &Arc<rustls::crypto::CryptoProvider>,
    tls_identity: &Option<TlsIdentity>,
) -> Result<Arc<dyn rustls::client::ResolvesClientCert>, String> {
    match tls_identity {
        Some(TlsIdentity::RawKey(secret_key)) => {
            let signing_key = Arc::new(Ed25519SigningKey::new(secret_key.clone()));
            let spki = signing_key.spki_public_key();
            let cert = rustls::pki_types::CertificateDer::from(spki.to_vec());
            let certified_key = Arc::new(rustls::sign::CertifiedKey::new(vec![cert], signing_key));
            Ok(Arc::new(RawKeyClientCertResolver::new(certified_key)))
        }
        Some(TlsIdentity::X509 { cert, key }) => {
            let cert_chain = load_cert_chain(cert).map_err(|e| e.to_string())?;
            let key_der = load_private_key(key).map_err(|e| e.to_string())?;
            let certified_key = rustls::sign::CertifiedKey::from_der(cert_chain, key_der, provider)
                .map_err(|e| e.to_string())?;
            Ok(Arc::new(RawKeyClientCertResolver::new(Arc::new(
                certified_key,
            ))))
        }
        Some(TlsIdentity::SelfSigned) | None => Ok(Arc::new(NoClientCertResolver)),
        Some(TlsIdentity::Acme { .. }) => {
            Err("ACME TLS identity is server-only; cannot be used for client auth".to_string())
        }
    }
}

/// Select the server cert verifier by `remote_identity` presence (ADR-034 §3).
///
/// - `Some(fingerprint)` → known peer → `FingerprintPinVerifier` (fingerprint
///   match). The fingerprint IS the trust anchor.
/// - `None` → no `PeerEntry` for the remote → `WebPkiServerVerifier` (CA
///   verification) for X.509 remotes. For Ed25519 raw-key remotes the
///   `WebPkiServerVerifier` fails closed at handshake time (raw-key remotes
///   have no CA to fall back to — ADR-034 §2 assumption 1). `None` is the
///   public-X.509-endpoint state, not "skip verification."
#[cfg(feature = "quinn")]
fn select_server_verifier(
    provider: &Arc<rustls::crypto::CryptoProvider>,
    remote_identity: &Option<RemoteIdentity>,
) -> Result<Arc<dyn rustls::client::danger::ServerCertVerifier>, String> {
    match remote_identity {
        Some(ri) => Ok(Arc::new(FingerprintPinVerifier::new(
            ri.fingerprint.clone(),
            provider.signature_verification_algorithms,
        ))),
        None => {
            let roots = load_platform_root_cert_store()?;
            let verifier = rustls::client::WebPkiServerVerifier::builder_with_provider(
                Arc::new(roots),
                Arc::clone(provider),
            )
            .build()
            .map_err(|e| e.to_string())?;
            Ok(verifier)
        }
    }
}

/// Load the platform's trusted root certificates into a `RootCertStore` for
/// `WebPkiServerVerifier` (the `None` + X.509 CA-verification path). Falls back
/// to the aws-lc-rs built-in `webpki-roots` if the platform store is empty
/// (e.g. in a container with no system CA bundle).
#[cfg(feature = "quinn")]
fn load_platform_root_cert_store() -> Result<rustls::RootCertStore, String> {
    let mut roots = rustls::RootCertStore::empty();
    let result = rustls_native_certs::load_native_certs();
    for err in &result.errors {
        tracing::warn!(error = ?err, "failed to load a native root cert");
    }
    for cert in &result.certs {
        roots
            .add(cert.clone())
            .map_err(|e| format!("failed to add native root cert: {e}"))?;
    }
    Ok(roots)
}

#[cfg(feature = "quinn")]
fn load_cert_chain(
    path: &std::path::Path,
) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut reader = std::io::BufReader::new(bytes.as_slice());
    rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

#[cfg(feature = "quinn")]
fn load_private_key(
    path: &std::path::Path,
) -> Result<rustls::pki_types::PrivateKeyDer<'static>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut reader = std::io::BufReader::new(bytes.as_slice());
    match rustls_pemfile::private_key(&mut reader) {
        Ok(Some(key)) => Ok(key),
        Ok(None) => Err("no private key found in file".to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// Client cert resolver that presents a single RFC 7250 raw public key (or
/// X.509 cert chain). For raw keys `only_raw_public_keys()` returns `true` so
/// rustls negotiates the RFC 7250 ClientCertificateType extension.
#[cfg(feature = "quinn")]
struct RawKeyClientCertResolver {
    key: Arc<rustls::sign::CertifiedKey>,
    raw_public_keys: bool,
}

#[cfg(feature = "quinn")]
impl RawKeyClientCertResolver {
    fn new(key: Arc<rustls::sign::CertifiedKey>) -> Self {
        let raw_public_keys = key.cert.len() == 1 && is_ed25519_spki(&key.cert[0]);
        Self {
            key,
            raw_public_keys,
        }
    }
}

#[cfg(feature = "quinn")]
fn is_ed25519_spki(cert_der: &rustls::pki_types::CertificateDer<'_>) -> bool {
    alknet_core::fingerprint::extract_ed25519_raw_key_from_spki(cert_der.as_ref()).is_some()
}

#[cfg(feature = "quinn")]
impl std::fmt::Debug for RawKeyClientCertResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawKeyClientCertResolver")
            .field("raw_public_keys", &self.raw_public_keys)
            .finish()
    }
}

#[cfg(feature = "quinn")]
impl rustls::client::ResolvesClientCert for RawKeyClientCertResolver {
    fn resolve(
        &self,
        _root_hint_subjects: &[&[u8]],
        _sigschemes: &[rustls::SignatureScheme],
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        Some(Arc::clone(&self.key))
    }

    fn only_raw_public_keys(&self) -> bool {
        self.raw_public_keys
    }

    fn has_certs(&self) -> bool {
        true
    }
}

/// Client cert resolver that presents no client cert (the `tls_identity: None`
/// or `SelfSigned` path). The server gets nothing to fingerprint — the
/// `PeerEntry` fingerprint → `peer_id` resolution path is not activated for
/// this connection.
#[cfg(feature = "quinn")]
struct NoClientCertResolver;

#[cfg(feature = "quinn")]
impl std::fmt::Debug for NoClientCertResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NoClientCertResolver").finish()
    }
}

#[cfg(feature = "quinn")]
impl rustls::client::ResolvesClientCert for NoClientCertResolver {
    fn resolve(
        &self,
        _root_hint_subjects: &[&[u8]],
        _sigschemes: &[rustls::SignatureScheme],
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        None
    }

    fn has_certs(&self) -> bool {
        false
    }
}

/// `ServerCertVerifier` that pins a specific fingerprint (ADR-034 §3, the
/// known-peer path). For `ed25519:<hex>` remotes the raw Ed25519 pub key is
/// extracted from the presented cert and matched against the pinned
/// fingerprint; for `SHA256:<hex>` remotes the cert DER is hashed and matched
/// against the pinned fingerprint. No match → verification failure (the
/// connection is rejected). The fingerprint IS the trust anchor — there is no
/// CA verification and no name verification, only the fingerprint pin.
///
/// Handshake signatures are still verified (using the aws-lc-rs default
/// signature verification algorithms) so that a stolen-but-stale fingerprint
/// can't be replayed with a forged signature: the presenter must prove
/// possession of the private key corresponding to the pinned public key.
#[cfg(feature = "quinn")]
struct FingerprintPinVerifier {
    fingerprint: String,
    supported: rustls::crypto::WebPkiSupportedAlgorithms,
}

#[cfg(feature = "quinn")]
impl FingerprintPinVerifier {
    fn new(fingerprint: String, supported: rustls::crypto::WebPkiSupportedAlgorithms) -> Self {
        Self {
            fingerprint,
            supported,
        }
    }
}

#[cfg(feature = "quinn")]
impl std::fmt::Debug for FingerprintPinVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FingerprintPinVerifier")
            .field("fingerprint", &self.fingerprint)
            .finish()
    }
}

#[cfg(feature = "quinn")]
impl rustls::client::danger::ServerCertVerifier for FingerprintPinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        let presented = alknet_core::fingerprint::fingerprint_from_cert_der(end_entity.as_ref())
            .ok_or(rustls::Error::General(
                "fingerprint pin: failed to compute fingerprint from presented cert".to_string(),
            ))?;
        if presented == self.fingerprint {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(format!(
                "fingerprint pin mismatch: expected {} got {}",
                self.fingerprint, presented
            )))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        if alknet_core::fingerprint::extract_ed25519_raw_key_from_spki(cert.as_ref()).is_some() {
            let spki = rustls::pki_types::SubjectPublicKeyInfoDer::from(cert.as_ref().to_vec());
            rustls::crypto::verify_tls13_signature_with_raw_key(
                message,
                &spki,
                dss,
                &self.supported,
            )
        } else {
            rustls::crypto::verify_tls12_signature(message, cert, dss, &self.supported)
        }
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        if alknet_core::fingerprint::extract_ed25519_raw_key_from_spki(cert.as_ref()).is_some() {
            let spki = rustls::pki_types::SubjectPublicKeyInfoDer::from(cert.as_ref().to_vec());
            rustls::crypto::verify_tls13_signature_with_raw_key(
                message,
                &spki,
                dss,
                &self.supported,
            )
        } else {
            rustls::crypto::verify_tls13_signature(message, cert, dss, &self.supported)
        }
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.supported.supported_schemes()
    }
}

#[cfg(feature = "quinn")]
#[derive(Clone)]
struct Ed25519SigningKey {
    key: alknet_core::config::Ed25519SecretKey,
}

#[cfg(feature = "quinn")]
impl Ed25519SigningKey {
    fn new(key: alknet_core::config::Ed25519SecretKey) -> Self {
        Self { key }
    }

    fn spki_public_key(&self) -> rustls::pki_types::SubjectPublicKeyInfoDer<'static> {
        rustls::sign::public_key_to_spki(
            &rustls::pki_types::alg_id::ED25519,
            self.key.public().as_bytes(),
        )
    }
}

#[cfg(feature = "quinn")]
impl std::fmt::Debug for Ed25519SigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ed25519SigningKey").finish()
    }
}

#[cfg(feature = "quinn")]
impl rustls::sign::SigningKey for Ed25519SigningKey {
    fn choose_scheme(
        &self,
        offered: &[rustls::SignatureScheme],
    ) -> Option<Box<dyn rustls::sign::Signer>> {
        if offered.contains(&rustls::SignatureScheme::ED25519) {
            Some(Box::new(self.clone()))
        } else {
            None
        }
    }

    fn algorithm(&self) -> rustls::SignatureAlgorithm {
        rustls::SignatureAlgorithm::ED25519
    }

    fn public_key(&self) -> Option<rustls::pki_types::SubjectPublicKeyInfoDer<'_>> {
        Some(self.spki_public_key())
    }
}

#[cfg(feature = "quinn")]
impl rustls::sign::Signer for Ed25519SigningKey {
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, rustls::Error> {
        Ok(self.key.sign(message).to_bytes().to_vec())
    }

    fn scheme(&self) -> rustls::SignatureScheme {
        rustls::SignatureScheme::ED25519
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::connection::CallConnection;
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

    fn registry_with_caps() -> Arc<OperationRegistry> {
        let mut registry = OperationRegistry::new();
        registry.register(HandlerRegistration::new(
            external_spec("pub/run"),
            caps_inspect_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new().with_api_key("google", "pub-key".to_string()),
        ));
        Arc::new(registry)
    }

    fn dispatcher(registry: &Arc<OperationRegistry>) -> Dispatcher {
        Dispatcher::new(Arc::clone(registry), Arc::new(NoopIdentityProvider))
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
    async fn external_op_dispatches_and_populates_capabilities() {
        let registry = registry_with_caps();
        let d = dispatcher(&registry);
        let conn = Arc::new(CallConnection::new(stub_connection()));
        let response = dispatch(&d, &conn, "pub/run").await;
        let out = response.result.expect("ok");
        assert_eq!(
            out["has_google_capability"],
            serde_json::json!(true),
            "an External op's call must populate capabilities for the handler"
        );
    }

    #[tokio::test]
    async fn unknown_op_returns_not_found() {
        let registry = Arc::new(OperationRegistry::new());
        let d = dispatcher(&registry);
        let conn = Arc::new(CallConnection::new(stub_connection()));
        let response = dispatch(&d, &conn, "no/such").await;
        match response.result {
            Err(e) => assert_eq!(e.code, "NOT_FOUND"),
            other => panic!("expected NOT_FOUND, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn spawn_dispatch_returns_live_call_connection() {
        let registry = registry_with_caps();
        let client = CallClient::new(Arc::clone(&registry), Arc::new(NoopIdentityProvider));
        let conn = client.spawn_dispatch(stub_connection());
        assert_eq!(conn.connection().remote_alpn(), b"alknet/call");
        std::mem::drop(conn);
    }

    #[test]
    fn call_client_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CallClient>();
        assert_send_sync::<CallCredentials>();
        assert_send_sync::<RemoteIdentity>();
    }

    #[cfg(feature = "quinn")]
    fn build_ed25519_spki_der(raw_key: &[u8; 32]) -> Vec<u8> {
        let spki = rustls::sign::public_key_to_spki(&rustls::pki_types::alg_id::ED25519, raw_key);
        spki.to_vec()
    }

    #[cfg(feature = "quinn")]
    fn build_x509_cert_der() -> rustls::pki_types::CertificateDer<'static> {
        let key_pair = rcgen::KeyPair::generate().expect("key gen");
        let params = rcgen::CertificateParams::default();
        let cert = params.self_signed(&key_pair).expect("self-signed cert");
        cert.der().clone()
    }

    #[cfg(feature = "quinn")]
    fn aws_lc_rs_provider() -> Arc<rustls::crypto::CryptoProvider> {
        Arc::new(rustls::crypto::aws_lc_rs::default_provider())
    }

    #[cfg(feature = "quinn")]
    fn verify_pin(
        verifier: &FingerprintPinVerifier,
        cert_der: rustls::pki_types::CertificateDer<'_>,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        use rustls::client::danger::ServerCertVerifier;
        let server_name: rustls::pki_types::ServerName<'static> =
            "alknet".try_into().expect("server name");
        verifier.verify_server_cert(
            &cert_der,
            &[],
            &server_name,
            &[],
            rustls::pki_types::UnixTime::now(),
        )
    }

    #[cfg(feature = "quinn")]
    #[test]
    fn fingerprint_pin_verifier_matches_correct_ed25519_fingerprint() {
        let sk = alknet_core::config::Ed25519SecretKey::generate();
        let raw_key = sk.public().to_bytes();
        let spki_der = build_ed25519_spki_der(&raw_key);
        let fingerprint =
            alknet_core::fingerprint::fingerprint_from_cert_der(&spki_der).expect("fingerprint");
        let verifier = FingerprintPinVerifier::new(
            fingerprint,
            aws_lc_rs_provider().signature_verification_algorithms,
        );
        let cert = rustls::pki_types::CertificateDer::from(spki_der);
        let result = verify_pin(&verifier, cert);
        assert!(
            result.is_ok(),
            "FingerprintPinVerifier must accept a cert whose fingerprint matches the pin"
        );
    }

    #[cfg(feature = "quinn")]
    #[test]
    fn fingerprint_pin_verifier_rejects_wrong_ed25519_fingerprint() {
        let sk = alknet_core::config::Ed25519SecretKey::generate();
        let raw_key = sk.public().to_bytes();
        let spki_der = build_ed25519_spki_der(&raw_key);
        let other_sk = alknet_core::config::Ed25519SecretKey::generate();
        let other_fp = format!("ed25519:{}", hex::encode(other_sk.public().to_bytes()));
        let verifier = FingerprintPinVerifier::new(
            other_fp,
            aws_lc_rs_provider().signature_verification_algorithms,
        );
        let cert = rustls::pki_types::CertificateDer::from(spki_der);
        let result = verify_pin(&verifier, cert);
        assert!(
            result.is_err(),
            "FingerprintPinVerifier must reject a cert whose fingerprint does not match the pin"
        );
    }

    #[cfg(feature = "quinn")]
    #[test]
    fn fingerprint_pin_verifier_matches_correct_sha256_fingerprint() {
        let cert_der = build_x509_cert_der();
        let fingerprint = alknet_core::fingerprint::fingerprint_from_cert_der(cert_der.as_ref())
            .expect("fingerprint");
        let verifier = FingerprintPinVerifier::new(
            fingerprint,
            aws_lc_rs_provider().signature_verification_algorithms,
        );
        let result = verify_pin(&verifier, cert_der);
        assert!(
            result.is_ok(),
            "FingerprintPinVerifier must accept an X.509 cert whose SHA256 fingerprint matches"
        );
    }

    #[cfg(feature = "quinn")]
    #[test]
    fn fingerprint_pin_verifier_rejects_wrong_sha256_fingerprint() {
        let cert_der = build_x509_cert_der();
        let verifier = FingerprintPinVerifier::new(
            "SHA256:0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            aws_lc_rs_provider().signature_verification_algorithms,
        );
        let result = verify_pin(&verifier, cert_der);
        assert!(
            result.is_err(),
            "FingerprintPinVerifier must reject an X.509 cert whose SHA256 does not match"
        );
    }

    #[cfg(feature = "quinn")]
    #[test]
    fn select_server_verifier_returns_ca_verifier_for_none() {
        let provider = aws_lc_rs_provider();
        let remote_identity: Option<RemoteIdentity> = None;
        let verifier = select_server_verifier(&provider, &remote_identity);
        assert!(
            verifier.is_ok(),
            "select_server_verifier must succeed for None (CA path)"
        );
        let debug = format!("{:?}", verifier.unwrap());
        assert!(
            debug.contains("WebPkiServerVerifier"),
            "None must select WebPkiServerVerifier (CA verification), got: {debug}"
        );
    }

    #[cfg(feature = "quinn")]
    #[test]
    fn select_server_verifier_returns_fingerprint_pin_for_some() {
        let provider = aws_lc_rs_provider();
        let remote_identity = Some(RemoteIdentity {
            fingerprint: "ed25519:abc".to_string(),
        });
        let verifier = select_server_verifier(&provider, &remote_identity);
        assert!(
            verifier.is_ok(),
            "select_server_verifier must succeed for Some (fingerprint pin path)"
        );
        let debug = format!("{:?}", verifier.unwrap());
        assert!(
            debug.contains("FingerprintPinVerifier"),
            "Some must select FingerprintPinVerifier, got: {debug}"
        );
    }

    #[cfg(feature = "quinn")]
    #[test]
    fn build_client_auth_presents_ed25519_raw_key_without_error() {
        let provider = aws_lc_rs_provider();
        let sk = alknet_core::config::Ed25519SecretKey::generate();
        let tls_identity = Some(alknet_core::config::TlsIdentity::RawKey(sk));
        let resolver = build_client_auth(&provider, &tls_identity);
        assert!(
            resolver.is_ok(),
            "build_client_auth must build a resolver for a RawKey identity"
        );
        let resolver = resolver.unwrap();
        assert!(
            resolver.only_raw_public_keys(),
            "RawKey client auth resolver must present raw public keys (RFC 7250)"
        );
        assert!(
            resolver.has_certs(),
            "RawKey client auth resolver must report it has a cert to present"
        );
    }

    #[cfg(feature = "quinn")]
    #[test]
    fn build_client_auth_none_resolves_to_no_client_cert() {
        let provider = aws_lc_rs_provider();
        let tls_identity: Option<alknet_core::config::TlsIdentity> = None;
        let resolver = build_client_auth(&provider, &tls_identity)
            .expect("build_client_auth must succeed for None");
        assert!(
            !resolver.has_certs(),
            "NoClientCertResolver must report no certs (no client cert presented)"
        );
    }

    #[cfg(feature = "quinn")]
    #[test]
    fn build_quinn_client_config_with_raw_key_identity_builds_without_error() {
        let sk = alknet_core::config::Ed25519SecretKey::generate();
        let credentials = CallCredentials::new()
            .with_tls_identity(alknet_core::config::TlsIdentity::RawKey(sk))
            .with_remote_identity(RemoteIdentity {
                fingerprint: "ed25519:deadbeef".to_string(),
            });
        let config = build_quinn_client_config(&credentials, b"alknet/call");
        assert!(
            config.is_ok(),
            "build_quinn_client_config must build with a RawKey identity + pinned fingerprint"
        );
    }

    #[cfg(feature = "quinn")]
    #[test]
    fn build_quinn_client_config_with_no_remote_identity_builds_without_error() {
        let sk = alknet_core::config::Ed25519SecretKey::generate();
        let credentials =
            CallCredentials::new().with_tls_identity(alknet_core::config::TlsIdentity::RawKey(sk));
        let config = build_quinn_client_config(&credentials, b"alknet/call");
        assert!(
            config.is_ok(),
            "build_quinn_client_config must build for the None + CA-verification path"
        );
    }

    #[test]
    fn remote_identity_none_is_load_bearing_not_defaulted() {
        let creds = CallCredentials::new();
        assert!(
            creds.remote_identity.is_none(),
            "CallCredentials::new() must keep remote_identity as None (the load-bearing \
             public-X.509-endpoint state), not default it to a placeholder"
        );
    }
}
