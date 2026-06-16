# ADR-010: ALPN Router and Endpoint

## Status

Proposed

## Context

ADR-001 establishes ALPN-based protocol dispatch: a single endpoint accepts connections, and the ALPN negotiated during the TLS handshake routes each connection to the correct `ProtocolHandler`. ADR-002 defines the `ProtocolHandler` trait. ADR-006 establishes one ALPN per connection. ADR-007 defines `Connection` and `BiStream`.

The question now is: **how does the endpoint work?** What accepts connections, negotiates ALPN, and hands connections to handlers? This is the central runtime piece of alknet-core — every handler depends on it.

### Multiple connectivity modes, not multiple transports

The reference implementation supports three connectivity modes that serve **fundamentally different deployment contexts**:

1. **QUIC+TLS (public)** — The node has a public IP and open ports. TLS provides protocol routing via ALPN negotiation. The TLS certificate is the node's **network-facing identity** — it's what clients verify when connecting to `alknet.example.com:4433`. This is the mode for replicators, VPS hosts, service providers. SSH key auth still handles **authentication** — the TLS cert is not the auth identity, it's the network identity.

2. **iroh P2P (NAT traversal)** — The node has no public IP or open ports. iroh's relay handles NAT traversal and connection brokering. Node identity comes from iroh's `NodeId` (Ed25519 key pair). The relay is a signaling service, not a proxy — it helps peers establish direct QUIC connections. This is the mode for home servers, IoT devices, anything behind NAT.

3. **TCP (local/dev)** — Bare SSH over TCP. Port 22. No TLS, no ALPN, no certs. SSH key exchange handles both identity and authentication. This is the mode for local network access and development.

These are not interchangeable "transports" to be abstracted behind a trait. They are **different ways a node can be reached**, each with different identity and authentication implications:

| Mode | Identity source | Auth mechanism | Requires public IP | Use case |
|------|-----------------|----------------|-------------------|----------|
| QUIC+TLS | TLS cert (network) + SSH key (auth) | SSH key, API key | Yes | VPS, replicators |
| iroh P2P | NodeId (Ed25519) | NodeId, SSH key | No | Home servers, NAT |
| TCP | SSH host key | SSH key | Yes (local) | Dev, LAN |

### What the old "stealth mode" actually was

The reference implementation's "stealth mode" is **SSH-over-TLS on port 443**. The TLS cert is NOT the node's identity — it's **camouflage**. The purpose is to make port 443 look like a web server to port scanners and DPI systems. Non-SSH traffic gets a fake nginx 404. SSH auth still happens via SSH key exchange *inside* the TLS tunnel.

In the new ALPN model, this concept maps to: the endpoint speaks QUIC+TLS with ALPN, and the `alknet/http` handler can serve a decoy website on `h2`/`http/1.1` while real services use `alknet/ssh`, `alknet/call`, etc. The ALPN router does the "stealth" job — unknown ALPNs get the HTTP handler, which can serve whatever fronting content is desired. No byte-peeking needed.

### iroh produces QUIC connections

iroh's `Endpoint::accept()` produces incoming QUIC connections. These connections have ALPNs. They can feed directly into the same `HandlerRegistry` dispatch. The iroh endpoint and the quinn endpoint both produce QUIC connections — the difference is how they're established (relay-assisted vs direct), not how handlers consume them.

This means: **the ALPN router is transport-agnostic**. It dispatches by ALPN string. It doesn't care whether the connection came from a quinn endpoint or an iroh endpoint. Both produce connections that handlers can use via the same `Connection` type.

### Key design questions

1. **How many endpoints can a node have?** A node may need to listen on quinn (public QUIC+TLS) AND iroh (P2P relay) simultaneously. These are not alternatives — they're complementary connectivity modes.
2. **Handler registration**: Static (at startup) or dynamic (at runtime)?
3. **Connection lifecycle**: Who owns the endpoints? How does graceful shutdown work?
4. **Error handling**: What happens when a handler panics? When ALPN negotiation fails?

## Decision

### A node can have multiple endpoints

`AlknetEndpoint` manages one or more QUIC connection sources. Each source produces connections that feed into the same `HandlerRegistry`:

```rust
pub struct AlknetEndpoint {
    // One or more QUIC connection sources
    quinn: Option<quinn::Endpoint>,       // Public QUIC+TLS
    iroh: Option<iroh::Endpoint>,         // P2P relay-assisted

    handlers: Arc<HandlerRegistry>,
    dynamic: Arc<ArcSwap<DynamicConfig>>,
    identity_provider: Arc<dyn IdentityProvider>,
    shutdown: watch::Receiver<bool>,
}
```

