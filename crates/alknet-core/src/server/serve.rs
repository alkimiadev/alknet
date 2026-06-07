//! Server configuration and accept loop.
//!
//! `Server` binds to a transport acceptor and runs an accept loop, handling
//! authentication, stealth mode protocol detection, and graceful shutdown.
//! `ServeOptions` provides a builder-pattern API for programmatic configuration.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use russh::server::{self, Config};
use tokio::io::{AsyncRead, AsyncWrite};
use tracing::{error, info, warn};

use crate::auth::keys::KeySource;
use crate::auth::server_auth::ServerAuthConfig;
use crate::config::{AuthPolicy, ConfigReloadHandle, DynamicConfig};
use crate::error::ConfigError;
use crate::server::handler::{ProxyConfig, ProxyMode, ServerHandler, TransportKind};
use crate::server::rate_limit::ConnectionRateLimiter;
use crate::server::stealth::{self, ProtocolDetection};

const DEFAULT_LISTEN_ADDR: &str = "0.0.0.0:22";
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

/// Transport mode for the server listener.
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

/// Programmatic configuration for an alknet server.
///
/// Construct with `ServeOptions::new(key_source)` and chain builder methods.
/// Call `validate()` before passing to `Server::new()`.
///
/// ```
/// use alknet_core::server::{ServeOptions, ServeTransportMode};
/// use alknet_core::auth::keys::KeySource;
///
/// let opts = ServeOptions::new(KeySource::File("/path/to/host_key".into()))
///     .transport_mode(ServeTransportMode::Tcp)
///     .listen_addr("0.0.0.0:22")
///     .max_connections_per_ip(5)
///     .max_auth_attempts(3);
/// opts.validate().unwrap();
/// ```
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

    pub fn validate(&self) -> Result<(), ConfigError> {
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
            .finish()
    }
}

/// Errors that can occur during server setup and operation.
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

/// The alknet SSH server.
///
/// Accepts connections over any `TransportAcceptor`, authenticates via Ed25519 keys
/// or certificate authority, and proxies `direct-tcpip` channels to their targets.
/// Supports stealth mode (TLS only), outbound proxy routing, and connection rate limiting.
pub struct Server {
    config: Arc<server::Config>,
    dynamic: Arc<ArcSwap<DynamicConfig>>,
    connection_limiter: Arc<ConnectionRateLimiter>,
    outbound_proxy: Option<ProxyConfig>,
    stealth: bool,
    transport_mode: ServeTransportMode,
    listen_addr: String,
    max_auth_attempts: usize,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
    sessions: Arc<tokio::sync::Mutex<Vec<ActiveSession>>>,
}

impl Server {
    pub fn new(opts: ServeOptions) -> Result<Self, ServeError> {
        opts.validate().map_err(ServeError::Config)?;

        let private_key = crate::auth::keys::load_private_key(opts.key.clone())
            .map_err(ServeError::KeyLoadFailed)?;

        let auth_config = ServerAuthConfig::from_keys_and_ca(
            opts.authorized_keys.clone(),
            opts.cert_authority.clone(),
        )
        .map_err(ServeError::KeyLoadFailed)?;

        let auth_policy = AuthPolicy::from_server_auth_config(auth_config);
        let dynamic_config = DynamicConfig::new(auth_policy);

        let max_auth_attempts = opts.max_auth_attempts;
        let max_connections_per_ip = opts.max_connections_per_ip;

        let config = Arc::new(Config {
            keys: vec![private_key],
            max_auth_attempts,
            methods: russh::MethodSet::PUBLICKEY,
            preferred: russh::Preferred::DEFAULT,
            ..Default::default()
        });

        let outbound_proxy = parse_proxy_config(opts.proxy.as_deref());

        let connection_limiter = Arc::new(ConnectionRateLimiter::new(max_connections_per_ip));

        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

        let dynamic = Arc::new(ArcSwap::new(Arc::new(dynamic_config)));

        Ok(Self {
            config,
            dynamic,
            connection_limiter,
            outbound_proxy,
            stealth: opts.stealth,
            transport_mode: opts.transport_mode,
            listen_addr: opts.listen_addr,
            max_auth_attempts,
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
        let transport_kind = match self.transport_mode {
            ServeTransportMode::Tcp => TransportKind::Tcp,
            ServeTransportMode::Tls => TransportKind::Tls,
            ServeTransportMode::Iroh => TransportKind::Iroh,
        };

        if self.transport_mode == ServeTransportMode::Iroh {
            if let Some(id) = endpoint_info {
                info!("alknet server running: transport=iroh endpoint_id={}", id);
            } else {
                info!("alknet server running: transport=iroh");
            }
        } else {
            info!(
                "alknet server running: transport={} listen={}",
                self.transport_mode, self.listen_addr
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
            let handler_transport_kind = transport_kind;

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
            let stealth = server.stealth;
            let transport_is_tls = server.transport_mode == ServeTransportMode::Tls;

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

fn parse_proxy_config(proxy: Option<&str>) -> Option<ProxyConfig> {
    proxy.map(|url| {
        if url.starts_with("socks5://") {
            let addr: SocketAddr = url
                .strip_prefix("socks5://")
                .unwrap()
                .parse()
                .expect("invalid socks5 proxy address");
            ProxyConfig {
                mode: ProxyMode::Socks5(addr),
            }
        } else if url.starts_with("http://") {
            let addr: SocketAddr = url
                .strip_prefix("http://")
                .unwrap()
                .parse()
                .expect("invalid http connect proxy address");
            ProxyConfig {
                mode: ProxyMode::HttpConnect(addr),
            }
        } else {
            panic!("unsupported proxy URL scheme: {url}");
        }
    })
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
    fn parse_proxy_config_socks5() {
        let config = parse_proxy_config(Some("socks5://127.0.0.1:9050"));
        assert!(config.is_some());
        match config.unwrap().mode {
            ProxyMode::Socks5(addr) => {
                assert_eq!(addr, "127.0.0.1:9050".parse().unwrap());
            }
            _ => panic!("expected Socks5"),
        }
    }

    #[test]
    fn parse_proxy_config_http() {
        let config = parse_proxy_config(Some("http://127.0.0.1:8080"));
        assert!(config.is_some());
        match config.unwrap().mode {
            ProxyMode::HttpConnect(addr) => {
                assert_eq!(addr, "127.0.0.1:8080".parse().unwrap());
            }
            _ => panic!("expected HttpConnect"),
        }
    }

    #[test]
    fn parse_proxy_config_none() {
        assert!(parse_proxy_config(None).is_none());
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
    fn server_holds_listen_addr() {
        let opts = ServeOptions::new(make_key_source()).listen_addr("0.0.0.0:443");
        let server = Server::new(opts).unwrap();
        assert_eq!(server.listen_addr, "0.0.0.0:443");
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
}
