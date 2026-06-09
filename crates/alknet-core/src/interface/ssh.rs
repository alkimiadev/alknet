use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use arc_swap::ArcSwap;
use async_trait::async_trait;
use russh::keys::ssh_key::HashAlg;
use russh::server::{self, Config};
use russh::Channel;
use russh::ChannelId;
use tokio::sync::mpsc;

use crate::auth::identity::{Identity, IdentityProvider};
use crate::call::frame::{FrameFramedReader, FrameFramedWriter};
use crate::call::EventEnvelope;
use crate::config::DynamicConfig;
use crate::interface::session::{InterfaceEvent, InterfaceSession};
use crate::interface::{StreamInterface, StreamInterfaceConfig, TransportStream};
use crate::server::control_channel::{
    ControlChannelHandler, ControlChannelRouter, DuplexStream, ALKNET_CONTROL_DESTINATION,
    ALKNET_PREFIX,
};
use crate::server::rate_limit::{AuthAttemptLimiter, ConnectionRateLimiter};
use crate::transport::TransportKind;

struct SshHandler {
    dynamic: Arc<ArcSwap<DynamicConfig>>,
    identity_provider: Arc<dyn IdentityProvider>,
    outbound_proxy: Option<crate::server::handler::ProxyConfig>,
    remote_addr: Option<SocketAddr>,
    transport: TransportKind,
    connection_limiter: Arc<ConnectionRateLimiter>,
    connection_allowed: bool,
    auth_limiter: AuthAttemptLimiter,
    authenticated_identity: Option<Identity>,
    control_channel_router: ControlChannelRouter,
    bridge_event_tx: Option<mpsc::Sender<InterfaceEvent>>,
    bridge_envelope_rx: Option<mpsc::Receiver<EventEnvelope>>,
    connected_at: Instant,
}

impl SshHandler {
    fn new(
        dynamic: Arc<ArcSwap<DynamicConfig>>,
        identity_provider: Arc<dyn IdentityProvider>,
        outbound_proxy: Option<crate::server::handler::ProxyConfig>,
        remote_addr: Option<SocketAddr>,
        transport: TransportKind,
        connection_limiter: Arc<ConnectionRateLimiter>,
        max_auth_attempts: usize,
    ) -> Self {
        let allowed = if let Some(addr) = remote_addr {
            let ip = addr.ip();
            if connection_limiter.check(ip) {
                connection_limiter.on_connect(ip);
                tracing::info!(
                    remote_addr = %addr,
                    transport = %transport,
                    "connection opened"
                );
                true
            } else {
                tracing::info!(
                    remote_addr = %addr,
                    transport = %transport,
                    "connection rejected"
                );
                false
            }
        } else {
            true
        };

        Self {
            dynamic,
            identity_provider,
            outbound_proxy,
            remote_addr,
            transport,
            connection_limiter,
            connection_allowed: allowed,
            auth_limiter: AuthAttemptLimiter::new(max_auth_attempts),
            authenticated_identity: None,
            control_channel_router: ControlChannelRouter::without_handler(),
            bridge_event_tx: None,
            bridge_envelope_rx: None,
            connected_at: Instant::now(),
        }
    }

    #[allow(dead_code)]
    fn with_control_channel_router(mut self, router: ControlChannelRouter) -> Self {
        self.control_channel_router = router;
        self
    }

    fn with_bridge_channels(
        mut self,
        event_tx: mpsc::Sender<InterfaceEvent>,
        envelope_rx: mpsc::Receiver<EventEnvelope>,
    ) -> Self {
        self.bridge_event_tx = Some(event_tx);
        self.bridge_envelope_rx = Some(envelope_rx);
        self
    }

    fn has_control_channel_bridge(&self) -> bool {
        self.bridge_event_tx.is_some() && self.bridge_envelope_rx.is_some()
    }
}

