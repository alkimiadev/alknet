use crate::server::handler::{ProxyConfig, ProxyMode};
use crate::server::serve::ServeTransportMode;
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

        let proxy_config = parse_proxy_config(opts.proxy.as_deref());

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
        };

        Ok((static_config, dynamic))
    }
}

fn parse_proxy_config(proxy: Option<&str>) -> Option<ProxyConfig> {
    proxy.map(|url| {
        if url.starts_with("socks5://") {
            let addr: SocketAddr = url
                .strip_prefix("socks5://")
                .unwrap()
                .parse()
                .expect("invalid socks5 proxy address");
            ProxyConfig {
                mode: ProxyMode::Socks5(addr),
            }
        } else if url.starts_with("http://") {
            let addr: SocketAddr = url
                .strip_prefix("http://")
                .unwrap()
                .parse()
                .expect("invalid http connect proxy address");
            ProxyConfig {
                mode: ProxyMode::HttpConnect(addr),
            }
        } else {
            panic!("unsupported proxy URL scheme: {url}");
        }
    })
}
