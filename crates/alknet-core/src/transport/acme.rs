use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use rustls::crypto::aws_lc_rs::default_provider;
use rustls::ServerConfig;
use rustls_acme::caches::DirCache;
use rustls_acme::{AcmeConfig, AcmeState, ResolvesServerCertAcme};
use tracing::{error, info};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor as TokioTlsAcceptor;

use super::{TransportAcceptor, TransportInfo, TransportKind};

const ACME_TLS_ALPN_NAME: &[u8] = b"acme-tls/1";

#[derive(Debug, Clone)]
pub enum AcmeMode {
    Domain { domain: String },
    Ip,
}

pub struct AcmeCertProvider {
    mode: AcmeMode,
    cache_dir: Option<PathBuf>,
    directory_url: String,
    contact: Vec<String>,
}

impl std::fmt::Debug for AcmeCertProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcmeCertProvider")
            .field("mode", &self.mode)
            .field("cache_dir", &self.cache_dir)
            .field("directory_url", &self.directory_url)
            .field("contact", &self.contact)
            .finish_non_exhaustive()
    }
}

impl AcmeCertProvider {
    pub fn new(mode: AcmeMode) -> Self {
        Self {
            mode,
            cache_dir: None,
            directory_url: rustls_acme::acme::LETS_ENCRYPT_STAGING_DIRECTORY.to_string(),
            contact: Vec::new(),
        }
    }

    pub fn domain(domain: impl Into<String>) -> Self {
        Self::new(AcmeMode::Domain {
            domain: domain.into(),
        })
    }

    pub fn ip() -> Self {
        Self::new(AcmeMode::Ip)
    }

    pub fn with_cache_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cache_dir = Some(dir.into());
        self
    }

    pub fn with_directory(mut self, url: impl Into<String>) -> Self {
        self.directory_url = url.into();
        self
    }

    pub fn with_production_directory(mut self) -> Self {
        self.directory_url = rustls_acme::acme::LETS_ENCRYPT_PRODUCTION_DIRECTORY.to_string();
        self
    }

    pub fn with_contact(mut self, contact: impl Into<String>) -> Self {
        self.contact.push(contact.into());
        self
    }

    pub fn mode(&self) -> &AcmeMode {
        &self.mode
    }

    fn build_acme_state(&self) -> (AcmeState<std::io::Error>, Arc<ResolvesServerCertAcme>) {
        let domains: Vec<String> = match &self.mode {
            AcmeMode::Domain { domain } => vec![domain.clone()],
            AcmeMode::Ip => vec![],
        };

        let base_config = AcmeConfig::new(domains)
            .directory(&self.directory_url)
            .contact(self.contact.clone());

        let state = match &self.cache_dir {
            Some(cache_dir) => {
                base_config.cache(DirCache::new(cache_dir.clone())).state()
            }
            None => {
                base_config
                    .cache(rustls_acme::caches::NoCache::default())
                    .state()
            }
        };

        let resolver = state.resolver();
        (state, resolver)
    }

    pub fn build_server_config_with_resolver(
        &self,
        resolver: Arc<ResolvesServerCertAcme>,
    ) -> Result<Arc<ServerConfig>> {
        let provider = default_provider().into();
        let mut config = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| anyhow!("failed to set protocol versions: {}", e))?
            .with_no_client_auth()
            .with_cert_resolver(resolver);
        config.alpn_protocols.push(ACME_TLS_ALPN_NAME.to_vec());
        Ok(Arc::new(config))
    }
}

pub struct AcmeTlsAcceptor {
    listener: TcpListener,
    listen_addr: SocketAddr,
    #[allow(dead_code)]
    server_config: Arc<ServerConfig>,
    tokio_acceptor: TokioTlsAcceptor,
}

impl AcmeTlsAcceptor {
    pub async fn bind_acme(
        addr: SocketAddr,
        provider: Arc<AcmeCertProvider>,
    ) -> Result<Self> {
        let (state, resolver) = provider.build_acme_state();

        let server_config = provider.build_server_config_with_resolver(resolver.clone())?;

        Self::spawn_state_worker(state, resolver);

        let listener = TcpListener::bind(addr).await?;
        let listen_addr = listener.local_addr()?;

        let tokio_acceptor = TokioTlsAcceptor::from(server_config.clone());

        Ok(Self {
            listener,
            listen_addr,
            server_config,
            tokio_acceptor,
        })
    }

    pub fn listen_addr(&self) -> SocketAddr {
        self.listen_addr
    }

    fn spawn_state_worker(state: AcmeState<std::io::Error>, resolver: Arc<ResolvesServerCertAcme>) {
        use futures::StreamExt;

        let task = async move {
            let mut state = state;
            while let Some(event) = state.next().await {
                match event {
                    Ok(ok) => {
                        if let rustls_acme::EventOk::DeployedNewCert = ok {
                            info!("ACME: new certificate deployed");
                        } else {
                            info!("ACME event: {:?}", ok);
                        }
                    }
                    Err(err) => error!("ACME event error: {:?}", err),
                }
                if Arc::strong_count(&resolver) == 1 {
                    info!("ACME resolver dropped, stopping background task");
                    break;
                }
            }
        };
        tokio::spawn(task);
    }
}

#[async_trait::async_trait]
impl TransportAcceptor for AcmeTlsAcceptor {
    type Stream = tokio_rustls::server::TlsStream<tokio::net::TcpStream>;

