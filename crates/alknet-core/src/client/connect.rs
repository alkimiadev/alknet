//! Client session management and connection logic.
//!
//! `ClientSession` establishes an SSH connection over a transport, authenticates,
//! starts a SOCKS5 proxy, sets up port forwards, and monitors for reconnection.
//! `ConnectOptions` provides a builder-pattern API for programmatic configuration.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use russh::client;
use russh::keys::PrivateKey;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use crate::auth::client_auth::{ClientAuthConfig, ClientHandler};
use crate::auth::keys::KeySource;
use crate::client::forward::{LocalForwarder, PortForwardSpec, RemoteForwarder};
use crate::error::ConfigError;
use crate::socks5::{HandleChannelOpener, Socks5Server};
use crate::transport::Transport;

const DEFAULT_SOCKS5_ADDR: &str = "127.0.0.1:1080";
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

/// Transport mode for the client connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportMode {
    Tcp,
    Tls,
    Iroh,
}

impl std::fmt::Display for TransportMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportMode::Tcp => write!(f, "tcp"),
            TransportMode::Tls => write!(f, "tls"),
            TransportMode::Iroh => write!(f, "iroh"),
        }
    }
}

/// Programmatic configuration for an alknet client session.
///
/// Construct with `ConnectOptions::new(key_source)` and chain builder methods.
/// Call `validate()` before passing to `ClientSession::new()`.
///
/// ```
/// use alknet_core::client::{ConnectOptions, TransportMode};
/// use alknet_core::auth::keys::KeySource;
///
/// let opts = ConnectOptions::new(KeySource::File("/path/to/key".into()))
///     .server("example.com:22")
///     .transport_mode(TransportMode::Tcp)
///     .socks5_addr("127.0.0.1:1080")
///     .forward("5432:db.internal:5432");
/// opts.validate().unwrap();
/// ```
#[derive(Clone)]
pub struct ConnectOptions {
    pub server: Option<String>,
    pub peer: Option<String>,
    pub transport_mode: TransportMode,
    pub identity: KeySource,
    pub socks5_addr: String,
    pub forwards: Vec<String>,
    pub remote_forwards: Vec<String>,
    pub proxy: Option<String>,
    pub iroh_relay: Option<String>,
    pub tls_server_name: Option<String>,
    pub insecure: bool,
}

impl ConnectOptions {
    pub fn new(identity: KeySource) -> Self {
        Self {
            server: None,
            peer: None,
            transport_mode: TransportMode::Tcp,
            identity,
            socks5_addr: DEFAULT_SOCKS5_ADDR.to_string(),
            forwards: Vec::new(),
            remote_forwards: Vec::new(),
            proxy: None,
            iroh_relay: None,
            tls_server_name: None,
            insecure: false,
        }
    }

    pub fn server(mut self, addr: impl Into<String>) -> Self {
        self.server = Some(addr.into());
        self
    }

    pub fn peer(mut self, endpoint_id: impl Into<String>) -> Self {
        self.peer = Some(endpoint_id.into());
        self
    }

    pub fn transport_mode(mut self, mode: TransportMode) -> Self {
        self.transport_mode = mode;
        self
    }

    pub fn socks5_addr(mut self, addr: impl Into<String>) -> Self {
        self.socks5_addr = addr.into();
        self
    }

    pub fn forward(mut self, spec: impl Into<String>) -> Self {
        self.forwards.push(spec.into());
        self
    }

    pub fn remote_forward(mut self, spec: impl Into<String>) -> Self {
        self.remote_forwards.push(spec.into());
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

    pub fn tls_server_name(mut self, name: impl Into<String>) -> Self {
        self.tls_server_name = Some(name.into());
        self
    }

    pub fn insecure(mut self, insecure: bool) -> Self {
        self.insecure = insecure;
        self
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        match self.transport_mode {
            TransportMode::Tcp | TransportMode::Tls => {
                if self.server.is_none() {
                    return Err(ConfigError::InvalidFlag {
                        name: "--server is required for tcp/tls transport".to_string(),
                    });
                }
            }
            TransportMode::Iroh => {
                if self.peer.is_none() {
                    return Err(ConfigError::InvalidFlag {
                        name: "--peer is required for iroh transport".to_string(),
                    });
                }
            }
        }
        Ok(())
    }
}

impl std::fmt::Debug for ConnectOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectOptions")
            .field("server", &self.server)
            .field("peer", &self.peer)
            .field("transport_mode", &self.transport_mode)
            .field("identity", &"<KeySource>")
            .field("socks5_addr", &self.socks5_addr)
            .field("forwards", &self.forwards)
            .field("remote_forwards", &self.remote_forwards)
            .field("proxy", &self.proxy)
            .field("iroh_relay", &self.iroh_relay)
            .field("tls_server_name", &self.tls_server_name)
            .field("insecure", &self.insecure)
            .finish()
    }
}

