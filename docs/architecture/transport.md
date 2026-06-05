---
status: reviewed
last_updated: 2026-06-02
---

# Transport Layer

## What

The transport layer produces a duplex byte stream (`AsyncRead + AsyncWrite + Unpin + Send`) that the SSH layer consumes via `russh::client::connect_stream()` or `russh::server::run_stream()`. The SSH layer is completely unaware of what transport it runs over.

## Why

Pluggable transports are the core architectural insight. They enable:

- **Simple deployment**: TCP on port 22 for basic use
- **Censorship resistance**: TLS on port 443 looks like HTTPS
- **NAT traversal**: iroh QUIC allows connections without public IPs
- **Composability**: transports can be layered (iroh through SOCKS5 through SSH through TLS)

Without this abstraction, each transport mode would need its own SSH connection logic. With it, there's one SSH implementation and N transport implementations.

## Architecture

### Transport Trait

```rust
// The core abstraction. Each transport produces ONE duplex stream.
// The SSH session runs over this stream for its entire lifetime.

#[async_trait]
pub trait Transport: Send + Sync + 'static {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    /// Connect to the remote endpoint and return a duplex stream.
    /// For client-side transports.
    async fn connect(&self) -> Result<Self::Stream>;

    /// Return a human-readable description of this transport for logging.
    fn describe(&self) -> String;
}
```

### Server-Side Transport Acceptor

```rust
#[async_trait]
pub trait TransportAcceptor: Send + Sync + 'static {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    /// Accept an incoming connection and return a duplex stream.
    async fn accept(&self) -> Result<(Self::Stream, TransportInfo)>;
}

/// Metadata about the incoming connection.
pub struct TransportInfo {
    pub remote_addr: Option<SocketAddr>,
    pub transport_kind: TransportKind,
}

pub enum TransportKind {
    Tcp,
    Tls { server_name: Option<String> },
    Iroh { endpoint_id: String },
}
```

### Transport Implementations

| Transport | Client | Server | Stream Type |
|-----------|--------|--------|-------------|
| **TcpTransport** | `TcpStream::connect(addr)` | `TcpListener::accept()` | `TcpStream` |
| **TlsTransport** | `TlsStream<TcpStream>` (client TLS) | `TlsStream<TcpStream>` (server TLS) | `tokio_rustls::client::TlsStream<TcpStream>` |
| **IrohTransport** | `endpoint.connect(peer, alpn)` then `conn.open_bi()` then `join(recv, send)` | `endpoint.accept()` then `conn.accept_bi()` then `join(recv, send)` | `tokio::io::Join<RecvStream, SendStream>` |

### Iroh Stream Join

Since QUIC splits streams into separate `RecvStream` (implements `AsyncRead`) and `SendStream` (implements `AsyncWrite`), while russh expects a single duplex stream, they are combined using `tokio::io::join(recv_stream, send_stream)` which produces a `Join<RecvStream, SendStream>` implementing both traits.

See ADR-003 for the decision to use `tokio::io::join` over a custom wrapper.

### iroh Relay Configuration

By default, iroh transport uses n0's free relay servers (`https://relay.iroh.network/`). This provides zero-config NAT traversal for testing and development. For production deployments, users override with `--iroh-relay <url>` to point to a self-hosted relay.

The relay URL is passed to iroh's `Endpoint::builder()` configuration. Self-hosted relay setup is documented in the project wiki.

See ADR-009 for the decision to default to n0's relay with override.

### Transport Chaining

Transports can be nested. The CLI supports `--transport iroh --proxy socks5://...` natively (ADR-010):

```bash
alknet connect --transport iroh --proxy socks5://127.0.0.1:1080
```

This routes iroh's outbound TCP connections through the specified SOCKS5 proxy. The iroh transport supports SOCKS5 and HTTP proxy configuration for its outbound connections — the proxy URL is applied during transport initialization.

For other combinations:
- TCP + TLS is already implicit (TLS wraps TCP in `TlsTransport`)
- TLS + SOCKS5 proxy is also supported via `--proxy` with `--transport tls`

**Note**: `--proxy` has different semantics on the client vs the server (ADR-019):
- **Client**: `--proxy` routes the *transport connection* through the proxy (e.g., iroh endpoint → SOCKS5 → iroh relay)
- **Server**: `--proxy` routes *outbound target connections* through the proxy (e.g., SSH channel request → SOCKS5 → target host)

### Connection Lifecycle

```
Client                                          Server
  │                                               │
  │  transport.connect()                          │  transport_acceptor.accept()
  │  ─────────────────────────────────────────────▶│
  │        (duplex byte stream established)        │
  │                                               │
  │  russh::client::connect_stream(config,        │  russh::server::run_stream(config,
  │      stream, handler)                          │      stream, handler)
  │                                               │
  │  ═══════ SSH session over stream ═════════════ │
  │  ═════════════════════════════════════════════ │
  │                                               │
  │  channel_open_direct_tcpip(host, port, ...)  │
  │  ─────────────────────────────────────────────▶│
  │                                               │
  │  ┌─────── TCP proxy ──────────────────┐       │
  │  │  SSH channel ←→ TcpStream::connect │       │
  │  └────────────────────────────────────┘       │
```

## Constraints

- SSH sees only the stream. It never opens its own TCP connections. (ADR-004)
- Each transport produces exactly one stream per SSH session. Multiple sessions need multiple `connect()` calls.
- The iroh transport reuses a single `Endpoint` across multiple sessions (one QUIC connection per peer, multiple `open_bi()` streams). The endpoint is created once and shared.
- TLS transport requires certificate configuration on the server side. The client can accept any certificate (self-signed) or verify against a CA. Server-side ACME is supported (ADR-008).

## Open Questions

None — all resolved.

## Design Decisions

| ADR | Decision | Summary |
|-----|----------|---------|
| [001](decisions/001-pluggable-transport.md) | Pluggable transport | Transport trait produces stream, SSH consumes it |
| [003](decisions/003-iroh-stream-join.md) | iroh stream join | `tokio::io::join` combines QUIC halves |
| [004](decisions/004-ssh-over-transport.md) | SSH over transport | SSH never touches TCP/iroh/TLS directly |
| [008](decisions/008-acme-lets-encrypt.md) | ACME/Let's Encrypt | Auto-provision TLS certs, domain and IP paths |
| [009](decisions/009-default-iroh-relay.md) | Default iroh relay | n0 relay by default, `--iroh-relay` override |
| [010](decisions/010-transport-chaining-cli.md) | Transport chaining | `--proxy` works with all transports natively |
| [019](decisions/019-proxy-dual-semantics.md) | Proxy dual semantics | `--proxy` routes transport on client, data on server |