---
status: draft
last_updated: 2026-06-17
---

# Endpoint

ALPN router, handler registry, connection accept loops, multi-connectivity, and graceful shutdown.

See [ADR-010](../../decisions/010-alpn-router-and-endpoint.md) for the full rationale.

## AlknetEndpoint

The central runtime type. Manages one or more QUIC connection sources, each feeding into the same ALPN router.

```rust
pub struct AlknetEndpoint {
    // QUIC connection sources — both optional, both can be active simultaneously
    quinn: Option<quinn::Endpoint>,       // Public QUIC+TLS
    iroh: Option<iroh::Endpoint>,         // P2P relay-assisted

    handlers: Arc<HandlerRegistry>,
    dynamic: Arc<ArcSwap<DynamicConfig>>,
    identity_provider: Arc<dyn IdentityProvider>,
    shutdown: watch::Receiver<bool>,
}
```

### Why multiple connection sources?

A node can be reachable through different paths depending on its network context:

| Source | Requires | Identity source | Use case |
|--------|----------|-----------------|----------|
| `quinn::Endpoint` | Public IP, TLS cert | TLS cert (network), SSH key (auth) | VPS, replicators, service hosts |
| `iroh::Endpoint` | Relay access | NodeId (Ed25519) | Home servers, NAT, IoT |

These are not interchangeable transports — they are **complementary connectivity modes**. A node behind NAT that also has a public IP can use both simultaneously. Both produce QUIC connections that dispatch through the same `HandlerRegistry` by ALPN string.

### TCP is NOT an endpoint concern

Bare TCP (SSH over port 22) does not use QUIC or ALPN. In the new model, TCP access is handled by individual handlers — the SSH handler can listen on a TCP socket independently. This is a handler-specific concern, not a core endpoint concern.

The reference implementation's TCP transport (`alknet-main/crates/alknet-core/src/transport/tcp.rs`) is SSH-specific. It doesn't generalize to the ALPN model.

## HandlerRegistry

Maps ALPN byte strings to `ProtocolHandler` instances.

```rust
pub struct HandlerRegistry {
    handlers: HashMap<&'static [u8], Arc<dyn ProtocolHandler>>,
}

impl HandlerRegistry {
    pub fn new() -> Self;
    pub fn register(&mut self, handler: Arc<dyn ProtocolController>);
    pub fn get(&self, alpn: &[u8]) -> Option<&Arc<dyn ProtocolHandler>>;
    pub fn alpn_strings(&self) -> Vec<Vec<u8>>;
}
```

- `register()`: Insert a handler. Panics if the ALPN is already registered.
- `get()`: Look up a handler by ALPN string.
- `alpn_strings()`: Return all registered ALPN strings. Used to build the TLS `ServerConfig` (for quinn) and the ALPN list (for iroh).

Registration is static at startup (see [OQ-04](../../open-questions.md)). The CLI builds a `HandlerRegistry`, inserts all handlers, and passes it to `AlknetEndpoint::new()`.

### ALPN strings in TLS ServerConfig and iroh endpoint

The quinn endpoint's `rustls::ServerConfig` ALPN list is set from `registry.alpn_strings()` at construction time. The iroh endpoint's ALPN list is similarly derived. Both connection sources advertise the same set of ALPNs.

## Accept Loops

Each active connection source runs its own accept loop. All loops dispatch through the same `HandlerRegistry`:

### Quinn accept loop (public QUIC+TLS)

```
loop {
    tokio::select! {
        incoming = quinn_endpoint.accept() => {
            let connection = incoming.await;  // TLS handshake + ALPN negotiation
            match connection {
                Ok(conn) => dispatch(conn),
                Err(e) => { /* log TLS handshake failure, continue */ }
            }
        }
        _ = shutdown.changed() => break,
    }
}
```

### iroh accept loop (P2P relay-assisted)

iroh's `Endpoint` natively supports ALPN negotiation (step 4 of its connection establishment). The `iroh::Endpoint::set_alpns()` method configures which ALPNs the endpoint advertises — the same mechanism iroh's own `Router` uses internally with its `ProtocolMap`.

