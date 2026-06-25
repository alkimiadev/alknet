//! Endpoint: `AlknetEndpoint`, `HandlerRegistry`, `EndpointError`.
//!
//! See `docs/architecture/crates/core/endpoint.md` for the full specification.

use std::collections::HashMap;
use std::io;
#[cfg(any(feature = "quinn", feature = "iroh"))]
use std::net::SocketAddr;
#[cfg(feature = "quinn")]
use std::path::Path;
use std::sync::Arc;
#[cfg(any(feature = "quinn", feature = "iroh"))]
use std::time::Duration;

#[cfg(any(feature = "quinn", feature = "iroh"))]
use arc_swap::ArcSwap;
#[cfg(any(feature = "quinn", feature = "iroh"))]
use tokio::sync::watch;
#[cfg(any(feature = "quinn", feature = "iroh"))]
use tracing::{debug, error, warn};

#[cfg(any(feature = "quinn", feature = "iroh"))]
use crate::auth::{AuthContext, IdentityProvider};
#[cfg(any(feature = "quinn", feature = "iroh"))]
use crate::config::{DynamicConfig, StaticConfig, TlsIdentity};
#[cfg(not(any(feature = "quinn", feature = "iroh")))]
use crate::types::ProtocolHandler;
#[cfg(any(feature = "quinn", feature = "iroh"))]
use crate::types::{Connection, ProtocolHandler};

#[derive(Debug, thiserror::Error)]
pub enum EndpointError {
    #[error("bind failed: {0}")]
    BindFailed(io::Error),
    #[error("tls config error: {0}")]
    TlsConfig(io::Error),
    #[error("handler not found for ALPN {0:?}")]
    HandlerNotFound(Vec<u8>),
}

pub struct HandlerRegistry {
    handlers: HashMap<&'static [u8], Arc<dyn ProtocolHandler>>,
}

impl HandlerRegistry {
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    pub fn register(&mut self, handler: Arc<dyn ProtocolHandler>) {
        let alpn = handler.alpn();
        if self.handlers.contains_key(alpn) {
            panic!(
                "HandlerRegistry: ALPN already registered: {:?}",
                String::from_utf8_lossy(alpn)
            );
        }
        self.handlers.insert(alpn, handler);
    }

    pub fn get(&self, alpn: &[u8]) -> Option<&Arc<dyn ProtocolHandler>> {
        self.handlers.get(alpn)
    }

    pub fn alpn_strings(&self) -> Vec<Vec<u8>> {
        self.handlers.keys().map(|k| k.to_vec()).collect()
    }
}

impl Default for HandlerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for HandlerRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HandlerRegistry")
            .field(
                "alpns",
                &self
                    .handlers
                    .keys()
                    .map(|k| String::from_utf8_lossy(k).to_string())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[cfg(any(feature = "quinn", feature = "iroh"))]
pub struct AlknetEndpoint {
    #[cfg(feature = "quinn")]
    quinn: Option<quinn::Endpoint>,
    #[cfg(feature = "iroh")]
    iroh: Option<iroh::Endpoint>,
    handlers: Arc<HandlerRegistry>,
    #[allow(dead_code)]
    dynamic: Arc<ArcSwap<DynamicConfig>>,
    identity_provider: Arc<dyn IdentityProvider>,
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
    drain_timeout: Duration,
    #[cfg(feature = "acme")]
    acme_state_handle: Option<tokio::task::JoinHandle<()>>,
}

#[cfg(any(feature = "quinn", feature = "iroh"))]
impl std::fmt::Debug for AlknetEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AlknetEndpoint")
            .field("handlers", &self.handlers)
            .field("drain_timeout", &self.drain_timeout)
            .finish()
    }
}

#[cfg(any(feature = "quinn", feature = "iroh"))]
impl AlknetEndpoint {
    pub async fn new(
        static_config: &StaticConfig,
        handlers: HandlerRegistry,
        dynamic: Arc<ArcSwap<DynamicConfig>>,
        identity_provider: Arc<dyn IdentityProvider>,
    ) -> Result<Self, EndpointError> {
        let handlers = Arc::new(handlers);
        let alpns = handlers.alpn_strings();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let drain_timeout = static_config.drain_timeout;

        #[cfg(feature = "quinn")]
        #[cfg_attr(not(feature = "acme"), allow(unused_variables))]
        let (quinn, acme_state_handle) = if let Some(listen_addr) = static_config.listen_addr {
            let tls_identity = static_config.tls_identity.as_ref().ok_or_else(|| {
                EndpointError::TlsConfig(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "quinn endpoint requires tls_identity in static config",
                ))
            })?;
            let tls_setup = TlsSetup::new(tls_identity, &alpns).await?;
            let server_config =
                build_quinn_server_config_from_rustls(tls_setup.server_config)?;
            let endpoint = quinn::Endpoint::server(server_config, listen_addr)
                .map_err(EndpointError::BindFailed)?;
            #[cfg(feature = "acme")]
            {
                (Some(endpoint), tls_setup.acme_state_handle)
            }
            #[cfg(not(feature = "acme"))]
            {
                (Some(endpoint), None::<tokio::task::JoinHandle<()>>)
            }
        } else {
            (None, None)
        };

        #[cfg(not(feature = "quinn"))]
        if static_config.listen_addr.is_some() {
            return Err(EndpointError::TlsConfig(io::Error::new(
                io::ErrorKind::Unsupported,
                "quinn feature is not enabled but listen_addr was set",
            )));
        }

        #[cfg(feature = "iroh")]
        let iroh = if static_config.iroh_relay.is_some() || has_iroh_identity(static_config) {
            Some(build_iroh_endpoint(static_config, &alpns).await?)
        } else {
            None
        };