/// An active SSH client session over a transport.
///
/// Establishes the connection, authenticates, and runs a SOCKS5 proxy plus
/// port forwards until shutdown or transport failure. On transport failure,
/// attempts reconnection with exponential backoff (1s, 2s, 4s, ..., 30s cap).
pub struct ClientSession<T: Transport> {
    opts: ConnectOptions,
    transport: Arc<T>,
    handle: Arc<Mutex<client::Handle<ClientHandler>>>,
    auth_config: Arc<ClientAuthConfig>,
    #[allow(dead_code)]
    private_key: Arc<PrivateKey>,
    #[allow(dead_code)]
    username: String,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
}

impl<T: Transport> ClientSession<T> {
    pub async fn new(opts: ConnectOptions, transport: Arc<T>) -> Result<Self, ConnectError> {
        opts.validate().map_err(ConnectError::Config)?;

        let auth_config = Arc::new(
            ClientAuthConfig::from_key_source(opts.identity.clone())
                .map_err(ConnectError::Config)?,
        );
        let private_key = auth_config.private_key();

        let username = derive_username();
        let handler = ClientHandler::from_config(&auth_config);

        let stream = transport.connect().await.map_err(|e| {
            error!("transport connect failed: {e}");
            ConnectError::ConnectionFailed
        })?;

        let config = Arc::new(client::Config::default());
        let mut handle = client::connect_stream(config, stream, handler)
            .await
            .map_err(|e| {
                error!("SSH connect failed: {e}");
                ConnectError::ConnectionFailed
            })?;

        let auth_ok = auth_config
            .authenticate(&mut handle, &username)
            .await
            .map_err(|_| ConnectError::AuthFailed)?;
        if !auth_ok {
            return Err(ConnectError::AuthFailed);
        }

        let handle = Arc::new(Mutex::new(handle));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

        Ok(Self {
            opts,
            transport,
            handle,
            auth_config,
            private_key,
            username,
            shutdown_tx,
            shutdown_rx,
        })
    }

    pub fn handle(&self) -> Arc<Mutex<client::Handle<ClientHandler>>> {
        Arc::clone(&self.handle)
    }

    pub fn auth_config(&self) -> &Arc<ClientAuthConfig> {
        &self.auth_config
    }

    pub fn transport(&self) -> &Arc<T> {
        &self.transport
    }

    pub fn options(&self) -> &ConnectOptions {
        &self.opts
    }

    pub fn shutdown_sender(&self) -> tokio::sync::watch::Sender<bool> {
        self.shutdown_tx.clone()
    }

