---
id: client/socks5-proxy
name: Implement SOCKS5 proxy support — Socks5ProxyConfig, Socks5Credentials, Socks5UdpSocket, and proxy integration in dial_quic/dial_tcp_tls
status: pending
depends_on: [client/dial-quic, client/dial-tcp-tls]
scope: broad
risk: high
impact: component
level: implementation
---

## Description

Phase 3, Task 7. Implement the SOCKS5 proxy support for `AlknetClient` per ADR-090.
This is the privacy posture: when a proxy is configured via `with_socks5_proxy`, the
rustls dials route their transport through the proxy — the hub sees the proxy's IP,
not the client's.

This task has four parts:
1. `Socks5ProxyConfig` and `Socks5Credentials` types in `src/socks5.rs`
2. `Socks5UdpSocket` — a `quinn::AsyncUdpSocket` impl that tunnels QUIC datagrams through SOCKS5 UDP ASSOCIATE
3. Proxy integration in `dial_quic` (UDP ASSOCIATE path)
4. Proxy integration in `dial_tcp_tls` (CONNECT path)

The iroh proxy path (force relay-only + HTTP-to-SOCKS5 bridge) is deferred — it's applied
at endpoint construction time by the assembly layer, not at dial time (ADR-090 §5).

### Part 1: `Socks5ProxyConfig` and `Socks5Credentials`

```rust
// src/socks5.rs

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
```

### Part 2: `Socks5UdpSocket` — QUIC over SOCKS5 UDP ASSOCIATE

The central technical piece: a `quinn::AsyncUdpSocket` implementation that tunnels
QUIC datagrams through a SOCKS5 UDP ASSOCIATE tunnel. This is the integration glue
between quinn's socket abstraction and the SOCKS5 proxy.

The implementation (~250 lines) follows the pattern validated by the quinn-proxy PoC
(`docs/research/quinn-quic-proxy/findings.md`):

```rust
#[cfg(feature = "socks5")]
struct Socks5UdpSocket {
    // The UDP socket to the proxy's relay address
    socket: tokio::net::UdpSocket,
    // The proxy's UDP relay address (where datagrams are sent)
    relay_addr: SocketAddr,
    // Keep the TCP control connection alive (dropping it tears down the UDP association)
    _control: tokio::net::TcpStream,
}

#[cfg(feature = "socks5")]
impl Socks5UdpSocket {
    /// Perform the SOCKS5 UDP ASSOCIATE handshake and return a socket
    /// that tunnels QUIC datagrams through the proxy.
    async fn bind(proxy: &Socks5ProxyConfig) -> Result<Self, ClientDialError> {
        // 1. TCP control connection to proxy.addr
        // 2. SOCKS5 handshake (no-auth or username/password per RFC 1929)
        // 3. UDP ASSOCIATE request (CMD = 0x03)
        // 4. Receive the proxy's UDP relay address
        // 5. Bind a local UDP socket
        // 6. Return Socks5UdpSocket with the relay address
    }
}

#[cfg(feature = "socks5")]
impl quinn::AsyncUdpSocket for Socks5UdpSocket {
    fn poll_send(
        &self,
        state: &quinn::udp::UdpState,
        cx: &mut std::task::Context,
        transmits: &[quinn::udp::Transmit],
    ) -> std::task::Poll<Result<usize, io::Error>> {
        // For each transmit:
        // 1. Prepend the SOCKS5 UDP header (RSV + FRAG + ATYP + DST.ADDR + DST.PORT)
        // 2. Send the wrapped datagram to the proxy's relay address
    }

    fn poll_recv(
        &self,
        cx: &mut std::task::Context,
        bufs: &[std::io::IoSliceMut],
        meta: &[quinn::udp::RecvMeta],
    ) -> std::task::Poll<io::Result<usize>> {
        // 1. Receive a datagram from the proxy
        // 2. Strip the SOCKS5 UDP header
        // 3. Fill in RecvMeta (addr, len, stride)
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    fn may_fragment(&self) -> bool {
        false // SOCKS5 UDP has no fragmentation support
    }
}
```

**Key limitations (accepted, documented):**
- **ECN is lost.** The SOCKS5 UDP header carries no ECN bits, so the proxied QUIC
  path falls back to non-ECN congestion control. A performance cost on congested
  links, not a correctness issue.
- **`may_fragment() == false`.** SOCKS5 UDP has no fragmentation mechanism.
- **The proxy must support UDP ASSOCIATE.** `ssh -D` does not work; a UDP-capable
  SOCKS5 daemon is needed. The dial surfaces a clear error when the proxy lacks
  UDP support.

