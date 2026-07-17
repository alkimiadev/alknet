//! SOCKS5 proxy support for `AlknetClient` (ADR-090).
//!
//! When a proxy is configured via `with_socks5_proxy`, the rustls dials
//! route their transport through the proxy — the hub sees the proxy's IP,
//! not the client's.
//!
//! Feature-gated on `socks5`. The `Socks5UdpSocket` additionally requires
//! the `quinn` feature (it implements `quinn::AsyncUdpSocket`).

use std::net::SocketAddr;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[cfg(feature = "quinn")]
use std::io;
#[cfg(feature = "quinn")]
use std::pin::Pin;
#[cfg(feature = "quinn")]
use std::sync::{Arc, Mutex};
#[cfg(feature = "quinn")]
use std::task::{Context, Poll, Waker};

#[cfg(feature = "quinn")]
use crate::error::ClientDialError;

/// Configuration for a SOCKS5 proxy (ADR-090).
///
/// When set on `AlknetClient` via `with_socks5_proxy`, all rustls dials
/// route their transport through this proxy: UDP ASSOCIATE for `dial_quic`,
/// CONNECT for `dial_tcp_tls`. The proxy config comes from `Capabilities` /
/// the assembly layer (ADR-014), never from environment variables.
#[derive(Debug, Clone)]
pub struct Socks5ProxyConfig {
    /// The proxy's TCP address (where the SOCKS5 control connection
    /// connects). For UDP ASSOCIATE (the QUIC dial), the proxy replies
    /// with a UDP relay address that may differ; the dial uses that.
    pub addr: SocketAddr,
    /// Optional username/password auth (RFC 1929). None = no-auth.
    pub credentials: Option<Socks5Credentials>,
}

/// SOCKS5 username/password credentials (RFC 1929).
#[derive(Debug, Clone)]
pub struct Socks5Credentials {
    pub username: String,
    pub password: String,
}

/// A `quinn::AsyncUdpSocket` implementation that tunnels QUIC datagrams
/// through a SOCKS5 UDP ASSOCIATE tunnel.
///
/// The implementation follows the pattern validated by the quinn-proxy PoC
/// (`docs/research/quinn-quic-proxy/findings.md`).
///
/// Requires both `socks5` and `quinn` features.
#[cfg(feature = "quinn")]
pub struct Socks5UdpSocket {
    socket: std::net::UdpSocket,
    relay_addr: SocketAddr,
    local_addr: SocketAddr,
    _control: TcpStream,
}

#[cfg(feature = "quinn")]
impl Socks5UdpSocket {
    /// Perform the SOCKS5 UDP ASSOCIATE handshake and return a socket
    /// that tunnels QUIC datagrams through the proxy.
    pub async fn bind(proxy: &Socks5ProxyConfig) -> Result<Self, ClientDialError> {
        let mut control = TcpStream::connect(proxy.addr)
            .await
            .map_err(|e| ClientDialError::Connect(e.to_string()))?;

        socks5_handshake(&mut control, proxy).await?;

        let relay_addr = socks5_udp_associate(&mut control).await?;

        let socket = std::net::UdpSocket::bind("0.0.0.0:0")
            .map_err(|e| ClientDialError::Connect(e.to_string()))?;
        socket
            .set_nonblocking(true)
            .map_err(|e| ClientDialError::Connect(e.to_string()))?;
        let local_addr = socket
            .local_addr()
            .map_err(|e| ClientDialError::Connect(e.to_string()))?;

        Ok(Self {
            socket,
            relay_addr,
            local_addr,
            _control: control,
        })
    }
}

