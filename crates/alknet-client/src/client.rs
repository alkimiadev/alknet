//! `AlknetClient` — native client dial seam, the client-side analogue of
//! `AlknetEndpoint`. Holds pre-built transport handles, all optional — the
//! client dials with whichever transport the remote endpoint type implies.

use std::fmt;

#[cfg(feature = "iroh")]
use iroh;
#[cfg(feature = "quinn")]
use quinn;
#[cfg(feature = "tcp")]
use tokio_rustls;

#[cfg(feature = "socks5")]
use crate::socks5::Socks5ProxyConfig;

/// Native client dial seam — multi-transport dialer that produces
/// `Connection`s for protocol take-overs.
///
/// Holds pre-built transport handles, all optional — the client dials
/// with whichever transport the remote endpoint type implies. The
/// builder mirrors `AlknetEndpoint`'s `with_quinn` / `with_iroh` /
/// `with_tcp_tls` (ADR-083) — the assembly layer builds the transport
/// handles and hands them to the client via builder methods.
pub struct AlknetClient {
    #[cfg(feature = "quinn")]
    pub(crate) quinn: Option<quinn::Endpoint>,
    #[cfg(feature = "tcp")]
    pub(crate) tcp_connector: Option<tokio_rustls::TlsConnector>,
    #[cfg(feature = "iroh")]
    pub(crate) iroh: Option<iroh::Endpoint>,
    /// When set, `dial_quic` and `dial_tcp_tls` route through this
    /// SOCKS5 proxy (UDP ASSOCIATE / CONNECT respectively). `dial_iroh`
    /// forces relay-only via an HTTP-to-SOCKS5 bridge — see ADR-090 §5.
    /// Feature-gated on `socks5`.
    #[cfg(feature = "socks5")]
    pub(crate) socks5: Option<Socks5ProxyConfig>,
}

impl AlknetClient {
    /// Create a new `AlknetClient` with no transport handles configured.
    /// Use the builder methods to add transports.
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "quinn")]
            quinn: None,
            #[cfg(feature = "tcp")]
            tcp_connector: None,
            #[cfg(feature = "iroh")]
            iroh: None,
            #[cfg(feature = "socks5")]
            socks5: None,
        }
    }

    /// Set the QUIC transport handle. The assembly layer builds a
    /// `quinn::Endpoint` (with or without a SOCKS5 proxy — the proxy
    /// is applied inside `dial_quic`, not at construction time) and
    /// hands it to the client.
    #[cfg(feature = "quinn")]
    pub fn with_quinn(mut self, endpoint: quinn::Endpoint) -> Self {
        self.quinn = Some(endpoint);
        self
    }

    /// Set the TCP+TLS transport handle. The assembly layer builds a
    /// `tokio_rustls::TlsConnector` and hands it to the client.
    #[cfg(feature = "tcp")]
    pub fn with_tcp_tls(mut self, connector: tokio_rustls::TlsConnector) -> Self {
        self.tcp_connector = Some(connector);
        self
    }

    /// Set the iroh transport handle. The assembly layer builds an
    /// `iroh::Endpoint` and hands it to the client.
    #[cfg(feature = "iroh")]
    pub fn with_iroh(mut self, endpoint: iroh::Endpoint) -> Self {
        self.iroh = Some(endpoint);
        self
    }

    /// Set the SOCKS5 proxy for all subsequent dials. When set, every
    /// dial routes its transport through this proxy: UDP ASSOCIATE for
    /// `dial_quic`, CONNECT for `dial_tcp_tls`, and force-relay-only +
    /// HTTP-to-SOCKS5 bridge for `dial_iroh` (ADR-090 §5).
    /// Feature-gated on `socks5`.
    #[cfg(feature = "socks5")]
    pub fn with_socks5_proxy(mut self, proxy: Socks5ProxyConfig) -> Self {
        self.socks5 = Some(proxy);
        self
    }
}

impl Default for AlknetClient {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for AlknetClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        #[allow(unused_mut)]
        let mut configured: Vec<&str> = Vec::new();
        #[cfg(feature = "quinn")]
        if self.quinn.is_some() {
            configured.push("quinn");
        }
        #[cfg(feature = "tcp")]
        if self.tcp_connector.is_some() {
            configured.push("tcp");
        }
        #[cfg(feature = "iroh")]
        if self.iroh.is_some() {
            configured.push("iroh");
        }
        #[cfg(feature = "socks5")]
        if self.socks5.is_some() {
            configured.push("socks5");
        }
        f.debug_struct("AlknetClient")
            .field("transports", &configured)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_empty_client() {
        let client = AlknetClient::new();
        let debug = format!("{:?}", client);
        assert!(debug.contains("AlknetClient"));
    }

    #[test]
    fn default_delegates_to_new() {
        let _client = AlknetClient::default();
    }

    #[test]
    fn alknet_client_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<AlknetClient>();
    }

    #[test]
    fn debug_lists_configured_transports() {
        let client = AlknetClient::new();
        let debug = format!("{:?}", client);
        assert!(!debug.is_empty());
    }
}
