//! NAPI `serve()` function and `AlknetServer` type.
//!
//! Starts an SSH server that emits new channel streams via a
//! `ThreadsafeFunction` callback. Supports TCP, TLS, and iroh transports.
//!
//! Dynamic configuration reload is supported via `reloadAuth()`, `reloadForwarding()`,
//! and `reloadAll()` methods on `AlknetServer`. All swaps are atomic via ArcSwap —
//! existing connections continue with their current config, new connections get new config.

use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;

use arc_swap::ArcSwap;
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;
use russh::keys::ssh_key::HashAlg;
use russh::server;
use russh::Channel;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;

use alknet_core::auth::identity::{ConfigIdentityProvider, Identity, IdentityProvider};
use alknet_core::auth::keys::KeySource;
use alknet_core::auth::server_auth::ServerAuthConfig;
use alknet_core::config::dynamic_config::{AuthPolicy, DynamicConfig};
use alknet_core::config::forwarding::{
    ForwardingAction, ForwardingPolicy, ForwardingRule, TargetPattern,
};
use alknet_core::config::ConfigReloadHandle;
use alknet_core::server::rate_limit::{AuthAttemptLimiter, ConnectionRateLimiter};
use alknet_core::server::serve::{ServeOptions, ServeTransportMode, Server};
use alknet_core::transport::{TcpAcceptor, TransportAcceptor, TransportKind};

#[napi(object)]
pub struct AlknetServeOptions {
    pub transport: String,
    pub host_key: Option<Either<String, Buffer>>,
    pub authorized_keys: Option<Either<String, Buffer>>,
    pub cert_authority: Option<Either<String, Buffer>>,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
    pub acme_domain: Option<String>,
    pub listen: Option<String>,
    pub iroh_relay: Option<String>,
    pub proxy: Option<String>,
}

#[napi(object)]
pub struct AuthConfigNapi {
    pub authorized_keys: Option<Buffer>,
    pub cert_authority: Option<Buffer>,
}

#[napi(object)]
pub struct ForwardingRuleConfig {
    pub target: String,
    pub action: String,
    pub principals: Option<Vec<String>>,
}

#[napi(object)]
pub struct ForwardingPolicyConfig {
    pub default: String,
    pub rules: Option<Vec<ForwardingRuleConfig>>,
}

fn resolve_key_source(
    key: &Option<Either<String, Buffer>>,
    field: &str,
) -> napi::Result<KeySource> {
    match key {
        None => Err(napi::Error::new(
            napi::Status::InvalidArg,
            format!(
                "{} is required: provide a file path (string) or key data (Buffer)",
                field
            ),
        )),
        Some(Either::A(path)) => Ok(KeySource::File(path.into())),
        Some(Either::B(buf)) => Ok(KeySource::Memory(buf.to_vec())),
    }
}

fn resolve_optional_key_source(key: &Option<Either<String, Buffer>>) -> Option<KeySource> {
    match key {
        None => None,
        Some(Either::A(path)) => Some(KeySource::File(path.into())),
        Some(Either::B(buf)) => Some(KeySource::Memory(buf.to_vec())),
    }
}

fn parse_addr(addr_str: &str) -> napi::Result<SocketAddr> {
    addr_str.parse().map_err(|e| {
        napi::Error::new(
            napi::Status::InvalidArg,
            format!("invalid address '{}': {}", addr_str, e),
        )
    })
}

fn parse_forwarding_action(action: &str) -> napi::Result<ForwardingAction> {
    match action.to_lowercase().as_str() {
        "allow" => Ok(ForwardingAction::Allow),
        "deny" => Ok(ForwardingAction::Deny),
        other => Err(napi::Error::new(
            napi::Status::InvalidArg,
            format!(
                "invalid forwarding action '{}'; expected 'allow' or 'deny'",
                other
            ),
        )),
    }
}

fn parse_target_pattern(target: &str) -> napi::Result<TargetPattern> {
    if target == "*" {
        return Ok(TargetPattern::Any);
    }
    if target.starts_with("alknet-") {
        return Ok(TargetPattern::AlknetPrefix);
    }
    if let Some(colon_pos) = target.rfind(':') {
        let host_part = &target[..colon_pos];
        let port_part = &target[colon_pos + 1..];
        if port_part == "*" {
            return Ok(TargetPattern::Host(host_part.to_string()));
        }
        if let Some(dash_pos) = port_part.find('-') {
            let start_str = &port_part[..dash_pos];
            let end_str = &port_part[dash_pos + 1..];
            if let (Ok(start), Ok(end)) = (start_str.parse::<u16>(), end_str.parse::<u16>()) {
                return Ok(TargetPattern::PortRange(host_part.to_string(), start..end));
            }
        }
    }
    if let Ok(network) = ipnetwork::IpNetwork::from_str(target) {
        return Ok(TargetPattern::Cidr(network));
    }
    Ok(TargetPattern::Host(target.to_string()))
}

fn build_forwarding_policy(config: &ForwardingPolicyConfig) -> napi::Result<ForwardingPolicy> {
    let default = parse_forwarding_action(&config.default)?;
    let mut rules = Vec::new();
    if let Some(ref rule_configs) = config.rules {
        for rc in rule_configs {
            let target = parse_target_pattern(&rc.target)?;
            let action = parse_forwarding_action(&rc.action)?;
            let principals = rc.principals.clone().unwrap_or_default();
            rules.push(ForwardingRule::new(target, action, principals, vec![]));
        }
    }
    Ok(ForwardingPolicy { default, rules })
}