### Part 3: Proxy integration in `dial_quic`

When `self.socks5` is `Some`, `dial_quic` does NOT use the pre-built quinn endpoint.
Instead, it:
1. Builds the `TlsClientConfig` and `quinn::ClientConfig` as usual
2. Creates a `Socks5UdpSocket` via `Socks5UdpSocket::bind(&proxy)`
3. Builds a temporary quinn endpoint via `quinn::Endpoint::new_with_abstract_socket`
4. Connects through that endpoint

```rust
#[cfg(feature = "quinn")]
pub async fn dial_quic(...) -> Result<Connection, ClientDialError> {
    let tls_config = TlsClientConfig::new(creds, alpn)?;
    let client_config = tls_config.for_quinn()?;

    let conn = if let Some(proxy) = &self.socks5 {
        #[cfg(feature = "socks5")]
        {
            let socket = Socks5UdpSocket::bind(proxy).await?;
            let mut endpoint = quinn::Endpoint::new_with_abstract_socket(
                quinn::EndpointConfig::default(),
                Some(socket.local_addr()?),
                socket,
                Arc::new(quinn::TokioRuntime),
            ).map_err(|e| ClientDialError::Connect(e.to_string()))?;
            endpoint
                .connect_with(client_config, addr, server_name)
                .map_err(|e| ClientDialError::Connect(e.to_string()))?
                .await
                .map_err(|e| ClientDialError::Connect(e.to_string()))?
        }
        #[cfg(not(feature = "socks5"))]
        unreachable!()
    } else {
        // Direct path (implemented in client/dial-quic)
        let endpoint = self.quinn.as_ref()
            .ok_or(ClientDialError::NoTransport { transport: "quinn" })?;
        endpoint
            .connect_with(client_config, addr, server_name)
            .map_err(|e| ClientDialError::Connect(e.to_string()))?
            .await
            .map_err(|e| ClientDialError::Connect(e.to_string()))?
    };

    Ok(Connection::from_quinn_with_alpn(conn, alpn.to_vec()))
}
```

### Part 4: Proxy integration in `dial_tcp_tls`

When `self.socks5` is `Some`, `dial_tcp_tls`:
1. Connects a `TcpStream` to the proxy's address
2. Performs the SOCKS5 CONNECT handshake (RFC 1928 §3) to the target `addr`
3. Wraps the resulting stream in `TlsConnector` as before

```rust
#[cfg(feature = "tcp")]
pub async fn dial_tcp_tls(...) -> Result<Connection, ClientDialError> {
    let tls_config = TlsClientConfig::new(creds, alpn)?;
    let connector = /* build or use pre-built TlsConnector */;

    let tls_stream = if let Some(proxy) = &self.socks5 {
        #[cfg(feature = "socks5")]
        {
            // 1. Connect to proxy
            let mut tcp = TcpStream::connect(proxy.addr)
                .await
                .map_err(|e| ClientDialError::Connect(e.to_string()))?;
            // 2. SOCKS5 CONNECT handshake to target addr
            socks5_connect(&mut tcp, proxy, addr)
                .await
                .map_err(|e| ClientDialError::Proxy(e))?;
            // 3. TLS over proxied stream
            let server_name = rustls::pki_types::ServerName::try_from(host)
                .map_err(|e| ClientDialError::Connect(e.to_string()))?;
            connector
                .connect(server_name, tcp)
                .await
                .map_err(|e| ClientDialError::Handshake(e.to_string()))?
        }
        #[cfg(not(feature = "socks5"))]
        unreachable!()
    } else {
        // Direct path (implemented in client/dial-tcp-tls)
        // ...
    };

    Ok(Connection::from_bidi(tls_stream, alpn.to_vec(), Some(addr)))
}
```

### SOCKS5 CONNECT helper

```rust
#[cfg(feature = "socks5")]
async fn socks5_connect(
    stream: &mut tokio::net::TcpStream,
    proxy: &Socks5ProxyConfig,
    target: SocketAddr,
) -> Result<(), String> {
    // 1. Greeting: send version + auth methods
    // 2. Auth: no-auth or username/password (RFC 1929)
    // 3. CONNECT request: CMD=0x01, ATYP=0x01 (IPv4) or 0x03 (domain), DST.ADDR, DST.PORT
    // 4. Read reply: version, REP (0x00 = success), ATYP, BND.ADDR, BND.PORT
    // 5. If REP != 0x00, return error with the REP code
}
```

### What this does NOT include

