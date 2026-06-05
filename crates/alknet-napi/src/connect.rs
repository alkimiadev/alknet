//! NAPI `connect()` function and `AlknetStream` type.
//!
//! Opens a single SSH channel as a duplex stream for programmatic use.
//! Unlike the CLI client, this does not start a SOCKS5 server or port forwards —
//! it provides a raw stream that JavaScript code can read from and write to.

use std::net::SocketAddr;
use std::sync::Arc;

use napi::bindgen_prelude::*;
use napi_derive::napi;
use russh::client;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;

use alknet_core::auth::client_auth::{ClientAuthConfig, ClientHandler};
use alknet_core::auth::keys::KeySource;
use alknet_core::transport::{IrohTransport, TcpTransport, TlsTransport, Transport};

const DEFAULT_HOST: &str = "alknet-control";
const DEFAULT_PORT: u32 = 0;

#[napi(object)]
pub struct AlknetConnectOptions {
    pub server: Option<String>,
    pub peer: Option<String>,
    pub transport: String,
    pub identity: Option<Either<String, Buffer>>,
    pub tls_server_name: Option<String>,
    pub insecure: Option<bool>,
    pub iroh_relay: Option<String>,
    pub proxy: Option<String>,
}

fn resolve_key_source(identity: &Option<Either<String, Buffer>>) -> Result<KeySource> {
    match identity {
        None => Err(Error::new(
            Status::InvalidArg,
            "identity is required: provide a file path (string) or key data (Buffer)",
        )),
        Some(Either::A(path)) => Ok(KeySource::File(path.into())),
        Some(Either::B(buf)) => Ok(KeySource::Memory(buf.to_vec())),
    }
}

fn parse_addr(addr_str: &str) -> Result<SocketAddr> {
    addr_str.parse().map_err(|e| {
        Error::new(
            Status::InvalidArg,
            format!("invalid server address '{}': {}", addr_str, e),
        )
    })
}

#[napi]
pub struct AlknetStream {
    read: Arc<Mutex<tokio::io::ReadHalf<russh::ChannelStream<client::Msg>>>>,
    write: Arc<Mutex<tokio::io::WriteHalf<russh::ChannelStream<client::Msg>>>>,
}

#[napi]
impl AlknetStream {
    #[napi]
    pub async fn read(&self, size: u32) -> Result<Buffer> {
        let mut buf = vec![0u8; size as usize];
        let mut guard = self.read.lock().await;
        let n = guard
            .read(&mut buf)
            .await
            .map_err(|e| Error::new(Status::GenericFailure, format!("read failed: {}", e)))?;
        if n == 0 {
            return Ok(Vec::<u8>::new().into());
        }
        buf.truncate(n);
        Ok(buf.into())
    }

    #[napi]
    pub async fn write(&self, data: Buffer) -> Result<()> {
        let mut guard = self.write.lock().await;
        guard
            .write_all(&data)
            .await
            .map_err(|e| Error::new(Status::GenericFailure, format!("write failed: {}", e)))?;
        Ok(())
    }

    #[napi]
    pub async fn close(&self) -> Result<()> {
        let mut guard = self.write.lock().await;
        guard
            .shutdown()
            .await
            .map_err(|e| Error::new(Status::GenericFailure, format!("close failed: {}", e)))
    }
}