fn build_auth_policy_from_napi(auth: &AuthConfigNapi) -> napi::Result<AuthPolicy> {
    let authorized_keys_source = auth
        .authorized_keys
        .as_ref()
        .map(|buf| KeySource::Memory(buf.to_vec()));
    let cert_authority_source = auth
        .cert_authority
        .as_ref()
        .map(|buf| KeySource::Memory(buf.to_vec()));

    let server_auth_config =
        ServerAuthConfig::from_keys_and_ca(authorized_keys_source, cert_authority_source).map_err(
            |e| {
                napi::Error::new(
                    napi::Status::InvalidArg,
                    format!("auth config error: {}", e),
                )
            },
        )?;

    Ok(AuthPolicy::from_server_auth_config(server_auth_config))
}

#[napi(object)]
pub struct ConnectionInfo {
    pub remote_addr: Option<String>,
    pub transport_kind: String,
}

#[napi]
pub struct AlknetServerStream {
    read: Arc<Mutex<tokio::io::ReadHalf<russh::ChannelStream<server::Msg>>>>,
    write: Arc<Mutex<tokio::io::WriteHalf<russh::ChannelStream<server::Msg>>>>,
}

#[napi]
impl AlknetServerStream {
    #[napi]
    pub async fn read(&self, size: u32) -> napi::Result<Buffer> {
        let mut buf = vec![0u8; size as usize];
        let mut guard = self.read.lock().await;
        let n = guard.read(&mut buf).await.map_err(|e| {
            napi::Error::new(napi::Status::GenericFailure, format!("read failed: {}", e))
        })?;
        if n == 0 {
            return Ok(Vec::<u8>::new().into());
        }
        buf.truncate(n);
        Ok(buf.into())
    }

    #[napi]
    pub async fn write(&self, data: Buffer) -> napi::Result<()> {
        let mut guard = self.write.lock().await;
        guard.write_all(&data).await.map_err(|e| {
            napi::Error::new(napi::Status::GenericFailure, format!("write failed: {}", e))
        })?;
        Ok(())
    }

    #[napi]
    pub async fn close(&self) -> napi::Result<()> {
        let mut guard = self.write.lock().await;
        guard.shutdown().await.map_err(|e| {
            napi::Error::new(napi::Status::GenericFailure, format!("close failed: {}", e))
        })
    }
}

struct NapiServerHandler {
    dynamic: Arc<ArcSwap<DynamicConfig>>,
    identity_provider: Arc<dyn IdentityProvider>,
    transport: TransportKind,
    remote_addr: Option<SocketAddr>,
    connection_limiter: Arc<ConnectionRateLimiter>,
    connection_allowed: bool,
    auth_limiter: AuthAttemptLimiter,
    authenticated_identity: Option<Identity>,
    channel_sender: Arc<Mutex<Option<tokio::sync::mpsc::UnboundedSender<Channel<server::Msg>>>>>,
}

impl NapiServerHandler {
    fn new(
        dynamic: Arc<ArcSwap<DynamicConfig>>,
        remote_addr: Option<SocketAddr>,
        transport: TransportKind,
        connection_limiter: Arc<ConnectionRateLimiter>,
        max_auth_attempts: usize,
        channel_sender: Arc<
            Mutex<Option<tokio::sync::mpsc::UnboundedSender<Channel<server::Msg>>>>,
        >,
    ) -> Self {
        let identity_provider: Arc<dyn IdentityProvider> =
            Arc::new(ConfigIdentityProvider::new(Arc::clone(&dynamic)));

        let allowed = if let Some(addr) = remote_addr {
            let ip = addr.ip();
            if connection_limiter.check(ip) {
                connection_limiter.on_connect(ip);
                true
            } else {
                false
            }
        } else {
            true
        };

        Self {
            dynamic,
            identity_provider,
            transport,
            remote_addr,
            connection_limiter,
            connection_allowed: allowed,
            auth_limiter: AuthAttemptLimiter::new(max_auth_attempts),
            authenticated_identity: None,
            channel_sender,
        }
    }

    fn is_connection_allowed(&self) -> bool {
        self.connection_allowed
    }
}

impl Drop for NapiServerHandler {
    fn drop(&mut self) {
        if let Some(addr) = self.remote_addr {
            if self.connection_allowed {
                self.connection_limiter.on_disconnect(addr.ip());
            }
        }
    }
}

#[async_trait::async_trait]
impl russh::server::Handler for NapiServerHandler {
    type Error = russh::Error;

