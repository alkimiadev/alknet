//! Client-side TLS configuration: `TlsClientConfig`, `FingerprintPinVerifier`,
//! `RawKeyClientCertResolver`, `NoClientCertResolver`, `select_server_verifier`,
//! `build_client_auth`, `load_platform_root_cert_store`.

use std::sync::Arc;

use alknet_core::config::TlsIdentity;
use alknet_core::credentials::{ConnectionCredentials, RemoteIdentity};

use crate::TlsError;

/// Client-side TLS configuration, transport-agnostic.
/// Wraps a `rustls::ClientConfig` built from `ConnectionCredentials`.
#[allow(dead_code)]
pub struct TlsClientConfig {
    pub(crate) rustls_config: rustls::ClientConfig,
}

impl TlsClientConfig {
    /// Build a client config from `ConnectionCredentials` and an ALPN.
    /// Selects the server cert verifier by `remote_identity` presence
    /// (ADR-034 §3): `Some` → fingerprint pin, `None` → CA verification.
    pub fn new(
        credentials: &ConnectionCredentials,
        alpn: &[u8],
    ) -> Result<Self, TlsError> {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());

        let client_auth = build_client_auth(&provider, &credentials.tls_identity)?;
        let verifier = select_server_verifier(&provider, &credentials.remote_identity)?;

        let mut config = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| TlsError::Config(e.to_string()))?
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_client_cert_resolver(client_auth);
        config.alpn_protocols = vec![alpn.to_vec()];
        config.enable_early_data = true;

        Ok(Self { rustls_config: config })
    }

    /// Convert to a `quinn::ClientConfig` for QUIC transport.
    #[cfg(feature = "quinn")]
    pub fn for_quinn(self) -> Result<quinn::ClientConfig, TlsError> {
        Ok(quinn::ClientConfig::new(Arc::new(
            quinn::crypto::rustls::QuicClientConfig::try_from(self.rustls_config)
                .map_err(|e| TlsError::Config(e.to_string()))?,
        )))
    }
}

/// Build the client-auth cert resolver that presents the local node's TLS
/// identity. For `TlsIdentity::RawKey` the Ed25519 key is presented as an RFC
/// 7250 raw public key client cert (`only_raw_public_keys() == true`) — the
/// client-side equivalent of the server's `RawKeyCertResolver`. For X.509 the
/// cert chain + key are loaded from disk. `None` (no `tls_identity` configured)
/// resolves to no client cert (the server gets nothing to fingerprint).
fn build_client_auth(
    provider: &Arc<rustls::crypto::CryptoProvider>,
    tls_identity: &Option<TlsIdentity>,
) -> Result<Arc<dyn rustls::client::ResolvesClientCert>, TlsError> {
    match tls_identity {
        Some(TlsIdentity::RawKey(secret_key)) => {
            let signing_key = Arc::new(crate::signing::Ed25519SigningKey::new(
                secret_key.clone(),
            ));
            let spki = signing_key.spki_public_key();
            let cert = rustls::pki_types::CertificateDer::from(spki.to_vec());
            let certified_key = Arc::new(rustls::sign::CertifiedKey::new(vec![cert], signing_key));
            Ok(Arc::new(RawKeyClientCertResolver::new(certified_key)))
        }
        Some(TlsIdentity::X509 { cert, key }) => {
            let cert_chain = crate::pem::load_cert_chain(cert)?;
            let key_der = crate::pem::load_private_key(key)?;
            let certified_key =
                rustls::sign::CertifiedKey::from_der(cert_chain, key_der, provider)
                    .map_err(|e| TlsError::Config(e.to_string()))?;
            Ok(Arc::new(RawKeyClientCertResolver::new(Arc::new(
                certified_key,
            ))))
        }
        Some(TlsIdentity::SelfSigned) | None => Ok(Arc::new(NoClientCertResolver)),
        Some(TlsIdentity::Acme { .. }) => Err(TlsError::Config(
            "ACME TLS identity is server-only; cannot be used for client auth".to_string(),
        )),
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
fn select_server_verifier(
    provider: &Arc<rustls::crypto::CryptoProvider>,
    remote_identity: &Option<RemoteIdentity>,
) -> Result<Arc<dyn rustls::client::danger::ServerCertVerifier>, TlsError> {
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
            .map_err(|e| TlsError::Config(e.to_string()))?;
            Ok(verifier)
        }
    }
}

/// Load the platform's trusted root certificates into a `RootCertStore` for
/// `WebPkiServerVerifier` (the `None` + X.509 CA-verification path). Falls back
/// to the built-in `webpki-roots` if the platform store is empty (e.g. in a
/// container with no system CA bundle) — ADR-088 §5.
fn load_platform_root_cert_store() -> Result<rustls::RootCertStore, TlsError> {
    let mut roots = rustls::RootCertStore::empty();
    let result = rustls_native_certs::load_native_certs();
    for err in &result.errors {
        tracing::warn!(error = ?err, "failed to load a native root cert");
    }
    for cert in &result.certs {
        roots
            .add(cert.clone())
            .map_err(|e| TlsError::Config(format!("failed to add native root cert: {e}")))?;
    }
    if roots.is_empty() {
        tracing::info!("platform root cert store is empty, falling back to webpki-roots");
        for anchor in webpki_roots::TLS_SERVER_ROOTS.iter() {
            roots.roots.push(anchor.to_owned());
        }
    }
    Ok(roots)
}

/// Client cert resolver that presents a single RFC 7250 raw public key (or
/// X.509 cert chain). For raw keys `only_raw_public_keys()` returns `true` so
/// rustls negotiates the RFC 7250 ClientCertificateType extension.
struct RawKeyClientCertResolver {
    key: Arc<rustls::sign::CertifiedKey>,
    raw_public_keys: bool,
}

impl RawKeyClientCertResolver {
    fn new(key: Arc<rustls::sign::CertifiedKey>) -> Self {
        let raw_public_keys = key.cert.len() == 1 && is_ed25519_spki(&key.cert[0]);
        Self {
            key,
            raw_public_keys,
        }
    }
}

fn is_ed25519_spki(cert_der: &rustls::pki_types::CertificateDer<'_>) -> bool {
    alknet_core::fingerprint::extract_ed25519_raw_key_from_spki(cert_der.as_ref()).is_some()
}

impl std::fmt::Debug for RawKeyClientCertResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawKeyClientCertResolver")
            .field("raw_public_keys", &self.raw_public_keys)
            .finish()
    }
}

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
struct NoClientCertResolver;

impl std::fmt::Debug for NoClientCertResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NoClientCertResolver").finish()
    }
}

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
struct FingerprintPinVerifier {
    fingerprint: String,
    supported: rustls::crypto::WebPkiSupportedAlgorithms,
}

impl FingerprintPinVerifier {
    fn new(fingerprint: String, supported: rustls::crypto::WebPkiSupportedAlgorithms) -> Self {
        Self {
            fingerprint,
            supported,
        }
    }
}

impl std::fmt::Debug for FingerprintPinVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FingerprintPinVerifier")
            .field("fingerprint", &self.fingerprint)
            .finish()
    }
}

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