A node that has a public IP runs with `quinn: Some(...)` — it listens on a public address with TLS+ALPN. A node behind NAT runs with `iroh: Some(...)` — it connects to a relay and accepts P2P connections. A node that has both runs with both — it's reachable via either path, and both feed into the same ALPN router.

**TCP mode is not an endpoint concern.** TCP mode in the reference implementation is SSH over raw TCP on port 22. This is not QUIC and doesn't have ALPN. In the new model, TCP access to SSH is handled by the SSH handler directly — it can listen on a TCP socket independently of the ALPN endpoint. This is a handler-specific concern, not a core endpoint concern.

### HandlerRegistry maps ALPN strings to ProtocolHandler instances

```rust
pub struct HandlerRegistry {
    handlers: HashMap<&'static [u8], Arc<dyn ProtocolHandler>>,
}
```

Registration is static at startup (OQ-04). The CLI binary constructs a `HandlerRegistry`, inserts handlers, and passes it to `AlknetEndpoint::new()`.

The ALPN strings for the quinn endpoint's TLS `ServerConfig` are derived from the registry's keys. The iroh endpoint's ALPN strings are also derived from the registry — both endpoints advertise the same set of ALPNs.

### Accept loop: accept from all sources, dispatch by ALPN

The endpoint runs accept loops for each active connection source. All loops dispatch through the same `HandlerRegistry`:

```
// Quinn accept loop (if configured)
loop {
    incoming = quinn_endpoint.accept().await
    connection = incoming.await  // TLS handshake + ALPN negotiation
    dispatch(connection)
}

// iroh accept loop (if configured)
loop {
    incoming = iroh_endpoint.accept().await
    connection = incoming.await  // iroh QUIC connection + ALPN
    dispatch(connection)
}

fn dispatch(connection) {
    alpn = connection.alpn()
    handler = registry.get(alpn)
    match handler {
        Some(h) => {
            auth = AuthContext::from_connection(&connection)
            conn = Connection::new(connection)
            tokio::spawn(h.handle(conn, &auth))
        }
        None => connection.close()
    }
}
```

Both accept loops are `tokio::select!`-ed against the shutdown signal.

### TLS certificate and the distinction between network identity and auth identity

For the quinn endpoint, the TLS cert serves as **network-facing identity** — it's what clients verify when connecting to a domain name. It is NOT the node's authentication identity. Authentication is handled by handlers (SSH key exchange, API tokens, etc.).

This is the same model as the reference implementation's TLS mode: the cert makes the port look legitimate and encrypts traffic, but SSH key exchange handles the actual authentication. The ALPN model extends this: the cert + ALPN routing is the network layer, handler-specific auth is the application layer.

For the iroh endpoint, the `NodeId` serves as network identity. No TLS cert is needed — iroh's QUIC uses the NodeId for connection verification.

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
- A node can be reachable via multiple paths simultaneously (public QUIC+TLS, iroh P2P)
- ALPN router is transport-agnostic — dispatches by ALPN string regardless of connection source
- Adding a handler is registering an ALPN string — no endpoint code changes
- Handler panics are isolated — one bad handler can't take down the endpoint
- "Stealth mode" maps naturally to the HTTP handler serving decoy content on `h2`/`http/1.1`
- Both iroh and quinn produce QUIC connections — same `Connection` type works for both

**Negative:**
- alknet-core depends on both quinn and iroh (mitigated: both are feature-gated; a node that only needs one doesn't compile the other)
- The endpoint is more complex than a single quinn listener — it manages multiple accept loops
- TLS cert provisioning is manual (file paths) for v1 — ACME auto-provisioning is a future feature (OQ-12)
- No runtime handler registration without regenerating the TLS config (mitigated: two-way door, start static, add ArcSwap later if needed)

## References

- ADR-001: ALPN-based protocol dispatch
- ADR-002: ProtocolHandler trait
- ADR-006: ALPN string convention and connection model
- ADR-007: BiStream type definition (Connection, SendStream, RecvStream)
- ADR-009: One-way door decision framework
- OQ-04: Dynamic handler registration (two-way door, start static)
- OQ-05: Multi-transport endpoint (now: multi-connectivity endpoint)
- iroh Router pattern: `docs/research/references/iroh/`
- Reference implementation: `alknet-main/crates/alknet-core/src/server/serve.rs`
- Reference stealth mode: `alknet-main/crates/alknet-core/src/server/stealth.rs`
- Reference iroh transport: `alknet-main/crates/alknet-core/src/transport/iroh_transport.rs`