# TLS Transport: Unified Multi-Interface Architecture

> Status: Research / Draft
> Last updated: 2026-06-08
> Part of: Phase 2 planning

## Overview

Alknet's existing stealth mode already does protocol detection: after a TLS handshake, the server peeks at the first bytes and routes SSH connections one way and HTTP connections another. This document extends that pattern into a unified architecture where a single TLS port supports SSH, REST, WebSocket, SSE, and gRPC — all routed by the first bytes after the TLS handshake. Alongside this, QUIC (UDP) supports WebTransport and iroh P2P, and DNS runs on its own port. Every interface resolves to the same call protocol operations through the `OperationRegistry`.

This replaces the earlier `(Transport, Interface)` pair model for TCP/TLS connections with a clearer distinction: persistent stream interfaces go through the peek-based router, message-based interfaces manage their own transports, and axum serves as the multiplexer for everything HTTP.

## Current State

The stealth mode implementation in `crates/alknet-core/src/server/stealth.rs` does byte-peeking after TLS handshake:

```rust
pub enum ProtocolDetection {
    Ssh,
    Http,
}

pub async fn detect_protocol<S>(stream: S) -> (ProtocolDetection, BufReader<S>) {
    // Peek first bytes: "SSH-2.0-" → Ssh, anything else → Http
}

pub async fn send_fake_nginx_404<S>(reader: &mut BufReader<S>) {
    // Currently: non-SSH gets a fake 404 and connection closed
}
```

This is almost exactly what we need. The `Http` detection currently sends a fake nginx 404. Instead, it should route to a real HTTP server.

## New Architecture

### TCP TLS Port 443: Peek-Based Routing

```
Client connects to port 443
    │
    TLS handshake completes
    │
    Peek first bytes
    │
    ├─ "SSH-2.0-" → SshInterface (russh, existing path)
    │
    └─ (anything else) → axum HTTP router
         │
         ├─ POST /v1/{namespace}/{op}           → registry.invoke()
         ├─ GET  /v1/{namespace}/{op}           → registry.invoke()
         ├─ GET  /v1/{namespace}/{op} (SSE)     → registry.subscribe()
         ├─ POST /v1/batch                       → batch invoke
         ├─ GET  /v1/schema                      → registry.list_operations()
         ├─ WebSocket upgrade /ws                → WebSocketInterface
         ├─ gRPC via tonic routes                → tonic services
         ├─ GET  /.well-known/alknet/schema      → OpenAPI spec generation
         └─ (anything else)                      → 404
```

The peek happens after TLS, so the client sees a valid HTTPS server. The `send_fake_nginx_404` function becomes `hand_to_axum(stream)`. axum handles everything that isn't SSH.

### UDP Port 443: QUIC with ALPN Routing

```
Client sends QUIC Initial to port 443 UDP
    │
    TLS 1.3 handshake with ALPN negotiation
    │
    ├─ ALPN "h3" (WebTransport)  → wtransport → RawFramingInterface
    │                                  │
    │                                  └─ SessionRequest → validate AuthToken
    │                                       from URL path or headers
    │                                       → OperationContext → call protocol
    │
    └─ ALPN "alknet" (iroh P2P)  → iroh endpoint → RawFramingInterface
                                       │
                                       └─ existing iroh accept loop
                                            → SshInterface or RawFramingInterface
```

wtransport and iroh both listen on UDP 443. Quinn supports multiple ALPN protocols — the QUIC handshake negotiates which handler gets the connection.

### DNS Port 53: MessageInterface

```
DNS query arrives on port 53 (UDP or TCP)
    │
    ├─ UDP query  → DnsInterface (MessageInterface)
    └─ TCP query  → DnsInterface over DoT (TLS on port 853)
         │
         └─ Encode EventEnvelope as DNS TXT query
            Decode response from DNS TXT record
            AuthToken embedded in query labels
            → IdentityProvider::resolve_from_token()
            → OperationContext → call protocol
```

DNS is a `MessageInterface` — it manages its own transport and handles individual request/response pairs. It doesn't sit on top of the TLS peek router.

### Revised Routing Table

