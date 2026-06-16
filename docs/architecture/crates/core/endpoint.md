---
status: draft
last_updated: 2026-06-16
---

# Endpoint

ALPN router, handler registry, connection accept loop, and graceful shutdown.

See [ADR-010](../../decisions/010-alpn-router-and-endpoint.md) for the full rationale.

## AlknetEndpoint

The central runtime type. Owns the QUIC endpoint, holds the handler registry, and runs the accept loop.

```rust
pub struct AlknetEndpoint {
    endpoint: quinn::Endpoint,
    handlers: Arc<HandlerRegistry>,
    dynamic: Arc<ArcSwap<DynamicConfig>>,
    identity_provider: Arc<dyn IdentityProvider>,
    shutdown: watch::Receiver<bool>,
}
```

### Construction

The CLI binary constructs an `AlknetEndpoint` at startup:

1. Build `HandlerRegistry` by inserting handlers for each ALPN.
2. Build `StaticConfig` from CLI arguments / config file.
3. Build `rustls::ServerConfig` from TLS cert/key and the registry's ALPN strings.
4. Bind `quinn::Endpoint` with the `ServerConfig`.
5. Create `ArcSwap<DynamicConfig>` and `ConfigIdentityProvider`.
6. Call `AlknetEndpoint::new(endpoint, handlers, dynamic, identity_provider)`.

### Accept Loop

```
loop {
    tokio::select! {
        incoming = endpoint.accept() => {
            let connection = incoming.await;  // TLS handshake + ALPN negotiation
            match connection {
                Ok(conn) => {
                    let alpn = conn.alpn();
                    match handlers.get(alpn) {
                        Some(handler) => {
                            let auth = AuthContext::from_connection(&conn);
                            let conn = Connection::new(conn);
                            tokio::spawn(async move {
                                if let Err(e) = handler.handle(conn, &auth).await {
                                    // log error, connection closes
                                }
                            });
                        }
                        None => {
                            // ALPN has no handler — should not happen
                            // (ServerConfig only advertises registered ALPNs)
                            conn.close(0u32, "no handler");
                        }
                    }
                }
                Err(e) => {
                    // TLS handshake or connection-level error
                    // log and continue accepting
                }
            }
        }
        _ = shutdown.changed() => {
            break;  // graceful shutdown
        }
    }
}
```

### What the accept loop does NOT do

- **No byte-peeking**: ALPN negotiation handles protocol detection. The old `stealth` module's `detect_protocol()` is unnecessary.
- **No per-handler accept loops**: The old model had `ListenerConfig::Stream`, `ListenerConfig::Http`, `ListenerConfig::Dns` with different accept paths. ALPN unifies this.
- **No SSH-specific logic**: The accept loop is ALPN-agnostic. It doesn't know or care what protocol the handler speaks.

## HandlerRegistry

Maps ALPN byte strings to `ProtocolHandler` instances.

```rust
pub struct HandlerRegistry {
    handlers: HashMap<&'static [u8], Arc<dyn ProtocolHandler>>,
}

impl HandlerRegistry {
    pub fn new() -> Self;
    pub fn register(&mut self, handler: Arc<dyn ProtocolHandler>);
    pub fn get(&self, alpn: &[u8]) -> Option<&Arc<dyn ProtocolHandler>>;
    pub fn alpn_strings(&self) -> Vec<Vec<u8>>;
}
```

- `register()`: Insert a handler. Panics if the ALPN is already registered (duplicate handlers are a bug).
- `get()`: Look up a handler by ALPN string. Returns `None` if no handler is registered.
- `alpn_strings()`: Return all registered ALPN strings. Used to build the TLS `ServerConfig`.

Registration is static at startup (see [OQ-04](../../open-questions.md) and ADR-010). The CLI builds a `HandlerRegistry`, inserts all handlers, and passes it to `AlknetEndpoint`. The registry is immutable after construction.

### ALPN strings in the TLS ServerConfig

The `rustls::ServerConfig`'s ALPN protocol list is set from `registry.alpn_strings()` at construction time. This means:
- Only registered handlers' ALPNs are advertised during TLS negotiation.
- If a client offers an ALPN that's not in the list, the TLS handshake fails — correct behavior.
- Adding a handler at runtime requires rebuilding the `ServerConfig` (see OQ-04).

## Graceful Shutdown

```rust
impl AlknetEndpoint {
    pub fn shutdown_sender(&self) -> watch::Sender<bool>;
    pub async fn shutdown(&self) -> Result<(), EndpointError>;
}
```

- `shutdown_sender()` returns a clone of the shutdown channel sender. Call `send(true)` to signal shutdown.
- `shutdown()` waits for in-flight connections to complete, with a drain timeout (default: 2 seconds). After the timeout, remaining connections are forcefully closed.
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

- **TLS handshake failure**: Log and continue. The client may have offered no compatible ALPN, or the cert may be untrusted by the client.
- **Handler panic**: Caught by tokio's task isolation. The connection is dropped. Other connections continue.
- **Connection-level errors** (quinn `ConnectionError`): Log and continue. The accept loop keeps running.

## TLS Certificate Provisioning

`StaticConfig` provides TLS configuration via file paths:

- **Manual**: `tls_cert` and `tls_key` file paths. Required for production use.
- **Self-signed**: For development. The endpoint can generate a self-signed cert on startup.

The `rustls::ServerConfig` is built from cert + key + ALPN list at startup.

ACME auto-provisioning (Let's Encrypt) is not in scope for v1. It will be added as a feature later (see OQ-12).

## Key Differences from Reference Implementation

| Aspect | Reference (`alknet-main`) | New Model |
|--------|---------------------------|-----------|
| Transport | `TransportAcceptor` trait, `TransportKind` enum | `quinn::Endpoint` directly |
| Listener config | `ListenerConfig` enum (Stream/Http/Dns) | Single endpoint, ALPN dispatch |
| Protocol detection | Byte-peeking (`stealth::detect_protocol`) | ALPN negotiation (TLS layer) |
| Accept loop | Per-transport, SSH-centric | ALPN-agnostic, handler-dispatched |
| Handler model | `ServerHandler` + `russh::server::Handler` | `ProtocolHandler::handle(Connection, &AuthContext)` |
| Config | `ServeOptions` builder | `StaticConfig` + `HandlerRegistry` + `AlknetEndpoint::new()` |

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| Static handler registration | [ADR-010](../../decisions/010-alpn-router-and-endpoint.md) | Two-way door, start static, add ArcSwap later |
| quinn::Endpoint directly, no TransportAcceptor | [ADR-010](../../decisions/010-alpn-router-and-endpoint.md) | Start with quinn, abstract later if needed |
| No byte-peeking, ALPN dispatch only | [ADR-001](../../decisions/001-alpn-protocol-dispatch.md) | TLS layer handles protocol detection |
| Handler panics isolated | [ADR-010](../../decisions/010-alpn-router-and-endpoint.md) | tokio task isolation, connection closes |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-04**: Resolved — HandlerRegistry is static at startup.
- **OQ-05**: Open — start with quinn, abstract later if needed.
- **OQ-12**: Resolved — start with file paths in StaticConfig, add ACME later.