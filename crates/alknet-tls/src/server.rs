//! Server-side TLS configuration: `TlsServerConfig`, `RawKeyCertResolver`,
//! `AcceptAnyCertVerifier`, `SelfSignedCert`, `generate_self_signed_cert`.

use std::sync::Arc;

use alknet_core::config::{Ed25519SecretKey, TlsIdentity};
#[cfg(feature = "acme")]
use alknet_core::config::AcmeDirectory;
#[cfg(feature = "acme")]
use tracing::{debug, error, warn};

use crate::signing::Ed25519SigningKey;
use crate::TlsError;

/// Server-side TLS configuration, transport-agnostic.
/// Wraps a `rustls::ServerConfig` plus optional ACME state.
#[allow(dead_code)]
pub struct TlsServerConfig {
    pub(crate) rustls_config: rustls::ServerConfig,
    #[cfg(feature = "acme")]
    pub(crate) acme_state_handle: Option<tokio::task::JoinHandle<()>>,
}

impl TlsServerConfig {
    /// Build a server config from a `TlsIdentity` and ALPN list.
    /// ACME identities spawn a background cert-renewal task.
    pub async fn new(
        tls_identity: &TlsIdentity,
        alpns: &[Vec<u8>],
    ) -> Result<Self, TlsError> {
        match tls_identity {
            TlsIdentity::Acme {
                domains,
                cache_dir,
                directory,
                contact,
            } => {
                #[cfg(feature = "acme")]
                {
                    Self::new_acme(domains, cache_dir, directory, contact, alpns).await
                }
                #[cfg(not(feature = "acme"))]
                {
                    let _ = (domains, cache_dir, directory, contact, alpns);
                    Err(TlsError::Config(
                        "ACME feature not enabled but TlsIdentity::Acme configured".to_string(),
                    ))
                }
            }
            _ => {
                let server_config = build_rustls_server_config(tls_identity, alpns)?;
                Ok(Self {
                    rustls_config: server_config,
                    #[cfg(feature = "acme")]
                    acme_state_handle: None,
                })
            }
        }
    }

    #[cfg(feature = "acme")]
    async fn new_acme(
        domains: &[String],
        cache_dir: &std::path::Path,
        directory: &AcmeDirectory,
        contact: &[String],
        alpns: &[Vec<u8>],
    ) -> Result<Self, TlsError> {
        use rustls_acme::caches::DirCache;
        use rustls_acme::{AcmeConfig, EventError, EventOk};

        let acme_config = AcmeConfig::new(domains.to_vec())
            .cache(DirCache::new(cache_dir.to_path_buf()))
            .directory(directory.url())
            .contact(contact.iter().map(|c| c.as_str()));

        let state = acme_config.state();
        let resolver = state.resolver();

        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let mut config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| TlsError::Config(e.to_string()))?
            .with_client_cert_verifier(Arc::new(AcceptAnyCertVerifier))
            .with_cert_resolver(resolver);
        config.max_early_data_size = u32::MAX;

        let mut alpn = alpns.to_vec();
        alpn.push(b"acme-tls/1".to_vec());
        config.alpn_protocols = alpn;

        let domains_owned: Vec<String> = domains.to_vec();
        let handle = tokio::spawn(async move {
            use futures::StreamExt;
            let mut state = state;
            while let Some(event) = state.next().await {
                match event {
                    Ok(EventOk::DeployedCachedCert) => {
                        debug!(domains = ?domains_owned, "ACME: deployed cached certificate");
                    }
                    Ok(EventOk::DeployedNewCert) => {
                        debug!(domains = ?domains_owned, "ACME: deployed new certificate");
                    }
                    Ok(EventOk::CertCacheStore) => {
                        debug!(domains = ?domains_owned, "ACME: certificate stored to cache");
                    }
                    Ok(EventOk::AccountCacheStore) => {
                        debug!(domains = ?domains_owned, "ACME: account stored to cache");
                    }
                    Err(EventError::CertCacheLoad(e)) => {
                        error!(domains = ?domains_owned, error = ?e, "ACME: certificate cache load failed");
                    }
                    Err(EventError::AccountCacheLoad(e)) => {
                        error!(domains = ?domains_owned, error = ?e, "ACME: account cache load failed");
                    }
                    Err(EventError::CertCacheStore(e)) => {
                        warn!(domains = ?domains_owned, error = ?e, "ACME: certificate cache store failed");
                    }
                    Err(EventError::AccountCacheStore(e)) => {
                        warn!(domains = ?domains_owned, error = ?e, "ACME: account cache store failed");
                    }
                    Err(EventError::CachedCertParse(e)) => {
                        error!(domains = ?domains_owned, error = ?e, "ACME: cached certificate parse failed");
                    }
                    Err(EventError::Order(e)) => {
                        warn!(domains = ?domains_owned, error = ?e, "ACME: certificate order failed, will retry");
                    }
                    Err(EventError::NewCertParse(e)) => {
                        error!(domains = ?domains_owned, error = ?e, "ACME: new certificate parse failed");
                    }
                }
            }
            debug!(domains = ?domains_owned, "ACME: state machine ended");
        });