#[napi]
pub async fn connect(options: AlknetConnectOptions) -> Result<AlknetStream> {
    let key_source = resolve_key_source(&options.identity)?;
    let auth_config = Arc::new(
        ClientAuthConfig::from_key_source(key_source)
            .map_err(|e| Error::new(Status::InvalidArg, format!("invalid identity key: {}", e)))?,
    );

    let transport_mode = options.transport.to_lowercase();
    let handler = ClientHandler::from_config(&auth_config);
    let username = "alknet".to_string();

    let config = Arc::new(client::Config::default());

    let mut handle: client::Handle<ClientHandler> = match transport_mode.as_str() {
        "tcp" => {
            let server = options.server.as_ref().ok_or_else(|| {
                Error::new(Status::InvalidArg, "server is required for tcp transport")
            })?;
            let addr = parse_addr(server)?;
            let transport = TcpTransport::new(addr);
            let stream = transport.connect().await.map_err(|e| {
                Error::new(Status::GenericFailure, format!("tcp connect failed: {}", e))
            })?;
            client::connect_stream(config, stream, handler)
                .await
                .map_err(|e| {
                    Error::new(
                        Status::GenericFailure,
                        format!("ssh handshake failed: {}", e),
                    )
                })?
        }
        "tls" => {
            let server = options.server.as_ref().ok_or_else(|| {
                Error::new(Status::InvalidArg, "server is required for tls transport")
            })?;
            let addr = parse_addr(server)?;
            let mut transport = TlsTransport::new(addr);
            if let Some(ref name) = options.tls_server_name {
                transport = transport.with_server_name(name);
            }
            if let Some(true) = options.insecure {
                transport = transport.with_insecure(true);
            }
            let stream = transport.connect().await.map_err(|e| {
                Error::new(Status::GenericFailure, format!("tls connect failed: {}", e))
            })?;
            client::connect_stream(config, stream, handler)
                .await
                .map_err(|e| {
                    Error::new(
                        Status::GenericFailure,
                        format!("ssh handshake failed: {}", e),
                    )
                })?
        }
        "iroh" => {
            let peer = options.peer.as_ref().ok_or_else(|| {
                Error::new(Status::InvalidArg, "peer is required for iroh transport")
            })?;
            let node_id: iroh::NodeId = peer.parse().map_err(|e| {
                Error::new(
                    Status::InvalidArg,
                    format!("invalid iroh peer ID '{}': {}", peer, e),
                )
            })?;
            let relay_url: Option<iroh::RelayUrl> = match options.iroh_relay.as_deref() {
                Some(u) => Some(u.parse().map_err(|e| {
                    Error::new(Status::InvalidArg, format!("invalid iroh relay URL: {}", e))
                })?),
                None => None,
            };
            let proxy_url: Option<url::Url> = match options.proxy.as_deref() {
                Some(u) => Some(u.parse().map_err(|e| {
                    Error::new(Status::InvalidArg, format!("invalid proxy URL: {}", e))
                })?),
                None => None,
            };
            let transport = IrohTransport::new(node_id, relay_url, proxy_url)
                .await
                .map_err(|e| {
                    Error::new(
                        Status::GenericFailure,
                        format!("iroh endpoint setup failed: {}", e),
                    )
                })?;
            let stream = transport.connect().await.map_err(|e| {
                Error::new(
                    Status::GenericFailure,
                    format!("iroh connect failed: {}", e),
                )
            })?;
            client::connect_stream(config, stream, handler)
                .await
                .map_err(|e| {
                    Error::new(
                        Status::GenericFailure,
                        format!("ssh handshake failed: {}", e),
                    )
                })?
        }
        _ => {
            return Err(Error::new(
                Status::InvalidArg,
                format!(
                    "unknown transport '{}'; expected tcp, tls, or iroh",
                    transport_mode
                ),
            ));
        }
    };

    let auth_ok = auth_config
        .authenticate(&mut handle, &username)
        .await
        .map_err(|e| Error::new(Status::GenericFailure, format!("ssh auth failed: {}", e)))?;
    if !auth_ok {
        return Err(Error::new(
            Status::GenericFailure,
            "ssh authentication rejected",
        ));
    }

    let channel = handle
        .channel_open_direct_tcpip(DEFAULT_HOST, DEFAULT_PORT, "127.0.0.1", 0)
        .await
        .map_err(|e| {
            Error::new(
                Status::GenericFailure,
                format!("failed to open ssh channel: {}", e),
            )
        })?;

    let stream = channel.into_stream();
    let (read_half, write_half) = tokio::io::split(stream);

    Ok(AlknetStream {
        read: Arc::new(Mutex::new(read_half)),
        write: Arc::new(Mutex::new(write_half)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ED25519_PRIVATE_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAACBOfInDyRS33JEeDNT8xd10qRdwFN8z/QukCOgEIkv01QAAAJiQ+NvMkPjb\nzAAAAAtzc2gtZWQyNTUxOQAAACBOfInDyRS33JEeDNT8xd10qRdwFN8z/QukCOgEIkv01Q\nAAAECIWwJf7+7MOuZAOOWmoQbE9i/5GxjKsFrtJHjZ34E/fk58icPJFLfckR4M1PzF3XSp\nF3AU3zP9C6QI6AQiS/TVAAAAD3VidW50dUBuczUyODA5NgECAwQFBg==\n-----END OPENSSH PRIVATE KEY-----\n";

    #[test]
    fn resolve_key_source_file_path() {
        let identity = Some(Either::<String, Buffer>::A("/path/to/key".to_string()));
        let result = resolve_key_source(&identity);
        assert!(result.is_ok());
        match result.unwrap() {
            KeySource::File(p) => assert_eq!(p.to_str(), Some("/path/to/key")),
            _ => panic!("expected File variant"),
        }
    }

    #[test]
    fn resolve_key_source_buffer() {
        let identity = Some(Either::<String, Buffer>::B(Buffer::from(
            ED25519_PRIVATE_KEY.as_bytes().to_vec(),
        )));
        let result = resolve_key_source(&identity);
        assert!(result.is_ok());
        match result.unwrap() {
            KeySource::Memory(data) => assert!(!data.is_empty()),
            _ => panic!("expected Memory variant"),
        }
    }

    #[test]
    fn resolve_key_source_missing() {
        let identity: Option<Either<String, Buffer>> = None;
        let result = resolve_key_source(&identity);
        assert!(result.is_err());
    }

    #[test]
    fn parse_addr_valid() {
        let addr = parse_addr("127.0.0.1:22");
        assert!(addr.is_ok());
        assert_eq!(addr.unwrap().port(), 22);
    }

    #[test]
    fn parse_addr_invalid() {
        let addr = parse_addr("not-an-address");
        assert!(addr.is_err());
    }

    #[test]
    fn auth_config_from_memory_key() {
        let source = KeySource::Memory(ED25519_PRIVATE_KEY.as_bytes().to_vec());
        let config = ClientAuthConfig::from_key_source(source);
        assert!(config.is_ok());
    }

    #[test]
    fn auth_config_from_invalid_key() {
        let source = KeySource::Memory(b"not-a-key".to_vec());
        let config = ClientAuthConfig::from_key_source(source);
        assert!(config.is_err());
    }
}
