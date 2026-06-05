//! SOCKS5 proxy server.
//!
//! Listens on a local port and routes each SOCKS5 connection through an SSH
//! `direct-tcpip` channel. Supports SOCKS5h (domain names resolved server-side)
//! to prevent DNS leaks. Uses the `ChannelOpener` trait to abstract over the
//! SSH channel mechanism, making it testable without a real SSH session.

mod protocol;

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tracing::debug;

use protocol::{Socks5Reply, Socks5Request, Socks5VersionMethod};

pub use protocol::Socks5Address;

const DEFAULT_SOCKS5_ADDR: &str = "127.0.0.1:1080";

pub trait ChannelOpener: Send + Sync + 'static {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    fn open_channel(
        &self,
        host: String,
        port: u16,
    ) -> impl std::future::Future<Output = Result<Self::Stream, ChannelOpenError>> + Send;
}

#[derive(Debug, thiserror::Error)]
pub enum ChannelOpenError {
    #[error("session closed")]
    SessionClosed,
    #[error("channel open failed")]
    ChannelOpenFailed,
    #[error("connection refused")]
    ConnectionRefused,
}

pub struct Socks5Server<C: ChannelOpener> {
    listen_addr: SocketAddr,
    channel_opener: Arc<C>,
}

impl<C: ChannelOpener> Socks5Server<C> {
    pub fn new(channel_opener: C) -> Self {
        Self::with_addr(channel_opener, DEFAULT_SOCKS5_ADDR)
    }

    pub fn with_addr(channel_opener: C, addr: &str) -> Self {
        let listen_addr: SocketAddr = addr
            .parse()
            .expect("invalid SOCKS5 listen address");
        Self {
            listen_addr,
            channel_opener: Arc::new(channel_opener),
        }
    }

    pub fn listen_addr(&self) -> SocketAddr {
        self.listen_addr
    }

    pub async fn run(self) -> Result<(), std::io::Error> {
        let listener = TcpListener::bind(self.listen_addr).await?;
        debug!("socks5 server listening on {}", self.listen_addr);
        loop {
            let (socket, _peer) = listener.accept().await?;
            let opener = Arc::clone(&self.channel_opener);
            tokio::spawn(async move {
                if let Err(e) = handle_socks5_connection(socket, opener).await {
                    debug!("socks5 connection error: {e}");
                }
            });
        }
    }
}

async fn handle_socks5_connection<S, C>(
    mut socket: S,
    opener: Arc<C>,
) -> Result<(), Socks5Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
    C: ChannelOpener,
{
    let vm = Socks5VersionMethod::read_from(&mut socket).await?;
    if vm.version != 0x05 {
        return Err(Socks5Error::InvalidVersion(vm.version));
    }
    if !vm.methods.contains(&0x00) {
        let reply = [0x05, 0xFF];
        socket.write_all(&reply).await?;
        socket.shutdown().await?;
        return Err(Socks5Error::NoAcceptableAuth);
    }
    let reply = [0x05, 0x00];
    socket.write_all(&reply).await?;

    let request = Socks5Request::read_from(&mut socket).await?;
    if request.version != 0x05 {
        return Err(Socks5Error::InvalidVersion(request.version));
    }
    if request.command != 0x01 {
        send_error_reply(&mut socket, Socks5Reply::command_not_supported()).await?;
        return Err(Socks5Error::UnsupportedCommand(request.command));
    }

    let (host, port) = match &request.address {
        Socks5Address::Ipv4(addr) => (addr.to_string(), request.port),
        Socks5Address::Ipv6(addr) => (addr.to_string(), request.port),
        Socks5Address::Domain(name) => (name.clone(), request.port),
    };

    match opener.open_channel(host, port).await {
        Ok(mut ssh_stream) => {
            let bind_addr = Socks5Address::Ipv4(std::net::Ipv4Addr::UNSPECIFIED);
            let reply = Socks5Reply::success(bind_addr, 0);
            reply.write_to(&mut socket).await?;
            tokio::io::copy_bidirectional(&mut socket, &mut ssh_stream).await?;
            Ok(())
        }
        Err(_) => {
            send_error_reply(&mut socket, Socks5Reply::connection_refused()).await?;
            Err(Socks5Error::ChannelOpenFailed)
        }
    }
}

