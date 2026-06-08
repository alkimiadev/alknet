//! Static (immutable) server configuration resolved at startup.
//!
//! See [ADR-030](docs/architecture/decisions/030-dynamic-config.md).

use crate::interface::InterfaceKind;
use crate::server::handler::{ProxyConfig, ProxyMode};
use crate::server::serve::{ListenerConfig, ServeTransportMode};
use crate::transport::TransportKind;
use std::net::SocketAddr;

pub struct StaticConfig {
    pub transport_mode: ServeTransportMode,
    pub listen_addr: String,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
    pub acme_domain: Option<String>,
    pub stealth: bool,
    pub host_key: russh::keys::PrivateKey,
    pub host_key_algorithm: russh::keys::Algorithm,
    pub max_auth_attempts: usize,
    pub max_connections_per_ip: usize,
    pub proxy_config: Option<ProxyConfig>,
    pub iroh_relay: Option<String>,
    pub listeners: Vec<ListenerConfig>,
}

impl std::fmt::Debug for StaticConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StaticConfig")
            .field("transport_mode", &self.transport_mode)
            .field("listen_addr", &self.listen_addr)
            .field("tls_cert", &self.tls_cert.as_ref().map(|_| "<redacted>"))
            .field("tls_key", &self.tls_key.as_ref().map(|_| "<redacted>"))
            .field("acme_domain", &self.acme_domain)
            .field("stealth", &self.stealth)
            .field("host_key_algorithm", &self.host_key_algorithm)
            .field("max_auth_attempts", &self.max_auth_attempts)
            .field("max_connections_per_ip", &self.max_connections_per_ip)
            .field("proxy_config", &self.proxy_config)
            .field("iroh_relay", &self.iroh_relay)
            .field("listeners", &self.listeners)
            .finish()
    }
}

impl StaticConfig {
    pub fn from_serve_options(
        opts: crate::server::serve::ServeOptions,
    ) -> Result<(Self, crate::config::DynamicConfig), crate::error::ConfigError> {
        opts.validate()?;

        let host_key = crate::auth::keys::load_private_key(opts.key.clone())?;
        let host_key_algorithm = host_key.algorithm();

        let auth_config = crate::auth::ServerAuthConfig::from_keys_and_ca(
            opts.authorized_keys.clone(),
            opts.cert_authority.clone(),
        )?;

        let auth_policy = crate::config::AuthPolicy::from_server_auth_config(auth_config);

        let dynamic = crate::config::DynamicConfig::new(auth_policy);

        let proxy_config = parse_proxy_config(opts.proxy.as_deref())?;

        let listeners = if let Some(listeners) = opts.listeners {
            listeners
        } else {
            vec![ListenerConfig {
                transport_kind: match opts.transport_mode {
                    ServeTransportMode::Tcp => TransportKind::Tcp,
                    ServeTransportMode::Tls => TransportKind::Tls { server_name: None },
                    ServeTransportMode::Iroh => TransportKind::Iroh {
                        endpoint_id: String::new(),
                    },
                },
                interface_kind: InterfaceKind::Ssh,
                listen_addr: opts.listen_addr.clone(),
                tls_cert: opts.tls_cert.clone(),
                tls_key: opts.tls_key.clone(),
                acme_domain: opts.acme_domain.clone(),
                stealth: opts.stealth,
                iroh_relay: opts.iroh_relay.clone(),
            }]
        };

        let static_config = StaticConfig {
            transport_mode: opts.transport_mode,
            listen_addr: opts.listen_addr,
            tls_cert: opts.tls_cert,
            tls_key: opts.tls_key,
            acme_domain: opts.acme_domain,
            stealth: opts.stealth,
            host_key,
            host_key_algorithm,
            max_auth_attempts: opts.max_auth_attempts,
            max_connections_per_ip: opts.max_connections_per_ip,
            proxy_config,
            iroh_relay: opts.iroh_relay,
            listeners,
        };

        Ok((static_config, dynamic))
    }
}

