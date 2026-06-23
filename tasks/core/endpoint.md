---
id: core/endpoint
name: Implement AlknetEndpoint, HandlerRegistry, accept loops (quinn + iroh), TLS identity, and graceful shutdown
status: pending
depends_on: [core/core-types, core/config, core/auth]
scope: broad
risk: high
impact: component
level: implementation
---

## Description

Implement the ALPN router and endpoint in `src/endpoint.rs`. This is the
integration point of alknet-core — it ties together the core types, config,
and auth into the central runtime that accepts connections and dispatches to
handlers by ALPN string.

### AlknetEndpoint

```rust
pub struct AlknetEndpoint {
    quinn: Option<quinn::Endpoint>,
    iroh: Option<iroh::Endpoint>,
    handlers: Arc<HandlerRegistry>,
    dynamic: Arc<ArcSwap<DynamicConfig>>,
    identity_provider: Arc<dyn IdentityProvider>,
    shutdown: watch::Receiver<bool>,
}
```

Manages one or more QUIC connection sources, each feeding into the same ALPN
router. Both quinn and iroh are optional (feature-gated), both can be active
simultaneously (ADR-010).

### HandlerRegistry

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

- `register()`: insert a handler. Panics if ALPN already registered.
- `get()`: look up by ALPN string.
- `alpn_strings()`: all registered ALPNs — used to build TLS ServerConfig
  (quinn) and ALPN list (iroh).
- Registration is **static at startup** (OQ-04, ADR-010). The CLI builds the
  registry, inserts all handlers, passes to `AlknetEndpoint::new()`.

### Accept loops

Each active connection source runs its own accept loop. Both dispatch through
the same `HandlerRegistry`.

**Quinn accept loop** (public QUIC+TLS):
```
loop {
    tokio::select! {
        incoming = quinn_endpoint.accept() => {
            let connection = incoming.await;
            match connection {
                Ok(conn) => dispatch(conn),
                Err(e) => { /* log TLS handshake failure, continue */ }
            }
        }
        _ = shutdown.changed() => break,
    }
}
```

**iroh accept loop** (P2P relay-assisted):
```
loop {
    tokio::select! {
        incoming = iroh_endpoint.accept() => {
            let accepting = incoming.accept();
            let alpn = accepting.alpn().await;
            match alpn {
                Ok(alpn) => dispatch(alpn, accepting),
                Err(e) => { /* log handshake failure, continue */ }
            }
        }
        _ = shutdown.changed() => break,
    }
}
```