        Ok(Self {
            rustls_config: config,
            acme_state_handle: Some(handle),
        })
    }

    /// Convert to a `quinn::ServerConfig` for QUIC transport.
    #[cfg(feature = "quinn")]
    pub fn for_quinn(self) -> Result<quinn::ServerConfig, TlsError> {
        use quinn::crypto::rustls::QuicServerConfig;
        let quic_server_config = QuicServerConfig::try_from(self.rustls_config)
            .map_err(|e| TlsError::Config(e.to_string()))?;
        Ok(quinn::ServerConfig::with_crypto(Arc::new(
            quic_server_config,
        )))
    }
}

fn build_rustls_server_config(
    tls_identity: &TlsIdentity,
    alpns: &[Vec<u8>],
) -> Result<rustls::ServerConfig, TlsError> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let client_verifier = Arc::new(AcceptAnyCertVerifier);
    match tls_identity {
        TlsIdentity::X509 { cert, key } => {
            let cert_chain = crate::pem::load_cert_chain(cert)?;
            let private_key = crate::pem::load_private_key(key)?;
            let mut config = rustls::ServerConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .map_err(|e| TlsError::Config(e.to_string()))?
                .with_client_cert_verifier(client_verifier)
                .with_single_cert(cert_chain, private_key)
                .map_err(|e| TlsError::Config(e.to_string()))?;
            config.alpn_protocols = alpns.to_vec();
            config.max_early_data_size = u32::MAX;
            Ok(config)
        }
        TlsIdentity::RawKey(secret_key) => {
            let resolver = Arc::new(RawKeyCertResolver::new(secret_key));
            let mut config = rustls::ServerConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .map_err(|e| TlsError::Config(e.to_string()))?
                .with_client_cert_verifier(client_verifier)
                .with_cert_resolver(resolver);
            config.alpn_protocols = alpns.to_vec();
            config.max_early_data_size = u32::MAX;
            Ok(config)
        }
        TlsIdentity::SelfSigned => {
            let cert = generate_self_signed_cert()?;
            let mut config = rustls::ServerConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .map_err(|e| TlsError::Config(e.to_string()))?
                .with_client_cert_verifier(client_verifier)
                .with_single_cert(cert.cert_chain, cert.private_key)
                .map_err(|e| TlsError::Config(e.to_string()))?;
            config.alpn_protocols = alpns.to_vec();
            config.max_early_data_size = u32::MAX;
            Ok(config)
        }
        TlsIdentity::Acme { .. } => {
            unreachable!(
                "TlsIdentity::Acme is handled by TlsServerConfig::new_acme, not build_rustls_server_config"
            )
        }
    }
}

struct SelfSignedCert {
    cert_chain: Vec<rustls::pki_types::CertificateDer<'static>>,
    private_key: rustls::pki_types::PrivateKeyDer<'static>,
}

fn generate_self_signed_cert() -> Result<SelfSignedCert, TlsError> {
    use rcgen::{CertificateParams, KeyPair};
    let key_pair =
        KeyPair::generate().map_err(|e| TlsError::Config(e.to_string()))?;
    let params = CertificateParams::default();
    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| TlsError::Config(e.to_string()))?;
    let cert_der = cert.der().clone();
    let key_der = rustls::pki_types::PrivateKeyDer::Pkcs8(
        rustls::pki_types::PrivatePkcs8KeyDer::from(key_pair.serialize_der()),
    );
    Ok(SelfSignedCert {
        cert_chain: vec![cert_der],
        private_key: key_der,
    })
}

/// Server-side "request-but-don't-require" client cert verifier (ADR-034).
///
/// Asks for a client TLS cert (X.509 or RFC 7250 raw key) so the endpoint can
/// extract the fingerprint via `peer_identity()`, but does not require one
/// and does not verify the presented cert against a CA. The fingerprint is
/// matched against `PeerEntry.fingerprints` by
/// `IdentityProvider::resolve_from_fingerprint()`.
///
/// **Server-side only.** This must not be reused as a client-side
/// `ServerCertVerifier` — the client-side verifier is selected by `PeerEntry`
/// presence (ADR-034 §3): CA verification for unknown X.509 remotes,
/// fingerprint pinning for known peers.
pub struct AcceptAnyCertVerifier;

impl std::fmt::Debug for AcceptAnyCertVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcceptAnyCertVerifier").finish()
    }
}

impl rustls::server::danger::ClientCertVerifier for AcceptAnyCertVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        false
    }

    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::server::danger::ClientCertVerified, rustls::Error> {
        Ok(rustls::server::danger::ClientCertVerified::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls12_signature(
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

pub struct RawKeyCertResolver {
    key: Arc<rustls::sign::CertifiedKey>,
}

impl RawKeyCertResolver {
    pub fn new(secret_key: &Ed25519SecretKey) -> Self {
        let signing_key = Arc::new(Ed25519SigningKey::new(secret_key.clone()));
        let public_key = signing_key.spki_public_key();
        let cert = rustls::pki_types::CertificateDer::from(public_key.to_vec());
        let certified_key = rustls::sign::CertifiedKey::new(vec![cert], signing_key);
        Self {
            key: Arc::new(certified_key),
        }
    }
}

impl rustls::server::ResolvesServerCert for RawKeyCertResolver {
    fn resolve(
        &self,
        _client_hello: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        Some(Arc::clone(&self.key))
    }

    fn only_raw_public_keys(&self) -> bool {
        true
    }
}

impl std::fmt::Debug for RawKeyCertResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawKeyCertResolver").finish()
    }
}