        Ok(Self {
            #[cfg(feature = "quinn")]
            quinn,
            #[cfg(feature = "iroh")]
            iroh,
            handlers,
            dynamic,
            identity_provider,
            shutdown_tx,
            shutdown_rx,
            drain_timeout,
            #[cfg(feature = "acme")]
            acme_state_handle,
        })
    }

    pub fn shutdown_sender(&self) -> watch::Sender<bool> {
        self.shutdown_tx.clone()
    }

    pub async fn shutdown(&self) -> Result<(), EndpointError> {
        let _ = self.shutdown_tx.send(true);

        #[cfg(feature = "quinn")]
        if let Some(quinn) = &self.quinn {
            quinn.close(0u32.into(), b"shutdown");
        }

        #[cfg(feature = "iroh")]
        if let Some(iroh) = &self.iroh {
            iroh.close().await;
        }

        #[cfg(feature = "acme")]
        if let Some(handle) = &self.acme_state_handle {
            handle.abort();
        }

        tokio::time::sleep(self.drain_timeout).await;

        #[cfg(feature = "quinn")]
        if let Some(quinn) = &self.quinn {
            quinn.wait_idle().await;
        }

        Ok(())
    }

    pub async fn run(self: Arc<Self>) {
        let mut tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();

        #[cfg(feature = "quinn")]
        if let Some(quinn) = &self.quinn {
            let quinn = quinn.clone();
            let handlers = self.handlers.clone();
            let identity_provider = self.identity_provider.clone();
            let mut shutdown_rx = self.shutdown_rx.clone();
            let task = tokio::spawn(async move {
                run_quinn_accept_loop(quinn, handlers, identity_provider, &mut shutdown_rx).await;
            });
            tasks.push(task);
        }

        #[cfg(feature = "iroh")]
        if let Some(iroh) = &self.iroh {
            let iroh = iroh.clone();
            let handlers = self.handlers.clone();
            let identity_provider = self.identity_provider.clone();
            let mut shutdown_rx = self.shutdown_rx.clone();
            let task = tokio::spawn(async move {
                run_iroh_accept_loop(iroh, handlers, identity_provider, &mut shutdown_rx).await;
            });
            tasks.push(task);
        }

        for task in tasks {
            let _ = task.await;
        }
    }
}

#[cfg(feature = "iroh")]
fn has_iroh_identity(static_config: &StaticConfig) -> bool {
    matches!(
        static_config.tls_identity.as_ref(),
        Some(TlsIdentity::RawKey(_))
    )
}

#[cfg(feature = "quinn")]
async fn run_quinn_accept_loop(
    quinn: quinn::Endpoint,
    handlers: Arc<HandlerRegistry>,
    identity_provider: Arc<dyn IdentityProvider>,
    shutdown_rx: &mut watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                debug!("quinn accept loop: shutdown signaled");
                break;
            }
            incoming = quinn.accept() => {
                let Some(incoming) = incoming else {
                    debug!("quinn accept loop: endpoint closed");
                    break;
                };
                let connecting = match incoming.accept() {
                    Ok(c) => c,
                    Err(e) => {
                        warn!("quinn accept failed: {e}");
                        continue;
                    }
                };
                let handlers = handlers.clone();
                let identity_provider = identity_provider.clone();
                tokio::spawn(async move {
                    let connection = match connecting.await {
                        Ok(conn) => conn,
                        Err(e) => {
                            warn!("quinn TLS handshake failure: {e}");
                            return;
                        }
                    };
                    dispatch_quinn(connection, &handlers, &identity_provider);
                });
            }
        }
    }
}

#[cfg(feature = "quinn")]
fn dispatch_quinn(
    connection: quinn::Connection,
    handlers: &HandlerRegistry,
    identity_provider: &Arc<dyn IdentityProvider>,
) {
    let alpn = extract_quinn_alpn(&connection);

    #[cfg(feature = "acme")]
    if alpn == b"acme-tls/1" {
        debug!("acme-tls/1 challenge connection completed at TLS layer; closing");
        connection.close(0u32.into(), b"acme done");
        return;
    }

    let handler = match handlers.get(&alpn) {
        Some(h) => h.clone(),
        None => {
            connection.close(0u32.into(), b"no handler");
            warn!(
                "quinn dispatch: no handler for ALPN {:?}",
                String::from_utf8_lossy(&alpn)
            );
            return;
        }
    };

    let remote_addr = Some(connection.remote_address());
    let fingerprint = extract_quinn_client_fingerprint(&connection);
    let auth = build_auth_context(&alpn, remote_addr, fingerprint, identity_provider);
    let conn = Connection::from_quinn_with_alpn(connection, alpn.clone());
    tokio::spawn(async move {
        if let Err(e) = handler.handle(conn, &auth).await {
            error!("handler returned error: {e}");
        }
    });
}

#[cfg(feature = "quinn")]
fn extract_quinn_alpn(connection: &quinn::Connection) -> Vec<u8> {
    use quinn::crypto::rustls::HandshakeData;
    if let Some(data) = connection.handshake_data() {
        if let Ok(hs) = data.downcast::<HandshakeData>() {
            if let Some(protocol) = hs.protocol {
                return protocol;
            }
        }
    }
    Vec::new()
}

#[cfg(feature = "quinn")]
fn extract_quinn_client_fingerprint(connection: &quinn::Connection) -> Option<String> {
    let identity = connection.peer_identity()?;
    let certs = identity
        .downcast::<Vec<rustls::pki_types::CertificateDer>>()
        .ok()?;
    let leaf = certs.first()?;
    fingerprint_from_cert_der(leaf.as_ref())
}

#[cfg(any(feature = "quinn", feature = "iroh"))]
fn fingerprint_from_cert_der(cert_der: &[u8]) -> Option<String> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(cert_der);
    let digest = hasher.finalize();
    Some(format!("SHA256:{}", hex::encode(digest)))
}