    async fn auth_publickey(
        &mut self,
        _user: &str,
        public_key: &russh::keys::ssh_key::PublicKey,
    ) -> std::result::Result<russh::server::Auth, Self::Error> {
        if !self.auth_limiter.check() {
            return Ok(russh::server::Auth::Reject {
                proceed_with_methods: None,
            });
        }

        let fingerprint = format!("{}", public_key.fingerprint(HashAlg::Sha256));
        let identity = self
            .identity_provider
            .resolve_from_fingerprint(&fingerprint);

        match identity {
            Some(id) => {
                self.authenticated_identity = Some(id);
                Ok(russh::server::Auth::Accept)
            }
            None => {
                self.auth_limiter.on_failure();
                Ok(russh::server::Auth::Reject {
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
        _originator_address: &str,
        _originator_port: u32,
        _session: &mut russh::server::Session,
    ) -> std::result::Result<bool, Self::Error> {
        if host_to_connect.starts_with("alknet-") {
            let guard = self.channel_sender.lock().await;
            if let Some(ref tx) = *guard {
                let _ = tx.send(channel);
            }
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
                target = %format!("{host_to_connect}:{port_to_connect}"),
                identity = %identity.id,
                transport = %self.transport,
                "forwarding denied by policy"
            );
            let _ = channel;
            return Ok(false);
        }

        let _ = channel;
        Ok(true)
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<server::Msg>,
        _session: &mut russh::server::Session,
    ) -> std::result::Result<bool, Self::Error> {
        tracing::warn!("rejected session channel (shell/exec not supported)");
        Ok(false)
    }

    async fn channel_open_x11(
        &mut self,
        _channel: Channel<server::Msg>,
        _originator_address: &str,
        _originator_port: u32,
        _session: &mut russh::server::Session,
    ) -> std::result::Result<bool, Self::Error> {
        tracing::warn!("rejected x11 channel");
        Ok(false)
    }

    async fn channel_open_forwarded_tcpip(
        &mut self,
        _channel: Channel<server::Msg>,
        host_to_connect: &str,
        port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        _session: &mut russh::server::Session,
    ) -> std::result::Result<bool, Self::Error> {
        tracing::warn!(
            target = %format!("{host_to_connect}:{port_to_connect}"),
            "rejected forwarded-tcpip channel"
        );
        Ok(false)
    }

    async fn exec_request(
        &mut self,
        channel: russh::ChannelId,
        data: &[u8],
        session: &mut russh::server::Session,
    ) -> std::result::Result<(), Self::Error> {
        tracing::warn!(channel = %channel, data_len = data.len(), "rejected exec request");
        let _ = session.channel_failure(channel);
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: russh::ChannelId,
        session: &mut russh::server::Session,
    ) -> std::result::Result<(), Self::Error> {
        tracing::warn!(channel = %channel, "rejected shell request");
        let _ = session.channel_failure(channel);
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel: russh::ChannelId,
        name: &str,
        session: &mut russh::server::Session,
    ) -> std::result::Result<(), Self::Error> {
        tracing::warn!(channel = %channel, subsystem = name, "rejected subsystem request");
        let _ = session.channel_failure(channel);
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: russh::ChannelId,
        term: &str,
        col_width: u32,
        row_height: u32,
        pix_width: u32,
        pix_height: u32,
        modes: &[(russh::Pty, u32)],
        session: &mut russh::server::Session,
    ) -> std::result::Result<(), Self::Error> {
        tracing::warn!(channel = %channel, term = term, "rejected pty request");
        let _ = (col_width, row_height, pix_width, pix_height, modes);
        let _ = session.channel_failure(channel);
        Ok(())
    }

    async fn env_request(
        &mut self,
        channel: russh::ChannelId,
        variable_name: &str,
        variable_value: &str,
        session: &mut russh::server::Session,
    ) -> std::result::Result<(), Self::Error> {
        tracing::warn!(channel = %channel, variable = variable_name, "rejected env request");
        let _ = variable_value;
        let _ = session.channel_failure(channel);
        Ok(())
    }

    async fn x11_request(
        &mut self,
        channel: russh::ChannelId,
        single_connection: bool,
        x11_auth_protocol: &str,
        x11_auth_cookie: &str,
        x11_screen_number: u32,
        session: &mut russh::server::Session,
    ) -> std::result::Result<(), Self::Error> {
        tracing::warn!(channel = %channel, "rejected x11 request");
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
        channel: russh::ChannelId,
        _session: &mut russh::server::Session,
    ) -> std::result::Result<bool, Self::Error> {
        tracing::warn!(channel = %channel, "rejected agent forwarding request");
        Ok(false)
    }

    async fn tcpip_forward(
        &mut self,
        address: &str,
        port: &mut u32,
        _session: &mut russh::server::Session,
    ) -> std::result::Result<bool, Self::Error> {
        tracing::warn!(
            address = address,
            port = *port,
            "rejected tcpip-forward request"
        );
        Ok(false)
    }

    async fn cancel_tcpip_forward(
        &mut self,
        address: &str,
        port: u32,
        _session: &mut russh::server::Session,
    ) -> std::result::Result<bool, Self::Error> {
        let _ = (address, port);
        Ok(false)
    }

    async fn streamlocal_forward(
        &mut self,
        socket_path: &str,
        _session: &mut russh::server::Session,
    ) -> std::result::Result<bool, Self::Error> {
        tracing::warn!(
            socket_path = socket_path,
            "rejected streamlocal-forward request"
        );
        Ok(false)
    }

    async fn signal(
        &mut self,
        channel: russh::ChannelId,
        signal: russh::Sig,
        _session: &mut russh::server::Session,
    ) -> std::result::Result<(), Self::Error> {
        tracing::debug!(channel = %channel, signal = ?signal, "received signal (ignored)");
        Ok(())
    }
}

type ServerTsfn = ThreadsafeFunction<ConnectionEventWrapper, (), ConnectionEventWrapper>;

#[napi]
pub struct AlknetServer {
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    listen_addr: String,
    endpoint_id: Option<String>,
    on_connection_tsfn: Arc<Mutex<Option<ServerTsfn>>>,
    reload_handle: ConfigReloadHandle,
}

struct ConnectionEventWrapper {
    stream: AlknetServerStream,
    info: ConnectionInfo,
}

impl ToNapiValue for ConnectionEventWrapper {
    unsafe fn to_napi_value(
        env: napi::sys::napi_env,
        val: Self,
    ) -> napi::Result<napi::sys::napi_value> {
        let mut raw_obj: napi::sys::napi_value = std::ptr::null_mut();
        napi::check_status!(
            napi::sys::napi_create_object(env, &mut raw_obj),
            "Failed to create object"
        )?;

        let stream_val = <AlknetServerStream as ToNapiValue>::to_napi_value(env, val.stream)?;
        let key_stream = std::ffi::CString::new("stream").unwrap();
        napi::check_status!(
            napi::sys::napi_set_named_property(env, raw_obj, key_stream.as_ptr(), stream_val),
            "Failed to set stream property"
        )?;

        let info_val = <ConnectionInfo as ToNapiValue>::to_napi_value(env, val.info)?;
        let key_info = std::ffi::CString::new("info").unwrap();
        napi::check_status!(
            napi::sys::napi_set_named_property(env, raw_obj, key_info.as_ptr(), info_val),
            "Failed to set info property"
        )?;

        Ok(raw_obj)
    }
}

impl TypeName for ConnectionEventWrapper {
    fn type_name() -> &'static str {
        "ConnectionEventWrapper"
    }

    fn value_type() -> napi::ValueType {
        napi::ValueType::Object
    }
}

impl ValidateNapiValue for ConnectionEventWrapper {}

#[napi]
impl AlknetServer {
    #[napi]
    pub async fn close(&self) -> napi::Result<()> {
        let _ = self.shutdown_tx.send(true);
        Ok(())
    }

    #[napi(ts_return_type = "void")]
    pub fn on_connection(&self, callback: Function<(), ()>) -> napi::Result<()> {
        let tsfn = callback
            .build_threadsafe_function::<ConnectionEventWrapper>()
            .callee_handled::<true>()
            .build_callback(|ctx| Ok(ctx.value))?;

        let holder = self.on_connection_tsfn.clone();
        *holder.blocking_lock() = Some(tsfn);
        Ok(())
    }

    #[napi(getter)]
    pub fn listen_addr(&self) -> napi::Result<String> {
        Ok(self.listen_addr.clone())
    }

    #[napi(getter, ts_return_type = "string | null")]
    pub fn endpoint_id(&self) -> napi::Result<Option<String>> {
        Ok(self.endpoint_id.clone())
    }

    #[napi]
    pub fn reload_auth(&self, auth: AuthConfigNapi) -> napi::Result<()> {
        let new_auth_policy = build_auth_policy_from_napi(&auth)?;
        let current = self.reload_handle.dynamic();
        let new_config = DynamicConfig::from_parts(
            new_auth_policy,
            current.forwarding.clone(),
            current.rate_limits.clone(),
        );
        self.reload_handle.reload(new_config);
        Ok(())
    }

    #[napi]
    pub fn reload_forwarding(&self, policy: ForwardingPolicyConfig) -> napi::Result<()> {
        let new_forwarding = build_forwarding_policy(&policy)?;
        let current = self.reload_handle.dynamic();
        let new_config = DynamicConfig::from_parts(
            current.auth.clone(),
            new_forwarding,
            current.rate_limits.clone(),
        );
        self.reload_handle.reload(new_config);
        Ok(())
    }

    #[napi]
    pub fn reload_all(
        &self,
        auth: AuthConfigNapi,
        forwarding: ForwardingPolicyConfig,
    ) -> napi::Result<()> {
        let new_auth_policy = build_auth_policy_from_napi(&auth)?;
        let new_forwarding = build_forwarding_policy(&forwarding)?;
        let current = self.reload_handle.dynamic();
        let new_config =
            DynamicConfig::from_parts(new_auth_policy, new_forwarding, current.rate_limits.clone());
        self.reload_handle.reload(new_config);
        Ok(())
    }
}

#[napi]
pub async fn serve(options: AlknetServeOptions) -> napi::Result<AlknetServer> {
    let host_key_source = resolve_key_source(&options.host_key, "hostKey")?;
    let authorized_keys_source = resolve_optional_key_source(&options.authorized_keys);
    let cert_authority_source = resolve_optional_key_source(&options.cert_authority);

    let transport_mode = match options.transport.to_lowercase().as_str() {
        "tcp" => ServeTransportMode::Tcp,
        "tls" => ServeTransportMode::Tls,
        "iroh" => ServeTransportMode::Iroh,
        other => {
            return Err(napi::Error::new(
                napi::Status::InvalidArg,
                format!("unknown transport '{}'; expected tcp, tls, or iroh", other),
            ));
        }
    };

    let listen_addr_str = options.listen.as_deref().unwrap_or("0.0.0.0:22");

    let mut serve_opts = ServeOptions::new(host_key_source.clone())
        .transport_mode(transport_mode.clone())
        .listen_addr(listen_addr_str);

    if let Some(aks) = authorized_keys_source.clone() {
        serve_opts = serve_opts.authorized_keys(aks);
    }
    if let Some(cas) = cert_authority_source.clone() {
        serve_opts = serve_opts.cert_authority(cas);
    }
    if let Some(ref cert) = options.tls_cert {
        serve_opts = serve_opts.tls_cert(cert);
    }
    if let Some(ref key) = options.tls_key {
        serve_opts = serve_opts.tls_key(key);
    }
    if let Some(ref domain) = options.acme_domain {
        serve_opts = serve_opts.acme_domain(domain);
    }
    if let Some(ref relay) = options.iroh_relay {
        serve_opts = serve_opts.iroh_relay(relay);
    }

    let core_server = Server::new(serve_opts).map_err(|e| {
        napi::Error::new(
            napi::Status::InvalidArg,
            format!("server config error: {}", e),
        )
    })?;

    let shutdown_tx = core_server.shutdown_sender();
    let reload_handle = core_server.config_reload_handle();

    let initial_auth_policy = {
        let server_auth =
            ServerAuthConfig::from_keys_and_ca(authorized_keys_source, cert_authority_source)
                .map_err(|e| {
                    napi::Error::new(
                        napi::Status::InvalidArg,
                        format!("auth config error: {}", e),
                    )
                })?;
        AuthPolicy::from_server_auth_config(server_auth)
    };

    {
        let current = reload_handle.dynamic();
        let initialized_config = DynamicConfig::from_parts(
            initial_auth_policy,
            current.forwarding.clone(),
            current.rate_limits.clone(),
        );
        drop(current);
        reload_handle.reload(initialized_config);
    }

    let dynamic = reload_handle.dynamic_arc();

    match transport_mode {
        ServeTransportMode::Tcp => {
            let addr = parse_addr(listen_addr_str)?;
            let acceptor = TcpAcceptor::bind(addr).await.map_err(|e| {
                napi::Error::new(
                    napi::Status::GenericFailure,
                    format!("tcp bind failed: {}", e),
                )
            })?;
            let actual_listen = acceptor.listen_addr().to_string();

            let private_key = alknet_core::auth::keys::load_private_key(host_key_source.clone())
                .map_err(|e| {
                    napi::Error::new(napi::Status::InvalidArg, format!("host key error: {}", e))
                })?;

            let config = Arc::new(server::Config {
                keys: vec![private_key],
                methods: russh::MethodSet::PUBLICKEY,
                preferred: russh::Preferred::DEFAULT,
                ..Default::default()
            });

            let connection_limiter = Arc::new(ConnectionRateLimiter::new(0));
            let shutdown_rx = shutdown_tx.subscribe();
            let tsfn_holder: Arc<Mutex<Option<ServerTsfn>>> = Arc::new(Mutex::new(None));

            let tsfn_for_loop = tsfn_holder.clone();
            let transport_kind = TransportKind::Tcp;

            tokio::spawn(async move {
                run_accept_loop(
                    acceptor,
                    config,
                    dynamic,
                    connection_limiter,
                    shutdown_rx,
                    tsfn_for_loop,
                    transport_kind,
                )
                .await;
            });

            Ok(AlknetServer {
                shutdown_tx,
                listen_addr: actual_listen,
                endpoint_id: None,
                on_connection_tsfn: tsfn_holder,
                reload_handle,
            })
        }
        ServeTransportMode::Tls => {
            use alknet_core::transport::TlsAcceptor;

            let addr = parse_addr(listen_addr_str)?;

            let tls_cert_path = options.tls_cert.as_ref().ok_or_else(|| {
                napi::Error::new(
                    napi::Status::InvalidArg,
                    "tlsCert is required for TLS transport".to_string(),
                )
            })?;
            let tls_key_path = options.tls_key.as_ref().ok_or_else(|| {
                napi::Error::new(
                    napi::Status::InvalidArg,
                    "tlsKey is required for TLS transport".to_string(),
                )
            })?;

            let cert_data = std::fs::read(tls_cert_path).map_err(|e| {
                napi::Error::new(
                    napi::Status::InvalidArg,
                    format!("failed to read TLS cert '{}': {}", tls_cert_path, e),
                )
            })?;
            let key_data = std::fs::read(tls_key_path).map_err(|e| {
                napi::Error::new(
                    napi::Status::InvalidArg,
                    format!("failed to read TLS key '{}': {}", tls_key_path, e),
                )
            })?;

            let certs: Vec<rustls_pki_types::CertificateDer<'static>> =
                rustls_pemfile::certs(&mut &cert_data[..])
                    .collect::<std::result::Result<Vec<_>, std::io::Error>>()
                    .map_err(|e| {
                        napi::Error::new(
                            napi::Status::InvalidArg,
                            format!("failed to parse TLS certificates: {}", e),
                        )
                    })?;
            let key: rustls_pki_types::PrivateKeyDer<'static> =
                rustls_pemfile::private_key(&mut &key_data[..])
                    .map_err(|e| {
                        napi::Error::new(
                            napi::Status::InvalidArg,
                            format!("failed to parse TLS private key: {}", e),
                        )
                    })?
                    .ok_or_else(|| {
                        napi::Error::new(
                            napi::Status::InvalidArg,
                            format!("no private key found in {}", tls_key_path),
                        )
                    })?;

            let acceptor = TlsAcceptor::bind(addr, certs, key, None)
                .await
                .map_err(|e| {
                    napi::Error::new(
                        napi::Status::GenericFailure,
                        format!("tls bind failed: {}", e),
                    )
                })?;
            let actual_listen = acceptor.listen_addr().to_string();

            let private_key = alknet_core::auth::keys::load_private_key(host_key_source.clone())
                .map_err(|e| {
                    napi::Error::new(napi::Status::InvalidArg, format!("host key error: {}", e))
                })?;

            let config = Arc::new(server::Config {
                keys: vec![private_key],
                methods: russh::MethodSet::PUBLICKEY,
                preferred: russh::Preferred::DEFAULT,
                ..Default::default()
            });

            let connection_limiter = Arc::new(ConnectionRateLimiter::new(0));
            let shutdown_rx = shutdown_tx.subscribe();
            let tsfn_holder: Arc<Mutex<Option<ServerTsfn>>> = Arc::new(Mutex::new(None));

            let tsfn_for_loop = tsfn_holder.clone();
            let transport_kind = TransportKind::Tls { server_name: None };

            tokio::spawn(async move {
                run_accept_loop(
                    acceptor,
                    config,
                    dynamic,
                    connection_limiter,
                    shutdown_rx,
                    tsfn_for_loop,
                    transport_kind,
                )
                .await;
            });

            Ok(AlknetServer {
                shutdown_tx,
                listen_addr: actual_listen,
                endpoint_id: None,
                on_connection_tsfn: tsfn_holder,
                reload_handle,
            })
        }
        ServeTransportMode::Iroh => {
            use alknet_core::transport::IrohAcceptor;

            let relay_url: Option<iroh::RelayUrl> = match options.iroh_relay.as_deref() {
                Some(u) => Some(u.parse().map_err(|e| {
                    napi::Error::new(
                        napi::Status::InvalidArg,
                        format!("invalid iroh relay URL: {}", e),
                    )
                })?),
                None => None,
            };

            let proxy_url: Option<url::Url> = match options.proxy.as_deref() {
                Some(u) => Some(u.parse().map_err(|e| {
                    napi::Error::new(
                        napi::Status::InvalidArg,
                        format!("invalid proxy URL: {}", e),
                    )
                })?),
                None => None,
            };

            let acceptor = IrohAcceptor::bind(relay_url, proxy_url)
                .await
                .map_err(|e| {
                    napi::Error::new(
                        napi::Status::GenericFailure,
                        format!("iroh bind failed: {}", e),
                    )
                })?;

            let iroh_endpoint_id = acceptor.endpoint_id();

            let private_key =
                alknet_core::auth::keys::load_private_key(host_key_source).map_err(|e| {
                    napi::Error::new(napi::Status::InvalidArg, format!("host key error: {}", e))
                })?;

            let config = Arc::new(server::Config {
                keys: vec![private_key],
                methods: russh::MethodSet::PUBLICKEY,
                preferred: russh::Preferred::DEFAULT,
                ..Default::default()
            });

            let connection_limiter = Arc::new(ConnectionRateLimiter::new(0));
            let shutdown_rx = shutdown_tx.subscribe();
            let tsfn_holder: Arc<Mutex<Option<ServerTsfn>>> = Arc::new(Mutex::new(None));

            let tsfn_for_loop = tsfn_holder.clone();
            let transport_kind = TransportKind::Iroh {
                endpoint_id: iroh_endpoint_id.clone(),
            };

            tokio::spawn(async move {
                run_accept_loop(
                    acceptor,
                    config,
                    dynamic,
                    connection_limiter,
                    shutdown_rx,
                    tsfn_for_loop,
                    transport_kind,
                )
                .await;
            });

            Ok(AlknetServer {
                shutdown_tx,
                listen_addr: String::new(),
                endpoint_id: Some(iroh_endpoint_id),
                on_connection_tsfn: tsfn_holder,
                reload_handle,
            })
        }
    }
}

async fn run_accept_loop<A>(
    acceptor: A,
    config: Arc<server::Config>,
    dynamic: Arc<ArcSwap<DynamicConfig>>,
    connection_limiter: Arc<ConnectionRateLimiter>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    tsfn_holder: Arc<Mutex<Option<ServerTsfn>>>,
    transport_kind: TransportKind,
) where
    A: TransportAcceptor + Send + Sync + 'static,
{
    loop {
        if *shutdown_rx.borrow() {
            break;
        }

        let accept_result = tokio::select! {
            result = acceptor.accept() => result,
            _ = shutdown_rx.changed() => break,
        };

        let (stream, info) = match accept_result {
            Ok(conn) => conn,
            Err(_) => continue,
        };

        let remote_addr = info.remote_addr;
        let (channel_tx, mut channel_rx) =
            tokio::sync::mpsc::unbounded_channel::<Channel<server::Msg>>();
        let channel_sender = Arc::new(Mutex::new(Some(channel_tx)));

        let handler = NapiServerHandler::new(
            Arc::clone(&dynamic),
            remote_addr,
            transport_kind.clone(),
            Arc::clone(&connection_limiter),
            10,
            channel_sender,
        );

        if !handler.is_connection_allowed() {
            continue;
        }

        let config = Arc::clone(&config);
        let tsfn_holder = tsfn_holder.clone();
        let remote_addr_str = remote_addr.map(|a| a.to_string());
        let transport_kind_str = transport_kind.to_string();

        tokio::spawn(async move {
            let running = match server::run_stream(config, stream, handler).await {
                Ok(r) => r,
                Err(_) => return,
            };

            loop {
                let channel = channel_rx.recv().await;
                match channel {
                    Some(ch) => {
                        let channel_stream = ch.into_stream();
                        let (read_half, write_half) = tokio::io::split(channel_stream);
                        let server_stream = AlknetServerStream {
                            read: Arc::new(Mutex::new(read_half)),
                            write: Arc::new(Mutex::new(write_half)),
                        };

                        let conn_info = ConnectionInfo {
                            remote_addr: remote_addr_str.clone(),
                            transport_kind: transport_kind_str.clone(),
                        };

                        let event = ConnectionEventWrapper {
                            stream: server_stream,
                            info: conn_info,
                        };

                        let tsfn_guard = tsfn_holder.lock().await;
                        if let Some(ref tsfn) = *tsfn_guard {
                            let _ = tsfn.call(Ok(event), ThreadsafeFunctionCallMode::NonBlocking);
                        }
                    }
                    None => break,
                }
            }

            let _ = running.await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alknet_core::config::dynamic_config::RateLimitConfig;
    use russh::server::Handler;

    const ED25519_PRIVATE_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAACBOfInDyRS33JEeDNT8xd10qRdwFN8z/QukCOgEIkv01QAAAJiQ+NvMkPjb\nzAAAAAtzc2gtZWQyNTUxOQAAACBOfInDyRS33JEeDNT8xd10qRdwFN8z/QukCOgEIkv01Q\nAAAECIWwJf7+7MOuZAOOWmoQbE9i/5GxjKsFrtJHjZ34E/fk58icPJFLfckR4M1PzF3XSp\nF3AU3P9C6QI6AQiS/TVAAAAD3VidW50dUBuczUyODA5NgECAwQFBg==\n-----END OPENSSH PRIVATE KEY-----\n";

    #[test]
    fn resolve_key_source_file_path() {
        let key = Some(Either::<String, Buffer>::A("/path/to/key".to_string()));
        let result = resolve_key_source(&key, "hostKey");
        assert!(result.is_ok());
        match result.unwrap() {
            KeySource::File(p) => assert_eq!(p.to_str(), Some("/path/to/key")),
            _ => panic!("expected File variant"),
        }
    }

    #[test]
    fn resolve_key_source_buffer() {
        let key = Some(Either::<String, Buffer>::B(Buffer::from(
            ED25519_PRIVATE_KEY.as_bytes().to_vec(),
        )));
        let result = resolve_key_source(&key, "hostKey");
        assert!(result.is_ok());
        match result.unwrap() {
            KeySource::Memory(data) => assert!(!data.is_empty()),
            _ => panic!("expected Memory variant"),
        }
    }

    #[test]
    fn resolve_key_source_missing() {
        let key: Option<Either<String, Buffer>> = None;
        assert!(resolve_key_source(&key, "hostKey").is_err());
    }

    #[test]
    fn resolve_optional_key_source_none() {
        let key: Option<Either<String, Buffer>> = None;
        assert!(resolve_optional_key_source(&key).is_none());
    }

    #[test]
    fn resolve_optional_key_source_file() {
        let key = Some(Either::<String, Buffer>::A("/path/to/keys".to_string()));
        let result = resolve_optional_key_source(&key);
        assert!(result.is_some());
        match result.unwrap() {
            KeySource::File(p) => assert_eq!(p.to_str(), Some("/path/to/keys")),
            _ => panic!("expected File variant"),
        }
    }

    #[test]
    fn resolve_optional_key_source_buffer() {
        let key = Some(Either::<String, Buffer>::B(Buffer::from(
            b"keydata".to_vec(),
        )));
        let result = resolve_optional_key_source(&key);
        assert!(result.is_some());
        match result.unwrap() {
            KeySource::Memory(data) => assert_eq!(data, b"keydata".to_vec()),
            _ => panic!("expected Memory variant"),
        }
    }

    #[test]
    fn parse_addr_valid() {
        let addr = parse_addr("127.0.0.1:22");
        assert!(addr.is_ok());
        assert_eq!(addr.unwrap().port(), 22);
    }

    #[test]
    fn parse_addr_invalid() {
        assert!(parse_addr("not-an-address").is_err());
    }

    #[test]
    fn connection_info_fields() {
        let info = ConnectionInfo {
            remote_addr: Some("127.0.0.1:12345".to_string()),
            transport_kind: "tcp".to_string(),
        };
        assert_eq!(info.remote_addr, Some("127.0.0.1:12345".to_string()));
        assert_eq!(info.transport_kind, "tcp");
    }

    #[test]
    fn napi_server_handler_allows_connection() {
        let dynamic = Arc::new(ArcSwap::new(Arc::new(DynamicConfig::default())));
        let (tx, _) = tokio::sync::mpsc::unbounded_channel::<Channel<server::Msg>>();
        let handler = NapiServerHandler::new(
            dynamic,
            None,
            TransportKind::Tcp,
            Arc::new(ConnectionRateLimiter::new(0)),
            10,
            Arc::new(Mutex::new(Some(tx))),
        );
        assert!(handler.is_connection_allowed());
    }

    #[tokio::test]
    async fn napi_server_handler_rejects_unknown_key() {
        let dynamic = Arc::new(ArcSwap::new(Arc::new(DynamicConfig::default())));
        let (tx, _) = tokio::sync::mpsc::unbounded_channel::<Channel<server::Msg>>();
        let mut handler = NapiServerHandler::new(
            dynamic,
            None,
            TransportKind::Tcp,
            Arc::new(ConnectionRateLimiter::new(0)),
            10,
            Arc::new(Mutex::new(Some(tx))),
        );

        let test_key_str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHeLC1lWiCYrXsf/85O/pkbUFZ6OGIt49PX3nw8iRoXE test@host";
        let public_key =
            russh::keys::parse_public_key_base64(test_key_str.split_whitespace().nth(1).unwrap())
                .unwrap();

        let result = handler
            .auth_publickey("testuser", &public_key)
            .await
            .unwrap();
        assert_eq!(
            result,
            russh::server::Auth::Reject {
                proceed_with_methods: None
            }
        );
    }

    #[test]
    fn napi_server_handler_connection_limiter() {
        let limiter = Arc::new(ConnectionRateLimiter::new(1));
        let dynamic = Arc::new(ArcSwap::new(Arc::new(DynamicConfig::default())));
        let (tx, _) = tokio::sync::mpsc::unbounded_channel::<Channel<server::Msg>>();
        let addr: SocketAddr = "10.0.0.1:22".parse().unwrap();

        let h1 = NapiServerHandler::new(
            dynamic.clone(),
            Some(addr),
            TransportKind::Tcp,
            limiter.clone(),
            10,
            Arc::new(Mutex::new(Some(tx.clone()))),
        );
        assert!(h1.is_connection_allowed());

        let h2 = NapiServerHandler::new(
            dynamic.clone(),
            Some(addr),
            TransportKind::Tcp,
            limiter.clone(),
            10,
            Arc::new(Mutex::new(Some(tx.clone()))),
        );
        assert!(!h2.is_connection_allowed());

        drop(h1);

        let h3 = NapiServerHandler::new(
            dynamic,
            Some(addr),
            TransportKind::Tcp,
            limiter,
            10,
            Arc::new(Mutex::new(None)),
        );
        assert!(h3.is_connection_allowed());
    }

    #[test]
    fn parse_forwarding_action_allow() {
        assert_eq!(
            parse_forwarding_action("allow").unwrap(),
            ForwardingAction::Allow
        );
    }

    #[test]
    fn parse_forwarding_action_deny() {
        assert_eq!(
            parse_forwarding_action("deny").unwrap(),
            ForwardingAction::Deny
        );
    }

    #[test]
    fn parse_forwarding_action_case_insensitive() {
        assert_eq!(
            parse_forwarding_action("Allow").unwrap(),
            ForwardingAction::Allow
        );
        assert_eq!(
            parse_forwarding_action("DENY").unwrap(),
            ForwardingAction::Deny
        );
    }

    #[test]
    fn parse_forwarding_action_invalid() {
        assert!(parse_forwarding_action("block").is_err());
    }

    #[test]
    fn parse_target_pattern_wildcard() {
        assert!(matches!(
            parse_target_pattern("*").unwrap(),
            TargetPattern::Any
        ));
    }

    #[test]
    fn parse_target_pattern_alknet_prefix() {
        assert!(matches!(
            parse_target_pattern("alknet-*").unwrap(),
            TargetPattern::AlknetPrefix
        ));
    }

    #[test]
    fn parse_target_pattern_host() {
        assert!(matches!(
            parse_target_pattern("example.com").unwrap(),
            TargetPattern::Host(_)
        ));
    }

    #[test]
    fn build_forwarding_policy_deny_all() {
        let config = ForwardingPolicyConfig {
            default: "deny".to_string(),
            rules: None,
        };
        let policy = build_forwarding_policy(&config).unwrap();
        assert_eq!(policy.default, ForwardingAction::Deny);
        assert!(policy.rules.is_empty());
    }

    #[test]
    fn build_forwarding_policy_with_rules() {
        let config = ForwardingPolicyConfig {
            default: "allow".to_string(),
            rules: Some(vec![ForwardingRuleConfig {
                target: "localhost:*".to_string(),
                action: "deny".to_string(),
                principals: None,
            }]),
        };
        let policy = build_forwarding_policy(&config).unwrap();
        assert_eq!(policy.default, ForwardingAction::Allow);
        assert_eq!(policy.rules.len(), 1);
        assert_eq!(policy.rules[0].action, ForwardingAction::Deny);
    }

    #[test]
    fn build_forwarding_policy_with_principals() {
        let config = ForwardingPolicyConfig {
            default: "deny".to_string(),
            rules: Some(vec![ForwardingRuleConfig {
                target: "*".to_string(),
                action: "allow".to_string(),
                principals: Some(vec!["admin".to_string()]),
            }]),
        };
        let policy = build_forwarding_policy(&config).unwrap();
        assert_eq!(policy.rules[0].principals, vec!["admin".to_string()]);
    }

    #[test]
    fn reload_handle_swaps_config() {
        let (arc_swap, handle) = alknet_core::config::new_dynamic_config();
        let initial = arc_swap.load();
        assert_eq!(initial.forwarding.default, ForwardingAction::Allow);

        let new_config = DynamicConfig::from_parts(
            AuthPolicy::empty(),
            ForwardingPolicy::deny_all(),
            RateLimitConfig::default(),
        );
        handle.reload(new_config);

        let updated = arc_swap.load();
        assert_eq!(updated.forwarding.default, ForwardingAction::Deny);
    }
}