#[cfg(feature = "quinn")]
impl quinn::AsyncUdpSocket for Socks5UdpSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn quinn::UdpPoller>> {
        Box::pin(UdpPollerImpl {
            socket: self.socket.try_clone().ok(),
            waker: Mutex::new(None),
        })
    }

    fn try_send(&self, transmit: &quinn::udp::Transmit) -> io::Result<()> {
        let mut buf = Vec::with_capacity(10 + transmit.contents.len());
        buf.extend_from_slice(&[0u8, 0, 0]);
        match transmit.destination {
            SocketAddr::V4(addr) => {
                buf.push(0x01);
                buf.extend_from_slice(&addr.ip().octets());
                buf.extend_from_slice(&addr.port().to_be_bytes());
            }
            SocketAddr::V6(addr) => {
                buf.push(0x04);
                buf.extend_from_slice(&addr.ip().octets());
                buf.extend_from_slice(&addr.port().to_be_bytes());
            }
        }
        buf.extend_from_slice(transmit.contents);

        let sent = self.socket.send_to(&buf, self.relay_addr)?;
        if sent < buf.len() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "partial send",
            ));
        }
        Ok(())
    }

    fn poll_recv(
        &self,
        _cx: &mut Context,
        bufs: &mut [io::IoSliceMut<'_>],
        meta: &mut [quinn::udp::RecvMeta],
    ) -> Poll<io::Result<usize>> {
        let mut buf = [0u8; 65536];
        match self.socket.recv_from(&mut buf) {
            Ok((n, _src)) => {
                if n < 10 {
                    return Poll::Ready(Ok(0));
                }
                let header_end = 3;
                let atyp = buf[header_end];
                let addr_len: usize = match atyp {
                    0x01 => 4,
                    0x04 => 16,
                    _ => return Poll::Ready(Ok(0)),
                };
                let payload_start = header_end + 1 + addr_len + 2;
                if n < payload_start {
                    return Poll::Ready(Ok(0));
                }
                let payload = &buf[payload_start..n];
                let copy_len = payload.len().min(bufs.iter().map(|b| b.len()).sum());
                let mut offset = 0;
                for b in bufs.iter_mut() {
                    let end = (offset + b.len()).min(copy_len);
                    if offset < end {
                        b.copy_from_slice(&payload[offset..end]);
                    }
                    offset = end;
                    if offset >= copy_len {
                        break;
                    }
                }
                meta[0] = quinn::udp::RecvMeta {
                    len: copy_len,
                    stride: copy_len,
                    addr: self.relay_addr,
                    ecn: None,
                    dst_ip: None,
                };
                Poll::Ready(Ok(1))
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.local_addr)
    }

    fn may_fragment(&self) -> bool {
        false
    }
}

#[cfg(feature = "quinn")]
struct UdpPollerImpl {
    socket: Option<std::net::UdpSocket>,
    waker: Mutex<Option<Waker>>,
}

#[cfg(feature = "quinn")]
impl std::fmt::Debug for UdpPollerImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UdpPollerImpl").finish()
    }
}

#[cfg(feature = "quinn")]
impl quinn::UdpPoller for UdpPollerImpl {
    fn poll_writable(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        if let Some(ref socket) = self.socket {
            match socket.send_to(&[], socket.local_addr().ok().unwrap_or_else(|| "0.0.0.0:0".parse().unwrap())) {
                Ok(_) => Poll::Ready(Ok(())),
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    *self.waker.lock().unwrap() = Some(cx.waker().clone());
                    Poll::Pending
                }
                Err(e) => Poll::Ready(Err(e)),
            }
        } else {
            Poll::Ready(Ok(()))
        }
    }
}

#[cfg(feature = "quinn")]
impl std::fmt::Debug for Socks5UdpSocket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Socks5UdpSocket")
            .field("relay_addr", &self.relay_addr)
            .field("local_addr", &self.local_addr)
            .finish()
    }
}

