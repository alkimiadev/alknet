//! Server configuration and accept loop.
//!
//! `Server` binds to a transport acceptor and runs an accept loop, handling
//! authentication, stealth mode protocol detection, and graceful shutdown.
//! `ServeOptions` provides a builder-pattern API for programmatic configuration.
//! Supports multiple listeners via `ListenerConfig` for multi-transport operation.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use russh::server::{self, Config};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};
use tracing::{error, info, warn};

use crate::auth::keys::KeySource;
use crate::config::{ConfigReloadHandle, DynamicConfig};
use crate::error::ConfigError;
use crate::interface::StreamInterfaceKind;
use crate::server::handler::{ProxyConfig, ServerHandler};
use crate::server::rate_limit::ConnectionRateLimiter;
use crate::server::stealth::{self, ProtocolDetection};
use crate::transport::TransportKind;

const DEFAULT_LISTEN_ADDR: &str = "0.0.0.0:22";
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServeTransportMode {
    Tcp,
    Tls,
    Iroh,
}

impl std::fmt::Display for ServeTransportMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServeTransportMode::Tcp => write!(f, "tcp"),
            ServeTransportMode::Tls => write!(f, "tls"),
            ServeTransportMode::Iroh => write!(f, "iroh"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamListenerConfig {
    pub transport_kind: TransportKind,
    pub interface: StreamInterfaceKind,
    pub listen_addr: String,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
    pub acme_domain: Option<String>,
    pub stealth: bool,
    pub iroh_relay: Option<String>,
}

impl StreamListenerConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.stealth && !matches!(self.transport_kind, TransportKind::Tls { .. }) {
            return Err(ConfigError::InvalidFlag {
                name: "stealth mode requires TLS transport".to_string(),
            });
        }

        match self.transport_kind {
            TransportKind::Tls { .. } => {
                if self.tls_cert.is_none() && self.acme_domain.is_none() {
                    return Err(ConfigError::InvalidFlag {
                        name: "TLS transport requires tls_cert/tls_key or acme_domain".to_string(),
                    });
                }
                if self.tls_cert.is_some() && self.tls_key.is_none() {
                    return Err(ConfigError::InvalidFlag {
                        name: "tls_cert requires tls_key".to_string(),
                    });
                }
                if self.tls_key.is_some() && self.tls_cert.is_none() {
                    return Err(ConfigError::InvalidFlag {
                        name: "tls_key requires tls_cert".to_string(),
                    });
                }
            }
            TransportKind::Tcp
            | TransportKind::Iroh { .. }
            | TransportKind::WebTransport { .. } => {
                if self.tls_cert.is_some() || self.tls_key.is_some() || self.acme_domain.is_some() {
                    return Err(ConfigError::IncompatibleOptions);
                }
            }
        }

        Ok(())
    }
}

impl std::fmt::Display for StreamListenerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.transport_kind {
            TransportKind::Iroh { .. } => {
                write!(f, "{} (iroh/{})", self.listen_addr, self.interface)
            }
            TransportKind::WebTransport { .. } => {
                write!(f, "{} (webtransport/{})", self.listen_addr, self.interface)
            }
            _ => write!(
                f,
                "{} ({}/{})",
                self.listen_addr, self.transport_kind, self.interface
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpListenerConfig {
    pub bind_addr: SocketAddr,
    pub tls: bool,
    pub stealth: bool,
}

impl std::fmt::Display for HttpListenerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (http)", self.bind_addr)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsListenerConfig {
    pub bind_addr: SocketAddr,
    pub tls: bool,
}

impl std::fmt::Display for DnsListenerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (dns)", self.bind_addr)
    }
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ListenerConfig {
    Stream { config: StreamListenerConfig },
    Http { config: HttpListenerConfig },
    Dns { config: DnsListenerConfig },
}

impl ListenerConfig {
    pub fn tcp(addr: impl Into<String>) -> Self {
        Self::Stream {
            config: StreamListenerConfig {
                transport_kind: TransportKind::Tcp,
                interface: StreamInterfaceKind::Ssh,
                listen_addr: addr.into(),
                tls_cert: None,
                tls_key: None,
                acme_domain: None,
                stealth: false,
                iroh_relay: None,
            },
        }
    }

    pub fn tls(addr: impl Into<String>) -> Self {
        Self::Stream {
            config: StreamListenerConfig {
                transport_kind: TransportKind::Tls { server_name: None },
                interface: StreamInterfaceKind::Ssh,
                listen_addr: addr.into(),
                tls_cert: None,
                tls_key: None,
                acme_domain: None,
                stealth: false,
                iroh_relay: None,
            },
        }
    }

    pub fn iroh(addr: impl Into<String>) -> Self {
        Self::Stream {
            config: StreamListenerConfig {
                transport_kind: TransportKind::Iroh {
                    endpoint_id: String::new(),
                },
                interface: StreamInterfaceKind::Ssh,
                listen_addr: addr.into(),
                tls_cert: None,
                tls_key: None,
                acme_domain: None,
                stealth: false,
                iroh_relay: None,
            },
        }
    }