#[cfg(feature = "iroh")]
async fn run_iroh_accept_loop(
    iroh: iroh::Endpoint,
    handlers: Arc<HandlerRegistry>,
    identity_provider: Arc<dyn IdentityProvider>,
    shutdown_rx: &mut watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                debug!("iroh accept loop: shutdown signaled");
                break;
            }
            incoming = iroh.accept() => {
                let Some(incoming) = incoming else {
                    debug!("iroh accept loop: endpoint closed");
                    break;
                };
                let handlers = handlers.clone();
                let identity_provider = identity_provider.clone();
                tokio::spawn(async move {
                    let mut connecting = match incoming.accept() {
                        Ok(c) => c,
                        Err(e) => {
                            warn!("iroh accept failed: {e}");
                            return;
                        }
                    };
                    let alpn = match connecting.alpn().await {
                        Ok(alpn) => alpn,
                        Err(e) => {
                            warn!("iroh ALPN negotiation failed: {e}");
                            return;
                        }
                    };
                    let connection = match connecting.await {
                        Ok(conn) => conn,
                        Err(e) => {
                            warn!("iroh handshake completion failed: {e}");
                            return;
                        }
                    };
                    dispatch_iroh(connection, alpn, &handlers, &identity_provider);
                });
            }
        }
    }
}

#[cfg(feature = "iroh")]
fn dispatch_iroh(
    connection: iroh::endpoint::Connection,
    alpn: Vec<u8>,
    handlers: &HandlerRegistry,
    identity_provider: &Arc<dyn IdentityProvider>,
) {
    let handler = match handlers.get(&alpn) {
        Some(h) => h.clone(),
        None => {
            connection.close(0u32.into(), b"no handler");
            warn!(
                "iroh dispatch: no handler for ALPN {:?}",
                String::from_utf8_lossy(&alpn)
            );
            return;
        }
    };

    let fingerprint = extract_iroh_client_fingerprint(&connection);
    let auth = build_auth_context(&alpn, None, fingerprint, identity_provider);
    let conn = Connection::from_iroh(connection);
    tokio::spawn(async move {
        if let Err(e) = handler.handle(conn, &auth).await {
            error!("handler returned error: {e}");
        }
    });
}

#[cfg(feature = "iroh")]
fn extract_iroh_client_fingerprint(connection: &iroh::endpoint::Connection) -> Option<String> {
    let node_id = connection.remote_node_id().ok()?;
    Some(format!("ed25519:{}", node_id))
}

#[cfg(any(feature = "quinn", feature = "iroh"))]
fn build_auth_context(
    alpn: &[u8],
    remote_addr: Option<SocketAddr>,
    tls_client_fingerprint: Option<String>,
    identity_provider: &Arc<dyn IdentityProvider>,
) -> AuthContext {
    let identity = tls_client_fingerprint
        .as_ref()
        .and_then(|fp| identity_provider.resolve_from_fingerprint(fp));
    AuthContext {
        identity,
        alpn: alpn.to_vec(),
        remote_addr,
        tls_client_fingerprint,
    }
}

#[cfg(feature = "quinn")]
struct TlsSetup {
    server_config: rustls::ServerConfig,
    #[cfg(feature = "acme")]
    acme_state_handle: Option<tokio::task::JoinHandle<()>>,
}
#[cfg(feature = "quinn")]
impl TlsSetup {
    async fn new(
        tls_identity: &TlsIdentity,
        alpns: &[Vec<u8>],
    ) -> Result<Self, EndpointError> {
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
                    Err(EndpointError::TlsConfig(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "ACME feature not enabled but TlsIdentity::Acme configured",
                    )))
                }
            }
            _ => {
                let server_config = build_rustls_server_config(tls_identity, alpns)?;
                Ok(Self {
                    server_config,
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
        directory: &crate::config::AcmeDirectory,
        contact: &[String],
        alpns: &[Vec<u8>],
    ) -> Result<Self, EndpointError> {
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
            .map_err(|e| EndpointError::TlsConfig(io::Error::other(e)))?
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
            server_config: config,
            acme_state_handle: Some(handle),
        })
    }
}

#[cfg(feature = "quinn")]
fn build_quinn_server_config_from_rustls(
    rustls_config: rustls::ServerConfig,
) -> Result<quinn::ServerConfig, EndpointError> {
    use quinn::crypto::rustls::QuicServerConfig;
    let quic_server_config = QuicServerConfig::try_from(rustls_config)
        .map_err(|e| EndpointError::TlsConfig(io::Error::other(e)))?;
    Ok(quinn::ServerConfig::with_crypto(Arc::new(
        quic_server_config,
    )))
}

#[cfg(feature = "quinn")]
fn build_rustls_server_config(
    tls_identity: &TlsIdentity,
    alpns: &[Vec<u8>],
) -> Result<rustls::ServerConfig, EndpointError> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let client_verifier = Arc::new(AcceptAnyCertVerifier);
    match tls_identity {
        TlsIdentity::X509 { cert, key } => {
            let cert_chain = load_cert_chain(cert)?;
            let private_key = load_private_key(key)?;
            let mut config = rustls::ServerConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .map_err(|e| EndpointError::TlsConfig(io::Error::other(e)))?
                .with_client_cert_verifier(client_verifier)
                .with_single_cert(cert_chain, private_key)
                .map_err(|e| EndpointError::TlsConfig(io::Error::other(e)))?;
            config.alpn_protocols = alpns.to_vec();
            config.max_early_data_size = u32::MAX;
            Ok(config)
        }
        TlsIdentity::RawKey(secret_key) => {
            let resolver = Arc::new(RawKeyCertResolver::new(secret_key));
            let mut config = rustls::ServerConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .map_err(|e| EndpointError::TlsConfig(io::Error::other(e)))?
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
                .map_err(|e| EndpointError::TlsConfig(io::Error::other(e)))?
                .with_client_cert_verifier(client_verifier)
                .with_single_cert(cert.cert_chain, cert.private_key)
                .map_err(|e| EndpointError::TlsConfig(io::Error::other(e)))?;
            config.alpn_protocols = alpns.to_vec();
            config.max_early_data_size = u32::MAX;
            Ok(config)
        }
        TlsIdentity::Acme { .. } => {
            unreachable!("TlsIdentity::Acme is handled by TlsSetup::new_acme, not build_rustls_server_config")
        }
    }
}