impl Drop for SshHandler {
    fn drop(&mut self) {
        if let Some(addr) = self.remote_addr {
            if self.connection_allowed {
                self.connection_limiter.on_disconnect(addr.ip());
                let duration = self.connected_at.elapsed();
                tracing::info!(
                    remote_addr = %addr,
                    duration_secs = duration.as_secs_f64(),
                    "connection closed"
                );
            }
        }
    }
}

#[async_trait]
impl server::Handler for SshHandler {
    type Error = russh::Error;

    async fn auth_publickey(
        &mut self,
        user: &str,
        public_key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<server::Auth, Self::Error> {
        if !self.auth_limiter.check() {
            let remote_addr_display = self
                .remote_addr
                .map_or("unknown".to_string(), |a| a.to_string());
            let fingerprint = format!("{}", public_key.fingerprint(HashAlg::Sha256));
            tracing::info!(
                remote_addr = %remote_addr_display,
                user = user,
                key_fingerprint = %fingerprint,
                result = "reject",
                "auth attempt"
            );
            return Ok(server::Auth::Reject {
                proceed_with_methods: None,
            });
        }

        let fingerprint = format!("{}", public_key.fingerprint(HashAlg::Sha256));
        let remote_addr_display = self
            .remote_addr
            .map_or("unknown".to_string(), |a| a.to_string());

        let identity = self
            .identity_provider
            .resolve_from_fingerprint(&fingerprint);

        match identity {
            Some(id) => {
                self.authenticated_identity = Some(id);
                tracing::info!(
                    remote_addr = %remote_addr_display,
                    user = user,
                    key_fingerprint = %fingerprint,
                    result = "accept",
                    "auth attempt"
                );
                Ok(server::Auth::Accept)
            }
            None => {
                self.auth_limiter.on_failure();
                tracing::info!(
                    remote_addr = %remote_addr_display,
                    user = user,
                    key_fingerprint = %fingerprint,
                    result = "reject",
                    "auth attempt"
                );
                Ok(server::Auth::Reject {
                    proceed_with_methods: None,
                })
            }
        }
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<server::Msg>,
        host_to_connect: &str,
        port_to_connect: u32,
        originator_address: &str,
        originator_port: u32,
        _session: &mut server::Session,
    ) -> Result<bool, Self::Error> {
        if host_to_connect.starts_with(ALKNET_PREFIX) {
            if host_to_connect == ALKNET_CONTROL_DESTINATION && self.has_control_channel_bridge() {
                let event_tx = self.bridge_event_tx.take().unwrap();
                let envelope_rx = self.bridge_envelope_rx.take().unwrap();
                let identity = self.authenticated_identity.clone();
                tokio::spawn(async move {
                    let stream = channel.into_stream();
                    let (read_half, write_half) = tokio::io::split(stream);
                    run_control_channel_bridge(
                        read_half,
                        write_half,
                        identity,
                        event_tx,
                        envelope_rx,
                    )
                    .await;
                });
                let _ = (originator_address, originator_port);
                return Ok(true);
            }
            if self.control_channel_router.has_handler() {
                if let Some(handler) = self.control_channel_router.take_handler() {
                    let stream: Box<dyn DuplexStream> = Box::new(channel.into_stream());
                    tokio::spawn(async move {
                        handler.handle_channel(stream).await;
                    });
                }
                let _ = (originator_address, originator_port);
                return Ok(true);
            }
            return Ok(false);
        }

        let identity = self
            .authenticated_identity
            .clone()
            .unwrap_or_else(|| Identity {
                id: String::new(),
                scopes: vec![],
                resources: std::collections::HashMap::new(),
            });

        let policy = self.dynamic.load();
        let allowed = policy.forwarding.check(
            host_to_connect,
            port_to_connect as u16,
            &identity,
            self.transport.clone(),
        );

        if !allowed {
            tracing::info!(
                remote_addr = ?self.remote_addr,
                target = %format!("{host_to_connect}:{port_to_connect}"),
                identity = %identity.id,
                transport = %self.transport,
                "forwarding denied by policy"
            );
            return Ok(false);
        }

        let target_host = host_to_connect.to_string();
        let target_port = port_to_connect;
        let proxy_config =
            self.outbound_proxy
                .clone()
                .unwrap_or(crate::server::handler::ProxyConfig {
                    mode: crate::server::handler::ProxyMode::Direct,
                });

        tokio::spawn(async move {
            let target = match format!("{target_host}:{target_port}")
                .parse::<std::net::SocketAddr>()
            {
                Ok(addr) => addr,
                Err(_) => {
                    match tokio::net::lookup_host((&target_host[..], target_port as u16)).await {
                        Ok(mut addrs) => match addrs.next() {
                            Some(addr) => addr,
                            None => return,
                        },
                        Err(_) => return,
                    }
                }
            };
            crate::server::channel_proxy::proxy_channel(
                channel.into_stream(),
                target,
                &proxy_config,
            )
            .await;
        });

        let _ = (originator_address, originator_port);
        Ok(true)
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<server::Msg>,
        session: &mut server::Session,
    ) -> Result<bool, Self::Error> {
        tracing::warn!(
            remote_addr = ?self.remote_addr,
            "rejected session channel (shell/exec not supported)"
        );
        let _ = session;
        Ok(false)
    }

    async fn channel_open_x11(
        &mut self,
        _channel: Channel<server::Msg>,
        _originator_address: &str,
        _originator_port: u32,
        session: &mut server::Session,
    ) -> Result<bool, Self::Error> {
        tracing::warn!(
            remote_addr = ?self.remote_addr,
            "rejected x11 channel"
        );
        let _ = session;
        Ok(false)
    }

    async fn channel_open_forwarded_tcpip(
        &mut self,
        _channel: Channel<server::Msg>,
        host_to_connect: &str,
        port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        session: &mut server::Session,
    ) -> Result<bool, Self::Error> {
        tracing::warn!(
            remote_addr = ?self.remote_addr,
            target = %format!("{host_to_connect}:{port_to_connect}"),
            "rejected forwarded-tcpip channel (remote port forwarding not supported)"
        );
        let _ = session;
        Ok(false)
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        tracing::warn!(
            remote_addr = ?self.remote_addr,
            channel = %channel,
            data_len = data.len(),
            "rejected exec request on channel (shell/exec not supported)"
        );
        let _ = session.channel_failure(channel);
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        tracing::warn!(
            remote_addr = ?self.remote_addr,
            channel = %channel,
            "rejected shell request on channel"
        );
        let _ = session.channel_failure(channel);
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel: ChannelId,
        name: &str,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        tracing::warn!(
            remote_addr = ?self.remote_addr,
            channel = %channel,
            subsystem = name,
            "rejected subsystem request on channel"
        );
        let _ = session.channel_failure(channel);
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        term: &str,
        col_width: u32,
        row_height: u32,
        pix_width: u32,
        pix_height: u32,
        modes: &[(russh::Pty, u32)],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        tracing::warn!(
            remote_addr = ?self.remote_addr,
            channel = %channel,
            term = term,
            "rejected pty request on channel"
        );
        let _ = (col_width, row_height, pix_width, pix_height, modes);
        let _ = session.channel_failure(channel);
        Ok(())
    }

    async fn env_request(
        &mut self,
        channel: ChannelId,
        variable_name: &str,
        variable_value: &str,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        tracing::warn!(
            remote_addr = ?self.remote_addr,
            channel = %channel,
            variable = variable_name,
            "rejected env request on channel"
        );
        let _ = variable_value;
        let _ = session.channel_failure(channel);
        Ok(())
    }

    async fn x11_request(
        &mut self,
        channel: ChannelId,
        single_connection: bool,
        x11_auth_protocol: &str,
        x11_auth_cookie: &str,
        x11_screen_number: u32,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        tracing::warn!(
            remote_addr = ?self.remote_addr,
            channel = %channel,
            "rejected x11 request on channel"
        );
        let _ = (
            single_connection,
            x11_auth_protocol,
            x11_auth_cookie,
            x11_screen_number,
        );
        let _ = session.channel_failure(channel);
        Ok(())
    }

    async fn agent_request(
        &mut self,
        channel: ChannelId,
        session: &mut server::Session,
    ) -> Result<bool, Self::Error> {
        tracing::warn!(
            remote_addr = ?self.remote_addr,
            channel = %channel,
            "rejected agent forwarding request on channel"
        );
        let _ = session;
        Ok(false)
    }

    async fn tcpip_forward(
        &mut self,
        address: &str,
        port: &mut u32,
        session: &mut server::Session,
    ) -> Result<bool, Self::Error> {
        tracing::warn!(
            remote_addr = ?self.remote_addr,
            address = address,
            port = *port,
            "rejected tcpip-forward request (remote port forwarding not supported)"
        );
        let _ = session;
        Ok(false)
    }

    async fn cancel_tcpip_forward(
        &mut self,
        address: &str,
        port: u32,
        session: &mut server::Session,
    ) -> Result<bool, Self::Error> {
        let _ = (address, port, session);
        Ok(false)
    }

    async fn streamlocal_forward(
        &mut self,
        socket_path: &str,
        session: &mut server::Session,
    ) -> Result<bool, Self::Error> {
        tracing::warn!(
            remote_addr = ?self.remote_addr,
            socket_path = socket_path,
            "rejected streamlocal-forward request"
        );
        let _ = session;
        Ok(false)
    }

    async fn signal(
        &mut self,
        channel: ChannelId,
        signal: russh::Sig,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        tracing::debug!(
            remote_addr = ?self.remote_addr,
            channel = %channel,
            signal = ?signal,
            "received signal on channel (ignored)"
        );
        let _ = session;
        Ok(())
    }
}

pub struct SshInterface {
    config: Arc<Config>,
    dynamic: Arc<ArcSwap<DynamicConfig>>,
    connection_limiter: Arc<ConnectionRateLimiter>,
    outbound_proxy: Option<crate::server::handler::ProxyConfig>,
    max_auth_attempts: usize,
}

impl SshInterface {
    pub fn new(config: Arc<Config>, dynamic: Arc<ArcSwap<DynamicConfig>>) -> Self {
        Self {
            config,
            dynamic,
            connection_limiter: Arc::new(ConnectionRateLimiter::new(0)),
            outbound_proxy: None,
            max_auth_attempts: 10,
        }
    }