/// Perform the SOCKS5 handshake (greeting + auth).
#[cfg(feature = "quinn")]
async fn socks5_handshake(
    stream: &mut TcpStream,
    proxy: &Socks5ProxyConfig,
) -> Result<(), ClientDialError> {
    if let Some(creds) = &proxy.credentials {
        stream
            .write_all(&[0x05, 0x01, 0x02])
            .await
            .map_err(|e| ClientDialError::Proxy(e.to_string()))?;
        let mut resp = [0u8; 2];
        stream
            .read_exact(&mut resp)
            .await
            .map_err(|e| ClientDialError::Proxy(e.to_string()))?;
        if resp[0] != 0x05 || resp[1] != 0x02 {
            return Err(ClientDialError::Proxy(
                "SOCKS5 server does not support username/password auth".into(),
            ));
        }
        let mut auth_msg = Vec::with_capacity(3 + creds.username.len() + creds.password.len());
        auth_msg.push(0x01);
        auth_msg.push(creds.username.len() as u8);
        auth_msg.extend_from_slice(creds.username.as_bytes());
        auth_msg.push(creds.password.len() as u8);
        auth_msg.extend_from_slice(creds.password.as_bytes());
        stream
            .write_all(&auth_msg)
            .await
            .map_err(|e| ClientDialError::Proxy(e.to_string()))?;
        let mut auth_resp = [0u8; 2];
        stream
            .read_exact(&mut auth_resp)
            .await
            .map_err(|e| ClientDialError::Proxy(e.to_string()))?;
        if auth_resp[1] != 0x00 {
            return Err(ClientDialError::Proxy(
                "SOCKS5 username/password authentication failed".into(),
            ));
        }
    } else {
        stream
            .write_all(&[0x05, 0x01, 0x00])
            .await
            .map_err(|e| ClientDialError::Proxy(e.to_string()))?;
        let mut resp = [0u8; 2];
        stream
            .read_exact(&mut resp)
            .await
            .map_err(|e| ClientDialError::Proxy(e.to_string()))?;
        if resp[0] != 0x05 || resp[1] != 0x00 {
            return Err(ClientDialError::Proxy(
                "SOCKS5 server rejected no-auth method".into(),
            ));
        }
    }
    Ok(())
}

/// Perform the SOCKS5 UDP ASSOCIATE request and return the relay address.
#[cfg(feature = "quinn")]
async fn socks5_udp_associate(stream: &mut TcpStream) -> Result<SocketAddr, ClientDialError> {
    let req = vec![0x05, 0x03, 0x00, 0x01, 0, 0, 0, 0, 0, 0];
    stream
        .write_all(&req)
        .await
        .map_err(|e| ClientDialError::Proxy(e.to_string()))?;

    let mut resp = [0u8; 10];
    stream
        .read_exact(&mut resp)
        .await
        .map_err(|e| ClientDialError::Proxy(e.to_string()))?;

    if resp[0] != 0x05 {
        return Err(ClientDialError::Proxy("invalid SOCKS5 version in reply".into()));
    }
    if resp[1] != 0x00 {
        return Err(ClientDialError::Proxy(format!(
            "SOCKS5 UDP ASSOCIATE rejected with code {}",
            resp[1]
        )));
    }

    let bind_port = u16::from_be_bytes([resp[8], resp[9]]);
    let bind_addr = match resp[3] {
        0x01 => {
            let mut addr = [0u8; 4];
            stream
                .read_exact(&mut addr)
                .await
                .map_err(|e| ClientDialError::Proxy(e.to_string()))?;
            SocketAddr::new(std::net::Ipv4Addr::from(addr).into(), bind_port)
        }
        0x04 => {
            let mut addr = [0u8; 16];
            stream
                .read_exact(&mut addr)
                .await
                .map_err(|e| ClientDialError::Proxy(e.to_string()))?;
            SocketAddr::new(std::net::Ipv6Addr::from(addr).into(), bind_port)
        }
        _ => {
            return Err(ClientDialError::Proxy(format!(
                "unsupported address type in UDP ASSOCIATE reply: {}",
                resp[3]
            )))
        }
    };

    Ok(bind_addr)
}

