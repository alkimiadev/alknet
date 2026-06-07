use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Instant;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use russh::keys::ssh_key::HashAlg;
use russh::server::{Auth, Handler, Msg, Session};
use russh::Channel;
use russh::ChannelId;

use crate::config::DynamicConfig;
use crate::server::control_channel::{ControlChannelHandler, ControlChannelRouter, ALKNET_PREFIX};
use crate::server::rate_limit::{AuthAttemptLimiter, ConnectionRateLimiter};

#[derive(Debug, Clone)]
pub enum ProxyMode {
    Direct,
    Socks5(SocketAddr),
    HttpConnect(SocketAddr),
}

#[derive(Debug, Clone)]
pub struct ProxyConfig {
    pub mode: ProxyMode,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TransportKind {
    Tcp,
    Tls,
    Iroh,
}

impl std::fmt::Display for TransportKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportKind::Tcp => write!(f, "tcp"),
            TransportKind::Tls => write!(f, "tls"),
            TransportKind::Iroh => write!(f, "iroh"),
        }
    }
}

pub struct ServerHandler {
    dynamic: Arc<ArcSwap<DynamicConfig>>,
    #[allow(dead_code)]
    outbound_proxy: Option<ProxyConfig>,
    remote_addr: Option<SocketAddr>,
    control_channel_router: ControlChannelRouter,
    #[allow(dead_code)]
    transport: TransportKind,
    connection_limiter: Arc<ConnectionRateLimiter>,
    connection_allowed: bool,
    auth_limiter: AuthAttemptLimiter,
    connected_at: Instant,
}

impl ServerHandler {
    pub fn new(
        dynamic: Arc<ArcSwap<DynamicConfig>>,
        outbound_proxy: Option<ProxyConfig>,
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
            outbound_proxy,
            remote_addr,
            control_channel_router: ControlChannelRouter::without_handler(),
            transport,
            connection_limiter,
            connection_allowed: allowed,
            auth_limiter: AuthAttemptLimiter::new(max_auth_attempts),
            connected_at: Instant::now(),
        }
    }

    pub fn is_connection_allowed(&self) -> bool {
        self.connection_allowed
    }

    pub fn remote_ip(&self) -> Option<IpAddr> {
        self.remote_addr.map(|a| a.ip())
    }
}

impl Drop for ServerHandler {
    fn drop(&mut self) {
        if let Some(addr) = self.remote_addr {
            if self.connection_allowed {
                self.connection_limiter.on_disconnect(addr.ip());
            }
            let duration = self.connected_at.elapsed();
            tracing::info!(
                remote_addr = %addr,
                duration_secs = duration.as_secs_f64(),
                "connection closed"
            );
        }
    }
}

impl ServerHandler {
    pub fn with_control_channel_handler(mut self, handler: Box<dyn ControlChannelHandler>) -> Self {
        self.control_channel_router = ControlChannelRouter::with_handler(handler);
        self
    }

    pub fn control_channel_router(&self) -> &ControlChannelRouter {
        &self.control_channel_router
    }
}

#[async_trait]
impl Handler for ServerHandler {
    type Error = russh::Error;