    pub fn with_connection_limiter(mut self, limiter: Arc<ConnectionRateLimiter>) -> Self {
        self.connection_limiter = limiter;
        self
    }

    pub fn with_outbound_proxy(
        mut self,
        proxy: Option<crate::server::handler::ProxyConfig>,
    ) -> Self {
        self.outbound_proxy = proxy;
        self
    }

    pub fn with_max_auth_attempts(mut self, max: usize) -> Self {
        self.max_auth_attempts = max;
        self
    }

    pub fn config(&self) -> &Arc<Config> {
        &self.config
    }

    pub fn dynamic(&self) -> &Arc<ArcSwap<DynamicConfig>> {
        &self.dynamic
    }

    async fn accept_inner(
        &self,
        stream: Box<dyn TransportStream>,
        ssh_config: &crate::interface::SshInterfaceConfig,
        remote_addr: Option<SocketAddr>,
        transport: TransportKind,
    ) -> Result<SshSession> {
        let identity_provider = Arc::clone(&ssh_config.auth);
        let _forwarding = Arc::clone(&ssh_config.forwarding);

        let (event_tx, event_rx) = mpsc::channel::<InterfaceEvent>(256);
        let (envelope_tx, envelope_rx) = mpsc::channel::<EventEnvelope>(256);

        let handler = SshHandler::new(
            Arc::clone(&self.dynamic),
            identity_provider,
            self.outbound_proxy.clone(),
            remote_addr,
            transport,
            Arc::clone(&self.connection_limiter),
            self.max_auth_attempts,
        )
        .with_bridge_channels(event_tx, envelope_rx);

        let running = server::run_stream(Arc::clone(&self.config), stream, handler).await?;
        let handle = running.handle();
        let join = tokio::spawn(async {
            let _ = running.await;
        });

        Ok(SshSession {
            handle,
            _join: join,
            event_rx,
            envelope_tx,
        })
    }
}

#[async_trait]
impl StreamInterface for SshInterface {
    type Session = SshSession;