#[cfg(feature = "iroh")]
async fn build_iroh_endpoint(
    static_config: &StaticConfig,
    alpns: &[Vec<u8>],
) -> Result<iroh::Endpoint, EndpointError> {
    let mut builder = iroh::Endpoint::builder();

    if let Some(TlsIdentity::RawKey(secret_key)) = static_config.tls_identity.as_ref() {
        let iroh_key = iroh::SecretKey::from_bytes(&secret_key.as_bytes());
        builder = builder.secret_key(iroh_key);
    } else {
        let mut csprng = rand::rngs::OsRng;
        builder = builder.secret_key(iroh::SecretKey::generate(&mut csprng));
    }

    builder = builder.alpns(alpns.to_vec());

    if let Some(relay_url) = static_config.iroh_relay.as_ref() {
        let relay_map = iroh::RelayMap::from(relay_url.clone());
        builder = builder.relay_mode(iroh::RelayMode::Custom(relay_map));
    } else {
        builder = builder.relay_mode(iroh::RelayMode::Disabled);
    }

    builder
        .bind()
        .await
        .map_err(|e| EndpointError::BindFailed(io::Error::other(e)))
}

#[cfg(feature = "quinn")]
fn load_cert_chain(
    path: &Path,
) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>, EndpointError> {
    let bytes = std::fs::read(path).map_err(|e| EndpointError::TlsConfig(io::Error::other(e)))?;
    let mut reader = std::io::BufReader::new(bytes.as_slice());
    rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| EndpointError::TlsConfig(io::Error::other(e)))
}

#[cfg(feature = "quinn")]
fn load_private_key(
    path: &Path,
) -> Result<rustls::pki_types::PrivateKeyDer<'static>, EndpointError> {
    let bytes = std::fs::read(path).map_err(|e| EndpointError::TlsConfig(io::Error::other(e)))?;
    let mut reader = std::io::BufReader::new(bytes.as_slice());
    match rustls_pemfile::private_key(&mut reader) {
        Ok(Some(key)) => Ok(key),
        Ok(None) => Err(EndpointError::TlsConfig(io::Error::new(
            io::ErrorKind::InvalidData,
            "no private key found in file",
        ))),
        Err(e) => Err(EndpointError::TlsConfig(io::Error::other(e))),
    }
}

#[cfg(feature = "quinn")]
struct SelfSignedCert {
    cert_chain: Vec<rustls::pki_types::CertificateDer<'static>>,
    private_key: rustls::pki_types::PrivateKeyDer<'static>,
}

#[cfg(feature = "quinn")]
fn generate_self_signed_cert() -> Result<SelfSignedCert, EndpointError> {
    use rcgen::{CertificateParams, KeyPair};
    let key_pair =
        KeyPair::generate().map_err(|e| EndpointError::TlsConfig(io::Error::other(e)))?;
    let params = CertificateParams::default();
    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| EndpointError::TlsConfig(io::Error::other(e)))?;
    let cert_der = cert.der().clone();
    let key_der = rustls::pki_types::PrivateKeyDer::Pkcs8(
        rustls::pki_types::PrivatePkcs8KeyDer::from(key_pair.serialize_der()),
    );
    Ok(SelfSignedCert {
        cert_chain: vec![cert_der],
        private_key: key_der,
    })
}

#[cfg(feature = "quinn")]
struct AcceptAnyCertVerifier;

#[cfg(feature = "quinn")]
impl std::fmt::Debug for AcceptAnyCertVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcceptAnyCertVerifier").finish()
    }
}

#[cfg(feature = "quinn")]
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

#[cfg(feature = "quinn")]
struct RawKeyCertResolver {
    key: Arc<rustls::sign::CertifiedKey>,
}

#[cfg(feature = "quinn")]
impl RawKeyCertResolver {
    fn new(secret_key: &crate::config::Ed25519SecretKey) -> Self {
        let signing_key = Arc::new(Ed25519SigningKey::new(secret_key.clone()));
        let public_key = signing_key.spki_public_key();
        let cert = rustls::pki_types::CertificateDer::from(public_key.to_vec());
        let certified_key = rustls::sign::CertifiedKey::new(vec![cert], signing_key);
        Self {
            key: Arc::new(certified_key),
        }
    }
}

#[cfg(feature = "quinn")]
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

#[cfg(feature = "quinn")]
impl std::fmt::Debug for RawKeyCertResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawKeyCertResolver").finish()
    }
}

#[cfg(feature = "quinn")]
#[derive(Clone)]
struct Ed25519SigningKey {
    key: crate::config::Ed25519SecretKey,
}

#[cfg(feature = "quinn")]
impl std::fmt::Debug for Ed25519SigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ed25519SigningKey").finish()
    }
}

#[cfg(feature = "quinn")]
impl Ed25519SigningKey {
    fn new(key: crate::config::Ed25519SecretKey) -> Self {
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
    use crate::auth::AuthContext;
    #[cfg(any(feature = "quinn", feature = "iroh"))]
    use crate::auth::{AuthToken, Identity, IdentityProvider};
    use crate::types::{Connection, HandlerError};
    use async_trait::async_trait;

    struct DummyHandler {
        alpn: &'static [u8],
    }

    #[async_trait]
    impl ProtocolHandler for DummyHandler {
        fn alpn(&self) -> &'static [u8] {
            self.alpn
        }
        async fn handle(
            &self,
            _connection: Connection,
            _auth: &AuthContext,
        ) -> Result<(), HandlerError> {
            Ok(())
        }
    }

    fn make_handler(alpn: &'static [u8]) -> Arc<dyn ProtocolHandler> {
        Arc::new(DummyHandler { alpn })
    }

    #[test]
    fn handler_registry_new_is_empty() {
        let reg = HandlerRegistry::new();
        assert!(reg.alpn_strings().is_empty());
        assert!(reg.get(b"alknet/test").is_none());
    }

    #[test]
    fn handler_registry_register_then_get() {
        let mut reg = HandlerRegistry::new();
        reg.register(make_handler(b"alknet/test"));
        assert_eq!(reg.alpn_strings(), vec![b"alknet/test".to_vec()]);
        assert!(reg.get(b"alknet/test").is_some());
        assert!(reg.get(b"alknet/other").is_none());
    }