    pub async fn run(self) -> Result<(), ConnectError> {
        let socks5_addr: SocketAddr = self.opts.socks5_addr.parse().map_err(|_| {
            ConnectError::Config(ConfigError::InvalidFlag {
                name: format!("invalid SOCKS5 address: {}", self.opts.socks5_addr),
            })
        })?;

        let channel_opener = HandleChannelOpener::from_arc(Arc::clone(&self.handle));
        let socks5_server = Socks5Server::with_addr(channel_opener, &socks5_addr.to_string());
        let socks5_listen = socks5_server.listen_addr();

        let local_forwarders = build_local_forwarders(&self.opts)?;
        let remote_specs = build_remote_specs(&self.opts)?;

        for spec in &remote_specs {
            let remote_forwarder =
                RemoteForwarder::new(spec.clone()).map_err(|_| ConnectError::ForwardFailed)?;
            let mut h = self.handle.lock().await;
            remote_forwarder.register(&mut h).await.map_err(|_| {
                warn!("failed to register remote forward {}", spec);
                ConnectError::ForwardFailed
            })?;
            info!("registered remote forward: {}", spec);
        }

        let socks5_task = tokio::spawn(async move {
            debug!("SOCKS5 server starting on {}", socks5_listen);
            if let Err(e) = socks5_server.run().await {
                error!("SOCKS5 server error: {e}");
            }
        });

        let fwd_handle = Arc::clone(&self.handle);
        let fwd_shutdown = self.shutdown_rx.clone();
        let forward_task = tokio::spawn(async move {
            crate::client::forward::run_local_forwarders(
                local_forwarders,
                fwd_handle,
                fwd_shutdown,
            )
            .await;
        });

        info!("alknet client running: SOCKS5 on {}", socks5_listen);

        #[cfg(unix)]
        let signal_done = {
            let sig_tx = self.shutdown_tx.clone();
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

        let mut wait_shutdown = self.shutdown_rx.clone();
        let reconnect_handle = Arc::clone(&self.handle);
        let reconnect_transport = Arc::clone(&self.transport);
        let reconnect_auth = Arc::clone(&self.auth_config);
        let reconnect_username = self.username.clone();
        let reconnect_shutdown = self.shutdown_rx.clone();
        let reconnect_remote_specs = remote_specs.clone();

        let reconnect_monitor = tokio::spawn(async move {
            let mut attempts: u32 = 0;
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                if *reconnect_shutdown.borrow() {
                    break;
                }
                let h = reconnect_handle.lock().await;
                if h.is_closed() {
                    drop(h);
                    info!("SSH session closed, starting reconnection");
                    let backoff = backoff_duration(attempts);
                    warn!("reconnect attempt #{}, waiting {:?}", attempts + 1, backoff);
                    tokio::time::sleep(backoff).await;

                    let handler = ClientHandler::from_config(&reconnect_auth);
                    let username = reconnect_username.clone();
                    match establish_session(
                        &*reconnect_transport,
                        handler,
                        &reconnect_auth,
                        &username,
                    )
                    .await
                    {
                        Ok(new_handle) => {
                            info!("reconnection successful");
                            {
                                let mut guard = reconnect_handle.lock().await;
                                *guard = new_handle;
                            }
                            for spec in &reconnect_remote_specs {
                                match RemoteForwarder::new(spec.clone()) {
                                    Ok(rf) => {
                                        let mut h = reconnect_handle.lock().await;
                                        match rf.register(&mut h).await {
                                            Ok(_) => {
                                                debug!("re-registered remote forward: {}", spec)
                                            }
                                            Err(e) => warn!(
                                                "failed to re-register remote forward {}: {e}",
                                                spec
                                            ),
                                        }
                                    }
                                    Err(e) => warn!("failed to create remote forwarder: {e}"),
                                }
                            }
                            attempts = 0;
                        }
                        Err(e) => {
                            warn!("reconnection attempt failed: {e}");
                            attempts += 1;
                        }
                    }
                }
            }
        });

        tokio::select! {
            _ = wait_shutdown.changed() => {
                if *wait_shutdown.borrow() {
                    info!("shutdown signal received");
                }
            }
            _ = socks5_task => {
                warn!("SOCKS5 server exited unexpectedly");
            }
        }

        reconnect_monitor.abort();

        #[cfg(unix)]
        signal_done.abort();

        self.shutdown().await?;

        forward_task.abort();
        let _ = forward_task.await;

        Ok(())
    }

    pub async fn shutdown(&self) -> Result<(), ConnectError> {
        info!("initiating graceful shutdown");

        let _ = self.shutdown_tx.send(true);

        {
            let handle = self.handle.lock().await;
            if !handle.is_closed() {
                if let Err(e) = handle
                    .disconnect(russh::Disconnect::ByApplication, "shutdown", "")
                    .await
                {
                    warn!("failed to send SSH disconnect: {e}");
                }
            }
        }

        tokio::time::sleep(DRAIN_TIMEOUT).await;

        info!("graceful shutdown complete");
        Ok(())
    }
}

fn derive_username() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "alknet".to_string())
}

async fn establish_session<T: Transport>(
    transport: &T,
    handler: ClientHandler,
    auth_config: &ClientAuthConfig,
    username: &str,
) -> Result<client::Handle<ClientHandler>, ConnectError> {
    let stream = transport.connect().await.map_err(|e| {
        error!("transport connect failed: {e}");
        ConnectError::ConnectionFailed
    })?;

    let config = Arc::new(client::Config::default());
    let mut handle = client::connect_stream(config, stream, handler)
        .await
        .map_err(|e| {
            error!("SSH connect failed: {e}");
            ConnectError::ConnectionFailed
        })?;

    let auth_ok = auth_config
        .authenticate(&mut handle, username)
        .await
        .map_err(|_| ConnectError::AuthFailed)?;
    if !auth_ok {
        return Err(ConnectError::AuthFailed);
    }

    Ok(handle)
}