| Protocol | Transport | Detection | Interface | Auth |
|---|---|---|---|---|
| SSH | TCP/TLS | Byte peek: `SSH-2.0-` prefix | SshInterface | SSH key fingerprint |
| HTTP REST | TCP/TLS | Byte peek: not SSH → axum | axum handler → registry | `Authorization: Bearer <AuthToken>` |
| WebSocket | TCP/TLS | Axum upgrade: `Upgrade: websocket` | axum upgrade handler | AuthToken in handshake |
| SSE | TCP/TLS | Axum route: `Accept: text/event-stream` | axum handler → registry.subscribe() | AuthToken in header |
| gRPC | TCP/TLS | Axum route: `content-type: application/grpc` | tonic via axum router | AuthToken in header/metadata |
| WebTransport | QUIC (UDP) | ALPN `h3` | wtransport → RawFramingInterface | AuthToken in CONNECT URL |
| iroh P2P | QUIC (UDP) | ALPN `alknet` | iroh → RawFramingInterface | iroh's existing auth |
| DNS | UDP/TCP | Own listener | DnsInterface (MessageInterface) | AuthToken in query labels |

## Implementation

### Extending ProtocolDetection

The current `ProtocolDetection` enum gains variants for known HTTP sub-protocols:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolDetection {
    Ssh,
    Http,         // Any HTTP — axum handles sub-routing
}
```

This stays simple. SSH vs. not-SSH is the only peek-level decision. Everything else is HTTP-content routing inside axum. We don't need to detect WebSocket, SSE, or gRPC at the byte level — axum routes those by HTTP headers and paths.

The accept loop becomes:

```rust
// After TLS handshake and peek:
match detect_protocol(tls_stream).await {
    (ProtocolDetection::Ssh, reader) => {
        // Existing SSH path: hand to SshInterface
        handle_ssh(reader, config).await;
    }
    (ProtocolDetection::Http, reader) => {
        // Hand to axum HTTP server
        handle_http(reader, config).await;
    }
}
```

### Axum Integration

The axum server is an HTTP `Service` that receives the TLS stream after the peek. Since the TLS handshake is already complete, axum receives a plaintext stream:

```rust
async fn handle_http(stream: BufReader<TlsStream>, config: ServerConfig) {
    let app = Router::new()
        .route("/v1/{namespace}/{op}", post(invoke_operation))
        .route("/v1/{namespace}/{op}", get(invoke_operation))
        .route("/v1/batch", post(invoke_batch))
        .route("/v1/schema", get(list_operations))
        .route("/ws", get(websocket_upgrade))
        // gRPC via tonic::Routes merged into axum router
        .layer(ExtractorLayer::new(config.identity_provider, config.registry))
        .layer(middleware::from_fn(auth_middleware));

    // Serve the axum app on the TLS stream
    hyper::server::conn::http1::Builder::new()
        .serve_connection(TokioIo::new(stream), app.into_make_service())
        .with_upgrades()  // Enables WebSocket upgrades
        .await;
}
```

The auth middleware extracts the `Authorization: Bearer <token>` header and calls `IdentityProvider::resolve_from_token()`. The operation handler constructs an `OperationContext` and calls `registry.invoke(namespace, op, input)`.

### WebTransport (QUIC/UDP)

WebTransport runs on UDP alongside iroh. The routing is by ALPN during the QUIC handshake:

```rust
// Quinn server config with two ALPN protocols:
let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(tls_config));
server_config.alpn_protocols = vec![
    WEBTRANSPORT_ALPN.to_vec(),   // b"h3"
    IROH_ALPN.to_vec(),           // existing iroh ALPN
];

// Accept loop:
loop {
    let incoming = quic_endpoint.accept().await;
    match incoming.alpn() {
        b"h3" => {
            // Hand to wtransport
            let session_request = IncomingSession::with_quic_incoming(incoming).await;
            // Validate AuthToken from URL path/headers
            // Create OperationContext
            // Route to call protocol via RawFramingInterface or HTTP-like handler
        }
        b"alknet" | IROH_ALPN => {
            // Hand to existing iroh accept loop
            handle_iroh(incoming).await;
        }
        _ => { /* reject unknown ALPN */ }
    }
}
```

wtransport's `with_quic_incoming()` escape hatch allows integrating with an externally managed Quinn endpoint, so alknet owns the Quinn `Endpoint` and routes WebTransport sessions to wtransport.

### Auth: Single Token Mechanism

Every interface except SSH uses the same `AuthToken` format defined in auth.md:

```
AuthToken = base64url(key_id || timestamp || signature)
  key_id    = SHA-256 fingerprint of the Ed25519 public key (32 bytes)
  timestamp = Unix seconds, big-endian u64 (8 bytes)
  signature = Ed25519 sign(key_id || timestamp_bytes, private_key)