    pub fn webtransport(addr: impl Into<String>) -> Self {
        Self::Stream {
            config: StreamListenerConfig {
                transport_kind: TransportKind::WebTransport { server_name: None },
                interface: StreamInterfaceKind::Ssh,
                listen_addr: addr.into(),
                tls_cert: None,
                tls_key: None,
                acme_domain: None,
                stealth: false,
                iroh_relay: None,
            },
        }
    }

    pub fn http(bind_addr: SocketAddr) -> Self {
        Self::Http {
            config: HttpListenerConfig {
                bind_addr,
                tls: false,
                stealth: false,
            },
        }
    }

    pub fn dns(bind_addr: SocketAddr) -> Self {
        Self::Dns {
            config: DnsListenerConfig {
                bind_addr,
                tls: false,
            },
        }
    }

    pub fn tls_cert(mut self, path: impl Into<String>) -> Self {
        if let ListenerConfig::Stream { ref mut config } = self {
            config.tls_cert = Some(path.into());
        }
        self
    }

    pub fn tls_key(mut self, path: impl Into<String>) -> Self {
        if let ListenerConfig::Stream { ref mut config } = self {
            config.tls_key = Some(path.into());
        }
        self
    }

    pub fn acme_domain(mut self, domain: impl Into<String>) -> Self {
        if let ListenerConfig::Stream { ref mut config } = self {
            config.acme_domain = Some(domain.into());
        }
        self
    }

    pub fn stealth(mut self, enabled: bool) -> Self {
        match &mut self {
            ListenerConfig::Stream { ref mut config } => config.stealth = enabled,
            ListenerConfig::Http { ref mut config } => config.stealth = enabled,
            ListenerConfig::Dns { .. } => {}
        }
        self
    }

    pub fn iroh_relay(mut self, url: impl Into<String>) -> Self {
        if let ListenerConfig::Stream { ref mut config } = self {
            config.iroh_relay = Some(url.into());
        }
        self
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        match self {
            ListenerConfig::Stream { config } => config.validate(),
            ListenerConfig::Http { .. } | ListenerConfig::Dns { .. } => Ok(()),
        }
    }
}

impl std::fmt::Display for ListenerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ListenerConfig::Stream { config } => write!(f, "{}", config),
            ListenerConfig::Http { config } => write!(f, "{}", config),
            ListenerConfig::Dns { config } => write!(f, "{}", config),
        }
    }
}

pub struct ServeOptions {
    pub key: KeySource,
    pub authorized_keys: Option<KeySource>,
    pub cert_authority: Option<KeySource>,
    pub transport_mode: ServeTransportMode,
    pub listen_addr: String,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
    pub acme_domain: Option<String>,
    pub stealth: bool,
    pub proxy: Option<String>,
    pub iroh_relay: Option<String>,
    pub max_connections_per_ip: usize,
    pub max_auth_attempts: usize,
    pub listeners: Option<Vec<ListenerConfig>>,
}

impl ServeOptions {
    pub fn new(key: KeySource) -> Self {
        Self {
            key,
            authorized_keys: None,
            cert_authority: None,
            transport_mode: ServeTransportMode::Tcp,
            listen_addr: DEFAULT_LISTEN_ADDR.to_string(),
            tls_cert: None,
            tls_key: None,
            acme_domain: None,
            stealth: false,
            proxy: None,
            iroh_relay: None,
            max_connections_per_ip: 0,
            max_auth_attempts: 10,
            listeners: None,
        }
    }

    pub fn authorized_keys(mut self, source: KeySource) -> Self {
        self.authorized_keys = Some(source);
        self
    }

    pub fn cert_authority(mut self, source: KeySource) -> Self {
        self.cert_authority = Some(source);
        self
    }

    pub fn transport_mode(mut self, mode: ServeTransportMode) -> Self {
        self.transport_mode = mode;
        self
    }

    pub fn listen_addr(mut self, addr: impl Into<String>) -> Self {
        self.listen_addr = addr.into();
        self
    }

    pub fn tls_cert(mut self, path: impl Into<String>) -> Self {
        self.tls_cert = Some(path.into());
        self
    }

    pub fn tls_key(mut self, path: impl Into<String>) -> Self {
        self.tls_key = Some(path.into());
        self
    }

    pub fn acme_domain(mut self, domain: impl Into<String>) -> Self {
        self.acme_domain = Some(domain.into());
        self
    }

    pub fn stealth(mut self, enabled: bool) -> Self {
        self.stealth = enabled;
        self
    }

    pub fn proxy(mut self, url: impl Into<String>) -> Self {
        self.proxy = Some(url.into());
        self
    }

    pub fn iroh_relay(mut self, url: impl Into<String>) -> Self {
        self.iroh_relay = Some(url.into());
        self
    }

    pub fn max_connections_per_ip(mut self, max: usize) -> Self {
        self.max_connections_per_ip = max;
        self
    }

    pub fn max_auth_attempts(mut self, max: usize) -> Self {
        self.max_auth_attempts = max;
        self
    }