- **iroh proxy path** (force relay-only + HTTP-to-SOCKS5 bridge): Applied at endpoint
  construction time by the assembly layer (ADR-090 §5), not at dial time. The iroh
  endpoint is built with `clear_ip_transports()` + `addr_filter(relay_only)` +
  `proxy_url` by the assembly layer when a proxy is configured. The dial itself
  doesn't change.
- **No silent fallback**: When a proxy is configured and the proxy rejects the
  command, the dial returns `ClientDialError::Proxy`. The dial does not silently
  fall back to a direct connection — that would defeat the privacy posture.
- **No HTTP CONNECT support**: SOCKS5 only. SOCKS5 supports both TCP (CONNECT) and
  UDP (UDP ASSOCIATE) within one protocol. HTTP CONNECT is TCP-only with no UDP
  equivalent.

### Dependency: `fast-socks5`

The `fast-socks5` crate (v1.0.0) is used for the SOCKS5 client handshake. The
client-side surface used is small (`Socks5Datagram::bind` / `bind_with_password`,
`new_udp_header`, the header parsing). Whether to keep `fast-socks5` as a dep or
vendor the ~100 lines of SOCKS5 handshake + header framing is a two-way-door
implementation detail — this task uses `fast-socks5` as the starting choice
(validated by the PoC).

## Acceptance Criteria

- [ ] `Socks5ProxyConfig` struct defined in `crates/alknet-client/src/socks5.rs` with `addr` and `credentials` fields
- [ ] `Socks5Credentials` struct defined with `username` and `password` fields
- [ ] Both types derive `Debug`, `Clone`
- [ ] Both types feature-gated on `#[cfg(feature = "socks5")]`
- [ ] `Socks5UdpSocket` implements `quinn::AsyncUdpSocket` (feature-gated on `socks5`)
- [ ] `Socks5UdpSocket::bind(proxy)` performs SOCKS5 UDP ASSOCIATE handshake
- [ ] `Socks5UdpSocket::poll_send` prepends SOCKS5 UDP header to each datagram
- [ ] `Socks5UdpSocket::poll_recv` strips SOCKS5 UDP header from received datagrams
- [ ] `Socks5UdpSocket::may_fragment()` returns `false`
- [ ] `dial_quic` uses `Socks5UdpSocket` + `new_with_abstract_socket` when proxy is configured
- [ ] `dial_quic` returns `Proxy` error when UDP ASSOCIATE fails
- [ ] `dial_tcp_tls` performs SOCKS5 CONNECT handshake when proxy is configured
- [ ] `dial_tcp_tls` returns `Proxy` error when CONNECT fails
- [ ] No silent fallback to direct connection when proxy is configured
- [ ] `cargo check -p alknet-client --features quinn,socks5` succeeds
- [ ] `cargo check -p alknet-client --features tcp,socks5` succeeds
- [ ] `cargo clippy -p alknet-client --features quinn,tcp,socks5` succeeds with no warnings
- [ ] `cargo build --workspace` still succeeds (old code untouched)

## References

- docs/architecture/crates/client/README.md — SOCKS5 proxy section (lines 238-325)
- docs/architecture/decisions/090-client-dial-socks5-proxy-seam.md — ADR-090 (full rationale, PoC grounding, limitations)
- docs/research/quinn-quic-proxy/findings.md — quinn-over-SOCKS5 PoC findings
- docs/research/iroh-proxy-poc/findings.md — iroh-proxy PoC findings (iroh path is deferred)
- crates/alknet-tls/src/client.rs — `TlsClientConfig` (the TLS config the dial consumes)
- crates/alknet-core/src/credentials.rs — `ConnectionCredentials` (the credential bundle)
- RFC 1928 (SOCKS5) §3 (CONNECT), §6/§7 (UDP ASSOCIATE + UDP request header)
- RFC 1929 (SOCKS5 username/password auth)

## Notes

> This is the most complex task in Phase 3 — the `Socks5UdpSocket` is ~250 lines of
> integration glue between quinn's `AsyncUdpSocket` trait and the SOCKS5 UDP ASSOCIATE
> protocol. The implementation is grounded in the quinn-proxy PoC
> (`docs/research/quinn-quic-proxy/findings.md`), which validated the approach
> end-to-end (5/5 runs clean). The `fast-socks5` crate handles the SOCKS5 handshake;
> the `Socks5UdpSocket` is alknet's own code. The iroh proxy path (force relay-only +
> HTTP-to-SOCKS5 bridge) is deferred — it's applied at endpoint construction time by
> the assembly layer, not at dial time. The `socks5` feature and `fast-socks5` dep
> are opt-in; deployments that don't use a proxy pay nothing.

## Summary

> To be filled on completion
