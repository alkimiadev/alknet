//! `dial_tcp_tls` — TCP+TLS dial via tokio-rustls, producing a `Connection`.
//!
//! Feature-gated on `tcp`. Builds a `TlsClientConfig` from
//! `ConnectionCredentials`, connects a `TcpStream` to `addr`, wraps with
//! `TlsConnector` using `host` as the SNI, and returns a `Connection` via
//! `Connection::from_bidi` (ADR-065).

use std::net::SocketAddr;
use std::sync::Arc;

use alknet_core::credentials::ConnectionCredentials;
use alknet_core::types::Connection;
use alknet_tls::client::TlsClientConfig;
use tokio::net::TcpStream;

use crate::client::AlknetClient;
use crate::error::ClientDialError;

impl AlknetClient {
    /// TCP+TLS dial. Builds a `TlsClientConfig` from `creds`,
    /// connects a `TcpStream` to `addr`, wraps with `TlsConnector`
    /// using `host` as the SNI, returns a `Connection` via
    /// `Connection::from_bidi` (ADR-065). Feature-gated on `tcp`.
    #[cfg(feature = "tcp")]
    pub async fn dial_tcp_tls(
        &self,
        host: &str,
        addr: SocketAddr,
        alpn: &[u8],
        creds: &ConnectionCredentials,
    ) -> Result<Connection, ClientDialError> {
        let tls_config = TlsClientConfig::new(creds, alpn)?;

        let connector = match &self.tcp_connector {
            Some(c) => c.clone(),
            None => {
                let rustls_config = Arc::new(tls_config.into_rustls_config());
                tokio_rustls::TlsConnector::from(rustls_config)
            }
        };

        #[cfg(feature = "socks5")]
        let tls_stream = if let Some(proxy) = &self.socks5 {
            let mut tcp = TcpStream::connect(proxy.addr)
                .await
                .map_err(|e| ClientDialError::Connect(e.to_string()))?;
            crate::socks5::socks5_connect(&mut tcp, proxy, addr)
                .await
                .map_err(ClientDialError::Proxy)?;
            let server_name: rustls::pki_types::ServerName = host.to_owned().try_into().map_err(
                |e: rustls::pki_types::InvalidDnsNameError| ClientDialError::Connect(e.to_string()),
            )?;
            connector
                .connect(server_name, tcp)
                .await
                .map_err(|e| ClientDialError::Handshake(e.to_string()))?
        } else {
            let tcp_stream = TcpStream::connect(addr)
                .await
                .map_err(|e| ClientDialError::Connect(e.to_string()))?;
            let server_name: rustls::pki_types::ServerName = host.to_owned().try_into().map_err(
                |e: rustls::pki_types::InvalidDnsNameError| ClientDialError::Connect(e.to_string()),
            )?;
            connector
                .connect(server_name, tcp_stream)
                .await
                .map_err(|e| ClientDialError::Handshake(e.to_string()))?
        };

        #[cfg(not(feature = "socks5"))]
        let tls_stream = {
            let tcp_stream = TcpStream::connect(addr)
                .await
                .map_err(|e| ClientDialError::Connect(e.to_string()))?;
            let server_name: rustls::pki_types::ServerName = host.to_owned().try_into().map_err(
                |e: rustls::pki_types::InvalidDnsNameError| ClientDialError::Connect(e.to_string()),
            )?;
            connector
                .connect(server_name, tcp_stream)
                .await
                .map_err(|e| ClientDialError::Handshake(e.to_string()))?
        };

        Ok(Connection::from_bidi(tls_stream, alpn.to_vec(), Some(addr)))
    }
}

#[cfg(all(test, feature = "tcp"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dial_tcp_tls_no_transport_error() {
        let client = AlknetClient::new();
        let creds = ConnectionCredentials::new();
        let result = client
            .dial_tcp_tls(
                "localhost",
                "127.0.0.1:0".parse().unwrap(),
                b"test/alpn",
                &creds,
            )
            .await;
        assert!(matches!(result, Err(ClientDialError::Connect(_))));
    }
}