    pub fn listeners(mut self, listeners: Vec<ListenerConfig>) -> Self {
        self.listeners = Some(listeners);
        self
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if let Some(ref listeners) = self.listeners {
            if listeners.is_empty() {
                return Err(ConfigError::InvalidFlag {
                    name: "listeners must not be empty".to_string(),
                });
            }
            for listener in listeners {
                listener.validate()?;
            }
            return Ok(());
        }

        if self.stealth && self.transport_mode != ServeTransportMode::Tls {
            return Err(ConfigError::InvalidFlag {
                name: "stealth mode requires TLS transport (--transport tls)".to_string(),
            });
        }

        match self.transport_mode {
            ServeTransportMode::Tls => {
                if self.tls_cert.is_none() && self.acme_domain.is_none() {
                    return Err(ConfigError::InvalidFlag {
                        name: "TLS transport requires --tls-cert/--tls-key or --acme-domain"
                            .to_string(),
                    });
                }
                if self.tls_cert.is_some() && self.tls_key.is_none() {
                    return Err(ConfigError::InvalidFlag {
                        name: "--tls-cert requires --tls-key".to_string(),
                    });
                }
                if self.tls_key.is_some() && self.tls_cert.is_none() {
                    return Err(ConfigError::InvalidFlag {
                        name: "--tls-key requires --tls-cert".to_string(),
                    });
                }
            }
            ServeTransportMode::Tcp | ServeTransportMode::Iroh => {
                if self.tls_cert.is_some() || self.tls_key.is_some() || self.acme_domain.is_some() {
                    return Err(ConfigError::IncompatibleOptions);
                }
            }
        }

        Ok(())
    }
}

impl std::fmt::Debug for ServeOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServeOptions")
            .field("key", &"<KeySource>")
            .field("authorized_keys", &"<KeySource>")
            .field("cert_authority", &"<KeySource>")
            .field("transport_mode", &self.transport_mode)
            .field("listen_addr", &self.listen_addr)
            .field("stealth", &self.stealth)
            .field("max_connections_per_ip", &self.max_connections_per_ip)
            .field("max_auth_attempts", &self.max_auth_attempts)
            .field("listeners", &self.listeners)
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    #[error("config error: {0}")]
    Config(#[from] ConfigError),
    #[error("bind failed: {0}")]
    BindFailed(anyhow::Error),
    #[error("key load failed: {0}")]
    KeyLoadFailed(ConfigError),
    #[error("accept failed")]
    AcceptFailed,
}

struct ActiveSession {
    handle: server::Handle,
    join: tokio::task::JoinHandle<()>,
}

pub struct Server {
    config: Arc<server::Config>,
    dynamic: Arc<ArcSwap<DynamicConfig>>,
    connection_limiter: Arc<ConnectionRateLimiter>,
    outbound_proxy: Option<ProxyConfig>,
    listeners: Vec<ListenerConfig>,
    max_auth_attempts: usize,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
    sessions: Arc<tokio::sync::Mutex<Vec<ActiveSession>>>,
}

impl Server {
    pub fn new(opts: ServeOptions) -> Result<Self, ServeError> {
        let (static_config, dynamic_config) =
            crate::config::StaticConfig::from_serve_options(opts).map_err(ServeError::Config)?;

        let connection_limiter = Arc::new(ConnectionRateLimiter::new(
            static_config.max_connections_per_ip,
        ));

        let config = Arc::new(Config {
            keys: vec![static_config.host_key],
            max_auth_attempts: static_config.max_auth_attempts,
            methods: russh::MethodSet::PUBLICKEY,
            preferred: russh::Preferred::DEFAULT,
            ..Default::default()
        });

        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

        let dynamic = Arc::new(ArcSwap::new(Arc::new(dynamic_config)));

        Ok(Self {
            config,
            dynamic,
            connection_limiter,
            outbound_proxy: static_config.proxy_config,
            listeners: static_config.listeners,
            max_auth_attempts: static_config.max_auth_attempts,
            shutdown_tx,
            shutdown_rx,
            sessions: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        })
    }

    pub fn shutdown_sender(&self) -> tokio::sync::watch::Sender<bool> {
        self.shutdown_tx.clone()
    }

    pub fn config_reload_handle(&self) -> ConfigReloadHandle {
        ConfigReloadHandle {
            dynamic: Arc::clone(&self.dynamic),
        }
    }

    pub async fn shutdown(&self) -> Result<(), ServeError> {
        info!("initiating graceful shutdown");
        let _ = self.shutdown_tx.send(true);

        {
            let sessions = self.sessions.lock().await;
            for session in sessions.iter() {
                if let Err(e) = session
                    .handle
                    .disconnect(
                        russh::Disconnect::ByApplication,
                        "shutdown".to_string(),
                        String::new(),
                    )
                    .await
                {
                    warn!("failed to send SSH disconnect: {e}");
                }
            }
        }

        tokio::time::sleep(DRAIN_TIMEOUT).await;

        {
            let mut sessions = self.sessions.lock().await;
            for session in sessions.drain(..) {
                session.join.abort();
            }
        }

        info!("graceful shutdown complete");
        Ok(())
    }