    async fn auth_publickey(
        &mut self,
        user: &str,
        public_key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<Auth, Self::Error> {
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
            return Ok(Auth::Reject {
                proceed_with_methods: None,
            });
        }

        let fingerprint = format!("{}", public_key.fingerprint(HashAlg::Sha256));
        let remote_addr_display = self
            .remote_addr
            .map_or("unknown".to_string(), |a| a.to_string());

        let russh_pub = russh::keys::PublicKey::new(public_key.key_data().clone(), user);
        let auth_config = self.dynamic.load();
        let result = auth_config.auth.authenticate_publickey(&russh_pub);

        match result {
            Ok(()) => {
                tracing::info!(
                    remote_addr = %remote_addr_display,
                    user = user,
                    key_fingerprint = %fingerprint,
                    result = "accept",
                    "auth attempt"
                );
                Ok(Auth::Accept)
            }
            Err(_) => {
                self.auth_limiter.on_failure();
                tracing::info!(
                    remote_addr = %remote_addr_display,
                    user = user,
                    key_fingerprint = %fingerprint,
                    result = "reject",
                    "auth attempt"
                );
                Ok(Auth::Reject {
                    proceed_with_methods: None,
                })
            }
        }
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host_to_connect: &str,
        port_to_connect: u32,
        originator_address: &str,
        originator_port: u32,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        if host_to_connect.starts_with(ALKNET_PREFIX) {
            if !self.control_channel_router.has_handler() {
                return Ok(false);
            }

            let _ = channel;
            return Ok(true);
        }

        let target_host = host_to_connect.to_string();
        let target_port = port_to_connect;
        let proxy_config = self.outbound_proxy.clone().unwrap_or(ProxyConfig {
            mode: ProxyMode::Direct,
        });

        tokio::spawn(async move {
            let target =
                match format!("{target_host}:{target_port}").parse::<std::net::SocketAddr>() {
                    Ok(addr) => addr,
                    Err(_) => match tokio::net::lookup_host((&target_host[..], target_port as u16))
                        .await
                    {
                        Ok(mut addrs) => match addrs.next() {
                            Some(addr) => addr,
                            None => return,
                        },
                        Err(_) => return,
                    },
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
        _channel: Channel<Msg>,
        session: &mut Session,
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
        _channel: Channel<Msg>,
        _originator_address: &str,
        _originator_port: u32,
        session: &mut Session,
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
        _channel: Channel<Msg>,
        host_to_connect: &str,
        port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        session: &mut Session,
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
        session: &mut Session,
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
        session: &mut Session,
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
        session: &mut Session,
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
        session: &mut Session,
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
        session: &mut Session,
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
        session: &mut Session,
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
        session: &mut Session,
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
        session: &mut Session,
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
        session: &mut Session,
    ) -> Result<bool, Self::Error> {
        let _ = (address, port, session);
        Ok(false)
    }

    async fn streamlocal_forward(
        &mut self,
        socket_path: &str,
        session: &mut Session,
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
        session: &mut Session,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::keys::KeySource;
    use crate::auth::ServerAuthConfig;
    use crate::config::AuthPolicy;
    use russh::keys::{decode_secret_key, PrivateKey};
    use std::io::Write;

    const ED25519_PRIVATE_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAACBOfInDyRS33JEeDNT8xd10qRdwFN8z/QukCOgEIkv01QAAAJiQ+NvMkPjb\nzAAAAAtzc2gtZWQyNTUxOQAAACBOfInDyRS33JEeDNT8xd10qRdwFN8z/QukCOgEIkv01Q\nAAAECIWwJf7+7MOuZAOOWmoQbE9i/5GxjKsFrtJHjZ34E/fk58icPJFLfckR4M1PzF3XSp\nF3AU3zP9C6QI6AQiS/TVAAAAD3VidW50dUBuczUyODA5NgECAwQFBg==\n-----END OPENSSH PRIVATE KEY-----\n";

    const ED25519_PUBLIC_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIE58icPJFLfckR4M1PzF3XSpF3AU3zP9C6QI6AQiS/TV ubuntu@ns528096";

    fn make_authorized_keys_file(keys_content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(keys_content.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    fn load_key() -> PrivateKey {
        decode_secret_key(ED25519_PRIVATE_KEY, None).unwrap()
    }

    fn make_auth_config(keys_content: &str) -> Arc<ArcSwap<DynamicConfig>> {
        let f = make_authorized_keys_file(keys_content);
        let server_auth =
            ServerAuthConfig::from_keys_and_ca(Some(KeySource::File(f.path().to_path_buf())), None)
                .unwrap();
        let auth_policy = AuthPolicy::from_server_auth_config(server_auth);
        let dynamic = DynamicConfig::new(auth_policy);
        Arc::new(ArcSwap::new(Arc::new(dynamic)))
    }

    fn make_empty_auth_config() -> Arc<ArcSwap<DynamicConfig>> {
        let dynamic = DynamicConfig::default();
        Arc::new(ArcSwap::new(Arc::new(dynamic)))
    }

    fn default_limiter() -> Arc<ConnectionRateLimiter> {
        Arc::new(ConnectionRateLimiter::new(0))
    }

    fn make_handler(
        dynamic: Arc<ArcSwap<DynamicConfig>>,
        outbound_proxy: Option<ProxyConfig>,
        remote_addr: Option<SocketAddr>,
    ) -> ServerHandler {
        ServerHandler::new(
            dynamic,
            outbound_proxy,
            remote_addr,
            TransportKind::Tcp,
            default_limiter(),
            10,
        )
    }

    #[tokio::test]
    async fn auth_delegation_accepts_known_key() {
        let auth_config = make_auth_config(ED25519_PUBLIC_KEY);
        let mut handler = make_handler(auth_config, None, None);

        let ssh_key = load_key().public_key().clone();
        let result = handler.auth_publickey("testuser", &ssh_key).await.unwrap();
        assert_eq!(result, Auth::Accept);
    }

    #[tokio::test]
    async fn auth_delegation_rejects_unknown_key() {
        let auth_config = make_auth_config(ED25519_PUBLIC_KEY);
        let mut handler = make_handler(auth_config, None, None);

        let other_key_text = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHeLC1lWiCYrXsf/85O/pkbUFZ6OGIt49PX3nw8iRoXE other@host";
        let other_ssh_key =
            russh::keys::parse_public_key_base64(other_key_text.split_whitespace().nth(1).unwrap())
                .unwrap();

        let result = handler
            .auth_publickey("testuser", &other_ssh_key)
            .await
            .unwrap();
        assert_eq!(
            result,
            Auth::Reject {
                proceed_with_methods: None
            }
        );
    }

    #[tokio::test]
    async fn auth_delegation_empty_config_rejects_all() {
        let auth_config = make_empty_auth_config();
        let mut handler = make_handler(auth_config, None, None);

        let ssh_key = load_key().public_key().clone();
        let result = handler.auth_publickey("testuser", &ssh_key).await.unwrap();
        assert_eq!(
            result,
            Auth::Reject {
                proceed_with_methods: None
            }
        );
    }

    #[tokio::test]
    async fn auth_logging_includes_remote_addr() {
        let auth_config = make_auth_config(ED25519_PUBLIC_KEY);
        let remote_addr: SocketAddr = "203.0.113.50:12345".parse().unwrap();
        let mut handler = make_handler(auth_config, None, Some(remote_addr));

        let ssh_key = load_key().public_key().clone();
        let _ = handler.auth_publickey("root", &ssh_key).await.unwrap();
    }

    #[test]
    fn reserved_alknet_destination_routing() {
        use crate::server::control_channel::is_reserved_destination;
        assert!(is_reserved_destination("alknet-control"));
        assert!(is_reserved_destination("alknet-status"));
        assert!(is_reserved_destination("alknet-events"));
        assert!(!is_reserved_destination("example.com"));
        assert!(!is_reserved_destination("localhost"));
        assert!(!is_reserved_destination("alknet.example.com"));
    }

    #[test]
    fn server_handler_without_control_handler_rejects_alknet_destinations() {
        let auth_config = make_empty_auth_config();
        let handler = make_handler(auth_config, None, None);
        assert!(!handler.control_channel_router().has_handler());
    }

    #[test]
    fn proxy_mode_variants() {
        let direct = ProxyMode::Direct;
        let socks5 = ProxyMode::Socks5("127.0.0.1:9050".parse().unwrap());
        let http = ProxyMode::HttpConnect("127.0.0.1:8080".parse().unwrap());

        match direct {
            ProxyMode::Direct => {}
            _ => panic!("expected Direct"),
        }
        match socks5 {
            ProxyMode::Socks5(_) => {}
            _ => panic!("expected Socks5"),
        }
        match http {
            ProxyMode::HttpConnect(_) => {}
            _ => panic!("expected HttpConnect"),
        }
    }

    #[test]
    fn server_handler_holds_config() {
        let auth_config = make_empty_auth_config();
        let proxy = Some(ProxyConfig {
            mode: ProxyMode::Socks5("127.0.0.1:9050".parse().unwrap()),
        });
        let remote: Option<SocketAddr> = Some("10.0.0.1:22".parse().unwrap());

        let handler = make_handler(auth_config, proxy.clone(), remote);
        assert!(handler.outbound_proxy.is_some());
        assert!(handler.remote_addr.is_some());
    }

    #[test]
    fn one_handler_per_connection() {
        let auth_config = make_empty_auth_config();
        let handler1 = make_handler(
            auth_config.clone(),
            None,
            Some("10.0.0.1:22".parse().unwrap()),
        );
        let handler2 = make_handler(
            auth_config.clone(),
            None,
            Some("10.0.0.2:22".parse().unwrap()),
        );

        assert!(handler1.remote_addr != handler2.remote_addr);
    }

    #[tokio::test]
    async fn auth_rate_limit_rejects_after_max_failures() {
        let auth_config = make_empty_auth_config();
        let limiter = Arc::new(ConnectionRateLimiter::new(0));
        let mut handler = ServerHandler::new(
            auth_config,
            None,
            Some("10.0.0.1:22".parse().unwrap()),
            TransportKind::Tcp,
            limiter,
            2,
        );

        let ssh_key = load_key().public_key().clone();

        let r1 = handler.auth_publickey("user", &ssh_key).await.unwrap();
        assert_eq!(
            r1,
            Auth::Reject {
                proceed_with_methods: None
            }
        );

        let r2 = handler.auth_publickey("user", &ssh_key).await.unwrap();
        assert_eq!(
            r2,
            Auth::Reject {
                proceed_with_methods: None
            }
        );

        assert!(!handler.auth_limiter.check());
    }

    #[test]
    fn connection_rate_limit_blocks_over_limit() {
        let limiter = Arc::new(ConnectionRateLimiter::new(1));
        let auth_config = make_empty_auth_config();
        let addr: SocketAddr = "10.0.0.1:22".parse().unwrap();

        let h1 = ServerHandler::new(
            auth_config.clone(),
            None,
            Some(addr),
            TransportKind::Tcp,
            limiter.clone(),
            10,
        );
        assert!(h1.is_connection_allowed());

        let h2 = ServerHandler::new(
            auth_config.clone(),
            None,
            Some(addr),
            TransportKind::Tcp,
            limiter.clone(),
            10,
        );
        assert!(!h2.is_connection_allowed());

        drop(h1);

        let h3 = ServerHandler::new(
            auth_config,
            None,
            Some(addr),
            TransportKind::Tcp,
            limiter,
            10,
        );
        assert!(h3.is_connection_allowed());
    }

    #[test]
    fn transport_kind_display() {
        assert_eq!(TransportKind::Tcp.to_string(), "tcp");
        assert_eq!(TransportKind::Tls.to_string(), "tls");
        assert_eq!(TransportKind::Iroh.to_string(), "iroh");
    }

    #[tokio::test]
    async fn auth_log_includes_user_field() {
        let auth_config = make_empty_auth_config();
        let mut handler = ServerHandler::new(
            auth_config,
            None,
            Some("203.0.113.50:12345".parse().unwrap()),
            TransportKind::Tls,
            Arc::new(ConnectionRateLimiter::new(0)),
            10,
        );

        let ssh_key = load_key().public_key().clone();
        let _ = handler.auth_publickey("root", &ssh_key).await.unwrap();
    }

    #[test]
    fn connection_closed_logs_duration_on_drop() {
        let auth_config = make_empty_auth_config();
        let _handler = ServerHandler::new(
            auth_config,
            None,
            Some("203.0.113.50:12345".parse().unwrap()),
            TransportKind::Tcp,
            Arc::new(ConnectionRateLimiter::new(0)),
            10,
        );
    }
}
