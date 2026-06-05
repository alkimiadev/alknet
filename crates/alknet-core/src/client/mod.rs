//! Client-side SSH session management.
//!
//! Provides `ClientSession` for establishing an SSH connection over any transport,
//! running a local SOCKS5 proxy, and managing port forwards. Also provides
//! `ChannelManager` for programmatic channel management with automatic reconnection.
//!
//! The client always starts a SOCKS5 proxy (default `127.0.0.1:1080`) when running
//! via `ClientSession::run()`. For VPN-like "route all traffic" behavior, use
//! [tun2proxy](https://github.com/tun2proxy/tun2proxy) alongside the SOCKS5 proxy.

pub mod channel_manager;
pub mod connect;
pub mod forward;

pub use channel_manager::{ChannelManager, ForwardRequest};
pub use connect::{ClientSession, ConnectError, ConnectOptions, TransportMode};
pub use forward::{LocalForwarder, PortForwardSpec, PortForwardSpecKind, RemoteForwarder};