    async fn accept(
        &self,
        stream: Box<dyn TransportStream>,
        config: &StreamInterfaceConfig,
    ) -> Result<Self::Session> {
        let ssh_config = match config {
            StreamInterfaceConfig::Ssh(c) => c,
            StreamInterfaceConfig::RawFraming(_) => {
                return Err(anyhow::anyhow!("SshInterface received RawFramingConfig"));
            }
        };

        self.accept_inner(stream, ssh_config, None, TransportKind::Tcp)
            .await
    }
}

pub struct SshSession {
    handle: server::Handle,
    _join: tokio::task::JoinHandle<()>,
    event_rx: mpsc::Receiver<InterfaceEvent>,
    envelope_tx: mpsc::Sender<EventEnvelope>,
}

impl SshSession {
    pub fn handle(&self) -> &server::Handle {
        &self.handle
    }
}

#[async_trait]
impl InterfaceSession for SshSession {
    async fn recv(&mut self) -> Option<InterfaceEvent> {
        self.event_rx.recv().await
    }

    async fn send(&mut self, envelope: EventEnvelope) -> Result<()> {
        self.envelope_tx
            .send(envelope)
            .await
            .map_err(|_| anyhow::anyhow!("control channel bridge closed"))
    }
}

async fn run_control_channel_bridge<R, W>(
    read_half: R,
    write_half: W,
    identity: Option<Identity>,
    event_tx: mpsc::Sender<InterfaceEvent>,
    mut envelope_rx: mpsc::Receiver<EventEnvelope>,
) where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut reader = FrameFramedReader::new(read_half);
    let mut writer = FrameFramedWriter::new(write_half);

