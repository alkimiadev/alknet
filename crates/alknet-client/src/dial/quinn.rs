//! `dial_quic` — QUIC dial via quinn, producing a `Connection`.
//!
//! Feature-gated on `quinn`. Builds a `TlsClientConfig` from
//! `ConnectionCredentials`, constructs a `quinn::ClientConfig`, dials
//! `addr` on `alpn`, and returns a `Connection` via
//! `Connection::from_quinn_with_alpn`.

use std::net::SocketAddr;
#[cfg(feature = "socks5")]
use std::sync::Arc;

use alknet_core::credentials::ConnectionCredentials;
use alknet_core::types::Connection;
use alknet_tls::client::TlsClientConfig;

use crate::error::ClientDialError;
use crate::client::AlknetClient;

impl AlknetClient {
    /// QUIC dial. Builds a `TlsClientConfig` from `creds`
    /// (ADR-034 verifier selection + ADR-084 provider), dials `addr`
    /// on `alpn`, returns a `Connection` via
    /// `Connection::from_quinn_with_alpn`. The `server_name` is the
    /// TLS SNI / name (for X.509; ignored for raw-key pinning).
    /// Feature-gated on `quinn`.
    #[cfg(feature = "quinn")]
    pub async fn dial_quic(
        &self,
        addr: SocketAddr,
        server_name: &str,
        alpn: &[u8],
        creds: &ConnectionCredentials,
    ) -> Result<Connection, ClientDialError> {
        let tls_config = TlsClientConfig::new(creds, alpn)?;
        let client_config = tls_config.for_quinn()?;

        #[cfg(feature = "socks5")]
        let conn = if let Some(proxy) = &self.socks5 {
            let socket = crate::socks5::Socks5UdpSocket::bind(proxy).await?;
            let endpoint = quinn::Endpoint::new_with_abstract_socket(
                quinn::EndpointConfig::default(),
                None,
                Arc::new(socket),
                Arc::new(quinn::TokioRuntime),
            )
            .map_err(|e| ClientDialError::Connect(e.to_string()))?;
            endpoint
                .connect_with(client_config, addr, server_name)
                .map_err(|e| ClientDialError::Connect(e.to_string()))?
                .await
                .map_err(|e| ClientDialError::Connect(e.to_string()))?
        } else {
            let endpoint = self
                .quinn
                .as_ref()
                .ok_or(ClientDialError::NoTransport {
                    transport: "quinn",
                })?;
            endpoint
                .connect_with(client_config, addr, server_name)
                .map_err(|e| ClientDialError::Connect(e.to_string()))?
                .await
                .map_err(|e| ClientDialError::Connect(e.to_string()))?
        };

        #[cfg(not(feature = "socks5"))]
        let conn = {
            let endpoint = self
                .quinn
                .as_ref()
                .ok_or(ClientDialError::NoTransport {
                    transport: "quinn",
                })?;
            endpoint
                .connect_with(client_config, addr, server_name)
                .map_err(|e| ClientDialError::Connect(e.to_string()))?
                .await
                .map_err(|e| ClientDialError::Connect(e.to_string()))?
        };

        Ok(Connection::from_quinn_with_alpn(conn, alpn.to_vec()))
    }
}