```

| Interface | Auth mechanism | Token location |
|---|---|---|
| SSH | SSH key handshake | In SSH protocol (not a token) |
| HTTP REST | `Authorization: Bearer <AuthToken>` | HTTP header |
| WebSocket | AuthToken in first message or query param | After upgrade |
| SSE | `Authorization: Bearer <AuthToken>` | HTTP header |
| gRPC | `Authorization: Bearer <AuthToken>` | HTTP/2 metadata |
| WebTransport | AuthToken in CONNECT URL or header | WebTransport session request |
| DNS | AuthToken embedded in DNS query labels | Encoded in domain name |

All token-based paths call `IdentityProvider::resolve_from_token()`. The `resolve_from_token()` implementation handles Ed25519 signature verification (for AuthTokens) and will also handle hash-verified API keys (shorter tokens for simpler integrations).

For services and automation where Ed25519 key pairs are inconvenient, short API keys work:

```
API key: "alk_dGhlX3NlY3JldA" (~20 chars)
Storage: SHA-256 hash of the full key
Lookup:  prefix match → hash verification → Identity
```

API keys are specified in `DynamicConfig.auth` or stored in `api_keys` tables (database-backed). Both AuthTokens and API keys go through the same `resolve_from_token()` method — the implementation discriminates by prefix or format.

### Contract Pattern: call / batch / schema / subscribe

Every interface exposes the same four primitive operations through `OperationRegistry`:

| Primitive | HTTP | MCP | DNS | Call protocol |
|---|---|---|---|---|
| `call(namespace, op, input)` | `POST /v1/{ns}/{op}` | `tools/call` | `{op}.{ns}.alk.dev TXT?` | `call.requested` |
| `batch([{ns, op, input}, ...])` | `POST /v1/batch` | (multiple `tools/call`) | (multiple queries) | (multiple `call.requested`) |
| `schema(namespace?)` | `GET /v1/schema` | `tools/list` | (not typically) | `call.requested` with special op |
| `subscribe(namespace, op, input)` | `GET /v1/{ns}/{op} SSE` | (future) | (not applicable) | `call.requested` with stream flag |

MCP's four core operations map directly:
- `tools/list` → `schema()`
- `tools/call` → `call()`
- `prompts/list` → `schema("prompts")`
- `prompts/get` → `call("prompts", "get", input)`

The `memory` tool pattern (one namespace gate dispatching to many operations behind it) is exactly `OperationRegistry` with `OperationSpec.access_control`:

```
memory({tool:"help"})     → registry.invoke("memory", "help", {})
memory({tool:"search"})   → registry.invoke("memory", "search", {query: "..."})
memory({tool:"store"})    → registry.invoke("memory", "store", {key: "...", value: "..."})
```

### Reverse: OpenAPI Spec Generation

The HTTP interface's `GET /v1/schema` endpoint (or `GET /.well-known/alknet/schema`) auto-generates an OpenAPI spec from the registered `OperationSpec`s. This creates a symmetry with `FromOpenAPI`:

```
Inbound:  HTTP request → axum handler → registry.invoke(namespace, op, input) → ResponseEnvelope → HTTP response
Outbound: OpenAPI spec → FromOpenAPI(spec, config) → registry.register_all(operations) → HTTP client → external service
```

Node A's HTTP interface produces an OpenAPI spec. Node B's `FromOpenAPI` consumes it. Alknet nodes can discover each other's capabilities via the schema endpoint.

## Relationship to StreamInterface / MessageInterface

The earlier `interface-model.md` research defined `StreamInterface` and `MessageInterface` traits. This doc refines the architecture:

**StreamInterface** — persistent byte stream, used for SSH and raw framing:
- `SshInterface`: (TLS, SSH) — existing path, unchanged
- `RawFramingInterface`: (TCP/TLS, raw framing) — for local mesh
- `RawFramingInterface`: (iroh/QUIC, raw framing) — for P2P mesh

**MessageInterface** — manages its own transport, handles individual requests:
- `DnsInterface`: Runs its own DNS server on port 53

**The HTTP case** is special. The axum router is not a `MessageInterface` in the same sense as DNS. It receives a stream (the TLS connection after peek), but it handles individual requests within that stream. It's better modeled as:

- A `StreamInterface` that internally routes to axum
- Axum is the implementation detail, not a trait boundary
- The call protocol handler receives `InterfaceRequest` and returns `InterfaceResponse` regardless of whether the request came from HTTP, DNS, SSH, or raw framing

The `InterfaceRequest` / `InterfaceResponse` types from `interface-model.md` still make sense as the normalized interface-agnostic request/response that all interfaces produce:

```rust
pub struct InterfaceRequest {
    pub operation_path: String,         // e.g., "/head/auth/verify"
    pub input: Value,                   // JSON input payload
    pub auth_token: Option<AuthToken>,  // Extracted from wire format
    pub metadata: HashMap<String, String>,
}