We use `iroh::Endpoint` directly (not iroh's `Router`) because our `HandlerRegistry` is shared between quinn and iroh connection sources, and our `AuthContext` construction differs per source. Our accept loop replaces iroh's `Router` accept loop with our own dispatch:

```
loop {
    tokio::select! {
        incoming = iroh_endpoint.accept() => {
            // incoming is an iroh::endpoint::Incoming
            let accepting = incoming.accept();  // Accepting state
            let alpn = accepting.alpn().await;  // ALPN from TLS handshake
            match alpn {
                Ok(alpn) => dispatch(alpn, accepting),
                Err(e) => { /* log handshake failure, continue */ }
            }
        }
        _ = shutdown.changed() => break,
    }
}
```

See iroh's `protocol.rs` (`/workspace/iroh/iroh/src/protocol.rs`) for the reference implementation of this pattern — `handle_connection()` reads the ALPN, looks up the handler in `ProtocolMap`, and calls `handler.accept(connection)`. Our dispatch is the same pattern with our `HandlerRegistry`.

### Dispatch function (shared)

```
fn dispatch(connection) {
    let alpn = connection.alpn();
    match handlers.get(alpn) {
        Some(handler) => {
            let auth = AuthContext::from_connection(&connection);
            let conn = Connection::new(connection);
            tokio::spawn(async move {
                if let Err(e) = handler.handle(conn, &auth).await {
                    // log error, connection closes
                }
            });
        }
        None => connection.close(0u32, "no handler"),
    }
}
```

### What the accept loops do NOT do

- **No byte-peeking**: ALPN negotiation handles protocol detection. The old `stealth` module's `detect_protocol()` is unnecessary.
- **No per-handler accept loops**: The old `ListenerConfig` enum had Stream/Http/Dns variants with different accept paths. ALPN unifies this.
- **No SSH-specific logic**: The accept loop is ALPN-agnostic. It doesn't know or care what protocol the handler speaks.

## Stealth Mode as ALPN Dispatch

The reference implementation's "stealth mode" is SSH-over-TLS on port 443. The TLS cert is **camouflage**, not identity — it makes the port look like a web server to port scanners and DPI systems. Non-SSH traffic gets a fake nginx 404.

In the ALPN model, this maps to:

- The `alknet/http` handler is registered for standard HTTP ALPNs (`h2`, `http/1.1`)
- The HTTP handler can serve a decoy website or a fake 404
- Real services use `alknet/ssh`, `alknet/call`, etc.
- Clients that don't offer alknet ALPNs get the HTTP handler — just like port scanners in stealth mode

No byte-peeking, no `ProtocolDetection` enum. ALPN does the routing.

## Network Identity vs Auth Identity

A key distinction that the ALPN model makes explicit:

| Layer | Purpose | Mechanism |
|-------|---------|-----------|
| **Network identity** | How a client finds and verifies the node | TLS cert (quinn), NodeId (iroh) |
| **Auth identity** | Who the peer is and what they can do | SSH key, API token, certificate (handlers) |

The TLS cert is the node's network-facing identity — it's what `alknet.example.com` resolves to. It's NOT the node's authentication identity. Auth happens inside the handler via `IdentityProvider`.

This matches the reference implementation: the TLS cert encrypts and camouflages, but SSH key exchange handles the actual authentication.

## TLS Certificate Provisioning

For the quinn endpoint, `StaticConfig` provides TLS configuration via file paths:

- **Manual**: `tls_cert` and `tls_key` file paths. Required for production use.
- **Self-signed**: For development. The endpoint can generate a self-signed cert on startup.

The `rustls::ServerConfig` is built from cert + key + ALPN list at startup.

ACME auto-provisioning (Let's Encrypt) is not in scope for v1. It will be added as a feature later (see OQ-12).

The iroh endpoint does not need TLS certs — it uses `NodeId` for identity.

## Graceful Shutdown

```rust
impl AlknetEndpoint {
    pub fn shutdown_sender(&self) -> watch::Sender<bool>;
    pub async fn shutdown(&self) -> Result<(), EndpointError>;
}
```

- `shutdown_sender()` returns a clone of the shutdown channel sender. Call `send(true)` to signal shutdown.
- `shutdown()` signals all accept loops to stop, waits for in-flight connections with a drain timeout (default: 2 seconds), then forcefully closes remaining connections.
- SIGTERM/SIGINT are wired to the shutdown channel by the CLI binary.

The drain timeout is configurable via `StaticConfig::drain_timeout`.

## Error Handling

### EndpointError

Fatal errors that prevent the endpoint from starting or continuing.

```rust
pub enum EndpointError {
    BindFailed(io::Error),
    TlsConfig(io::Error),
    HandlerNotFound(Vec<u8>),  // ALPN string with no registered handler
}
```

### HandlerError

Non-fatal errors within a handler. See [core-types.md](core-types.md) for details.

### Accept loop errors

- **TLS handshake failure**: Log and continue. The client may have offered no compatible ALPN, or the cert may be untrusted.
- **Handler panic**: Caught by tokio's task isolation. The connection is dropped. Other connections continue.
- **Connection-level errors** (quinn/iroh `ConnectionError`): Log and continue. The accept loop keeps running.

## Key Differences from Reference Implementation

| Aspect | Reference (`alknet-main`) | New Model |
|--------|---------------------------|-----------|
| Transport | `TransportAcceptor` trait, `TransportKind` enum | `quinn::Endpoint` + `iroh::Endpoint`, ALPN dispatch |
| Listener config | `ListenerConfig` enum (Stream/Http/Dns) | Single `HandlerRegistry`, ALPN dispatch |
| Protocol detection | Byte-peeking (`stealth::detect_protocol`) | ALPN negotiation (TLS layer) |
| Stealth mode | SSH-over-TLS with byte-peek | HTTP handler on `h2`/`http/1.1` serves decoy |
| Accept loop | Per-transport, SSH-centric | Per-connection-source, ALPN-agnostic |
| Handler model | `ServerHandler` + `russh::server::Handler` | `ProtocolHandler::handle(Connection, &AuthContext)` |
| Config | `ServeOptions` builder | `StaticConfig` + `HandlerRegistry` + `AlknetEndpoint::new()` |
| iroh | Separate `IrohAcceptor` + `IrohTransport` | `Option<iroh::Endpoint>` on `AlknetEndpoint` |
| Network vs auth identity | Conflated (TLS cert + SSH key both "auth") | Explicitly separated (TLS/NodeId = network, SSH key/token = auth) |

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| Multi-connectivity endpoint (quinn + iroh) | [ADR-010](../../decisions/010-alpn-router-and-endpoint.md) | Both optional, both feed same ALPN router |
| Static handler registration | [ADR-010](../../decisions/010-alpn-router-and-endpoint.md) | Two-way door, start static, add ArcSwap later |
| TCP is not an endpoint concern | [ADR-010](../../decisions/010-alpn-router-and-endpoint.md) | TCP SSH is a handler concern, not core |
| No byte-peeking, ALPN dispatch only | [ADR-001](../../decisions/001-alpn-protocol-dispatch.md) | TLS layer handles protocol detection |
| Stealth mode = HTTP handler on standard ALPNs | [ADR-010](../../decisions/010-alpn-router-and-endpoint.md) | Decoy via ALPN routing, not byte-peek |
| Network identity ≠ auth identity | [ADR-010](../../decisions/010-alpn-router-and-endpoint.md) | TLS cert/NodeId = network, SSH key/token = auth |
| Handler panics isolated | [ADR-010](../../decisions/010-alpn-router-and-endpoint.md) | tokio task isolation, connection closes |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-04**: Resolved — HandlerRegistry is static at startup.
- **OQ-05**: Resolved — multi-connectivity endpoint with quinn + iroh, both feature-gated.
- **OQ-12**: Resolved — start with file paths in StaticConfig, add ACME later.