    pub fn is_shutdown(&self) -> bool {
        *self.shutdown_rx.borrow()
    }

    pub async fn run<A>(self, acceptor: A, endpoint_info: Option<&str>) -> Result<(), ServeError>
    where
        A: crate::transport::TransportAcceptor,
    {
        let listener = self
            .listeners
            .first()
            .expect("at least one listener required");

        let (transport_kind, stealth, listen_addr) = match listener {
            ListenerConfig::Stream { config } => (
                config.transport_kind.clone(),
                config.stealth,
                config.listen_addr.clone(),
            ),
            ListenerConfig::Http { config } => (
                TransportKind::Tcp,
                config.stealth,
                config.bind_addr.to_string(),
            ),
            ListenerConfig::Dns { config } => {
                (TransportKind::Tcp, false, config.bind_addr.to_string())
            }
        };

        if matches!(transport_kind, TransportKind::Iroh { .. }) {
            if let Some(id) = endpoint_info {
                info!("alknet server running: transport=iroh endpoint_id={}", id);
            } else {
                info!("alknet server running: transport=iroh");
            }
        } else {
            info!(
                "alknet server running: transport={} listen={}",
                transport_kind, listen_addr
            );
        }

        let server = Arc::new(self);

        let mut shutdown_rx = server.shutdown_rx.clone();

        #[cfg(unix)]
        let signal_done = {
            let sig_tx = server.shutdown_tx.clone();
            tokio::spawn(async move {
                let mut sigterm_stream =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .expect("failed to install SIGTERM handler");
                tokio::select! {
                    _ = sigterm_stream.recv() => {
                        info!("received SIGTERM");
                    }
                    _ = tokio::signal::ctrl_c() => {
                        info!("received SIGINT (Ctrl+C)");
                    }
                }
                let _ = sig_tx.send(true);
            })
        };

        loop {
            let shutdown = *shutdown_rx.borrow();
            if shutdown {
                info!("shutdown signaled, stopping accept loop");
                break;
            }

            let accept_result = tokio::select! {
                result = acceptor.accept() => result,
                _ = shutdown_rx.changed() => {
                    info!("shutdown signaled while waiting for connection");
                    break;
                }
            };

            let (stream, info) = match accept_result {
                Ok(conn) => conn,
                Err(e) => {
                    error!("accept failed: {e}");
                    continue;
                }
            };

            let remote_addr = info.remote_addr;
            let handler_transport_kind = transport_kind.clone();

            let handler = ServerHandler::new(
                Arc::clone(&server.dynamic),
                server.outbound_proxy.clone(),
                remote_addr,
                handler_transport_kind,
                Arc::clone(&server.connection_limiter),
                server.max_auth_attempts,
            );

            if !handler.is_connection_allowed() {
                continue;
            }

            let config = Arc::clone(&server.config);
            let sessions = Arc::clone(&server.sessions);
            let transport_is_tls = matches!(transport_kind, TransportKind::Tls { .. });

            tokio::spawn(async move {
                let result =
                    handle_connection(stream, config, handler, sessions, stealth, transport_is_tls)
                        .await;

                if let Err(e) = result {
                    warn!("connection error: {e}");
                }
            });
        }

        #[cfg(unix)]
        signal_done.abort();

        server.shutdown().await?;

        Ok(())
    }

    pub fn listeners(&self) -> &[ListenerConfig] {
        &self.listeners
    }
}