async fn send_error_reply<S: AsyncRead + AsyncWrite + Unpin>(
    socket: &mut S,
    reply: Socks5Reply,
) -> Result<(), Socks5Error> {
    reply.write_to(socket).await?;
    let _ = socket.shutdown().await;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum Socks5Error {
    #[error("invalid SOCKS version: {0}")]
    InvalidVersion(u8),
    #[error("no acceptable auth method")]
    NoAcceptableAuth,
    #[error("unsupported command: {0}")]
    UnsupportedCommand(u8),
    #[error("channel open failed")]
    ChannelOpenFailed,
    #[error("io error")]
    Io(#[from] std::io::Error),
}

pub struct HandleChannelOpener<H: russh::client::Handler> {
    handle: Arc<Mutex<russh::client::Handle<H>>>,
}

impl<H: russh::client::Handler> HandleChannelOpener<H> {
    pub fn new(handle: russh::client::Handle<H>) -> Self {
        Self {
            handle: Arc::new(Mutex::new(handle)),
        }
    }

    pub fn from_arc(handle: Arc<Mutex<russh::client::Handle<H>>>) -> Self {
        Self { handle }
    }
}

impl<H: russh::client::Handler + Send + Sync + 'static> ChannelOpener for HandleChannelOpener<H> {
    type Stream = russh::ChannelStream<russh::client::Msg>;

    async fn open_channel(&self, host: String, port: u16) -> Result<Self::Stream, ChannelOpenError> {
        let handle = self.handle.lock().await;
        if handle.is_closed() {
            return Err(ChannelOpenError::SessionClosed);
        }
        let channel = handle
            .channel_open_direct_tcpip(host, port as u32, "127.0.0.1", 0)
            .await
            .map_err(|_| ChannelOpenError::ChannelOpenFailed)?;
        Ok(channel.into_stream())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt, DuplexStream};

    struct MockChannelOpener {
        fail: bool,
    }

    impl ChannelOpener for MockChannelOpener {
        type Stream = DuplexStream;

        async fn open_channel(
            &self,
            _host: String,
            _port: u16,
        ) -> Result<Self::Stream, ChannelOpenError> {
            if self.fail {
                Err(ChannelOpenError::ChannelOpenFailed)
            } else {
                let (client, _server) = duplex(4096);
                Ok(client)
            }
        }
    }

    fn build_socks5_greeting(methods: &[u8]) -> Vec<u8> {
        let mut buf = vec![0x05, methods.len() as u8];
        buf.extend_from_slice(methods);
        buf
    }

    fn build_socks5_connect_ipv4(addr: [u8; 4], port: u16) -> Vec<u8> {
        let mut buf = vec![0x05, 0x01, 0x00, 0x01];
        buf.extend_from_slice(&addr);
        buf.extend_from_slice(&port.to_be_bytes());
        buf
    }

    fn build_socks5_connect_domain(domain: &str, port: u16) -> Vec<u8> {
        let mut buf = vec![0x05, 0x01, 0x00, 0x03];
        buf.push(domain.len() as u8);
        buf.extend_from_slice(domain.as_bytes());
        buf.extend_from_slice(&port.to_be_bytes());
        buf
    }

    fn build_socks5_connect_ipv6(addr: [u8; 16], port: u16) -> Vec<u8> {
        let mut buf = vec![0x05, 0x01, 0x00, 0x04];
        buf.extend_from_slice(&addr);
        buf.extend_from_slice(&port.to_be_bytes());
        buf
    }

    async fn do_handshake(client: &mut DuplexStream) -> [u8; 2] {
        client.write_all(&build_socks5_greeting(&[0x00])).await.unwrap();
        client.flush().await.unwrap();
        let mut resp = [0u8; 2];
        client.read_exact(&mut resp).await.unwrap();
        resp
    }

    async fn do_connect_ipv4(client: &mut DuplexStream, addr: [u8; 4], port: u16) -> Vec<u8> {
        client
            .write_all(&build_socks5_connect_ipv4(addr, port))
            .await
            .unwrap();
        client.flush().await.unwrap();
        let mut reply_buf = [0u8; 10];
        client.read_exact(&mut reply_buf).await.unwrap();
        reply_buf.to_vec()
    }

    #[tokio::test]
    async fn handshake_no_auth_method() {
        let (mut client, server) = duplex(4096);
        let opener = MockChannelOpener { fail: false };

        let server_handle = tokio::spawn(async move {
            handle_socks5_connection(server, Arc::new(opener)).await
        });

        let resp = do_handshake(&mut client).await;
        assert_eq!(resp, [0x05, 0x00]);

        let reply_buf = do_connect_ipv4(&mut client, [127, 0, 0, 1], 80).await;
        assert_eq!(reply_buf[0], 0x05);
        assert_eq!(reply_buf[1], 0x00);

        drop(client);
        let _ = server_handle.await;
    }

    #[tokio::test]
    async fn handshake_rejects_no_acceptable_method() {
        let (mut client, server) = duplex(4096);
        let opener = MockChannelOpener { fail: false };

        let server_handle = tokio::spawn(async move {
            handle_socks5_connection(server, Arc::new(opener)).await
        });

        client
            .write_all(&build_socks5_greeting(&[0x02]))
            .await
            .unwrap();
        client.flush().await.unwrap();

        let mut resp = [0u8; 2];
        client.read_exact(&mut resp).await.unwrap();
        assert_eq!(resp, [0x05, 0xFF]);

        drop(client);
        let result = server_handle.await.unwrap();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            Socks5Error::NoAcceptableAuth
        ));
    }

    #[tokio::test]
    async fn address_type_ipv4() {
        let (mut client, server) = duplex(4096);
        let opener = MockChannelOpener { fail: false };

        let server_handle = tokio::spawn(async move {
            handle_socks5_connection(server, Arc::new(opener)).await
        });

        do_handshake(&mut client).await;
        let reply_buf = do_connect_ipv4(&mut client, [10, 0, 0, 1], 443).await;
        assert_eq!(reply_buf[1], 0x00);

        drop(client);
        let _ = server_handle.await;
    }

    #[tokio::test]
    async fn address_type_domain() {
        let (mut client, server) = duplex(4096);
        let opener = MockChannelOpener { fail: false };

        let server_handle = tokio::spawn(async move {
            handle_socks5_connection(server, Arc::new(opener)).await
        });

        do_handshake(&mut client).await;

        client
            .write_all(&build_socks5_connect_domain("example.com", 443))
            .await
            .unwrap();
        client.flush().await.unwrap();

        let mut reply_buf = [0u8; 10];
        client.read_exact(&mut reply_buf).await.unwrap();
        assert_eq!(reply_buf[1], 0x00);

        drop(client);
        let _ = server_handle.await;
    }

    #[tokio::test]
    async fn address_type_ipv6() {
        let (mut client, server) = duplex(4096);
        let opener = MockChannelOpener { fail: false };

        let server_handle = tokio::spawn(async move {
            handle_socks5_connection(server, Arc::new(opener)).await
        });

        do_handshake(&mut client).await;

        let ipv6_addr = [0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
        client
            .write_all(&build_socks5_connect_ipv6(ipv6_addr, 443))
            .await
            .unwrap();
        client.flush().await.unwrap();

        let mut reply_buf = [0u8; 10];
        client.read_exact(&mut reply_buf).await.unwrap();
        assert_eq!(reply_buf[0], 0x05);
        assert_eq!(reply_buf[1], 0x00);

        drop(client);
        let _ = server_handle.await;
    }

    #[tokio::test]
    async fn channel_open_failure_returns_socks5_error() {
        let (mut client, server) = duplex(4096);
        let opener = MockChannelOpener { fail: true };

        let server_handle = tokio::spawn(async move {
            handle_socks5_connection(server, Arc::new(opener)).await
        });

        do_handshake(&mut client).await;
        let reply_buf = do_connect_ipv4(&mut client, [10, 0, 0, 1], 80).await;
        assert_eq!(reply_buf[0], 0x05);
        assert_eq!(reply_buf[1], 0x05);

        drop(client);
        let _ = server_handle.await;
    }

    #[tokio::test]
    async fn unsupported_command_returns_error() {
        let (mut client, server) = duplex(4096);
        let opener = MockChannelOpener { fail: false };

        let server_handle = tokio::spawn(async move {
            handle_socks5_connection(server, Arc::new(opener)).await
        });

        do_handshake(&mut client).await;

        let mut bind_req = vec![0x05, 0x02, 0x00, 0x01];
        bind_req.extend_from_slice(&[127, 0, 0, 1]);
        bind_req.extend_from_slice(&80u16.to_be_bytes());
        client.write_all(&bind_req).await.unwrap();
        client.flush().await.unwrap();

        let mut reply_buf = [0u8; 10];
        client.read_exact(&mut reply_buf).await.unwrap();
        assert_eq!(reply_buf[1], 0x07);

        drop(client);
        let _ = server_handle.await;
    }

    #[tokio::test]
    async fn bidirectional_proxy_flow() {
        let (mut client_sock, server_sock) = duplex(4096);
        let (ssh_client, mut ssh_server) = duplex(4096);

        let ssh_stream = Arc::new(Mutex::new(Some(ssh_client)));

        struct ProxyOpener {
            stream: Arc<Mutex<Option<DuplexStream>>>,
        }

        impl ChannelOpener for ProxyOpener {
            type Stream = DuplexStream;

            async fn open_channel(
                &self,
                _host: String,
                _port: u16,
            ) -> Result<Self::Stream, ChannelOpenError> {
                self.stream
                    .lock()
                    .await
                    .take()
                    .ok_or(ChannelOpenError::ChannelOpenFailed)
            }
        }

        let opener = ProxyOpener {
            stream: Arc::clone(&ssh_stream),
        };

        let server_handle = tokio::spawn(async move {
            handle_socks5_connection(server_sock, Arc::new(opener)).await
        });

        do_handshake(&mut client_sock).await;
        let reply_buf = do_connect_ipv4(&mut client_sock, [127, 0, 0, 1], 80).await;
        assert_eq!(reply_buf[1], 0x00);

        let test_data = b"hello through tunnel";
        client_sock.write_all(test_data).await.unwrap();
        client_sock.flush().await.unwrap();

        let mut received = vec![0u8; test_data.len()];
        AsyncReadExt::read_exact(&mut ssh_server, &mut received)
            .await
            .unwrap();
        assert_eq!(&received, test_data);

        let echo_data = b"response from tunnel";
        ssh_server.write_all(echo_data).await.unwrap();
        ssh_server.flush().await.unwrap();

        let mut received_back = vec![0u8; echo_data.len()];
        client_sock.read_exact(&mut received_back).await.unwrap();
        assert_eq!(&received_back, echo_data);

        drop(client_sock);
        drop(ssh_server);
        let _ = server_handle.await;
    }

    #[tokio::test]
    async fn default_listen_address() {
        let opener = MockChannelOpener { fail: false };
        let server = Socks5Server::new(opener);
        assert_eq!(server.listen_addr(), "127.0.0.1:1080".parse().unwrap());
    }

    #[tokio::test]
    async fn custom_listen_address() {
        let opener = MockChannelOpener { fail: false };
        let server = Socks5Server::with_addr(opener, "127.0.0.1:9050");
        assert_eq!(server.listen_addr(), "127.0.0.1:9050".parse().unwrap());
    }
}