    #[test]
    fn handler_registry_multiple_alpns() {
        let mut reg = HandlerRegistry::new();
        reg.register(make_handler(b"alknet/ssh"));
        reg.register(make_handler(b"alknet/call"));
        let mut alpns = reg
            .alpn_strings()
            .into_iter()
            .map(|a| String::from_utf8(a).unwrap())
            .collect::<Vec<_>>();
        alpns.sort();
        assert_eq!(alpns, vec!["alknet/call", "alknet/ssh"]);
        assert!(reg.get(b"alknet/ssh").is_some());
        assert!(reg.get(b"alknet/call").is_some());
    }

    #[test]
    #[should_panic(expected = "ALPN already registered")]
    fn handler_registry_register_panics_on_duplicate() {
        let mut reg = HandlerRegistry::new();
        reg.register(make_handler(b"alknet/test"));
        reg.register(make_handler(b"alknet/test"));
    }

    #[test]
    fn handler_registry_debug_lists_alpns() {
        let mut reg = HandlerRegistry::new();
        reg.register(make_handler(b"alknet/test"));
        let s = format!("{:?}", reg);
        assert!(s.contains("alknet/test"));
    }

    #[test]
    fn endpoint_error_display() {
        let e = EndpointError::BindFailed(io::Error::new(io::ErrorKind::AddrInUse, "busy"));
        assert!(format!("{e}").contains("bind failed"));
        let e = EndpointError::TlsConfig(io::Error::new(io::ErrorKind::InvalidData, "bad"));
        assert!(format!("{e}").contains("tls config error"));
        let e = EndpointError::HandlerNotFound(b"alknet/test".to_vec());
        assert!(format!("{e}").contains("handler not found"));
    }

