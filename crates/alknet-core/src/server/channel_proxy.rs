//! Outbound connection proxy for SSH channel targets.
//!
//! Connects to the requested `host:port` either directly, via SOCKS5 proxy, or
//! via HTTP CONNECT proxy, then proxies bytes bidirectionally between the SSH
//! channel and the outbound TCP stream.

use std::net::SocketAddr;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::handler::{ProxyConfig, ProxyMode};

#[derive(Debug, thiserror::Error)]
pub enum ChannelProxyError {
    #[error("connection refused")]
    ConnectionRefused,
    #[error("target unreachable")]
    TargetUnreachable,
    #[error("socks5 proxy handshake failed")]
    Socks5HandshakeFailed,
    #[error("socks5 proxy rejected connection")]
    Socks5ProxyRejected,
    #[error("http connect proxy handshake failed")]
    HttpConnectHandshakeFailed,
    #[error("http connect proxy rejected: {0}")]
    HttpConnectProxyRejected(String),
    #[error("io error")]
    Io(#[from] std::io::Error),
}

pub async fn connect_outbound(
    target: SocketAddr,
    proxy: &ProxyConfig,
) -> Result<TcpStream, ChannelProxyError> {
    match &proxy.mode {
        ProxyMode::Direct => connect_direct(target).await,
        ProxyMode::Socks5(addr) => connect_socks5(target, *addr).await,
        ProxyMode::HttpConnect(addr) => connect_http_connect(target, *addr).await,
    }
}

async fn connect_direct(target: SocketAddr) -> Result<TcpStream, ChannelProxyError> {
    TcpStream::connect(target)
        .await
        .map_err(|e| map_connection_error(e, target))
}

async fn connect_socks5(target: SocketAddr, proxy_addr: SocketAddr) -> Result<TcpStream, ChannelProxyError> {
    let mut stream = TcpStream::connect(proxy_addr)
        .await
        .map_err(ChannelProxyError::from)?;

    stream.write_all(&[0x05, 0x01, 0x00]).await?;
    stream.flush().await?;

    let mut resp = [0u8; 2];
    stream.read_exact(&mut resp).await?;
    if resp[0] != 0x05 || resp[1] != 0x00 {
        return Err(ChannelProxyError::Socks5HandshakeFailed);
    }

    let ip_bytes = target.ip().to_string();
    let mut connect_req = vec![0x05, 0x01, 0x00, 0x03];
    connect_req.push(ip_bytes.len() as u8);
    connect_req.extend_from_slice(ip_bytes.as_bytes());
    connect_req.extend_from_slice(&target.port().to_be_bytes());
    stream.write_all(&connect_req).await?;
    stream.flush().await?;

    let mut reply_header = [0u8; 4];
    stream.read_exact(&mut reply_header).await?;
    if reply_header[0] != 0x05 {
        return Err(ChannelProxyError::Socks5HandshakeFailed);
    }
    if reply_header[1] != 0x00 {
        return Err(ChannelProxyError::Socks5ProxyRejected);
    }

    let atyp = reply_header[3];
    match atyp {
        0x01 => {
            let mut _addr = [0u8; 4];
            stream.read_exact(&mut _addr).await?;
        }
        0x04 => {
            let mut _addr = [0u8; 16];
            stream.read_exact(&mut _addr).await?;
        }
        0x03 => {
            let len = stream.read_u8().await?;
            let mut _domain = vec![0u8; len as usize];
            stream.read_exact(&mut _domain).await?;
        }
        _ => {
            return Err(ChannelProxyError::Socks5HandshakeFailed);
        }
    }
    let mut _port = [0u8; 2];
    stream.read_exact(&mut _port).await?;

    Ok(stream)
}

async fn connect_http_connect(
    target: SocketAddr,
    proxy_addr: SocketAddr,
) -> Result<TcpStream, ChannelProxyError> {
    let mut stream = TcpStream::connect(proxy_addr)
        .await
        .map_err(ChannelProxyError::from)?;

    let connect_request = format!(
        "CONNECT {}:{} HTTP/1.1\r\nHost: {}:{}\r\n\r\n",
        target.ip(),
        target.port(),
        target.ip(),
        target.port()
    );
    stream.write_all(connect_request.as_bytes()).await?;
    stream.flush().await?;

    let mut response = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            return Err(ChannelProxyError::HttpConnectHandshakeFailed);
        }
        response.extend_from_slice(&buf[..n]);
        if response.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }

    let response_str = String::from_utf8_lossy(&response);
    let status_line = response_str
        .lines()
        .next()
        .unwrap_or("");

    if status_line.contains("200") {
        Ok(stream)
    } else {
        Err(ChannelProxyError::HttpConnectProxyRejected(
            status_line.to_string(),
        ))
    }
}