    async fn accept(&self) -> Result<(Self::Stream, TransportInfo)> {
        let (tcp_stream, remote_addr) = self.listener.accept().await?;
        let tls_stream = self.tokio_acceptor.accept(tcp_stream).await?;

        let server_name = tls_stream
            .get_ref()
            .1
            .server_name()
            .map(|s| s.to_string());

        let info = TransportInfo {
            remote_addr: Some(remote_addr),
            transport_kind: TransportKind::Tls { server_name },
        };

        Ok((tls_stream, info))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acme_cert_provider_domain_mode() {
        let provider = AcmeCertProvider::domain("example.com");
        assert!(matches!(provider.mode(), AcmeMode::Domain { .. }));
        if let AcmeMode::Domain { domain } = provider.mode() {
            assert_eq!(domain, "example.com");
        }
    }

    #[test]
    fn acme_cert_provider_ip_mode() {
        let provider = AcmeCertProvider::ip();
        assert!(matches!(provider.mode(), AcmeMode::Ip));
    }

    #[test]
    fn acme_cert_provider_default_staging_directory() {
        let provider = AcmeCertProvider::domain("example.com");
        assert_eq!(
            provider.directory_url,
            rustls_acme::acme::LETS_ENCRYPT_STAGING_DIRECTORY
        );
    }

    #[test]
    fn acme_cert_provider_production_directory() {
        let provider = AcmeCertProvider::domain("example.com").with_production_directory();
        assert_eq!(
            provider.directory_url,
            rustls_acme::acme::LETS_ENCRYPT_PRODUCTION_DIRECTORY
        );
    }

    #[test]
    fn acme_cert_provider_custom_directory() {
        let provider =
            AcmeCertProvider::domain("example.com").with_directory("https://custom.acme.dir/");
        assert_eq!(provider.directory_url, "https://custom.acme.dir/");
    }

    #[test]
    fn acme_cert_provider_with_cache_dir() {
        let provider = AcmeCertProvider::domain("example.com").with_cache_dir("/tmp/acme_cache");
        assert_eq!(provider.cache_dir, Some(PathBuf::from("/tmp/acme_cache")));
    }

    #[test]
    fn acme_cert_provider_with_contact() {
        let provider =
            AcmeCertProvider::domain("example.com").with_contact("mailto:admin@example.com");
        assert_eq!(
            provider.contact,
            vec!["mailto:admin@example.com".to_string()]
        );
    }

    #[test]
    fn acme_cert_provider_build_state_domain() {
        let provider = AcmeCertProvider::domain("example.com");
        let (_state, resolver) = provider.build_acme_state();
        assert!(Arc::strong_count(&resolver) >= 2);
    }

    #[test]
    fn acme_cert_provider_build_state_with_cache() {
        let provider =
            AcmeCertProvider::domain("example.com").with_cache_dir("/tmp/test_cache");
        let (_state, resolver) = provider.build_acme_state();
        assert!(Arc::strong_count(&resolver) >= 2);
    }

    #[test]
    fn acme_cert_provider_build_server_config() {
        let _ = default_provider().install_default();
        let provider = AcmeCertProvider::domain("example.com");
        let (_, resolver) = provider.build_acme_state();
        let config = provider.build_server_config_with_resolver(resolver).unwrap();
        assert!(!config.alpn_protocols.is_empty());
        assert!(config
            .alpn_protocols
            .iter()
            .any(|p| p == ACME_TLS_ALPN_NAME));
    }

    #[test]
    fn acme_mode_domain_debug() {
        let mode = AcmeMode::Domain {
            domain: "test.example.com".to_string(),
        };
        let debug_str = format!("{:?}", mode);
        assert!(debug_str.contains("test.example.com"));
    }

    #[test]
    fn acme_mode_ip_debug() {
        let mode = AcmeMode::Ip;
        let debug_str = format!("{:?}", mode);
        assert!(debug_str.contains("Ip"));
    }

    #[test]
    fn acme_cert_provider_builder_chain() {
        let provider = AcmeCertProvider::domain("test.example.com")
            .with_production_directory()
            .with_cache_dir("/tmp/cache")
            .with_contact("mailto:admin@test.example.com");
        assert!(matches!(provider.mode(), AcmeMode::Domain { .. }));
        assert_eq!(
            provider.directory_url,
            rustls_acme::acme::LETS_ENCRYPT_PRODUCTION_DIRECTORY
        );
        assert_eq!(provider.cache_dir, Some(PathBuf::from("/tmp/cache")));
        assert_eq!(provider.contact.len(), 1);
    }

    #[tokio::test]
    async fn acme_tls_acceptor_bind_acme() {
        let _ = default_provider().install_default();
        let provider = Arc::new(AcmeCertProvider::domain("example.com"));
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let acceptor = AcmeTlsAcceptor::bind_acme(addr, provider).await.unwrap();
        assert_ne!(acceptor.listen_addr().port(), 0);
    }

    #[tokio::test]
    #[ignore]
    async fn acme_staging_domain_cert_provisioning() {
        let _ = default_provider().install_default();

        let cache_dir = tempfile::tempdir().unwrap();
        let provider = Arc::new(
            AcmeCertProvider::domain("acme-test.example.com")
                .with_cache_dir(cache_dir.path())
                .with_contact("mailto:admin@example.com"),
        );

        let addr: SocketAddr = "0.0.0.0:443".parse().unwrap();
        let result = AcmeTlsAcceptor::bind_acme(addr, provider).await;
        assert!(
            result.is_ok(),
            "ACME TlsAcceptor should bind: {:?}",
            result.err()
        );

        let acceptor = result.unwrap();
        assert_eq!(acceptor.listen_addr().port(), 443);
    }
}