    #[cfg(any(feature = "quinn", feature = "iroh"))]
    #[test]
    fn build_auth_context_resolves_identity_from_fingerprint() {
        struct StaticProvider;
        impl IdentityProvider for StaticProvider {
            fn resolve_from_fingerprint(&self, fp: &str) -> Option<Identity> {
                if fp == "SHA256:known" {
                    Some(Identity {
                        id: "SHA256:known".to_string(),
                        scopes: vec![],
                        resources: HashMap::new(),
                    })
                } else {
                    None
                }
            }
            fn resolve_from_token(&self, _token: &AuthToken) -> Option<Identity> {
                None
            }
        }
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticProvider);
        let auth = build_auth_context(
            b"alknet/test",
            None,
            Some("SHA256:known".to_string()),
            &provider,
        );
        assert_eq!(auth.identity.as_ref().unwrap().id, "SHA256:known");
        assert_eq!(auth.alpn, b"alknet/test");
        assert_eq!(auth.tls_client_fingerprint.as_deref(), Some("SHA256:known"));
    }

    #[cfg(any(feature = "quinn", feature = "iroh"))]
    #[test]
    fn build_auth_context_no_fingerprint_no_identity() {
        struct NoProvider;
        impl IdentityProvider for NoProvider {
            fn resolve_from_fingerprint(&self, _fp: &str) -> Option<Identity> {
                None
            }
            fn resolve_from_token(&self, _token: &AuthToken) -> Option<Identity> {
                None
            }
        }
        let provider: Arc<dyn IdentityProvider> = Arc::new(NoProvider);
        let auth = build_auth_context(b"alknet/test", None, None, &provider);
        assert!(auth.identity.is_none());
        assert!(auth.tls_client_fingerprint.is_none());
    }

    #[cfg(any(feature = "quinn", feature = "iroh"))]
    #[test]
    fn build_auth_context_fingerprint_unknown_identity_none() {
        struct StaticProvider;
        impl IdentityProvider for StaticProvider {
            fn resolve_from_fingerprint(&self, _fp: &str) -> Option<Identity> {
                None
            }
            fn resolve_from_token(&self, _token: &AuthToken) -> Option<Identity> {
                None
            }
        }
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticProvider);
        let auth = build_auth_context(
            b"alknet/test",
            None,
            Some("SHA256:unknown".to_string()),
            &provider,
        );
        assert!(auth.identity.is_none());
        assert!(auth.tls_client_fingerprint.is_some());
    }

    #[cfg(feature = "quinn")]
    #[test]
    fn raw_key_cert_resolver_only_raw_public_keys() {
        use rustls::server::ResolvesServerCert;
        let sk = crate::config::Ed25519SecretKey::generate();
        let resolver = RawKeyCertResolver::new(&sk);
        assert!(resolver.only_raw_public_keys());
    }

    #[cfg(feature = "iroh")]
    #[tokio::test]
    async fn endpoint_constructs_with_iroh_raw_key_identity() {
        let static_config = StaticConfig {
            listen_addr: None,
            tls_identity: Some(TlsIdentity::RawKey(crate::config::Ed25519SecretKey::generate())),
            iroh_relay: None,
            drain_timeout: Duration::from_millis(10),
        };
        let dynamic = Arc::new(ArcSwap::from_pointee(DynamicConfig::default()));
        struct NoProvider;
        impl IdentityProvider for NoProvider {
            fn resolve_from_fingerprint(&self, _: &str) -> Option<Identity> {
                None
            }
            fn resolve_from_token(&self, _: &AuthToken) -> Option<Identity> {
                None
            }
        }
        let provider: Arc<dyn IdentityProvider> = Arc::new(NoProvider);
        let mut registry = HandlerRegistry::new();
        registry.register(make_handler(b"alknet/test"));
        let endpoint = AlknetEndpoint::new(&static_config, registry, dynamic, provider)
            .await
            .expect("endpoint constructs");
        assert!(endpoint.shutdown_sender().send(true).is_ok());
        endpoint.shutdown().await.expect("shutdown ok");
    }

    #[cfg(feature = "iroh")]
    #[tokio::test]
    async fn iroh_endpoint_runs_accept_loop_and_shutdown() {
        use std::sync::Mutex;
        let server_sk = crate::config::Ed25519SecretKey::generate();
        let static_config = StaticConfig {
            listen_addr: None,
            tls_identity: Some(TlsIdentity::RawKey(server_sk)),
            iroh_relay: None,
            drain_timeout: Duration::from_millis(20),
        };
        let dynamic = Arc::new(ArcSwap::from_pointee(DynamicConfig::default()));
        struct NoProvider;
        impl IdentityProvider for NoProvider {
            fn resolve_from_fingerprint(&self, _: &str) -> Option<Identity> {
                None
            }
            fn resolve_from_token(&self, _: &AuthToken) -> Option<Identity> {
                None
            }
        }
        let provider: Arc<dyn IdentityProvider> = Arc::new(NoProvider);

        let connected = Arc::new(Mutex::new(false));
        let connected_clone = connected.clone();
        struct CountingHandler {
            alpn: &'static [u8],
            connected: Arc<Mutex<bool>>,
        }
        #[async_trait]
        impl ProtocolHandler for CountingHandler {
            fn alpn(&self) -> &'static [u8] {
                self.alpn
            }
            async fn handle(
                &self,
                _conn: Connection,
                _auth: &AuthContext,
            ) -> Result<(), HandlerError> {
                *self.connected.lock().unwrap() = true;
                Ok(())
            }
        }
        let mut registry = HandlerRegistry::new();
        registry.register(Arc::new(CountingHandler {
            alpn: b"alknet/test",
            connected: connected_clone,
        }));
        let endpoint = Arc::new(
            AlknetEndpoint::new(&static_config, registry, dynamic, provider)
                .await
                .expect("endpoint constructs"),
        );

        let run_endpoint = endpoint.clone();
        let run_task = tokio::spawn(async move {
            run_endpoint.run().await;
        });

        let _ = endpoint.shutdown_sender().send(true);
        endpoint.shutdown().await.ok();
        let _ = run_task.await;
        assert!(!*connected.lock().unwrap());
    }

    #[cfg(feature = "quinn")]
    #[test]
    fn self_signed_cert_generation_produces_cert_and_key() {
        let cert = generate_self_signed_cert().expect("self-signed cert generates");
        assert!(!cert.cert_chain.is_empty());
        assert!(!cert.private_key.secret_der().is_empty());
    }

    #[cfg(any(feature = "quinn", feature = "iroh"))]
    #[test]
    fn dispatch_decision_logic_lookup_and_auth() {
        let mut registry = HandlerRegistry::new();
        registry.register(make_handler(b"alknet/ssh"));
        registry.register(make_handler(b"alknet/call"));

        struct StaticProvider;
        impl IdentityProvider for StaticProvider {
            fn resolve_from_fingerprint(&self, fp: &str) -> Option<Identity> {
                if fp == "SHA256:caller" {
                    Some(Identity {
                        id: "SHA256:caller".to_string(),
                        scopes: vec!["relay:connect".to_string()],
                        resources: HashMap::new(),
                    })
                } else {
                    None
                }
            }
            fn resolve_from_token(&self, _: &AuthToken) -> Option<Identity> {
                None
            }
        }
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticProvider);

        let ssh_handler = registry.get(b"alknet/ssh").expect("ssh handler registered");
        assert_eq!(ssh_handler.alpn(), b"alknet/ssh");
        let auth = build_auth_context(
            b"alknet/ssh",
            Some(std::net::SocketAddr::new(
                std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                1234,
            )),
            Some("SHA256:caller".to_string()),
            &provider,
        );
        assert_eq!(auth.identity.as_ref().unwrap().id, "SHA256:caller");
        assert_eq!(auth.alpn, b"alknet/ssh");

        let unknown = registry.get(b"alknet/unknown");
        assert!(unknown.is_none(), "unknown ALPN has no handler");
    }

    #[cfg(any(feature = "quinn", feature = "iroh"))]
    #[test]
    fn fingerprint_from_cert_der_produces_sha256_hex_format() {
        let cert_der = b"fake-leaf-cert-der-bytes";
        let fp = fingerprint_from_cert_der(cert_der).expect("non-empty cert produces fingerprint");
        assert!(
            fp.starts_with("SHA256:"),
            "fingerprint must be SHA256-prefixed, got: {fp}"
        );
        let hex_part = &fp["SHA256:".len()..];
        assert_eq!(
            hex_part.len(),
            64,
            "hex digest must be 64 chars (32 bytes), got: {fp}"
        );
        assert!(
            hex_part.chars().all(|c| c.is_ascii_hexdigit()),
            "hex part must be lowercase hex, got: {fp}"
        );

        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(cert_der);
        let expected = format!("SHA256:{}", hex::encode(hasher.finalize()));
        assert_eq!(fp, expected, "fingerprint must match SHA-256 of cert DER");
    }

    #[cfg(any(feature = "quinn", feature = "iroh"))]
    #[test]
    fn fingerprint_from_cert_der_deterministic() {
        let cert = b"some-cert";
        let a = fingerprint_from_cert_der(cert).unwrap();
        let b = fingerprint_from_cert_der(cert).unwrap();
        assert_eq!(a, b, "same cert DER must produce same fingerprint");
    }

    #[test]
    fn acme_directory_production_url() {
        use crate::config::AcmeDirectory;
        let dir = AcmeDirectory::Production;
        assert_eq!(
            dir.url(),
            "https://acme-v02.api.letsencrypt.org/directory"
        );
    }

    #[test]
    fn acme_directory_staging_url() {
        use crate::config::AcmeDirectory;
        let dir = AcmeDirectory::Staging;
        assert_eq!(
            dir.url(),
            "https://acme-staging-v02.api.letsencrypt.org/directory"
        );
    }

    #[test]
    fn acme_directory_custom_url() {
        use crate::config::AcmeDirectory;
        let url = "https://custom-acme.example.com/directory";
        let dir = AcmeDirectory::Custom(url.to_string());
        assert_eq!(dir.url(), url);
    }

    #[cfg(feature = "quinn")]
    #[tokio::test]
    async fn tls_setup_x509_returns_no_acme_state() {
        use rcgen::{CertificateParams, KeyPair};
        let key_pair = KeyPair::generate().unwrap();
        let params = CertificateParams::default();
        let cert = params.self_signed(&key_pair).unwrap();
        let cert_pem = cert.pem();
        let key_pem = key_pair.serialize_pem();

        let dir = tempfile::tempdir().unwrap();
        let cert_path = dir.path().join("cert.pem");
        let key_path = dir.path().join("key.pem");
        std::fs::write(&cert_path, cert_pem).unwrap();
        std::fs::write(&key_path, key_pem).unwrap();

        let tls_identity = TlsIdentity::X509 {
            cert: cert_path,
            key: key_path,
        };
        let setup = TlsSetup::new(&tls_identity, &[b"alknet/test".to_vec()])
            .await
            .expect("X509 tls setup should succeed");
        let _ = setup.server_config;
        #[cfg(feature = "acme")]
        assert!(setup.acme_state_handle.is_none());
    }

    // --- Tier A: directly-callable TLS / rustls helpers -------------------

    #[cfg(feature = "quinn")]
    #[test]
    fn handler_registry_default_is_empty() {
        let reg = HandlerRegistry::default();
        assert!(reg.alpn_strings().is_empty());
        assert!(reg.get(b"alknet/test").is_none());
    }

    #[cfg(any(feature = "quinn", feature = "iroh"))]
    #[test]
    fn handler_registry_debug_lists_alpns_via_default() {
        let reg = HandlerRegistry::default();
        let s = format!("{reg:?}");
        assert!(s.contains("HandlerRegistry"));
    }

    #[cfg(feature = "iroh")]
    #[test]
    fn has_iroh_identity_true_for_raw_key() {
        let cfg = StaticConfig {
            listen_addr: None,
            tls_identity: Some(TlsIdentity::RawKey(crate::config::Ed25519SecretKey::generate())),
            iroh_relay: None,
            drain_timeout: Duration::from_millis(10),
        };
        assert!(has_iroh_identity(&cfg));
    }

    #[cfg(feature = "iroh")]
    #[test]
    fn has_iroh_identity_false_for_x509() {
        let cfg = StaticConfig {
            listen_addr: None,
            tls_identity: Some(TlsIdentity::X509 {
                cert: std::path::PathBuf::from("/x.pem"),
                key: std::path::PathBuf::from("/x.pem"),
            }),
            iroh_relay: None,
            drain_timeout: Duration::from_millis(10),
        };
        assert!(!has_iroh_identity(&cfg));
    }

    #[cfg(feature = "iroh")]
    #[test]
    fn has_iroh_identity_false_when_no_identity() {
        let cfg = StaticConfig {
            listen_addr: None,
            tls_identity: None,
            iroh_relay: None,
            drain_timeout: Duration::from_millis(10),
        };
        assert!(!has_iroh_identity(&cfg));
    }

    #[cfg(feature = "quinn")]
    #[test]
    fn build_rustls_server_config_raw_key_succeeds() {
        let sk = crate::config::Ed25519SecretKey::generate();
        let identity = TlsIdentity::RawKey(sk);
        let alpns = vec![b"alknet/test".to_vec(), b"alknet/call".to_vec()];
        let config = build_rustls_server_config(&identity, &alpns).expect("raw key config builds");
        assert_eq!(config.alpn_protocols, alpns);
        assert_eq!(config.max_early_data_size, u32::MAX);
    }

    #[cfg(feature = "quinn")]
    #[test]
    fn build_rustls_server_config_self_signed_succeeds() {
        let identity = TlsIdentity::SelfSigned;
        let alpns = vec![b"alknet/test".to_vec()];
        let config =
            build_rustls_server_config(&identity, &alpns).expect("self-signed config builds");
        assert_eq!(config.alpn_protocols, alpns);
        assert_eq!(config.max_early_data_size, u32::MAX);
    }

    #[cfg(feature = "quinn")]
    #[test]
    #[should_panic(expected = "TlsIdentity::Acme is handled by TlsSetup::new_acme")]
    fn build_rustls_server_config_acme_is_unreachable() {
        let identity = TlsIdentity::Acme {
            domains: vec!["example.com".to_string()],
            cache_dir: std::path::PathBuf::from("/tmp/alknet-acme-test"),
            directory: crate::config::AcmeDirectory::Staging,
            contact: vec!["mailto:dev@example.com".to_string()],
        };
        let _ = build_rustls_server_config(&identity, &[]);
    }

    #[cfg(feature = "quinn")]
    #[test]
    fn build_quinn_server_config_from_rustls_succeeds() {
        let sk = crate::config::Ed25519SecretKey::generate();
        let rustls_config =
            build_rustls_server_config(&TlsIdentity::RawKey(sk), &[b"alknet/test".to_vec()])
                .expect("rustls config builds");
        let quinn_config =
            build_quinn_server_config_from_rustls(rustls_config).expect("quinn config converts");
        let _ = quinn_config;
    }

    #[cfg(feature = "quinn")]
    #[test]
    fn load_private_key_returns_error_when_no_key_present() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty.key");
        std::fs::write(&empty, b"# no key here\njust a comment\n").unwrap();
        let err = load_private_key(&empty);
        assert!(
            matches!(err, Err(EndpointError::TlsConfig(_))),
            "empty key file must yield TlsConfig error, got {err:?}"
        );
    }

    #[cfg(feature = "quinn")]
    #[test]
    fn load_private_key_returns_error_when_file_missing() {
        let err = load_private_key(std::path::Path::new("/nonexistent/alknet-coverage/missing.key"));
        assert!(
            matches!(err, Err(EndpointError::TlsConfig(_))),
            "missing key file must yield TlsConfig error, got {err:?}"
        );
    }

    #[cfg(feature = "quinn")]
    #[test]
    fn load_cert_chain_returns_error_when_file_missing() {
        let err = load_cert_chain(std::path::Path::new("/nonexistent/alknet-coverage/missing.pem"));
        assert!(
            matches!(err, Err(EndpointError::TlsConfig(_))),
            "missing cert file must yield TlsConfig error, got {err:?}"
        );
    }

    // --- AcceptAnyCertVerifier trait methods ------------------------------

    #[cfg(feature = "quinn")]
    #[test]
    fn accept_any_cert_verifier_offers_and_does_not_require_client_auth() {
        use rustls::server::danger::ClientCertVerifier;
        let verifier = AcceptAnyCertVerifier;
        assert!(verifier.offer_client_auth());
        assert!(!verifier.client_auth_mandatory());
        assert!(verifier.root_hint_subjects().is_empty());
    }

    #[cfg(feature = "quinn")]
    #[test]
    fn accept_any_cert_verifier_verifies_any_client_cert() {
        use rustls::pki_types::{CertificateDer, UnixTime};
        use rustls::server::danger::ClientCertVerifier;
        let verifier = AcceptAnyCertVerifier;
        let cert = CertificateDer::from(b"fake-cert-der".to_vec());
        let result = verifier.verify_client_cert(&cert, &[], UnixTime::now());
        assert!(result.is_ok(), "AcceptAnyCertVerifier must accept any client cert");
    }

    #[cfg(feature = "quinn")]
    #[test]
    fn accept_any_cert_verifier_supported_schemes_are_non_empty() {
        use rustls::server::danger::ClientCertVerifier;
        let verifier = AcceptAnyCertVerifier;
        let schemes = verifier.supported_verify_schemes();
        assert!(!schemes.is_empty(), "must advertise at least one scheme");
        assert!(schemes.contains(&rustls::SignatureScheme::ED25519));
        assert!(schemes.contains(&rustls::SignatureScheme::RSA_PSS_SHA256));
    }

    #[cfg(feature = "quinn")]
    #[test]
    fn accept_any_cert_verifier_debug_is_implemented() {
        let verifier = AcceptAnyCertVerifier;
        let s = format!("{verifier:?}");
        assert!(s.contains("AcceptAnyCertVerifier"));
    }

    // --- Ed25519SigningKey trait impls ------------------------------------

    #[cfg(feature = "quinn")]
    #[test]
    fn ed25519_signing_key_choose_scheme_returns_some_for_ed25519() {
        use rustls::sign::SigningKey;
        let sk = crate::config::Ed25519SecretKey::generate();
        let signing_key = Ed25519SigningKey::new(sk);
        let signer = signing_key.choose_scheme(&[rustls::SignatureScheme::ED25519]);
        assert!(signer.is_some(), "must produce a signer when ED25519 is offered");
    }

    #[cfg(feature = "quinn")]
    #[test]
    fn ed25519_signing_key_choose_scheme_returns_none_without_ed25519() {
        use rustls::sign::SigningKey;
        let sk = crate::config::Ed25519SecretKey::generate();
        let signing_key = Ed25519SigningKey::new(sk);
        let signer = signing_key.choose_scheme(&[rustls::SignatureScheme::RSA_PSS_SHA256]);
        assert!(
            signer.is_none(),
            "must not produce a signer when ED25519 is not offered"
        );
    }

    #[cfg(feature = "quinn")]
    #[test]
    fn ed25519_signing_key_algorithm_is_ed25519() {
        use rustls::sign::SigningKey;
        let sk = crate::config::Ed25519SecretKey::generate();
        let signing_key = Ed25519SigningKey::new(sk);
        assert_eq!(signing_key.algorithm(), rustls::SignatureAlgorithm::ED25519);
    }

    #[cfg(feature = "quinn")]
    #[test]
    fn ed25519_signing_key_public_key_returns_spki() {
        use rustls::sign::SigningKey;
        let sk = crate::config::Ed25519SecretKey::generate();
        let signing_key = Ed25519SigningKey::new(sk);
        let spki = signing_key.public_key();
        assert!(spki.is_some(), "public_key must return an SPKI");
        assert!(!spki.unwrap().as_ref().is_empty(), "SPKI must be non-empty");
    }

    #[cfg(feature = "quinn")]
    #[test]
    fn ed25519_signing_key_signer_signs_message() {
        use rustls::sign::SigningKey;
        let sk = crate::config::Ed25519SecretKey::generate();
        let signing_key = Ed25519SigningKey::new(sk);
        let signer = signing_key
            .choose_scheme(&[rustls::SignatureScheme::ED25519])
            .expect("ED25519 offered");
        let message = b"alknet coverage signing test";
        let sig = signer.sign(message).expect("sign must succeed");
        assert_eq!(sig.len(), 64, "ed25519 signature must be 64 bytes");
        assert_eq!(signer.scheme(), rustls::SignatureScheme::ED25519);
    }

    #[cfg(feature = "quinn")]
    #[test]
    fn ed25519_signing_key_debug_does_not_leak_material() {
        let sk = crate::config::Ed25519SecretKey::generate();
        let signing_key = Ed25519SigningKey::new(sk);
        let dbg = format!("{signing_key:?}");
        assert!(dbg.contains("Ed25519SigningKey"));
    }

    #[cfg(feature = "quinn")]
    #[test]
    fn raw_key_cert_resolver_debug_is_implemented() {
        let sk = crate::config::Ed25519SecretKey::generate();
        let resolver = RawKeyCertResolver::new(&sk);
        let s = format!("{resolver:?}");
        assert!(s.contains("RawKeyCertResolver"));
    }

    #[cfg(feature = "quinn")]
    #[tokio::test]
    async fn debug_for_alknet_endpoint_is_implemented_without_panicking() {
        let sk = crate::config::Ed25519SecretKey::generate();
        let static_config = StaticConfig {
            listen_addr: None,
            tls_identity: Some(TlsIdentity::RawKey(sk)),
            iroh_relay: None,
            drain_timeout: Duration::from_millis(10),
        };
        struct NoProvider;
        impl IdentityProvider for NoProvider {
            fn resolve_from_fingerprint(&self, _: &str) -> Option<Identity> {
                None
            }
            fn resolve_from_token(&self, _: &AuthToken) -> Option<Identity> {
                None
            }
        }
        let provider: Arc<dyn IdentityProvider> = Arc::new(NoProvider);
        let dynamic = Arc::new(ArcSwap::from_pointee(DynamicConfig::default()));
        let registry = HandlerRegistry::new();
        let endpoint = AlknetEndpoint::new(&static_config, registry, dynamic, provider)
            .await
            .expect("endpoint constructs");
        let s = format!("{endpoint:?}");
        assert!(s.contains("AlknetEndpoint"));
        assert!(s.contains("drain_timeout"));
    }
}
