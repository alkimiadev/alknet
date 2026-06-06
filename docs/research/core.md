# Alknet Core: Transport, Call Protocol, Auth, and DNS

> Status: Research / Draft
> Last updated: 2026-06-05

## Overview

`alknet-core` is the foundational crate providing pluggable transports, the bidirectional call protocol, Ed25519 authentication, and (future) DNS transport + naming. Everything else (storage, flowgraph, relay) builds on top of this.

## Transport Layer

### Architecture

The transport layer produces a duplex byte stream (`AsyncRead + AsyncWrite + Unpin + Send`) that the SSH layer consumes via `russh::client::connect_stream()` or `russh::server::run_stream()`. SSH is completely unaware of what transport it runs over.

### Transport Trait

```rust
#[async_trait]
pub trait Transport: Send + Sync + 'static {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;
    async fn connect(&self) -> Result<Self::Stream>;
    fn describe(&self) -> String;
}

#[async_trait]
pub trait TransportAcceptor: Send + Sync + 'static {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;
    async fn accept(&self) -> Result<(Self::Stream, TransportInfo)>;
}

#[derive(Debug, Clone)]
pub struct TransportInfo {
    pub remote_addr: Option<SocketAddr>,
    pub transport_kind: TransportKind,
}

#[derive(Debug, Clone)]
pub enum TransportKind {
    Tcp,
    Tls { server_name: Option<String> },
    Iroh { endpoint_id: String },
    Dns { domain: String },           // NEW
    WebTransport { host: String },    // NEW (planned)
}
```

### Existing Transports

| Transport | Client | Server | Stream Type |
|-----------|--------|--------|-------------|
| TcpTransport | `TcpStream::connect(addr)` | `TcpListener::accept()` | `TcpStream` |
| TlsTransport | `TlsStream<TcpStream>` | `TlsStream<TcpStream>` | tokio_rustls |
| IrohTransport | `endpoint.connect(peer, alpn)` then `conn.open_bi()` then `join(recv, send)` | `endpoint.accept()` then `conn.accept_bi()` then `join(recv, send)` | `tokio::io::Join<RecvStream, SendStream>` |
| AcmeTlsAcceptor | Auto-provision via Let's Encrypt | ACME cert provision + TLS accept | TlsStream |

### Transport Chaining

```bash
alknet connect --transport iroh --proxy socks5://127.0.0.1:1080
alknet connect --transport tls --proxy socks5://127.0.0.1:1080
```

`--proxy` routes outbound connections. Client: routes transport connection. Server: routes data-channel TCP targets.

### Stealth Mode

When `--stealth` is enabled with TLS transport on port 443: after TLS handshake, peek first bytes. If `SSH-2.0-`, run SSH. Otherwise, return `HTTP/1.1 404 Not Found\r\nServer: nginx\r\n\r\n` and close. Makes the server indistinguishable from an HTTPS site.

## Call Protocol

### Wire Format

Every message is a length-prefixed JSON `EventEnvelope`:

```rust
pub struct EventEnvelope {
    pub r#type: String,    // "call.requested", "call.responded", etc.
    pub id: String,        // Correlation key (requestId, topic, or "" for broadcasts)
    pub payload: Value,   // JSON payload — schema depends on event type
}

// Frame: 4-byte big-endian length prefix + UTF-8 JSON body
```

This is the same format used by `@alkdev/pubsub` adapters. The envelope is transport-agnostic — it runs over SSH channels, WebTransport streams, iroh bidirectional streams, WebSocket, Worker postMessage, or DNS queries.

Binary payloads are base64-encoded in the `payload` field. The envelope itself stays JSON for cross-language compatibility.

### Call Protocol Events

| Event | Direction | Purpose |
|-------|-----------|---------|
| `call.requested` | Caller → Handler | Initiate a call or subscription |
| `call.responded` | Handler → Caller | Deliver a result (one for calls, many for subscriptions) |
| `call.completed` | Handler → Caller | Signal end of subscription stream |
| `call.aborted` | Either side | Cancel the call/subscription |
| `call.error` | Handler → Caller | Signal an error |

A call is just a subscribe that resolves after one event. Both `call()` and `subscribe()` send the same `call.requested` event.

### Operation Paths

```
/{spoke}/{service}/{op}
```

- **spoke** — identity prefix of the node that exposes the operation
- **service** — logical service namespace (e.g., `fs`, `bash`, `agent`)
- **op** — specific operation (e.g., `readFile`, `exec`, `chat`)