async fn handle_connection<S>(
    stream: S,
    config: Arc<Config>,
    handler: ServerHandler,
    sessions: Arc<tokio::sync::Mutex<Vec<ActiveSession>>>,
    stealth: bool,
    transport_is_tls: bool,
) -> Result<(), anyhow::Error>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    if stealth && transport_is_tls {
        let (protocol, mut reader) = stealth::detect_protocol(stream).await;
        match protocol {
            ProtocolDetection::Http => {
                stealth::send_fake_nginx_404(&mut reader).await;
                return Ok(());
            }
            ProtocolDetection::Ssh => {
                let running = server::run_stream(config, reader, handler).await?;
                let handle = running.handle();
                let join = tokio::spawn(async {
                    let _ = running.await;
                });
                sessions.lock().await.push(ActiveSession { handle, join });
                return Ok(());
            }
        }
    }

    let running = server::run_stream(config, stream, handler).await?;
    let handle = running.handle();
    let join = tokio::spawn(async {
        let _ = running.await;
    });
    sessions.lock().await.push(ActiveSession { handle, join });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ED25519_PRIVATE_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAACBOfInDyRS33JEeDNT8xd10qRdwFN8z/QukCOgEIkv01QAAAJiQ+NvMkPjb\nzAAAAAtzc2gtZWQyNTUxOQAAACBOfInDyRS33JEeDNT8xd10qRdwFN8z/QukCOgEIkv01Q\nAAAECIWwJf7+7MOuZAOOWmoQbE9i/5GxjKsFrtJHjZ34E/fk58icPJFLfckR4M1PzF3XSp\nF3AU3zP9C6QI6AQiS/TVAAAAD3VidW50dUBuczUyODA5NgECAwQFBg==\n-----END OPENSSH PRIVATE KEY-----\n";

    const ED25519_PUBLIC_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIE58icPJFLfckR4M1PzF3XSpF3AU3zP9C6QI6AQiS/TV ubuntu@ns528096";

    fn make_key_source() -> KeySource {
        KeySource::Memory(ED25519_PRIVATE_KEY.as_bytes().to_vec())
    }

    fn make_authorized_keys_source() -> KeySource {
        KeySource::Memory(ED25519_PUBLIC_KEY.as_bytes().to_vec())
    }

    #[test]
    fn serve_options_default_fields() {
        let opts = ServeOptions::new(make_key_source());
        assert!(opts.authorized_keys.is_none());
        assert!(opts.cert_authority.is_none());
        assert_eq!(opts.transport_mode, ServeTransportMode::Tcp);
        assert_eq!(opts.listen_addr, "0.0.0.0:22");
        assert!(opts.tls_cert.is_none());
        assert!(opts.tls_key.is_none());
        assert!(opts.acme_domain.is_none());
        assert!(!opts.stealth);
        assert!(opts.proxy.is_none());
        assert!(opts.iroh_relay.is_none());
        assert_eq!(opts.max_connections_per_ip, 0);
        assert_eq!(opts.max_auth_attempts, 10);
        assert!(opts.listeners.is_none());
    }

    #[test]
    fn serve_options_builder_pattern() {
        let opts = ServeOptions::new(make_key_source())
            .authorized_keys(make_authorized_keys_source())
            .cert_authority(make_authorized_keys_source())
            .transport_mode(ServeTransportMode::Tls)
            .listen_addr("0.0.0.0:443")
            .tls_cert("/etc/ssl/cert.pem")
            .tls_key("/etc/ssl/key.pem")
            .stealth(true)
            .proxy("socks5://127.0.0.1:9050")
            .iroh_relay("https://relay.example.com")
            .max_connections_per_ip(5)
            .max_auth_attempts(3);

        assert!(opts.authorized_keys.is_some());
        assert!(opts.cert_authority.is_some());
        assert_eq!(opts.transport_mode, ServeTransportMode::Tls);
        assert_eq!(opts.listen_addr, "0.0.0.0:443");
        assert_eq!(opts.tls_cert.as_deref(), Some("/etc/ssl/cert.pem"));
        assert_eq!(opts.tls_key.as_deref(), Some("/etc/ssl/key.pem"));
        assert!(opts.stealth);
        assert_eq!(opts.proxy.as_deref(), Some("socks5://127.0.0.1:9050"));
        assert_eq!(
            opts.iroh_relay.as_deref(),
            Some("https://relay.example.com")
        );
        assert_eq!(opts.max_connections_per_ip, 5);
        assert_eq!(opts.max_auth_attempts, 3);
    }

    #[test]
    fn serve_options_validate_steam_without_tls_rejected() {
        let opts = ServeOptions::new(make_key_source()).stealth(true);
        assert!(opts.validate().is_err());
    }

    #[test]
    fn serve_options_validate_stealth_with_tls_ok() {
        let opts = ServeOptions::new(make_key_source())
            .transport_mode(ServeTransportMode::Tls)
            .tls_cert("/cert.pem")
            .tls_key("/key.pem")
            .stealth(true);
        assert!(opts.validate().is_ok());
    }

    #[test]
    fn serve_options_validate_tcp_no_tls_options_ok() {
        let opts = ServeOptions::new(make_key_source());
        assert!(opts.validate().is_ok());
    }

    #[test]
    fn serve_options_validate_tls_requires_certs() {
        let opts = ServeOptions::new(make_key_source()).transport_mode(ServeTransportMode::Tls);
        assert!(opts.validate().is_err());
    }

    #[test]
    fn serve_options_validate_tls_cert_without_key_rejected() {
        let opts = ServeOptions::new(make_key_source())
            .transport_mode(ServeTransportMode::Tls)
            .tls_cert("/cert.pem");
        assert!(opts.validate().is_err());
    }

    #[test]
    fn serve_options_validate_tls_key_without_cert_rejected() {
        let opts = ServeOptions::new(make_key_source())
            .transport_mode(ServeTransportMode::Tls)
            .tls_key("/key.pem");
        assert!(opts.validate().is_err());
    }

    #[test]
    fn serve_options_validate_tcp_with_acme_rejected() {
        let opts = ServeOptions::new(make_key_source()).acme_domain("example.com");
        assert!(opts.validate().is_err());
    }

    #[test]
    fn serve_options_validate_acme_domain_with_tls_ok() {
        let opts = ServeOptions::new(make_key_source())
            .transport_mode(ServeTransportMode::Tls)
            .acme_domain("example.com");
        assert!(opts.validate().is_ok());
    }

    #[test]
    fn server_new_creates_server() {
        let opts =
            ServeOptions::new(make_key_source()).authorized_keys(make_authorized_keys_source());
        let server = Server::new(opts).unwrap();
        assert_eq!(server.max_auth_attempts, 10);
    }

    #[test]
    fn server_new_stealth_without_tls_fails() {
        let opts = ServeOptions::new(make_key_source()).stealth(true);
        let result = Server::new(opts);
        assert!(result.is_err());
    }

    #[test]
    fn server_new_invalid_key_fails() {
        let opts = ServeOptions::new(KeySource::Memory(b"not a key".to_vec()));
        let result = Server::new(opts);
        assert!(result.is_err());
    }

    #[test]
    fn serve_transport_mode_display() {
        assert_eq!(ServeTransportMode::Tcp.to_string(), "tcp");
        assert_eq!(ServeTransportMode::Tls.to_string(), "tls");
        assert_eq!(ServeTransportMode::Iroh.to_string(), "iroh");
    }

    #[test]
    fn serve_transport_mode_equality() {
        assert_eq!(ServeTransportMode::Tcp, ServeTransportMode::Tcp);
        assert_ne!(ServeTransportMode::Tcp, ServeTransportMode::Tls);
        assert_ne!(ServeTransportMode::Tls, ServeTransportMode::Iroh);
    }

    #[test]
    fn serve_options_debug_redacts_keys() {
        let opts =
            ServeOptions::new(make_key_source()).authorized_keys(make_authorized_keys_source());
        let debug_str = format!("{:?}", opts);
        assert!(debug_str.contains("<KeySource>"));
        assert!(!debug_str.contains("OPENSSH"));
    }

    #[test]
    fn serve_error_variants() {
        assert_eq!(ServeError::AcceptFailed.to_string(), "accept failed");
    }

    #[test]
    fn default_listen_addr() {
        assert_eq!(DEFAULT_LISTEN_ADDR, "0.0.0.0:22");
    }

    #[test]
    fn drain_timeout_is_two_seconds() {
        assert_eq!(DRAIN_TIMEOUT, Duration::from_secs(2));
    }

    #[test]
    fn server_shutdown_sender_clones() {
        let opts =
            ServeOptions::new(make_key_source()).authorized_keys(make_authorized_keys_source());
        let server = Server::new(opts).unwrap();
        let sender = server.shutdown_sender();
        assert!(!server.is_shutdown());
        let _ = sender.send(true);
        assert!(server.is_shutdown());
    }

    #[test]
    fn listener_config_tcp_constructor() {
        let lc = ListenerConfig::tcp("0.0.0.0:22");
        match &lc {
            ListenerConfig::Stream { config } => {
                assert_eq!(config.transport_kind, TransportKind::Tcp);
                assert_eq!(config.listen_addr, "0.0.0.0:22");
                assert!(!config.stealth);
                assert!(config.tls_cert.is_none());
            }
            _ => panic!("expected Stream variant"),
        }
    }

    #[test]
    fn listener_config_tls_constructor() {
        let lc = ListenerConfig::tls("0.0.0.0:443")
            .tls_cert("/cert.pem")
            .tls_key("/key.pem")
            .stealth(true);
        match &lc {
            ListenerConfig::Stream { config } => {
                assert_eq!(
                    config.transport_kind,
                    TransportKind::Tls { server_name: None }
                );
                assert_eq!(config.listen_addr, "0.0.0.0:443");
                assert!(config.stealth);
                assert_eq!(config.tls_cert.as_deref(), Some("/cert.pem"));
                assert_eq!(config.tls_key.as_deref(), Some("/key.pem"));
            }
            _ => panic!("expected Stream variant"),
        }
    }

    #[test]
    fn listener_config_iroh_constructor() {
        let lc = ListenerConfig::iroh("0.0.0.0:0").iroh_relay("https://relay.example.com");
        match &lc {
            ListenerConfig::Stream { config } => {
                assert_eq!(
                    config.transport_kind,
                    TransportKind::Iroh {
                        endpoint_id: String::new()
                    }
                );
                assert_eq!(
                    config.iroh_relay.as_deref(),
                    Some("https://relay.example.com")
                );
            }
            _ => panic!("expected Stream variant"),
        }
    }

    #[test]
    fn listener_config_http_constructor() {
        let lc = ListenerConfig::http("127.0.0.1:8080".parse().unwrap());
        match &lc {
            ListenerConfig::Http { config } => {
                assert_eq!(
                    config.bind_addr,
                    "127.0.0.1:8080".parse::<SocketAddr>().unwrap()
                );
                assert!(!config.tls);
                assert!(!config.stealth);
            }
            _ => panic!("expected Http variant"),
        }
    }

    #[test]
    fn listener_config_dns_constructor() {
        let lc = ListenerConfig::dns("127.0.0.1:53".parse().unwrap());
        match &lc {
            ListenerConfig::Dns { config } => {
                assert_eq!(
                    config.bind_addr,
                    "127.0.0.1:53".parse::<SocketAddr>().unwrap()
                );
                assert!(!config.tls);
            }
            _ => panic!("expected Dns variant"),
        }
    }

    #[test]
    fn listener_config_webtransport_constructor() {
        let lc = ListenerConfig::webtransport("example.com");
        match &lc {
            ListenerConfig::Stream { config } => {
                assert_eq!(
                    config.transport_kind,
                    TransportKind::WebTransport { server_name: None }
                );
                assert_eq!(config.listen_addr, "example.com");
            }
            _ => panic!("expected Stream variant"),
        }
    }

    #[test]
    fn listener_config_validate_tls_requires_certs() {
        let lc = ListenerConfig::tls("0.0.0.0:443");
        assert!(lc.validate().is_err());
    }

    #[test]
    fn listener_config_validate_tls_with_certs_ok() {
        let lc = ListenerConfig::tls("0.0.0.0:443")
            .tls_cert("/cert.pem")
            .tls_key("/key.pem");
        assert!(lc.validate().is_ok());
    }

    #[test]
    fn listener_config_validate_tls_with_acme_ok() {
        let lc = ListenerConfig::tls("0.0.0.0:443").acme_domain("example.com");
        assert!(lc.validate().is_ok());
    }

    #[test]
    fn listener_config_validate_stealth_without_tls_rejected() {
        let lc = ListenerConfig::tcp("0.0.0.0:22").stealth(true);
        assert!(lc.validate().is_err());
    }

    #[test]
    fn listener_config_validate_tcp_cannot_have_tls_certs() {
        let lc = ListenerConfig::tcp("0.0.0.0:22").tls_cert("/cert.pem");
        assert!(lc.validate().is_err());
    }

    #[test]
    fn listener_config_display() {
        let tcp = ListenerConfig::tcp("0.0.0.0:22");
        assert_eq!(format!("{}", tcp), "0.0.0.0:22 (tcp/ssh)");

        let tls = ListenerConfig::tls("0.0.0.0:443");
        assert_eq!(format!("{}", tls), "0.0.0.0:443 (tls/ssh)");

        let iroh = ListenerConfig::iroh("0.0.0.0:0");
        assert_eq!(format!("{}", iroh), "0.0.0.0:0 (iroh/ssh)");

        let http = ListenerConfig::http("0.0.0.0:8080".parse().unwrap());
        assert_eq!(format!("{}", http), "0.0.0.0:8080 (http)");

        let dns = ListenerConfig::dns("0.0.0.0:53".parse().unwrap());
        assert_eq!(format!("{}", dns), "0.0.0.0:53 (dns)");
    }

    #[test]
    fn listener_config_equality() {
        let lc1 = ListenerConfig::tcp("0.0.0.0:22");
        let lc2 = ListenerConfig::tcp("0.0.0.0:22");
        assert_eq!(lc1, lc2);

        let lc3 = ListenerConfig::tls("0.0.0.0:443");
        assert_ne!(lc1, lc3);
    }

    #[test]
    fn serve_options_with_listeners() {
        let listeners = vec![
            ListenerConfig::tcp("0.0.0.0:22"),
            ListenerConfig::tls("0.0.0.0:443")
                .tls_cert("/cert.pem")
                .tls_key("/key.pem"),
        ];
        let opts = ServeOptions::new(make_key_source())
            .authorized_keys(make_authorized_keys_source())
            .listeners(listeners);

        assert!(opts.listeners.is_some());
        assert_eq!(opts.listeners.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn serve_options_validate_listeners_ok() {
        let listeners = vec![
            ListenerConfig::tcp("0.0.0.0:22"),
            ListenerConfig::tls("0.0.0.0:443")
                .tls_cert("/cert.pem")
                .tls_key("/key.pem"),
        ];
        let opts = ServeOptions::new(make_key_source())
            .authorized_keys(make_authorized_keys_source())
            .listeners(listeners);
        assert!(opts.validate().is_ok());
    }

    #[test]
    fn serve_options_validate_listeners_bypasses_single_validation() {
        let listeners = vec![ListenerConfig::tcp("0.0.0.0:22")];
        let opts = ServeOptions::new(make_key_source())
            .stealth(true)
            .listeners(listeners);
        assert!(opts.validate().is_ok());
    }

    #[test]
    fn serve_options_validate_listeners_per_listener_stealth_requires_tls() {
        let listeners = vec![ListenerConfig::tcp("0.0.0.0:22").stealth(true)];
        let opts = ServeOptions::new(make_key_source())
            .authorized_keys(make_authorized_keys_source())
            .listeners(listeners);
        assert!(opts.validate().is_err());
    }

    #[test]
    fn serve_options_validate_empty_listeners_rejected() {
        let opts = ServeOptions::new(make_key_source())
            .authorized_keys(make_authorized_keys_source())
            .listeners(vec![]);
        assert!(opts.validate().is_err());
    }

    #[test]
    fn server_new_with_listeners() {
        let listeners = vec![ListenerConfig::tcp("0.0.0.0:22")];
        let opts = ServeOptions::new(make_key_source())
            .authorized_keys(make_authorized_keys_source())
            .listeners(listeners);
        let server = Server::new(opts).unwrap();
        assert_eq!(server.listeners.len(), 1);
    }

    #[test]
    fn server_new_single_transport_creates_listener() {
        let opts =
            ServeOptions::new(make_key_source()).authorized_keys(make_authorized_keys_source());
        let server = Server::new(opts).unwrap();
        assert_eq!(server.listeners.len(), 1);
        match &server.listeners[0] {
            ListenerConfig::Stream { config } => {
                assert_eq!(config.transport_kind, TransportKind::Tcp);
                assert_eq!(config.listen_addr, "0.0.0.0:22");
            }
            _ => panic!("expected Stream variant"),
        }
    }

    #[test]
    fn server_new_tls_transport_creates_tls_listener() {
        let opts = ServeOptions::new(make_key_source())
            .authorized_keys(make_authorized_keys_source())
            .transport_mode(ServeTransportMode::Tls)
            .tls_cert("/cert.pem")
            .tls_key("/key.pem")
            .listen_addr("0.0.0.0:443")
            .stealth(true);
        let server = Server::new(opts).unwrap();
        assert_eq!(server.listeners.len(), 1);
        match &server.listeners[0] {
            ListenerConfig::Stream { config } => {
                assert_eq!(
                    config.transport_kind,
                    TransportKind::Tls { server_name: None }
                );
                assert!(config.stealth);
                assert_eq!(config.tls_cert.as_deref(), Some("/cert.pem"));
            }
            _ => panic!("expected Stream variant"),
        }
    }

    #[test]
    fn server_listeners_accessor() {
        let listeners = vec![
            ListenerConfig::tcp("0.0.0.0:22"),
            ListenerConfig::tls("0.0.0.0:443")
                .tls_cert("/cert.pem")
                .tls_key("/key.pem"),
        ];
        let opts = ServeOptions::new(make_key_source())
            .authorized_keys(make_authorized_keys_source())
            .listeners(listeners);
        let server = Server::new(opts).unwrap();
        assert_eq!(server.listeners().len(), 2);
    }

    #[test]
    fn server_new_multi_listener_tcp_and_tls() {
        let listeners = vec![
            ListenerConfig::tcp("0.0.0.0:22"),
            ListenerConfig::tls("0.0.0.0:443")
                .tls_cert("/cert.pem")
                .tls_key("/key.pem"),
        ];
        let opts = ServeOptions::new(make_key_source())
            .authorized_keys(make_authorized_keys_source())
            .listeners(listeners);
        let server = Server::new(opts).unwrap();
        assert_eq!(server.listeners.len(), 2);

        let dynamic = server.config_reload_handle();
        let config = dynamic.dynamic();
        assert!(config.auth.authorized_keys.len() > 0);
    }

    #[tokio::test]
    async fn integration_server_accept_loop_and_shutdown() {
        use crate::transport::TcpAcceptor;

        let acceptor = TcpAcceptor::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();

        let opts = ServeOptions::new(make_key_source())
            .authorized_keys(make_authorized_keys_source())
            .listen_addr(acceptor.listen_addr().to_string());

        let server = Server::new(opts).unwrap();
        let shutdown_tx = server.shutdown_sender();

        let server_handle =
            tokio::spawn(
                async move { server.run(acceptor, None).await.expect("server run failed") },
            );

        tokio::time::sleep(Duration::from_millis(50)).await;

        let _ = shutdown_tx.send(true);

        let result = tokio::time::timeout(Duration::from_secs(5), server_handle).await;

        assert!(
            result.is_ok(),
            "server should have shut down within timeout"
        );
    }

    #[test]
    fn http_listener_config_display() {
        let config = HttpListenerConfig {
            bind_addr: "127.0.0.1:8080".parse().unwrap(),
            tls: true,
            stealth: false,
        };
        assert_eq!(config.to_string(), "127.0.0.1:8080 (http)");
    }

    #[test]
    fn dns_listener_config_display() {
        let config = DnsListenerConfig {
            bind_addr: "0.0.0.0:53".parse().unwrap(),
            tls: true,
        };
        assert_eq!(config.to_string(), "0.0.0.0:53 (dns)");
    }

    #[test]
    fn http_listener_config_serialization() {
        let config = HttpListenerConfig {
            bind_addr: "127.0.0.1:8080".parse().unwrap(),
            tls: true,
            stealth: false,
        };
        let serialized = serde_json::to_string(&config).unwrap();
        let deserialized: HttpListenerConfig = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.bind_addr, config.bind_addr);
        assert_eq!(deserialized.tls, config.tls);
    }

    #[test]
    fn dns_listener_config_serialization() {
        let config = DnsListenerConfig {
            bind_addr: "0.0.0.0:53".parse().unwrap(),
            tls: true,
        };
        let serialized = serde_json::to_string(&config).unwrap();
        let deserialized: DnsListenerConfig = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.bind_addr, config.bind_addr);
        assert_eq!(deserialized.tls, config.tls);
    }
}
