use std::sync::Arc;

use arc_swap::ArcSwap;
use russh::keys::PrivateKey;
use serde::{Deserialize, Serialize};

use crate::auth::IdentityProvider;
use crate::config::DynamicConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StreamInterfaceKind {
    Ssh,
    RawFraming,
}

impl std::fmt::Display for StreamInterfaceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreamInterfaceKind::Ssh => write!(f, "ssh"),
            StreamInterfaceKind::RawFraming => write!(f, "raw-framing"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MessageInterfaceKind {
    Http,
    Dns,
}

impl std::fmt::Display for MessageInterfaceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageInterfaceKind::Http => write!(f, "http"),
            MessageInterfaceKind::Dns => write!(f, "dns"),
        }
    }
}

#[non_exhaustive]
pub enum InterfaceConfig {
    Ssh(SshInterfaceConfig),
    RawFraming(RawFramingConfig),
}

impl InterfaceConfig {
    pub fn kind(&self) -> StreamInterfaceKind {
        #[allow(unreachable_patterns)]
        match self {
            InterfaceConfig::Ssh(_) => StreamInterfaceKind::Ssh,
            InterfaceConfig::RawFraming(_) => StreamInterfaceKind::RawFraming,
            _ => StreamInterfaceKind::Ssh,
        }
    }
}

#[non_exhaustive]
pub enum StreamInterfaceConfig {
    Ssh(SshInterfaceConfig),
    RawFraming(RawFramingConfig),
}

impl StreamInterfaceConfig {
    pub fn kind(&self) -> StreamInterfaceKind {
        match self {
            StreamInterfaceConfig::Ssh(_) => StreamInterfaceKind::Ssh,
            StreamInterfaceConfig::RawFraming(_) => StreamInterfaceKind::RawFraming,
        }
    }
}

impl std::fmt::Display for StreamInterfaceConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreamInterfaceConfig::Ssh(_) => write!(f, "ssh"),
            StreamInterfaceConfig::RawFraming(_) => write!(f, "raw-framing"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MessageInterfaceConfig {
    Http(HttpInterfaceConfig),
    Dns(DnsInterfaceConfig),
}

impl MessageInterfaceConfig {
    pub fn kind(&self) -> MessageInterfaceKind {
        match self {
            MessageInterfaceConfig::Http(_) => MessageInterfaceKind::Http,
            MessageInterfaceConfig::Dns(_) => MessageInterfaceKind::Dns,
        }
    }
}

impl std::fmt::Display for MessageInterfaceConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageInterfaceConfig::Http(_) => write!(f, "http"),
            MessageInterfaceConfig::Dns(_) => write!(f, "dns"),
        }
    }
}

pub struct SshInterfaceConfig {
    pub auth: Arc<dyn IdentityProvider>,
    pub forwarding: Arc<ArcSwap<DynamicConfig>>,
    pub host_key: Arc<PrivateKey>,
}

pub struct RawFramingConfig {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpInterfaceConfig {
    pub bind_addr: std::net::SocketAddr,
    pub tls: bool,
    pub stealth: bool,
}

impl std::fmt::Display for HttpInterfaceConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "http {}", self.bind_addr)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsInterfaceConfig {
    pub bind_addr: std::net::SocketAddr,
    pub tls: bool,
}

impl std::fmt::Display for DnsInterfaceConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "dns {}", self.bind_addr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_interface_kind_display() {
        assert_eq!(StreamInterfaceKind::Ssh.to_string(), "ssh");
        assert_eq!(StreamInterfaceKind::RawFraming.to_string(), "raw-framing");
    }

    #[test]
    fn message_interface_kind_display() {
        assert_eq!(MessageInterfaceKind::Http.to_string(), "http");
        assert_eq!(MessageInterfaceKind::Dns.to_string(), "dns");
    }

    #[test]
    fn stream_interface_config_kind() {
        let auth = Arc::new(crate::auth::ConfigIdentityProvider::new(Arc::new(
            ArcSwap::new(Arc::new(DynamicConfig::default())),
        )));
        let ssh_config = StreamInterfaceConfig::Ssh(SshInterfaceConfig {
            auth,
            forwarding: Arc::new(ArcSwap::new(Arc::new(DynamicConfig::default()))),
            host_key: Arc::new(
                russh::keys::PrivateKey::random(
                    &mut rand_core::OsRng,
                    russh::keys::Algorithm::Ed25519,
                )
                .unwrap(),
            ),
        });
        assert_eq!(ssh_config.kind(), StreamInterfaceKind::Ssh);

        let raw_config = StreamInterfaceConfig::RawFraming(RawFramingConfig {});
        assert_eq!(raw_config.kind(), StreamInterfaceKind::RawFraming);
    }

    #[test]
    fn message_interface_config_kind() {
        let http_config = MessageInterfaceConfig::Http(HttpInterfaceConfig {
            bind_addr: "127.0.0.1:8080".parse().unwrap(),
            tls: false,
            stealth: false,
        });
        assert_eq!(http_config.kind(), MessageInterfaceKind::Http);

        let dns_config = MessageInterfaceConfig::Dns(DnsInterfaceConfig {
            bind_addr: "127.0.0.1:53".parse().unwrap(),
            tls: false,
        });
        assert_eq!(dns_config.kind(), MessageInterfaceKind::Dns);
    }

    #[test]
    fn stream_interface_kind_equality() {
        assert_eq!(StreamInterfaceKind::Ssh, StreamInterfaceKind::Ssh);
        assert_eq!(
            StreamInterfaceKind::RawFraming,
            StreamInterfaceKind::RawFraming
        );
        assert_ne!(StreamInterfaceKind::Ssh, StreamInterfaceKind::RawFraming);
    }

    #[test]
    fn message_interface_kind_equality() {
        assert_eq!(MessageInterfaceKind::Http, MessageInterfaceKind::Http);
        assert_eq!(MessageInterfaceKind::Dns, MessageInterfaceKind::Dns);
        assert_ne!(MessageInterfaceKind::Http, MessageInterfaceKind::Dns);
    }

    #[test]
    fn raw_framing_config_minimal() {
        let _config = RawFramingConfig {};
    }

    #[test]
    fn http_interface_config_display() {
        let config = HttpInterfaceConfig {
            bind_addr: "127.0.0.1:8080".parse().unwrap(),
            tls: true,
            stealth: true,
        };
        assert_eq!(config.to_string(), "http 127.0.0.1:8080");
    }

    #[test]
    fn dns_interface_config_display() {
        let config = DnsInterfaceConfig {
            bind_addr: "127.0.0.1:53".parse().unwrap(),
            tls: false,
        };
        assert_eq!(config.to_string(), "dns 127.0.0.1:53");
    }

    #[test]
    fn http_interface_config_serialization() {
        let config = HttpInterfaceConfig {
            bind_addr: "127.0.0.1:8080".parse().unwrap(),
            tls: true,
            stealth: false,
        };
        let serialized = serde_json::to_string(&config).unwrap();
        let deserialized: HttpInterfaceConfig = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.bind_addr, config.bind_addr);
        assert_eq!(deserialized.tls, config.tls);
    }

    #[test]
    fn dns_interface_config_serialization() {
        let config = DnsInterfaceConfig {
            bind_addr: "0.0.0.0:53".parse().unwrap(),
            tls: true,
        };
        let serialized = serde_json::to_string(&config).unwrap();
        let deserialized: DnsInterfaceConfig = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.bind_addr, config.bind_addr);
        assert_eq!(deserialized.tls, config.tls);
    }
}