Examples:

| Path | Meaning |
|------|---------|
| `/dev1/fs/readFile` | Spoke `dev1`, service `fs`, op `readFile` |
| `/hub/agent/chat` | Hub's own `agent` service, op `chat` |
| `/hub/sessions/list` | Hub's `sessions` service, op `list` |

### PendingRequestMap

Manages in-flight calls and subscriptions. Correlates `call.responded` events back to the original `call.requested`:

```rust
pub struct PendingRequestMap {
    pending: HashMap<String, PendingEntry>,
}

enum PendingEntry {
    Call { tx: oneshot::Sender<Result<Value>>, timeout: Instant },
    Subscribe { tx: mpsc::Sender<Result<Value>>, timeout: Option<Instant> },
}
```

### Operation Registry

```rust
pub struct OperationSpec {
    pub name: String,                    // "/fs/readFile", "/agent/chat"
    pub namespace: String,               // "fs", "agent"
    pub op_type: OperationType,          // Query, Mutation, Subscription
    pub input_schema: Value,             // JSON Schema for input
    pub output_schema: Value,            // JSON Schema for output
    pub access_control: AccessControl,   // Required scopes/resources
}

pub enum OperationType {
    Query,         // Read-only, idempotent
    Mutation,      // Side effects
    Subscription,  // Streaming
}

pub struct AccessControl {
    pub required_scopes: Vec<String>,
    pub required_scopes_any: Option<Vec<String>>,
    pub resource_type: Option<String>,
    pub resource_action: Option<String>,
}
```

Specs and handlers are separated — downstream consumers register both without modifying core:

```rust
registry.register(OperationSpec { name: "/services/list", ... }, list_services_handler);
registry.register(OperationSpec { name: "/fs/readFile", ... }, fs_read_handler);
```

### Protocol Adapter Layer

| Transport | Channel mechanism | Direction |
|-----------|-------------------|-----------|
| SSH | Reserved `direct_tcpip` destination `alknet-control:0` | Bidirectional over SSH channel |
| WebTransport | Bidirectional stream after CONNECT | Bidirectional over WT stream |
| iroh QUIC | `open_bi()` / `accept_bi()` | Bidirectional over QUIC stream |
| WebSocket | Single WS connection | Bidirectional over WS frames |
| Worker | `postMessage` | Bidirectional over structured clone |
| DNS | Query TXT records (client) / serve TXT records (server) | Request/response over DNS |

### Hub/Spoke Architecture

```
         ┌─────────────────────────────────┐
         │              Hub                │
         │                                 │
         │  Hub-local services:            │
         │  /hub/agent/chat               │
         │  /hub/agent/complete            │
         │  /hub/sessions/list             │
         │                                 │
         │  Spoke registry:                │
         │  /dev1/fs/* → dev1 connection    │
         │  /browser-1/notify/* → WT conn  │
         └──────┬───────┬──────────────────┘
                │       │
      ┌─────────▼┐ ┌───▼────────────┐
      │  Spoke    │ │Browser Spoke   │
      │  "dev1"   │ │"browser-1"     │
      │  /fs/*    │ │/notify/*       │
      └───────────┘ └────────────────┘
```

Spokes register operations on connect:

```json
{
  "type": "call.requested",
  "id": "uuid-123",
  "payload": {
    "operationId": "/hub/services/register",
    "input": {
      "spoke": "dev1",
      "operations": ["/fs/readFile", "/bash/exec"]
    }
  }
}
```

## Authentication

Ed25519 keys for SSH authentication. A separate authentication mechanism for browsers where they sign a token using the same Ed25519 keys. Hot key rotation without server restart (mechanism in core for programmatic key updates).

Peer credentials are stored in `peer_credentials` table (fingerprint-based lookup). Account credentials via `api_keys` table (SHA-256 hash for high-entropy keys).

## DNS Transport (Planned)

### Two DNS Concepts

1. **DNS as Transport** — Encode `EventEnvelope` frames as DNS queries/responses. Censorship resistance. Request/response maps to `call.requested`/`call.responded` naturally.

2. **DNS as Naming/Discovery** — Publish/resolve endpoint information via DNS TXT records (iroh-dns style). Smart contract provides on-chain `name → namespaceId + relays`. DNS transport carries the data flow when other transports are blocked.

### DNS as Call Protocol Transport

The call protocol is transport-agnostic. DNS becomes another adapter:

