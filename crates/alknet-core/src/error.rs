//! Error types for alknet-core.
//!
//! Layered error hierarchy:
//! - `TransportError` — connection/handshake/timeout errors (trigger reconnection on client)
//! - `AuthError` — key rejection, certificate validation failures
//! - `ChannelError` — per-channel failures (target unreachable, channel closed)
//! - `ConfigError` — invalid configuration (flags, key files, bind failures)
//! - `ForwardError` — port forward setup and connection failures

use std::io;

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("connection failed")]
    ConnectionFailed,
    #[error("handshake failed")]
    HandshakeFailed {
        #[source]
        source: io::Error,
    },
    #[error("transport timeout")]
    Timeout,
    #[error("proxy failed")]
    ProxyFailed {
        #[source]
        source: io::Error,
    },
}

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum AuthError {
    #[error("key rejected")]
    KeyRejected,
    #[error("certificate invalid")]
    CertInvalid,
    #[error("certificate expired")]
    CertExpired,
    #[error("certificate principal mismatch")]
    CertPrincipalMismatch,
    #[error("no matching key")]
    NoMatchingKey,
}

#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    #[error("target unreachable")]
    TargetUnreachable,
    #[error("proxy connect failed")]
    ProxyConnectFailed {
        #[source]
        source: io::Error,
    },
    #[error("channel closed")]
    ChannelClosed,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("invalid flag: {name}")]
    InvalidFlag { name: String },
    #[error("key file not found: {path}")]
    KeyFileNotFound { path: String },
    #[error("bind failed")]
    BindFailed {
        #[source]
        source: io::Error,
    },
    #[error("incompatible options")]
    IncompatibleOptions,
    #[error("invalid proxy config: {message}")]
    ProxyConfigInvalid { message: String },
}

#[derive(Debug, thiserror::Error)]
pub enum ForwardError {
    #[error("invalid port forward spec: {spec}")]
    InvalidSpec { spec: String },
    #[error("bind failed")]
    BindFailed {
        #[source]
        source: io::Error,
    },
    #[error("channel open failed")]
    ChannelOpenFailed {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("connect to local target failed")]
    LocalConnectFailed {
        #[source]
        source: io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn transport_error_display() {
        assert_eq!(
            TransportError::ConnectionFailed.to_string(),
            "connection failed"
        );
        assert_eq!(
            TransportError::HandshakeFailed {
                source: io::Error::new(io::ErrorKind::ConnectionRefused, "tls failed")
            }
            .to_string(),
            "handshake failed"
        );
        assert_eq!(TransportError::Timeout.to_string(), "transport timeout");
        assert_eq!(
            TransportError::ProxyFailed {
                source: io::Error::new(io::ErrorKind::ConnectionRefused, "proxy err")
            }
            .to_string(),
            "proxy failed"
        );
    }

    #[test]
    fn auth_error_display() {
        assert_eq!(AuthError::KeyRejected.to_string(), "key rejected");
        assert_eq!(AuthError::CertInvalid.to_string(), "certificate invalid");
        assert_eq!(AuthError::CertExpired.to_string(), "certificate expired");
        assert_eq!(
            AuthError::CertPrincipalMismatch.to_string(),
            "certificate principal mismatch"
        );
        assert_eq!(AuthError::NoMatchingKey.to_string(), "no matching key");
    }

    #[test]
    fn channel_error_display() {
        assert_eq!(
            ChannelError::TargetUnreachable.to_string(),
            "target unreachable"
        );
        assert_eq!(
            ChannelError::ProxyConnectFailed {
                source: io::Error::new(io::ErrorKind::ConnectionRefused, "refused")
            }
            .to_string(),
            "proxy connect failed"
        );
        assert_eq!(ChannelError::ChannelClosed.to_string(), "channel closed");
    }

    #[test]
    fn config_error_display() {
        assert_eq!(
            ConfigError::InvalidFlag {
                name: "--bad".to_string()
            }
            .to_string(),
            "invalid flag: --bad"
        );
        assert_eq!(
            ConfigError::KeyFileNotFound {
                path: "/missing".to_string()
            }
            .to_string(),
            "key file not found: /missing"
        );
        assert_eq!(
            ConfigError::BindFailed {
                source: io::Error::new(io::ErrorKind::AddrInUse, "in use")
            }
            .to_string(),
            "bind failed"
        );
        assert_eq!(
            ConfigError::IncompatibleOptions.to_string(),
            "incompatible options"
        );
        assert_eq!(
            ConfigError::ProxyConfigInvalid {
                message: "bad proxy".to_string()
            }
            .to_string(),
            "invalid proxy config: bad proxy"
        );
    }

    #[test]
    fn error_source_chaining() {
        let io_err = io::Error::new(io::ErrorKind::ConnectionRefused, "refused");
        let transport_err = TransportError::HandshakeFailed { source: io_err };
        assert!(transport_err.source().is_some());

        let io_err = io::Error::new(io::ErrorKind::ConnectionRefused, "proxy");
        let channel_err = ChannelError::ProxyConnectFailed { source: io_err };
        assert!(channel_err.source().is_some());

        let io_err = io::Error::new(io::ErrorKind::AddrInUse, "addr");
        let config_err = ConfigError::BindFailed { source: io_err };
        assert!(config_err.source().is_some());

        let plain = AuthError::KeyRejected;
        assert!(plain.source().is_none());
    }

    #[test]
    fn forward_error_display() {
        assert_eq!(
            ForwardError::InvalidSpec {
                spec: "bad".to_string()
            }
            .to_string(),
            "invalid port forward spec: bad"
        );
        assert_eq!(
            ForwardError::BindFailed {
                source: io::Error::new(io::ErrorKind::AddrInUse, "in use")
            }
            .to_string(),
            "bind failed"
        );
        assert_eq!(
            ForwardError::LocalConnectFailed {
                source: io::Error::new(io::ErrorKind::ConnectionRefused, "refused")
            }
            .to_string(),
            "connect to local target failed"
        );
    }

    #[test]
    fn forward_error_source_chaining() {
        let io_err = io::Error::new(io::ErrorKind::AddrInUse, "in use");
        let forward_err = ForwardError::BindFailed { source: io_err };
        assert!(forward_err.source().is_some());

        let plain = ForwardError::InvalidSpec {
            spec: "bad".to_string(),
        };
        assert!(plain.source().is_none());
    }
}