fn parse_proxy_config(
    proxy: Option<&str>,
) -> Result<Option<ProxyConfig>, crate::error::ConfigError> {
    match proxy {
        None => Ok(None),
        Some(url) => {
            if let Some(rest) = url.strip_prefix("socks5://") {
                let addr: SocketAddr = rest.parse().map_err(|e| {
                    crate::error::ConfigError::ProxyConfigInvalid {
                        message: format!("invalid socks5 proxy address '{}': {}", rest, e),
                    }
                })?;
                Ok(Some(ProxyConfig {
                    mode: ProxyMode::Socks5(addr),
                }))
            } else if let Some(rest) = url.strip_prefix("http://") {
                let addr: SocketAddr = rest.parse().map_err(|e| {
                    crate::error::ConfigError::ProxyConfigInvalid {
                        message: format!(
                            "invalid http connect proxy address '{}': {}",
                            rest, e
                        ),
                    }
                })?;
                Ok(Some(ProxyConfig {
                    mode: ProxyMode::HttpConnect(addr),
                }))
            } else {
                Err(crate::error::ConfigError::ProxyConfigInvalid {
                    message: format!("unsupported proxy URL scheme: {}", url),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::keys::KeySource;
    use crate::server::serve::ServeOptions;
    use crate::transport::TransportKind;

    const ED25519_PRIVATE_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAACBOfInDyRS33JEeDNT8xd10qRdwFN8z/QukCOgEIkv01QAAAJiQ+NvMkPjb\nzAAAAAtzc2gtZWQyNTUxOQAAACBOfInDyRS33JEeDNT8xd10qRdwFN8z/QukCOgEIkv01Q\nAAAECIWwJf7+7MOuZAOOWmoQbE9i/5GxjKsFrtJHjZ34E/fk58icPJFLfckR4M1PzF3XSp\nF3AU3zP9C6QI6AQiS/TVAAAAD3VidW50dUBuczUyODA5NgECAwQFBg==\n-----END OPENSSH PRIVATE KEY-----\n";

    const ED25519_PUBLIC_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIE58icPJFLfckR4M1PzF3XSpF3AU3zP9C6QI6AQiS/TV ubuntu@ns528096";

    fn make_key_source() -> KeySource {
        KeySource::Memory(ED25519_PRIVATE_KEY.as_bytes().to_vec())
    }

    fn make_authorized_keys_source() -> KeySource {
        KeySource::Memory(ED25519_PUBLIC_KEY.as_bytes().to_vec())
    }

    #[test]
    fn parse_proxy_config_socks5() {
        let config = parse_proxy_config(Some("socks5://127.0.0.1:9050")).unwrap();
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
        let config = parse_proxy_config(Some("http://127.0.0.1:8080")).unwrap();
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
        assert!(parse_proxy_config(None).unwrap().is_none());
    }

    #[test]
    fn parse_proxy_config_invalid_scheme() {
        let result = parse_proxy_config(Some("ftp://127.0.0.1:9050"));
        assert!(result.is_err());
        match result.unwrap_err() {
            crate::error::ConfigError::ProxyConfigInvalid { message } => {
                assert!(message.contains("unsupported proxy URL scheme"));
            }
            e => panic!("expected ProxyConfigInvalid, got {:?}", e),
        }
    }

    #[test]
    fn parse_proxy_config_invalid_address() {
        let result = parse_proxy_config(Some("socks5://not-an-address"));
        assert!(result.is_err());
        match result.unwrap_err() {
            crate::error::ConfigError::ProxyConfigInvalid { message } => {
                assert!(message.contains("invalid socks5 proxy address"));
            }
            e => panic!("expected ProxyConfigInvalid, got {:?}", e),
        }
    }

    #[test]
    fn static_config_from_serve_options_basic() {
        let opts =
            ServeOptions::new(make_key_source()).authorized_keys(make_authorized_keys_source());
        let (static_config, dynamic) = StaticConfig::from_serve_options(opts).unwrap();
        assert_eq!(static_config.listen_addr, "0.0.0.0:22");
        assert_eq!(static_config.max_auth_attempts, 10);
        assert!(dynamic.auth.authorized_keys.len() > 0);
    }

    #[test]
    fn static_config_from_serve_options_with_proxy() {
        let opts = ServeOptions::new(make_key_source())
            .authorized_keys(make_authorized_keys_source())
            .proxy("socks5://127.0.0.1:9050");
        let (static_config, _) = StaticConfig::from_serve_options(opts).unwrap();
        assert!(static_config.proxy_config.is_some());
    }

    #[test]
    fn static_config_from_serve_options_with_listeners() {
        let listeners = vec![ListenerConfig::tcp("0.0.0.0:22")];
        let opts = ServeOptions::new(make_key_source())
            .authorized_keys(make_authorized_keys_source())
            .listeners(listeners);
        let (static_config, _) = StaticConfig::from_serve_options(opts).unwrap();
        assert_eq!(static_config.listeners.len(), 1);
        assert_eq!(
            static_config.listeners[0].transport_kind,
            TransportKind::Tcp
        );
    }

    #[test]
    fn static_config_from_serve_options_invalid_proxy_returns_err() {
        let opts = ServeOptions::new(make_key_source())
            .authorized_keys(make_authorized_keys_source())
            .proxy("ftp://bad-scheme");
        let result = StaticConfig::from_serve_options(opts);
        assert!(result.is_err());
        match result.unwrap_err() {
            crate::error::ConfigError::ProxyConfigInvalid { message } => {
                assert!(message.contains("unsupported proxy URL scheme"));
            }
            e => panic!("expected ProxyConfigInvalid, got {:?}", e),
        }
    }

    #[test]
    fn static_config_from_serve_options_malformed_proxy_address_returns_err() {
        let opts = ServeOptions::new(make_key_source())
            .authorized_keys(make_authorized_keys_source())
            .proxy("socks5://not-a-valid-addr");
        let result = StaticConfig::from_serve_options(opts);
        assert!(result.is_err());
        match result.unwrap_err() {
            crate::error::ConfigError::ProxyConfigInvalid { message } => {
                assert!(message.contains("invalid socks5 proxy address"));
            }
            e => panic!("expected ProxyConfigInvalid, got {:?}", e),
        }
    }
}