```
Transport Layer:
  SSH channel       → EventEnvelope frames → CallHandler
  WebTransport      → EventEnvelope frames → CallHandler
  iroh QUIC stream  → EventEnvelope frames → CallHandler
  DNS query/response → EventEnvelope frames → CallHandler  ← NEW
```

**Upstream (client → server)**: Encode `EventEnvelope` JSON as base32 DNS query labels.
**Downstream (server → client)**: Return `EventEnvelope` JSON in TXT record responses.
**Polling**: For `call.responded` after `call.requested`, client polls `requestId.alk.dev TXT?`.

The `DnsTransportAdapter` implements the same adapter pattern as `@alkdev/pubsub`'s event targets, making DNS a first-class transport for control channel operations.

### DNS as Full Transport (SSH Tunneling)

Full-duplex SSH tunneling over DNS requires a framing protocol:
- Chunk SSH data into fixed-size frames (e.g., 220-byte frames with 4-byte header for seq/ack)
- Encode upstream in base32 subdomain labels
- Encode downstream in TXT records or CNAME targets
- Handle resequencing and retransmission

This is higher latency (~1-50 KB/s) but works when all other transports are blocked. Fine for interactive SSH. Log a warning at connect time.

### iroh-dns Relationship

iroh-dns publishes `EndpointInfo` via `_iroh.<z32-endpoint-id>.<origin> TXT` records. alknet can extend this:

- Add `tunnel=dnst.example.com` attribute to indicate DNS transport availability
- Use iroh-dns `DnsResolver` for endpoint discovery
- When a client sees the `tunnel` attribute and QUIC is blocked, fall back to DNS transport

### DnsTransport Implementation Sketch

```rust
#[cfg(feature = "dns")]
mod dns;

pub struct DnsTransport {
    domain: String,           // e.g. "t.alk.dev"
    resolver_addr: SocketAddr,
    protocol: DnsProtocol,    // Udp, Tcp, Tls, Https
    auth_token: Option<String>,
}

pub struct DnsAcceptor {
    domain: String,
    listen_addr: SocketAddr,
    protocol: DnsProtocol,
}

// DnsStream: virtual duplex backed by DNS poll/push
// Uses tokio::io::duplex() internally with a background task that:
// - Chunks outgoing bytes into DNS queries (client) or response records (server)
// - Reassembles incoming DNS payloads into the read buffer
// - Handles ACK/NACK for reliability
```

### DnsProtocol in iroh-dns

iroh-dns already supports multiple DNS protocols:

```rust
pub enum DnsProtocol {
    Udp,       // Classic DNS
    Tcp,       // DNS over TCP
    Tls,       // DNS over TLS (DoT) — RFC 7858
    Https,     // DNS over HTTPS (DoH) — RFC 8484
}
```

alknet's DNS transport should support all of these. DoH (port 443, looks like HTTPS) is particularly valuable for censorship resistance since it's indistinguishable from normal web traffic.

## Design Decisions

| ADR | Decision | Summary |
|-----|----------|---------|
| 001 | Pluggable transport | Transport trait produces stream, SSH consumes it |
| 003 | iroh stream join | `tokio::io::join` combines QUIC halves |
| 004 | SSH over transport | SSH never touches TCP/iroh/TLS directly |
| 008 | ACME/Let's Encrypt | Auto-provision TLS certs |
| 009 | Default iroh relay | n0 relay by default, `--iroh-relay` override |
| 010 | Transport chaining | `--proxy` works with all transports natively |
| 017 | Stealth mode | Peek first bytes, return 404 for non-SSH on port 443 |
| 018 | Control channel for pubsub | Reserved destination for event bus |
| 019 | Proxy dual semantics | `--proxy` routes transport on client, data on server |
| 023 | Unified auth | Shared Ed25519 key material across auth mechanisms |
| 024 | Bidirectional call protocol | Both sides can call, generalized from ADR-018 |
| 025 | Handler/spec separation | Downstream registers operations without modifying core |

## References

- `@alkdev/pubsub` — TypeScript event target adapters and `EventEnvelope`
- `@alkdev/operations` — TypeScript call protocol, `OperationSpec`, registry
- `@alkdev/flowgraph` — TypeScript operation graph and call graph (planned Rust port)
- `@alkdev/storage` — TypeScript metagraph, identity, ACL (planned Rust port as `alknet-storage`)
- iroh-dns — DNS resolver and endpoint info (naming/discovery)
- iroh-live-relay — WebTransport relay (planned transport reference)
- irpc — iroh streaming RPC (postcard-only, Rust-to-Rust)