pub struct InterfaceResponse {
    pub result: Result<Value, CallError>,
    pub status: u16,                    // HTTP status, DNS result code, etc.
    pub headers: HashMap<String, String>,
}
```

But the HTTP implementation doesn't need to construct `InterfaceRequest` explicitly — it constructs `OperationContext` directly from the axum request and calls `registry.invoke()`. The `InterfaceRequest` abstraction is more useful for DNS where there's no framework doing routing for you.

## ListenerConfig Update

The `ListenerConfig` enum from the integration plan gains a `Http` variant alongside existing `Stream`:

```rust
pub enum ListenerConfig {
    Stream {
        transport: TransportKind,
        interface: StreamInterfaceKind,
    },
    Http {
        bind_addr: SocketAddr,
        tls: bool,                    // true = TLS, false = plain TCP
        stealth: bool,                 // true = byte-peek protocol detection
    },
    Dns {
        bind_addr: SocketAddr,
        tls: bool,                    // true = DoT, false = plain DNS
    },
}

pub enum StreamInterfaceKind {
    Ssh,
    RawFraming,
}

pub enum TransportKind {
    Tcp,
    Tls { server_name: Option<String> },
    Iroh { endpoint_id: String },
    // NO Dns variant — DNS is a MessageInterface, not a Transport
}
```

For the common production deployment on port 443:

```toml
[[listeners]]
type = "stream"
transport = { tls = {} }
interface = "ssh"
bind = "0.0.0.0:443"

[[listeners]]
type = "http"
bind = "0.0.0.0:443"
tls = true
stealth = true

# If separate ports are preferred:
[[listeners]]
type = "http"
bind = "0.0.0.0:8080"
tls = false
stealth = false
```

When `stealth = true` on an HTTP listener sharing a port with an SSH listener, the accept loop uses the byte-peek pattern to route connections to the correct handler.

When the HTTP listener is on its own port, no peeking is needed — everything is HTTP.

## Phasing

| Work | Phase | Notes |
|---|---|---|
| Extend `ProtocolDetection` to route `Http` to axum | Phase 1 (now) | Replace `send_fake_nginx_404` with axum handoff |
| Axum HTTP server with `/v1/{ns}/{op}` routes | Phase 1 (now) | Core REST API for call protocol operations |
| Auth middleware (`Authorization: Bearer`) | Phase 1 (now) | Uses existing `IdentityProvider::resolve_from_token()` |
| `ListenerConfig::Http` variant | Phase 1 (now) | Define alongside existing `Stream` variant |
| Remove `TransportKind::Dns` | Phase 1 (now) | Cleanup before code depends on it |
| WebSocket upgrade handler | Phase 2 | axum `.with_upgrades()` is already available |
| SSE streaming handler | Phase 2 | axum + `axum-streams` or `tokio-stream` |
| gRPC via tonic integration | Phase 3 | `tonic::Routes` merges into axum router |
| WebTransport (QUIC/UDP) | Phase 3 | wtransport integration, ALPN routing |
| DNS interface | Phase 3+ | Uses `MessageInterface` trait, own listener |
| OpenAPI spec generation from registry | Phase 3+ | `GET /v1/schema` or `GET /.well-known/alknet/schema` |
| ALPN multiplexing on UDP 443 | Phase 3+ | Quinn ALPN routing between iroh and wtransport |

## References

- [stealth.rs](../../../crates/alknet-core/src/server/stealth.rs) — Current protocol detection implementation
- [auth.md](../../architecture/auth.md) — AuthToken format, IdentityProvider, unified auth
- [interface-model.md](interface-model.md) — StreamInterface / MessageInterface trait design
- [credential-provider.md](credential-provider.md) — CredentialProvider, outbound auth
- [call-protocol.md](../../architecture/call-protocol.md) — OperationRegistry, OperationEnv
- [services.md](../../architecture/services.md) — irpc service definitions, OperationContext
- [ADR-026](../../architecture/decisions/026-transport-interface-separation.md) — Three-layer model
- [wtransport](/workspace/wtransport/) — WebTransport server implementation (QUIC/HTTP3, ALPN h3)
- [iroh-relay](/workspace/iroh/iroh-relay/) — HTTP + WebSocket relay (hyper, MaybeTlsStream)
- [hickory-dns](/workspace/hickory-dns/) — DNS server with DoT/DoH/DoQ/DoH3
- [tonic](/workspace/tonic/) — gRPC framework (axum + hyper integration, ALPN h2)