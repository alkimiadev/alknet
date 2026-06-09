//! Server-side SSH connection handling.
//!
//! Provides `Server` for accepting SSH connections over any transport and proxying
//! `direct-tcpip` channel requests to targets. Supports Ed25519 and certificate-authority
//! auth, connection rate limiting, auth attempt limiting, stealth mode (fake nginx 404),
//! and outbound proxy routing (direct/SOCKS5/HTTP CONNECT).
//!
//! Destination hosts starting with `alknet-` are reserved for internal use (control channel, ADR-018).

pub mod channel_proxy;
pub mod control_channel;
pub mod handler;
pub mod rate_limit;
pub mod serve;
pub mod stealth;

pub use channel_proxy::{connect_outbound, proxy_channel};
pub use control_channel::{
    is_reserved_destination, ControlChannelHandler, ControlChannelRouter, DuplexStream,
    ALKNET_CONTROL_DESTINATION, ALKNET_PREFIX,
};
pub use handler::{ProxyConfig, ProxyMode, ServerHandler};
pub use rate_limit::{AuthAttemptLimiter, ConnectionRateLimiter};
pub use serve::{
    DnsListenerConfig, HttpListenerConfig, ListenerConfig, ServeError, ServeOptions,
    ServeTransportMode, Server, StreamListenerConfig,
};

pub use crate::transport::TransportKind;
pub use stealth::{
    detect_protocol, send_fake_nginx_404, validate_stealth_config, ProtocolDetection,
};