fn backoff_duration(attempt: u32) -> Duration {
    let secs: u64 = match attempt {
        0 => 1,
        1 => 2,
        2 => 4,
        3 => 8,
        4 => 16,
        _ => 30,
    };
    Duration::from_secs(secs)
}

fn build_local_forwarders(opts: &ConnectOptions) -> Result<Vec<LocalForwarder>, ConnectError> {
    let mut forwarders = Vec::new();
    for spec_str in &opts.forwards {
        let spec = PortForwardSpec::local(spec_str).map_err(|e| {
            warn!("invalid local forward spec '{}': {}", spec_str, e);
            ConnectError::Config(ConfigError::InvalidFlag {
                name: format!("invalid forward spec: {}", spec_str),
            })
        })?;
        forwarders.push(LocalForwarder::new(spec).map_err(|e| {
            warn!("failed to create local forwarder: {}", e);
            ConnectError::ForwardFailed
        })?);
    }
    Ok(forwarders)
}

fn build_remote_specs(opts: &ConnectOptions) -> Result<Vec<PortForwardSpec>, ConnectError> {
    let mut specs = Vec::new();
    for spec_str in &opts.remote_forwards {
        let spec = PortForwardSpec::remote(spec_str).map_err(|e| {
            warn!("invalid remote forward spec '{}': {}", spec_str, e);
            ConnectError::Config(ConfigError::InvalidFlag {
                name: format!("invalid remote forward spec: {}", spec_str),
            })
        })?;
        specs.push(spec);
    }
    Ok(specs)
}