Use `iroh::Endpoint` directly (not iroh's `Router`) because our HandlerRegistry
is shared between quinn and iroh, and our AuthContext construction differs per
source. See iroh's `protocol.rs` for the reference pattern.

### Dispatch function (shared)

```
fn dispatch(connection) {
    let alpn = connection.alpn();
    match handlers.get(alpn) {
        Some(handler) => {
            let auth = AuthContext::from_connection(&connection);
            let conn = Connection::from_quinn(connection); // or from_iroh
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

### AuthContext construction

The endpoint constructs `AuthContext` from the QUIC connection:
1. `alpn`: from `connection.alpn()` — always present
2. `remote_addr`: from `connection.remote_addr()` — may be None for iroh
3. `tls_client_fingerprint`: extracted from TLS session's client cert, if presented
4. `identity`: if fingerprint available, call `IdentityProvider::resolve_from_fingerprint()`.
   If resolves, `identity = Some(resolved)`. If not, `identity = None`.

### TLS Identity

Three modes per `TlsIdentity` (OQ-12):

**RawKey (RFC 7250, default for P2P)**:
- Build `rustls::ServerConfig` with `only_raw_public_keys() -> true`
- `ResolvesServerCert` generates cert on-the-fly from the Ed25519 key
- ~100 lines — see `iroh/iroh/src/tls/resolver.rs` for the reference pattern
- Works natively with SSH auth and git; browsers do NOT support RFC 7250

**X509 (domain-hosted)**:
- Load cert/key from file paths
- Standard `rustls::ServerConfig`
- For browser/WebTransport clients and public domain services

**SelfSigned (dev only)**:
- Generate self-signed cert on startup
- External clients will not trust it

**ACME (future, not in this task)**:
- The reverse-proxy project demonstrates the complete ACME pattern. It will be
  adapted as an additional `TlsIdentity` variant or `ResolvesServerCert` impl.
  For now, X509 with manual certs is the domain path. Note this as a TODO.

The quinn endpoint's `rustls::ServerConfig` ALPN list is set from
`registry.alpn_strings()` at construction time. The iroh endpoint's ALPN list
is similarly derived. Both advertise the same set of ALPNs.

### Graceful shutdown

```rust
impl AlknetEndpoint {
    pub fn shutdown_sender(&self) -> watch::Sender<bool>;
    pub async fn shutdown(&self) -> Result<(), EndpointError>;
}
```

- `shutdown_sender()`: clone of shutdown channel sender. `send(true)` signals shutdown.
- `shutdown()`: signals all accept loops to stop, waits for in-flight connections
  with drain timeout (default 2s from StaticConfig), then forcefully closes remaining.
- SIGTERM/SIGINT wired to shutdown channel by the CLI binary (not core's concern).

### EndpointError

```rust
pub enum EndpointError {
    BindFailed(io::Error),
    TlsConfig(io::Error),
    HandlerNotFound(Vec<u8>),
}
```

Fatal errors that prevent the endpoint from starting or continuing.

### Accept loop error handling

- **TLS handshake failure**: log and continue. Client may have offered no
  compatible ALPN, or cert may be untrusted.
- **Handler panic**: caught by tokio's task isolation. Connection dropped,
  others continue.
- **Connection-level errors** (quinn/iroh ConnectionError): log and continue.
  Accept loop keeps running.

### What the accept loops do NOT do

- No byte-peeking (ALPN handles protocol detection)
- No per-handler accept loops (ALPN unifies)
- No SSH-specific logic (accept loop is ALPN-agnostic)

### TCP is NOT an endpoint concern

Bare TCP (SSH over port 22) does not use QUIC or ALPN. TCP access is handled by
individual handlers (the SSH handler can listen on TCP independently). This is
handler-specific, not core endpoint.

## Acceptance Criteria

- [ ] `AlknetEndpoint` struct with quinn/iroh (both Option, both feature-gated)
- [ ] `HandlerRegistry` with new/register/get/alpn_strings
- [ ] `register()` panics on duplicate ALPN
- [ ] Quinn accept loop runs, dispatches by ALPN, respects shutdown
- [ ] iroh accept loop runs, dispatches by ALPN, respects shutdown
- [ ] Dispatch function spawns handler task via `tokio::spawn`
- [ ] AuthContext constructed from connection (alpn, remote_addr, fingerprint, identity)
- [ ] TLS RawKey mode: rustls ServerConfig with `only_raw_public_keys()`, on-the-fly cert
- [ ] TLS X509 mode: load cert/key from files, standard ServerConfig
- [ ] TLS SelfSigned mode: generate self-signed cert on startup
- [ ] ALPN list in TLS ServerConfig set from `registry.alpn_strings()`
- [ ] Graceful shutdown: signal accept loops to stop, drain timeout, force close
- [ ] `EndpointError` enum with all variants
- [ ] Accept loop errors logged, loop continues (no crash on handshake failure)
- [ ] Handler panics caught by tokio task isolation (connection dropped, others continue)
- [ ] No byte-peeking, no per-handler accept loops, no SSH-specific logic
- [ ] Unit test: HandlerRegistry register/get/alpn_strings
- [ ] Unit test: HandlerRegistry register panics on duplicate ALPN
- [ ] Integration test: endpoint with mock handler, verify dispatch by ALPN
- [ ] `cargo test -p alknet-core` succeeds
- [ ] `cargo clippy -p alknet-core` succeeds with no warnings

## References

- docs/architecture/crates/core/endpoint.md — full endpoint spec
- docs/architecture/decisions/001-alpn-protocol-dispatch.md — ADR-001
- docs/architecture/decisions/010-alpn-router-and-endpoint.md — ADR-010
- docs/architecture/decisions/006-alpn-convention-and-connection-model.md — ADR-006
- docs/architecture/decisions/007-bistream-type-definition.md — ADR-007
- iroh reference: `/workspace/iroh/iroh/src/protocol.rs` (accept loop pattern)
- iroh reference: `/workspace/iroh/iroh/src/tls/resolver.rs` (RFC 7250 raw key)

## Notes

> This is the integration point of alknet-core — it ties together types, config,
> and auth. The highest-risk task in core because it involves QUIC connection
> handling, TLS identity (3 modes), and graceful shutdown. The RFC 7250 raw key
> path is ~100 lines (iroh has a reference implementation). ACME is deferred —
> note as TODO, use X509 manual certs for the domain path for now. TCP is NOT
> an endpoint concern — it's handler-specific.

## Summary

> To be filled on completion