fn map_connection_error(e: std::io::Error, _target: SocketAddr) -> ChannelProxyError {
    match e.kind() {
        std::io::ErrorKind::ConnectionRefused => ChannelProxyError::ConnectionRefused,
        std::io::ErrorKind::AddrNotAvailable
        | std::io::ErrorKind::NetworkUnreachable
        | std::io::ErrorKind::HostUnreachable => ChannelProxyError::TargetUnreachable,
        _ => ChannelProxyError::Io(e),
    }
}

pub async fn proxy_channel<S>(channel: S, target: SocketAddr, proxy: &ProxyConfig)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    if let Ok(outbound) = connect_outbound(target, proxy).await {
        let (mut read_chan, mut write_chan) = tokio::io::split(channel);
        let (mut read_out, mut write_out) = outbound.into_split();

        let client_to_target = tokio::spawn(async move {
            let _ = tokio::io::copy(&mut read_chan, &mut write_out).await;
            let _ = write_out.shutdown().await;
        });

        let target_to_client = tokio::spawn(async move {
            let _ = tokio::io::copy(&mut read_out, &mut write_chan).await;
            let _ = write_chan.shutdown().await;
        });

        let _ = client_to_target.await;
        let _ = target_to_client.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt, DuplexStream};
    use tokio::net::TcpListener;

    fn direct_config() -> ProxyConfig {
        ProxyConfig {
            mode: ProxyMode::Direct,
        }
    }

    fn socks5_config(addr: SocketAddr) -> ProxyConfig {
        ProxyConfig {
            mode: ProxyMode::Socks5(addr),
        }
    }

    fn http_connect_config(addr: SocketAddr) -> ProxyConfig {
        ProxyConfig {
            mode: ProxyMode::HttpConnect(addr),
        }
    }

    #[tokio::test]
    async fn direct_connection_to_echo_server() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 64];
            let n = sock.read(&mut buf).await.unwrap();
            sock.write_all(&buf[..n]).await.unwrap();
        });

        let stream = connect_outbound(addr, &direct_config()).await.unwrap();
        let (mut read, mut write) = stream.into_split();
        write.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 5];
        read.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");

        let _ = server.await;
    }

    #[tokio::test]
    async fn direct_connection_target_unreachable() {
        let target: SocketAddr = "240.0.0.1:1".parse().unwrap();
        let result = connect_outbound(target, &direct_config()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn socks5_proxy_handshake() {
        let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr = proxy_listener.local_addr().unwrap();

        let target_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_addr = target_listener.local_addr().unwrap();

        let target_server = tokio::spawn(async move {
            let (mut sock, _) = target_listener.accept().await.unwrap();
            let mut buf = [0u8; 64];
            let n = sock.read(&mut buf).await.unwrap();
            sock.write_all(&buf[..n]).await.unwrap();
        });

        let proxy_server = tokio::spawn(async move {
            let (mut proxy_sock, _) = proxy_listener.accept().await.unwrap();

            let mut greeting = [0u8; 3];
            proxy_sock.read_exact(&mut greeting).await.unwrap();
            assert_eq!(greeting[0], 0x05);
            proxy_sock.write_all(&[0x05, 0x00]).await.unwrap();

            let mut req_header = [0u8; 4];
            proxy_sock.read_exact(&mut req_header).await.unwrap();
            assert_eq!(req_header[0], 0x05);
            assert_eq!(req_header[1], 0x01);

            let atyp = req_header[3];
            assert_eq!(atyp, 0x03);

            let domain_len = proxy_sock.read_u8().await.unwrap() as usize;
            let mut domain = vec![0u8; domain_len];
            proxy_sock.read_exact(&mut domain).await.unwrap();
            let mut port_bytes = [0u8; 2];
            proxy_sock.read_exact(&mut port_bytes).await.unwrap();

            let target: SocketAddr = format!(
                "{}:{}",
                String::from_utf8_lossy(&domain),
                u16::from_be_bytes(port_bytes)
            )
            .parse()
            .unwrap();

            let reply = vec![
                0x05, 0x00, 0x00, 0x01,
                0, 0, 0, 0,
                0, 0,
            ];
            proxy_sock.write_all(&reply).await.unwrap();

            let mut target_stream = TcpStream::connect(target).await.unwrap();
            let _ = tokio::io::copy_bidirectional(&mut proxy_sock, &mut target_stream).await;
        });

        let config = socks5_config(proxy_addr);
        let mut stream = connect_outbound(target_addr, &config).await.unwrap();
        stream.write_all(b"hello socks").await.unwrap();
        let mut buf = [0u8; 11];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello socks");
        drop(stream);

        let _ = target_server.await;
        let _ = proxy_server.await;
    }

    #[tokio::test]
    async fn socks5_proxy_rejected() {
        let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr = proxy_listener.local_addr().unwrap();

        let proxy_server = tokio::spawn(async move {
            let (mut proxy_sock, _) = proxy_listener.accept().await.unwrap();

            let mut greeting = [0u8; 3];
            proxy_sock.read_exact(&mut greeting).await.unwrap();
            proxy_sock.write_all(&[0x05, 0x00]).await.unwrap();

            let mut req_header = [0u8; 4];
            proxy_sock.read_exact(&mut req_header).await.unwrap();

            let domain_len = proxy_sock.read_u8().await.unwrap() as usize;
            let mut domain = vec![0u8; domain_len];
            proxy_sock.read_exact(&mut domain).await.unwrap();
            let mut port_bytes = [0u8; 2];
            proxy_sock.read_exact(&mut port_bytes).await.unwrap();

            let reply = vec![
                0x05, 0x05, 0x00, 0x01,
                0, 0, 0, 0,
                0, 0,
            ];
            proxy_sock.write_all(&reply).await.unwrap();
        });

        let target: SocketAddr = "127.0.0.1:9999".parse().unwrap();
        let config = socks5_config(proxy_addr);
        let result = connect_outbound(target, &config).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ChannelProxyError::Socks5ProxyRejected
        ));

        let _ = proxy_server.await;
    }

    #[tokio::test]
    async fn http_connect_proxy_handshake() {
        let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr = proxy_listener.local_addr().unwrap();

        let target_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_addr = target_listener.local_addr().unwrap();

        let target_server = tokio::spawn(async move {
            let (mut sock, _) = target_listener.accept().await.unwrap();
            let mut buf = [0u8; 64];
            let n = sock.read(&mut buf).await.unwrap();
            sock.write_all(&buf[..n]).await.unwrap();
        });

        let proxy_server = tokio::spawn(async move {
            let (mut proxy_sock, _) = proxy_listener.accept().await.unwrap();

            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            loop {
                let n = proxy_sock.read(&mut buf).await.unwrap();
                request.extend_from_slice(&buf[..n]);
                if request.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }

            let response = "HTTP/1.1 200 Connection Established\r\n\r\n";
            proxy_sock.write_all(response.as_bytes()).await.unwrap();

            let target_str = extract_connect_target(&String::from_utf8_lossy(&request));
            let mut target_stream = TcpStream::connect(target_str).await.unwrap();
            let _ = tokio::io::copy_bidirectional(&mut proxy_sock, &mut target_stream).await;
        });

        let config = http_connect_config(proxy_addr);
        let mut stream = connect_outbound(target_addr, &config).await.unwrap();
        stream.write_all(b"hello http").await.unwrap();
        let mut buf = [0u8; 10];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello http");
        drop(stream);

        let _ = target_server.await;
        let _ = proxy_server.await;
    }

    fn extract_connect_target(request: &str) -> String {
        let connect_line = request.lines().next().unwrap_or("");
        let parts: Vec<&str> = connect_line.split_whitespace().collect();
        if parts.len() >= 2 {
            parts[1].to_string()
        } else {
            String::new()
        }
    }

    #[tokio::test]
    async fn http_connect_proxy_rejected() {
        let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr = proxy_listener.local_addr().unwrap();

        let proxy_server = tokio::spawn(async move {
            let (mut proxy_sock, _) = proxy_listener.accept().await.unwrap();

            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            loop {
                let n = proxy_sock.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..n]);
                if request.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }

            let response = "HTTP/1.1 403 Forbidden\r\n\r\n";
            proxy_sock.write_all(response.as_bytes()).await.unwrap();
        });

        let target: SocketAddr = "127.0.0.1:9999".parse().unwrap();
        let config = http_connect_config(proxy_addr);
        let result = connect_outbound(target, &config).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ChannelProxyError::HttpConnectProxyRejected(msg) => {
                assert!(msg.contains("403"));
            }
            other => panic!("expected HttpConnectProxyRejected, got {:?}", other),
        }

        let _ = proxy_server.await;
    }

    #[tokio::test]
    async fn target_unreachable_returns_appropriate_error() {
        let target: SocketAddr = "240.0.0.1:1".parse().unwrap();
        let result = connect_outbound(target, &direct_config()).await;
        match result.unwrap_err() {
            ChannelProxyError::TargetUnreachable
            | ChannelProxyError::ConnectionRefused
            | ChannelProxyError::Io(_) => {}
            other => panic!("unexpected error type: {:?}", other),
        }
    }

    #[tokio::test]
    async fn socks5_proxy_unreachable() {
        let target: SocketAddr = "127.0.0.1:9999".parse().unwrap();
        let bad_proxy: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let config = socks5_config(bad_proxy);
        let result = connect_outbound(target, &config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn http_connect_proxy_unreachable() {
        let target: SocketAddr = "127.0.0.1:9999".parse().unwrap();
        let bad_proxy: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let config = http_connect_config(bad_proxy);
        let result = connect_outbound(target, &config).await;
        assert!(result.is_err());
    }

    struct MockChannel {
        read_half: tokio::io::ReadHalf<DuplexStream>,
        write_half: tokio::io::WriteHalf<DuplexStream>,
    }

    impl tokio::io::AsyncRead for MockChannel {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::pin::Pin::new(&mut self.get_mut().read_half).poll_read(cx, buf)
        }
    }

    impl tokio::io::AsyncWrite for MockChannel {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::pin::Pin::new(&mut self.get_mut().write_half).poll_write(cx, buf)
        }

        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::pin::Pin::new(&mut self.get_mut().write_half).poll_flush(cx)
        }

        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::pin::Pin::new(&mut self.get_mut().write_half).poll_shutdown(cx)
        }
    }

    fn make_mock_channel() -> (MockChannel, DuplexStream) {
        let (client, server) = duplex(4096);
        let (read_half, write_half) = tokio::io::split(client);
        (
            MockChannel {
                read_half,
                write_half,
            },
            server,
        )
    }

    #[tokio::test]
    async fn proxy_channel_bidirectional_data_flow() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_addr = listener.local_addr().unwrap();

        let echo_server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 64];
            let n = sock.read(&mut buf).await.unwrap();
            sock.write_all(&buf[..n]).await.unwrap();
        });

        let (channel, mut channel_peer) = make_mock_channel();

        let target = target_addr;
        let proxy = direct_config();
        tokio::spawn(async move {
            proxy_channel(channel, target, &proxy).await;
        });

        channel_peer.write_all(b"ping").await.unwrap();
        channel_peer.flush().await.unwrap();

        let mut buf = [0u8; 4];
        channel_peer.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");

        drop(channel_peer);
        let _ = echo_server.await;
    }

    #[tokio::test]
    async fn proxy_channel_target_unreachable_closes_cleanly() {
        let target: SocketAddr = "240.0.0.1:1".parse().unwrap();
        let (channel, _channel_peer) = make_mock_channel();

        let proxy = direct_config();
        proxy_channel(channel, target, &proxy).await;
    }
}