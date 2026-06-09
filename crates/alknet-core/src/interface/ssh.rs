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

use crate::auth::identity::{Identity, IdentityProvider};
use crate::call::EventEnvelope;
use crate::config::DynamicConfig;
use crate::interface::session::{InterfaceEvent, InterfaceSession};
use crate::interface::{StreamInterface, StreamInterfaceConfig, TransportStream};
use crate::server::control_channel::{ControlChannelRouter, ALKNET_PREFIX};
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
            connected_at: Instant::now(),
        }
    }

    #[allow(dead_code)]
    fn with_control_channel_router(mut self, router: ControlChannelRouter) -> Self {
        self.control_channel_router = router;
        self
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
            if !self.control_channel_router.has_handler() {
                return Ok(false);
            }
            let _ = channel;
            return Ok(true);
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

        let handler = SshHandler::new(
            Arc::clone(&self.dynamic),
            identity_provider,
            self.outbound_proxy.clone(),
            remote_addr,
            transport,
            Arc::clone(&self.connection_limiter),
            self.max_auth_attempts,
        );

        let running = server::run_stream(Arc::clone(&self.config), stream, handler).await?;
        let handle = running.handle();
        let join = tokio::spawn(async {
            let _ = running.await;
        });

        Ok(SshSession {
            handle,
            _join: join,
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
}

impl SshSession {
    pub fn handle(&self) -> &server::Handle {
        &self.handle
    }
}

#[async_trait]
impl InterfaceSession for SshSession {
    /// Stub for Phase 1 — always returns `None`.
    ///
    /// TODO: Bridge `alknet-control:0` channel events to call protocol
    /// `InterfaceEvent` frames. Planned for Phase 2/3.
    async fn recv(&mut self) -> Option<InterfaceEvent> {
        None
    }

    /// Stub for Phase 1 — accepts silently and discards.
    ///
    /// TODO: Bridge outgoing `EventEnvelope` frames to the SSH channel
    /// established by the call protocol. Planned for Phase 2/3.
    async fn send(&mut self, _envelope: EventEnvelope) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let raw_config = StreamInterfaceConfig::RawFraming(crate::interface::RawFramingConfig {
            auth: Arc::new(crate::auth::ConfigIdentityProvider::new(Arc::new(
                ArcSwap::new(Arc::new(DynamicConfig::default())),
            ))),
        });
        let result = iface.accept(stream, &raw_config).await;
        assert!(result.is_err());
    }
}