/// Perform the SOCKS5 CONNECT handshake to the target address.
pub async fn socks5_connect(
    stream: &mut TcpStream,
    proxy: &Socks5ProxyConfig,
    target: SocketAddr,
) -> Result<(), String> {
    socks5_handshake_connect(stream, proxy).await?;

    let mut req = vec![0x05, 0x01, 0x00];
    match target {
        SocketAddr::V4(addr) => {
            req.push(0x01);
            req.extend_from_slice(&addr.ip().octets());
            req.extend_from_slice(&addr.port().to_be_bytes());
        }
        SocketAddr::V6(addr) => {
            req.push(0x04);
            req.extend_from_slice(&addr.ip().octets());
            req.extend_from_slice(&addr.port().to_be_bytes());
        }
    }
    stream
        .write_all(&req)
        .await
        .map_err(|e| format!("SOCKS5 CONNECT write: {e}"))?;

    let mut resp = [0u8; 10];
    stream
        .read_exact(&mut resp)
        .await
        .map_err(|e| format!("SOCKS5 CONNECT read: {e}"))?;

    if resp[0] != 0x05 {
        return Err("invalid SOCKS5 version in CONNECT reply".into());
    }
    if resp[1] != 0x00 {
        return Err(format!("SOCKS5 CONNECT rejected with code {}", resp[1]));
    }

    match resp[3] {
        0x01 => {
            let mut _addr = [0u8; 4];
            stream
                .read_exact(&mut _addr)
                .await
                .map_err(|e| format!("SOCKS5 CONNECT bind addr read: {e}"))?;
        }
        0x04 => {
            let mut _addr = [0u8; 16];
            stream
                .read_exact(&mut _addr)
                .await
                .map_err(|e| format!("SOCKS5 CONNECT bind addr read: {e}"))?;
        }
        _ => {}
    }

    Ok(())
}

async fn socks5_handshake_connect(
    stream: &mut TcpStream,
    proxy: &Socks5ProxyConfig,
) -> Result<(), String> {
    if let Some(creds) = &proxy.credentials {
        stream
            .write_all(&[0x05, 0x01, 0x02])
            .await
            .map_err(|e| format!("SOCKS5 greeting write: {e}"))?;
        let mut resp = [0u8; 2];
        stream
            .read_exact(&mut resp)
            .await
            .map_err(|e| format!("SOCKS5 greeting read: {e}"))?;
        if resp[0] != 0x05 || resp[1] != 0x02 {
            return Err("SOCKS5 server does not support username/password auth".into());
        }
        let mut auth_msg = Vec::with_capacity(3 + creds.username.len() + creds.password.len());
        auth_msg.push(0x01);
        auth_msg.push(creds.username.len() as u8);
        auth_msg.extend_from_slice(creds.username.as_bytes());
        auth_msg.push(creds.password.len() as u8);
        auth_msg.extend_from_slice(creds.password.as_bytes());
        stream
            .write_all(&auth_msg)
            .await
            .map_err(|e| format!("SOCKS5 auth write: {e}"))?;
        let mut auth_resp = [0u8; 2];
        stream
            .read_exact(&mut auth_resp)
            .await
            .map_err(|e| format!("SOCKS5 auth read: {e}"))?;
        if auth_resp[1] != 0x00 {
            return Err("SOCKS5 username/password authentication failed".into());
        }
    } else {
        stream
            .write_all(&[0x05, 0x01, 0x00])
            .await
            .map_err(|e| format!("SOCKS5 greeting write: {e}"))?;
        let mut resp = [0u8; 2];
        stream
            .read_exact(&mut resp)
            .await
            .map_err(|e| format!("SOCKS5 greeting read: {e}"))?;
        if resp[0] != 0x05 || resp[1] != 0x00 {
            return Err("SOCKS5 server rejected no-auth method".into());
        }
    }
    Ok(())
}
