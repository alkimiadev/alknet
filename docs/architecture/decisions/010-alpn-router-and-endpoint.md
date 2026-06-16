# ADR-010: ALPN Router and Endpoint

## Status

Proposed

## Context

ADR-001 establishes ALPN-based protocol dispatch: a single QUIC+TLS endpoint accepts connections, and the ALPN negotiated during the TLS handshake routes each connection to the correct `ProtocolHandler`. ADR-002 defines the `ProtocolHandler` trait. ADR-006 establishes one ALPN per connection. ADR-007 defines `Connection` and `BiStream`.

The question now is: **how does the endpoint work?** What accepts QUIC connections, negotiates ALPN, and hands connections to handlers? This is the central runtime piece of alknet-core — every handler depends on it.

The reference implementation (`alknet-main`) uses a `Server` struct that binds a `TransportAcceptor`, runs an accept loop, and dispatches to a `ServerHandler` based on transport type and interface kind. This has three problems that the ALPN model solves:

1. **Multiple listener types**: `ListenerConfig` has three variants (Stream, Http, Dns) with per-variant configuration and validation. ALPN eliminates this — one endpoint, one listener, ALPN does the routing.
2. **Protocol detection by byte-peeking**: The `stealth` module reads the first bytes to detect SSH vs HTTP. ALPN negotiation makes this unnecessary — the TLS handshake tells you the protocol before any application bytes are read.
3. **SSH-centric accept loop**: The current `handle_connection` immediately enters `russh::server::run_stream`. In the new model, the accept loop is ALPN-agnostic — it doesn't know or care what protocol the handler speaks.

### iroh's pattern

iroh's `Router` registers `ProtocolHandler` instances with ALPN strings, then calls `endpoint.accept()` in a loop. For each incoming connection, it reads the negotiated ALPN, looks up the handler, and calls `handler.accept(connection)`. This is clean and proven.

### Key design questions

1. **Handler registration**: Static (at startup) or dynamic (at runtime)?
2. **TLS certificate management**: How does the endpoint get TLS certs? Where does ACME fit?
3. **Connection lifecycle**: Who owns the `quinn::Endpoint`? How does graceful shutdown work?
4. **Error handling**: What happens when a handler panics? When ALPN negotiation fails?

## Decision

### Endpoint owns the QUIC endpoint

`alknet-core` owns the `quinn::Endpoint` directly. The endpoint binds to a single address, configures TLS with a `rustls::ServerConfig` that includes the ALPN strings from all registered handlers, and accepts connections in a loop.

```rust
pub struct AlknetEndpoint {
    endpoint: quinn::Endpoint,
    handlers: Arc<HandlerRegistry>,
    dynamic: Arc<ArcSwap<DynamicConfig>>,
    identity_provider: Arc<dyn IdentityProvider>,
    shutdown: watch::Receiver<bool>,
}
```

There is no `TransportAcceptor` trait, no `TransportKind` enum, no `ListenerConfig` enum. QUIC+TLS+ALPN replaces all of that.

### HandlerRegistry maps ALPN strings to ProtocolHandler instances

```rust
pub struct HandlerRegistry {
    handlers: HashMap<&'static [u8], Arc<dyn ProtocolHandler>>,
}
```

Registration is static at startup. The CLI binary constructs a `HandlerRegistry` by inserting handlers for each ALPN, then passes it to `AlknetEndpoint::new()`. The ALPN strings in the TLS `ServerConfig` are derived from the registry's keys.

This is a two-way door (OQ-04): starting static is simple. If dynamic registration is needed later, the registry can be wrapped in `ArcSwap<HandlerRegistry>` and the TLS `ServerConfig` can be regenerated. But ALPN negotiation happens during the TLS handshake, so adding a handler at runtime requires the next connection to use the new ALPN — which the client already has to know about. Dynamic registration has limited value for v1.

### Accept loop: connect, dispatch, spawn

```
loop {
    incoming = endpoint.accept().await
    connection = incoming.await  // TLS handshake + ALPN negotiation
    alpn = connection.alpn()
    handler = registry.get(alpn)
    
    match handler {
        Some(h) => {
            auth = resolve_endpoint_auth(connection)  // TLS client cert, etc.
            tokio::spawn(h.handle(connection, &auth))
        }
        None => connection.close()
    }
}
```

Key behaviors:
- **ALPN mismatch**: The TLS handshake fails. This is correct — the client and server have no protocol in common.
- **Handler not found**: Should not happen — the `ServerConfig` only advertises ALPNs that have registered handlers. If somehow a connection negotiates an ALPN with no handler, the connection is closed with an error log.
- **Handler panic**: The handler runs in a spawned tokio task. If it panics, the task is caught by tokio's panic handler. The connection is dropped. Other connections are unaffected.
- **Graceful shutdown**: A `watch::Sender<bool>` signals the accept loop to stop accepting new connections. Existing connections are given a drain timeout (2 seconds default), then forcefully closed.

### TLS certificate configuration

TLS certs come from `StaticConfig`:
- File paths (`tls_cert`, `tls_key`) for manual provisioning
- Self-signed for development

The `rustls::ServerConfig` is built from the cert + key + ALPN list at startup. The ALPN list is derived from `HandlerRegistry::alpn_strings()`.

ACME auto-provisioning (Let's Encrypt) is not in scope for v1. It will be added as a feature later (see OQ-12).

### Error taxonomy

```rust
pub enum EndpointError {
    BindFailed(io::Error),
    TlsConfig(io::Error),
    HandlerNotFound(Vec<u8>),  // ALPN string with no registered handler
}

pub enum HandlerError {
    ConnectionClosed,
    StreamError(io::Error),
    AuthRequired,
    Internal(Box<dyn std::error::Error + Send + Sync>),
}
```

- `EndpointError`: Problems starting or running the endpoint. Fatal — the endpoint cannot accept connections.
- `HandlerError`: Problems within a handler's `handle()` method. Non-fatal — the connection is closed, but the endpoint keeps running.

## Consequences

**Positive:**
- Single accept loop replaces multiple listener types and byte-peeking
- ALPN negotiation happens at the TLS layer — no application-level protocol detection
- Adding a handler is registering an ALPN string — no endpoint code changes
- Handler panics are isolated — one bad handler can't take down the endpoint
- `quinn::Endpoint` is the only transport — no TransportAcceptor trait needed for v1
- The endpoint is testable: give it mock handlers and a test ALPN, verify dispatch

**Negative:**
- Direct quinn dependency in alknet-core — WASM targets can't use quinn (mitigated: WASM clients don't run endpoints, they connect to them; the WASM door is for client-side handlers, not the endpoint itself)
- No runtime handler registration without regenerating the TLS config (mitigated: two-way door, start static, add ArcSwap later if needed)
- TLS cert provisioning is manual (file paths) for v1 — ACME auto-provisioning is a future feature (OQ-12)
- One address per endpoint — if you need to listen on multiple addresses, run multiple endpoints (acceptable for v1)

## References

- ADR-001: ALPN-based protocol dispatch
- ADR-002: ProtocolHandler trait
- ADR-006: ALPN string convention and connection model
- ADR-007: BiStream type definition (Connection, SendStream, RecvStream)
- ADR-009: One-way door decision framework
- OQ-04: Dynamic handler registration (two-way door, start static)
- OQ-05: Multi-transport endpoint (two-way door, start with quinn)
- iroh Router pattern: `docs/research/references/iroh/`
- Reference implementation: `alknet-main/crates/alknet-core/src/server/serve.rs`