/// Errors that can occur during client connection setup and operation.
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("connection failed")]
    ConnectionFailed,
    #[error("authentication failed")]
    AuthFailed,
    #[error("forward setup failed")]
    ForwardFailed,
    #[error("config error: {0}")]
    Config(#[from] ConfigError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::duplex;

    const ED25519_PRIVATE_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAACBOfInDyRS33JEeDNT8xd10qRdwFN8z/QukCOgEIkv01QAAAJiQ+NvMkPjb\nzAAAAAtzc2gtZWQyNTUxOQAAACBOfInDyRS33JEeDNT8xd10qRdwFN8z/QukCOgEIkv01Q\nAAAECIWwJf7+7MOuZAOOWmoQbE9i/5GxjKsFrtJHjZ34E/fk58icPJFLfckR4M1PzF3XSp\nF3AU3zP9C6QI6AQiS/TVAAAAD3VidW50dUBuczUyODA5NgECAwQFBg==\n-----END OPENSSH PRIVATE KEY-----\n";

    fn make_identity() -> KeySource {
        KeySource::Memory(ED25519_PRIVATE_KEY.as_bytes().to_vec())
    }

    #[test]
    fn connect_options_default_fields() {
        let opts = ConnectOptions::new(make_identity());
        assert!(opts.server.is_none());
        assert!(opts.peer.is_none());
        assert_eq!(opts.transport_mode, TransportMode::Tcp);
        assert_eq!(opts.socks5_addr, "127.0.0.1:1080");
        assert!(opts.forwards.is_empty());
        assert!(opts.remote_forwards.is_empty());
        assert!(opts.proxy.is_none());
        assert!(opts.iroh_relay.is_none());
        assert!(opts.tls_server_name.is_none());
        assert!(!opts.insecure);
    }

    #[test]
    fn connect_options_builder_pattern() {
        let opts = ConnectOptions::new(make_identity())
            .server("example.com:22")
            .transport_mode(TransportMode::Tls)
            .socks5_addr("127.0.0.1:9050")
            .forward("127.0.0.1:5432:db:5432")
            .remote_forward("0.0.0.0:8080:127.0.0.1:3000")
            .proxy("socks5://127.0.0.1:1080")
            .iroh_relay("https://relay.example.com")
            .tls_server_name("alknet.test")
            .insecure(true);

        assert_eq!(opts.server.as_deref(), Some("example.com:22"));
        assert_eq!(opts.transport_mode, TransportMode::Tls);
        assert_eq!(opts.socks5_addr, "127.0.0.1:9050");
        assert_eq!(opts.forwards.len(), 1);
        assert_eq!(opts.remote_forwards.len(), 1);
        assert_eq!(opts.proxy.as_deref(), Some("socks5://127.0.0.1:1080"));
        assert_eq!(
            opts.iroh_relay.as_deref(),
            Some("https://relay.example.com")
        );
        assert_eq!(opts.tls_server_name.as_deref(), Some("alknet.test"));
        assert!(opts.insecure);
    }

    #[test]
    fn connect_options_validate_tcp_requires_server() {
        let opts = ConnectOptions::new(make_identity()).transport_mode(TransportMode::Tcp);
        assert!(opts.validate().is_err());
    }

    #[test]
    fn connect_options_validate_tcp_with_server_ok() {
        let opts = ConnectOptions::new(make_identity()).server("example.com:22");
        assert!(opts.validate().is_ok());
    }

    #[test]
    fn connect_options_validate_tls_requires_server() {
        let opts = ConnectOptions::new(make_identity()).transport_mode(TransportMode::Tls);
        assert!(opts.validate().is_err());
    }

    #[test]
    fn connect_options_validate_tls_with_server_ok() {
        let opts = ConnectOptions::new(make_identity())
            .transport_mode(TransportMode::Tls)
            .server("example.com:443");
        assert!(opts.validate().is_ok());
    }

    #[test]
    fn connect_options_validate_iroh_requires_peer() {
        let opts = ConnectOptions::new(make_identity()).transport_mode(TransportMode::Iroh);
        assert!(opts.validate().is_err());
    }

    #[test]
    fn connect_options_validate_iroh_with_peer_ok() {
        let opts = ConnectOptions::new(make_identity())
            .transport_mode(TransportMode::Iroh)
            .peer("some-endpoint-id");
        assert!(opts.validate().is_ok());
    }

    #[test]
    fn identity_accepts_key_source_file() {
        let file_source = KeySource::File(std::path::PathBuf::from("/path/to/key"));
        let opts = ConnectOptions::new(file_source);
        match &opts.identity {
            KeySource::File(p) => assert_eq!(p, &std::path::PathBuf::from("/path/to/key")),
            _ => panic!("expected File variant"),
        }
    }

    #[test]
    fn identity_accepts_key_source_memory() {
        let mem_source = KeySource::Memory(b"key-data".to_vec());
        let opts = ConnectOptions::new(mem_source);
        match &opts.identity {
            KeySource::Memory(d) => assert_eq!(d, b"key-data"),
            _ => panic!("expected Memory variant"),
        }
    }

    #[test]
    fn transport_mode_display() {
        assert_eq!(TransportMode::Tcp.to_string(), "tcp");
        assert_eq!(TransportMode::Tls.to_string(), "tls");
        assert_eq!(TransportMode::Iroh.to_string(), "iroh");
    }

    #[test]
    fn connect_error_variants() {
        assert_eq!(
            ConnectError::ConnectionFailed.to_string(),
            "connection failed"
        );
        assert_eq!(
            ConnectError::AuthFailed.to_string(),
            "authentication failed"
        );
        assert_eq!(
            ConnectError::ForwardFailed.to_string(),
            "forward setup failed"
        );
    }

    #[test]
    fn connect_options_debug_redacts_identity() {
        let opts = ConnectOptions::new(make_identity());
        let debug_str = format!("{:?}", opts);
        assert!(debug_str.contains("<KeySource>"));
        assert!(!debug_str.contains("OPENSSH"));
    }

    struct FailTransport;

    #[async_trait::async_trait]
    impl Transport for FailTransport {
        type Stream = tokio::io::DuplexStream;

        async fn connect(&self) -> anyhow::Result<Self::Stream> {
            Err(anyhow::anyhow!("always fails"))
        }

        fn describe(&self) -> String {
            "fail".to_string()
        }
    }

    struct DuplexTransport {
        connect_count: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl Transport for DuplexTransport {
        type Stream = tokio::io::DuplexStream;

        async fn connect(&self) -> anyhow::Result<Self::Stream> {
            self.connect_count.fetch_add(1, Ordering::SeqCst);
            let (client, _) = duplex(4096);
            Ok(client)
        }

        fn describe(&self) -> String {
            "duplex".to_string()
        }
    }

    #[tokio::test]
    async fn client_session_new_transport_fails() {
        let opts = ConnectOptions::new(make_identity()).server("example.com:22");
        let transport = Arc::new(FailTransport);
        let result = ClientSession::new(opts, transport).await;
        assert!(result.is_err());
        assert!(matches!(
            result.err().unwrap(),
            ConnectError::ConnectionFailed
        ));
    }

    #[tokio::test]
    async fn client_session_new_ssh_handshake_fails() {
        let transport = Arc::new(DuplexTransport {
            connect_count: Arc::new(AtomicUsize::new(0)),
        });
        let opts = ConnectOptions::new(make_identity()).server("example.com:22");
        let result = ClientSession::new(opts, transport).await;
        assert!(result.is_err());
        assert!(matches!(
            result.err().unwrap(),
            ConnectError::ConnectionFailed
        ));
    }

    #[test]
    fn build_local_forwarders_empty() {
        let opts = ConnectOptions::new(make_identity());
        let result = build_local_forwarders(&opts);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn build_local_forwarders_valid() {
        let opts = ConnectOptions::new(make_identity()).forward("127.0.0.1:5432:db:5432");
        let result = build_local_forwarders(&opts);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 1);
    }

    #[test]
    fn build_local_forwarders_invalid_spec() {
        let opts = ConnectOptions::new(make_identity()).forward("bad-spec");
        let result = build_local_forwarders(&opts);
        assert!(result.is_err());
    }

    #[test]
    fn build_remote_specs_empty() {
        let opts = ConnectOptions::new(make_identity());
        let result = build_remote_specs(&opts);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn build_remote_specs_valid() {
        let opts =
            ConnectOptions::new(make_identity()).remote_forward("0.0.0.0:8080:127.0.0.1:3000");
        let result = build_remote_specs(&opts);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 1);
    }

    #[test]
    fn build_remote_specs_invalid() {
        let opts = ConnectOptions::new(make_identity()).remote_forward("bad");
        let result = build_remote_specs(&opts);
        assert!(result.is_err());
    }

    #[test]
    fn default_socks5_addr() {
        assert_eq!(DEFAULT_SOCKS5_ADDR, "127.0.0.1:1080");
    }

    #[test]
    fn drain_timeout_is_two_seconds() {
        assert_eq!(DRAIN_TIMEOUT, Duration::from_secs(2));
    }

    #[test]
    fn transport_mode_equality() {
        assert_eq!(TransportMode::Tcp, TransportMode::Tcp);
        assert_ne!(TransportMode::Tcp, TransportMode::Tls);
        assert_ne!(TransportMode::Tls, TransportMode::Iroh);
    }

    #[tokio::test]
    async fn shutdown_sends_disconnect_and_drains() {
        let transport = Arc::new(DuplexTransport {
            connect_count: Arc::new(AtomicUsize::new(0)),
        });
        let opts = ConnectOptions::new(make_identity()).server("example.com:22");
        let result = ClientSession::new(opts, transport).await;
        assert!(result.is_err());
    }

    #[test]
    fn socks5_is_always_enabled_by_default() {
        let opts = ConnectOptions::new(make_identity());
        assert!(!opts.socks5_addr.is_empty());
    }

    #[tokio::test]
    async fn integration_mock_transport_session() {
        use crate::socks5::{ChannelOpenError, ChannelOpener};
        use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};
        use tokio::net::{TcpListener, TcpStream};

        struct MockOpener;

        impl ChannelOpener for MockOpener {
            type Stream = tokio::io::DuplexStream;

            async fn open_channel(
                &self,
                _host: String,
                _port: u16,
            ) -> Result<Self::Stream, ChannelOpenError> {
                let (client, _server) = duplex(4096);
                Ok(client)
            }
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound_addr = listener.local_addr().unwrap();
        drop(listener);

        let opener = MockOpener;
        let server = Socks5Server::with_addr(opener, &bound_addr.to_string());

        let _server_task = tokio::spawn(async move {
            let _ = server.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut conn = TcpStream::connect(bound_addr).await.unwrap();

        let greeting = [0x05, 0x01, 0x00];
        conn.write_all(&greeting).await.unwrap();

        let mut auth_resp = [0u8; 2];
        conn.read_exact(&mut auth_resp).await.unwrap();
        assert_eq!(auth_resp, [0x05, 0x00]);

        let connect_req = [0x05, 0x01, 0x00, 0x01, 127, 0, 0, 1, 0, 80];
        conn.write_all(&connect_req).await.unwrap();

        let mut reply = [0u8; 10];
        conn.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply[1], 0x00);

        conn.write_all(b"test data").await.unwrap();
        conn.shutdown().await.unwrap();
    }
}