    loop {
        tokio::select! {
            frame = reader.read_frame() => {
                match frame {
                    Ok(Some(envelope)) => {
                        let event = match &identity {
                            Some(id) => InterfaceEvent::with_identity(envelope, id.clone()),
                            None => InterfaceEvent::new(envelope),
                        };
                        if event_tx.send(event).await.is_err() {
                            return;
                        }
                    }
                    Ok(None) => return,
                    Err(_) => return,
                }
            }
            envelope = envelope_rx.recv() => {
                match envelope {
                    Some(envelope) => {
                        if writer.write_frame(&envelope).await.is_err() {
                            return;
                        }
                    }
                    None => return,
                }
            }
        }
    }
}

pub struct ControlChannelBridge {
    identity: Option<Identity>,
}

impl ControlChannelBridge {
    pub fn new(identity: Option<Identity>) -> Self {
        Self { identity }
    }
}

#[async_trait]
impl ControlChannelHandler for ControlChannelBridge {
    async fn handle_channel(&self, stream: Box<dyn DuplexStream>) {
        let (event_tx, _event_rx) = mpsc::channel::<InterfaceEvent>(256);
        let (_envelope_tx, envelope_rx) = mpsc::channel::<EventEnvelope>(256);

        let identity = self.identity.clone();
        let (read_half, write_half) = tokio::io::split(stream);
        tokio::spawn(run_control_channel_bridge(
            read_half,
            write_half,
            identity,
            event_tx,
            envelope_rx,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call::frame::{FrameFramedReader, FrameFramedWriter};
    use tokio::io::duplex;

    #[test]
    fn ssh_interface_constructs_with_config() {
        let config = Arc::new(Config {
            keys: vec![russh::keys::PrivateKey::random(
                &mut rand_core::OsRng,
                russh::keys::Algorithm::Ed25519,
            )
            .unwrap()],
            ..Default::default()
        });
        let dynamic = Arc::new(ArcSwap::new(Arc::new(DynamicConfig::default())));

        let iface = SshInterface::new(config, dynamic);
        assert!(iface.config().keys.len() >= 1);
    }

    #[test]
    fn ssh_interface_builder_pattern() {
        let config = Arc::new(Config {
            keys: vec![russh::keys::PrivateKey::random(
                &mut rand_core::OsRng,
                russh::keys::Algorithm::Ed25519,
            )
            .unwrap()],
            ..Default::default()
        });
        let dynamic = Arc::new(ArcSwap::new(Arc::new(DynamicConfig::default())));
        let limiter = Arc::new(ConnectionRateLimiter::new(5));

        let iface = SshInterface::new(config, dynamic)
            .with_connection_limiter(limiter)
            .with_max_auth_attempts(3);

        assert!(iface.config().keys.len() >= 1);
    }

    #[test]
    fn ssh_handler_auth_delegates_to_identity_provider() {
        use std::collections::HashMap;

        struct MockProvider {
            identities: HashMap<String, Identity>,
        }

        impl IdentityProvider for MockProvider {
            fn resolve_from_fingerprint(&self, fp: &str) -> Option<Identity> {
                self.identities.get(fp).cloned()
            }
            fn resolve_from_token(&self, _t: &crate::auth::AuthToken) -> Option<Identity> {
                None
            }
        }

        let mut ids = HashMap::new();
        ids.insert(
            "SHA256:testkey".to_string(),
            Identity {
                id: "SHA256:testkey".to_string(),
                scopes: vec!["admin".to_string()],
                resources: HashMap::new(),
            },
        );

        let provider: Arc<dyn IdentityProvider> = Arc::new(MockProvider { identities: ids });
        let dynamic = Arc::new(ArcSwap::new(Arc::new(DynamicConfig::default())));
        let limiter = Arc::new(ConnectionRateLimiter::new(0));

        let handler = SshHandler::new(
            dynamic,
            provider,
            None,
            None,
            TransportKind::Tcp,
            limiter,
            10,
        );

        assert!(handler.authenticated_identity.is_none());
    }

    #[test]
    fn ssh_handler_connection_rate_limiting() {
        let dynamic = Arc::new(ArcSwap::new(Arc::new(DynamicConfig::default())));
        let provider: Arc<dyn IdentityProvider> = Arc::new(
            crate::auth::identity::ConfigIdentityProvider::new(Arc::clone(&dynamic)),
        );
        let limiter = Arc::new(ConnectionRateLimiter::new(1));
        let addr: SocketAddr = "10.0.0.1:22".parse().unwrap();

        let h1 = SshHandler::new(
            Arc::clone(&dynamic),
            Arc::clone(&provider),
            None,
            Some(addr),
            TransportKind::Tcp,
            Arc::clone(&limiter),
            10,
        );
        assert!(h1.connection_allowed);

        let h2 = SshHandler::new(
            dynamic,
            provider,
            None,
            Some(addr),
            TransportKind::Tcp,
            limiter,
            10,
        );
        assert!(!h2.connection_allowed);
    }

    #[tokio::test]
    async fn ssh_interface_rejects_raw_framing_config() {
        let config = Arc::new(Config {
            keys: vec![russh::keys::PrivateKey::random(
                &mut rand_core::OsRng,
                russh::keys::Algorithm::Ed25519,
            )
            .unwrap()],
            ..Default::default()
        });
        let dynamic = Arc::new(ArcSwap::new(Arc::new(DynamicConfig::default())));
        let iface = SshInterface::new(config, dynamic);
        let (_client, server) = tokio::io::duplex(1024);
        let stream: Box<dyn TransportStream> = Box::new(server);

        let raw_config = StreamInterfaceConfig::RawFraming(crate::interface::RawFramingConfig {});
        let result = iface.accept(stream, &raw_config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn ssh_session_round_trip_event_envelope() {
        let (client, server) = duplex(4096);

        let (event_tx, mut event_rx) = mpsc::channel::<InterfaceEvent>(256);
        let (envelope_tx, envelope_rx) = mpsc::channel::<EventEnvelope>(256);

        let identity = Identity {
            id: "SHA256:test".to_string(),
            scopes: vec![],
            resources: std::collections::HashMap::new(),
        };
        let identity_clone = identity.clone();

        let (server_read, server_write) = tokio::io::split(server);
        tokio::spawn(run_control_channel_bridge(
            server_read,
            server_write,
            Some(identity_clone),
            event_tx,
            envelope_rx,
        ));

        let (client_read, client_write) = tokio::io::split(client);
        let mut client_reader = FrameFramedReader::new(client_read);
        let mut client_writer = FrameFramedWriter::new(client_write);

        let envelope = EventEnvelope::call_requested("req-1", serde_json::json!({"op": "test"}));
        client_writer.write_frame(&envelope).await.unwrap();

        let received_event =
            tokio::time::timeout(std::time::Duration::from_secs(2), event_rx.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(received_event.envelope, envelope);
        assert_eq!(received_event.identity.as_ref().unwrap().id, "SHA256:test");

        let response = EventEnvelope::call_responded("req-1", serde_json::json!({"result": 42}));
        envelope_tx.send(response.clone()).await.unwrap();

        let read_back = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            client_reader.read_frame(),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        assert_eq!(read_back, response);
    }

    #[tokio::test]
    async fn ssh_session_recv_without_identity() {
        let (client, server) = duplex(4096);

        let (event_tx, mut event_rx) = mpsc::channel::<InterfaceEvent>(256);
        let (_envelope_tx, envelope_rx) = mpsc::channel::<EventEnvelope>(256);

        let (server_read, server_write) = tokio::io::split(server);
        tokio::spawn(run_control_channel_bridge(
            server_read,
            server_write,
            None,
            event_tx,
            envelope_rx,
        ));

        let (client_read, client_write) = tokio::io::split(client);
        let mut client_writer = FrameFramedWriter::new(client_write);
        let _client_reader = FrameFramedReader::new(client_read);

        let envelope = EventEnvelope::call_requested("req-2", serde_json::json!({"op": "no-id"}));
        client_writer.write_frame(&envelope).await.unwrap();

        let received_event =
            tokio::time::timeout(std::time::Duration::from_secs(2), event_rx.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(received_event.envelope, envelope);
        assert!(received_event.identity.is_none());
    }

    #[tokio::test]
    async fn control_channel_router_with_handler_routes_data() {
        let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let called_clone = called.clone();

        struct TrackingHandler {
            called: std::sync::Arc<std::sync::atomic::AtomicBool>,
        }

        #[async_trait]
        impl ControlChannelHandler for TrackingHandler {
            async fn handle_channel(&self, _stream: Box<dyn DuplexStream>) {
                self.called.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }

        let router = ControlChannelRouter::with_handler(Box::new(TrackingHandler {
            called: called_clone,
        }));
        assert!(router.has_handler());

        let (_client, server) = duplex(64);
        let stream: Box<dyn DuplexStream> = Box::new(server);
        let result = router.route(stream).await;
        assert!(result.is_ok());
        assert!(called.load(std::sync::atomic::Ordering::SeqCst));
    }
}
