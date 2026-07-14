---
status: draft
last_updated: 2026-07-15
---

# Endpoint

ALPN router, handler registry, connection accept loops, multi-connectivity, and graceful shutdown.

See [ADR-010](../../decisions/010-alpn-router-and-endpoint.md) for the full rationale.

## AlknetEndpoint

The central runtime type. Manages one or more QUIC connection sources, each feeding into the same ALPN router.

```rust
pub struct AlknetEndpoint {
    // One or more connection sources — all optional, all can be active simultaneously
    quinn: Option<quinn::Endpoint>,       // Public QUIC+TLS
    iroh: Option<iroh::Endpoint>,         // P2P relay-assisted
    #[cfg(feature = "tcp")]
    tcp_tls: Option<TcpTlsListener>,       // TCP+TLS (TcpListener + TlsAcceptor)

    handlers: Arc<HandlerRegistry>,
    dynamic: Arc<ArcSwap<DynamicConfig>>,
    identity_provider: Arc<dyn IdentityProvider>,
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
    drain_timeout: Duration,
}
```

See [ADR-083](../../decisions/083-endpoint-as-accept-loop-runner.md) for
the full design (the endpoint takes no `StaticConfig` or TLS config;
transports are built by the assembly layer and handed via
`with_quinn` / `with_iroh` / `with_tcp_tls`).

### Why multiple connection sources?

A node can be reachable through different paths depending on its network context:

| Source | Requires | Identity source | Use case |
|--------|----------|-----------------|----------|
| `quinn::Endpoint` | Public IP, TLS cert | TLS cert (network), SSH key (auth) | VPS, replicators, service hosts |
| `iroh::Endpoint` | Relay access | NodeId (Ed25519) | Home servers, NAT, IoT |

These are not interchangeable transports — they are **complementary connectivity modes**. A node behind NAT that also has a public IP can use both simultaneously. Both produce QUIC connections that dispatch through the same `HandlerRegistry` by ALPN string.

> **Terminology — hub, worker, hub-worker.** A *hub* accepts inbound
> connections from workers and browsers (see
> [`crates/hub/README.md`](../hub/README.md) for the hub-and-spoke
> topology). A *worker* dials out to a hub. A *hub-worker* does both. A
> *pure worker* has no inbound endpoints. These terms come from ADR-029
> / ADR-034; "assembly layer" (ADR-014) is the deployment binary that
> wires crates — in practice, today, usually a hub or hub-worker.

### TCP+TLS is a first-class owned transport

TCP+TLS is a listener transport, same shape as quinn and iroh. The
endpoint owns it via `with_tcp_tls(listener, acceptor)` (behind a `tcp`
feature) and runs its accept loop inside `run()` — `tcp.accept()` →
`tls.accept()` → extract ALPN + fingerprint → `Connection::from_bidi` →
`dispatch`. No external sibling loop, no duplicated dispatch logic.

This reverses ADR-010's original "TCP is not an endpoint struct concern."
The reason TCP was excluded — the endpoint built transports internally,
and TCP+TLS couldn't fit that shape — is gone (ADR-083: the endpoint no
longer builds transports; it runs accept loops on whatever it's given).
TCP+TLS fits the same listener shape as quinn and iroh.

The `dispatch` method is public for transports the endpoint **can't
own** — SSH channels (one connection, many channels with different
ALPNs — a multiplexing shape, not a listener) and future WebTransport
streams (one QUIC connection, many WT streams). These are
connection-internal multiplexing, not listener transports.

See [ADR-083](../../decisions/083-endpoint-as-accept-loop-runner.md)
for the full design.

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

- `register()`: Insert a handler. Panics if the ALPN is already registered.
- `get()`: Look up a handler by ALPN string.
- `alpn_strings()`: Return all registered ALPN strings. Used to build the TLS `ServerConfig` (for quinn) and the ALPN list (for iroh).

Registration is static at startup (see [OQ-04](../../open-questions.md)). The CLI builds a `HandlerRegistry`, inserts all handlers, and passes it to `AlknetEndpoint::new()`.

### ALPN strings in TLS ServerConfig and iroh endpoint

ALPN-list construction is the **assembly layer's** responsibility, not
the endpoint's. After ADR-083, the endpoint takes no TLS config and
builds no transports — the assembly layer reads `registry.alpn_strings()`
and passes the appropriate ALPN list to each `TlsServerConfig::new()`
(see [`crates/tls/README.md`](../tls/README.md)). For a single-config
deployment (one identity, one set of ALPNs), all transports advertise
the same set. For a two-config hub (raw key + X.509/ACME — see OQ-62),
the assembly layer may pass different lists to each config; that split
is a hub-assembly-layer concern, not an endpoint concern.

The iroh endpoint's ALPN list is set via `iroh::Endpoint::builder().alpns()`
by the assembly layer at construction time, from the same
`registry.alpn_strings()` source.

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

The public `dispatch` method is the shared dispatch path for every
transport — the endpoint's own accept loops (quinn, iroh, TCP+TLS) call
it after transport-specific extraction, and external dispatch callers
(SSH channels, future WebTransport streams) call it after their own
extraction.

```
pub fn dispatch(&self, connection: Connection, alpn: Vec<u8>,
                fingerprint: Option<String>, remote_addr: Option<SocketAddr>) {
    // ACME guard (transport-agnostic — ADR-083)
    if alpn == b"acme-tls/1" {
        connection.close(0, "acme done");
        return;
    }
    match handlers.get(&alpn) {
        Some(handler) => {
            let auth = build_auth_context(&alpn, remote_addr, fingerprint, &identity_provider);
            tokio::spawn(async move {
                if let Err(e) = handler.handle(connection, &auth).await {
                    // log error, connection closes
                }
            });
        }
        None => { connection.close(0, "no handler"); /* log warning */ }
    }
}
```

Synchronous (non-async): spawns the handler on its own task and returns
immediately. The caller's accept loop is not blocked. See
[ADR-083](../../decisions/083-endpoint-as-accept-loop-runner.md) for the
full dispatch contract.

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
| **Network identity** | How a client finds and verifies the node | X.509 cert (domain) or RFC 7250 raw key (Ed25519) or iroh NodeId |
| **Auth identity** | Who the peer is and what they can do | SSH key, API token, certificate (handlers) |

The TLS cert (or raw public key, or NodeId) is the node's network-facing identity. It's NOT the node's authentication identity. Auth happens inside the handler via `IdentityProvider`.

This matches the reference implementation: the TLS cert encrypts and camouflages, but SSH key exchange handles the actual authentication.

## RFC 7250: Raw Public Keys in TLS

RFC 7250 raw public keys are the **default TLS identity mode** for most alknet nodes. They eliminate the need for domain names, CAs, and certificate renewal — the Ed25519 public key IS the node's identity.

iroh uses this model with its `NodeId`. The implementation is ~100 lines (see `iroh/iroh/src/tls/resolver.rs`): take an Ed25519 key, wrap its SPKI public key as a `CertificateDer`, tell rustls `only_raw_public_keys() -> true`. No X.509, no CAs, no domain names, no cert renewal.

Key implications:

- **Default for alknet-native clients**: SSH, git, and alknet-native clients all work with raw Ed25519 keys out of the box. The same key type used for SSH auth can serve as the TLS identity. This is the most common deployment mode.
- **No domain required**: A node without a domain name uses raw public keys for the quinn path — key-based identity with direct QUIC over UDP.
- **Key = identity**: The Ed25519 public key IS the node's identity. No CA trust chain, no cert expiry. The key can be derived from alknet-vault.
- **X.509 is for domain-hosted services**: Domain-facing identity (replicators, public services, browsers) uses X.509 certs. This is a separate use case, not the default.
- **Browser limitation**: Browsers don't support RFC 7250. For browser/WebTransport clients, X.509 certs are needed. For all other clients, raw public keys work fine.

The quinn and iroh paths share the same key-based identity model via RFC 7250. They're distinguished by **connection establishment** (direct UDP vs relay-assisted), not by identity:

| Path | Connection establishment | Default identity | Alternative identity |
|------|------------------------|-----------------|---------------------|
| quinn | Direct UDP, public IP | RFC 7250 raw key (most nodes) | X.509 cert (domain-hosted, browsers) |
| iroh | Relay-assisted P2P | RFC 7250 raw key (NodeId) | N/A |

## TLS Identity

TLS identity in alknet has two distinct use cases, each with a different trust model and provisioning mechanism. See OQ-12 for the full rationale.

### Use case 1: P2P / Key-based identity (default)

Most alknet nodes use RFC 7250 raw Ed25519 public keys for TLS identity. No domain name, no CA, no certificate renewal. The Ed25519 public key IS the node's identity — the same key model as iroh's `NodeId`, but for direct QUIC connections.

`TlsIdentity::RawKey` in `StaticConfig` configures this mode. The endpoint builds a `rustls::ServerConfig` with `only_raw_public_keys() -> true` and a `ResolvesServerCert` that generates the certificate on-the-fly from the key, exactly as iroh does (see `iroh/iroh/src/tls/resolver.rs`).

This mode works natively with SSH auth (same key type) and git (SSH key-based auth). It is the default for alknet-native clients. **Browser/WebTransport clients do not support RFC 7250** — they require X.509 certificates.

### Use case 2: Domain-hosted services

Nodes that serve browser/WebTransport clients, or nodes with public domain names, use X.509 certificates. This has two sub-cases:

- **Manual**: Provide cert/key file paths via `TlsIdentity::X509`. The endpoint loads them at startup and builds a standard `rustls::ServerConfig`.
- **ACME auto-provisioning**: Let's Encrypt via `rustls-acme`. `TlsIdentity::Acme { domains, cache_dir, directory, contact }` carries the static config; the endpoint constructs the `AcmeState` async state machine and `ResolvesServerCertAcme` at setup time (ADR-027). The `acme` feature gate keeps `rustls-acme` out of non-ACME builds. See [ADR-027](../../decisions/027-tls-identity-redesign-acme-rawkey-decoupling.md) for the full design.

`TlsIdentity::SelfSigned` is for development only — the endpoint generates a self-signed cert on startup. External clients will not trust it.

### iroh endpoint identity

The iroh endpoint does not need TLS certificate configuration — it uses `NodeId` (Ed25519) for identity, which is RFC 7250 raw key identity built into the iroh endpoint.

### Identity model comparison

| Path | Identity model | Client compatibility | Use case |
|------|---------------|---------------------|----------|
| quinn + `TlsIdentity::RawKey` | RFC 7250 Ed25519 raw key | alknet-native, SSH, git | Personal nodes, P2P, most deployments |
| quinn + `TlsIdentity::X509` | X.509 domain certificate (manual) | All clients including browsers | Relays, public services, WebTransport |
| quinn + `TlsIdentity::Acme` | X.509 via ACME auto-provisioning | All clients including browsers | Public relays, domain-hosted services |
| quinn + `TlsIdentity::SelfSigned` | X.509 self-signed cert | None (dev only) | Local development |
| iroh | NodeId (Ed25519, RFC 7250 built-in) | alknet-native, iroh clients | NAT traversal, home servers |

Note: `TlsIdentity::RawKey` uses `Ed25519SecretKey` (alknet-core-owned,
backed by `ed25519-dalek`), not `iroh::SecretKey`. It is available in
quinn-only builds without the `iroh` feature. When the iroh transport is
also configured, `build_iroh_endpoint` converts the key to
`iroh::SecretKey::from_bytes` (ADR-027). The iroh dep is on `1.0`
(`default-features = false, features = ["tls-aws-lc-rs"]`, matching the
quinn path's aws-lc-rs crypto provider); migrated from `0.35` in commit
`acd049e` (2026-07-09) — 6 API surface edits, no architectural change.

## Graceful Shutdown

```rust
impl AlknetEndpoint {
    pub fn shutdown_sender(&self) -> watch::Sender<bool>;
    pub async fn shutdown(&self) -> Result<(), EndpointError>;
}
```

- `shutdown_sender()` returns a clone of the shutdown channel sender. Call `send(true)` to signal shutdown. The assembly layer uses this for any external dispatch callers (SSH, future WT); the endpoint's own loops are signaled internally.
- `shutdown()` signals all owned accept loops (quinn, iroh, TCP+TLS) to stop, waits for in-flight dispatched handlers with a drain timeout, then forcefully closes remaining connections. One owner, one shutdown — no external loop coordination (ADR-083).
- SIGTERM/SIGINT are wired to the shutdown channel by the CLI binary.

The drain timeout is passed to `AlknetEndpoint::new()` directly (as
`drain_timeout: Duration`), not via `StaticConfig` — the endpoint no
longer takes `StaticConfig` (ADR-083). The assembly layer reads
`StaticConfig::drain_timeout` and passes it in.

## Error Handling

### EndpointError

Fatal errors that prevent the endpoint from starting or continuing.

```rust
pub enum EndpointError {
    BindFailed(io::Error),
    HandlerNotFound(Vec<u8>),  // ALPN string with no registered handler
}
```

After ADR-083, the endpoint takes no TLS config and constructs no
transports — TLS config errors now surface as `TlsError` in
`alknet-tls` / the assembly layer (see
[`crates/tls/README.md`](../tls/README.md) and OQ-62), not as
`EndpointError`. The `TlsConfig(io::Error)` variant that existed when
the endpoint built TLS internally is removed. `BindFailed` covers
listener bind failures (quinn, iroh, TCP+TLS `TcpListener::bind`).

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
| Multi-connectivity endpoint (quinn + iroh + TCP+TLS) | [ADR-010](../../decisions/010-alpn-router-and-endpoint.md), [ADR-083](../../decisions/083-endpoint-as-accept-loop-runner.md) | All three optional, all feed same dispatch; endpoint owns all accept loops |
| Endpoint takes no TLS config; assembly layer builds transports | [ADR-083](../../decisions/083-endpoint-as-accept-loop-runner.md) | `new()` takes `drain_timeout` + builder methods, no `StaticConfig` or `Arc<TlsServerConfig>` |
| TCP+TLS is a first-class owned transport | [ADR-083](../../decisions/083-endpoint-as-accept-loop-runner.md) | `with_tcp_tls(listener, acceptor)` — reverses ADR-010's "TCP is not an endpoint struct concern" |
| Public `dispatch` for SSH/WT (multiplexing shapes) | [ADR-083](../../decisions/083-endpoint-as-accept-loop-runner.md) | `dispatch` is public for connection-internal multiplexing, not for listener transports |
| Static handler registration | [ADR-010](../../decisions/010-alpn-router-and-endpoint.md) | Two-way door, start static, add ArcSwap later |
| No byte-peeking, ALPN dispatch only | [ADR-001](../../decisions/001-alpn-protocol-dispatch.md) | TLS layer handles protocol detection |
| Stealth mode = HTTP handler on standard ALPNs | [ADR-010](../../decisions/010-alpn-router-and-endpoint.md) | Decoy via ALPN routing, not byte-peek |
| Network identity ≠ auth identity | [ADR-010](../../decisions/010-alpn-router-and-endpoint.md) | TLS cert/NodeId = network, SSH key/token = auth |
| Handler panics isolated | [ADR-010](../../decisions/010-alpn-router-and-endpoint.md) | tokio task isolation, connection closes |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-04**: Resolved — HandlerRegistry is static at startup.
- **OQ-05**: Resolved — multi-connectivity endpoint with quinn + iroh, both feature-gated.
- **OQ-12**: Resolved — two distinct TLS identity use cases: RFC 7250 raw keys (default, P2P) and X.509 certs (domain-hosted, browsers). ACME auto-provisioning designed in [ADR-027](../../decisions/027-tls-identity-redesign-acme-rawkey-decoupling.md); RawKey decoupled from the `iroh` feature (available in quinn-only builds).
- **OQ-60**: Resolved — transport construction is inlined by the assembly layer; the TCP+TLS loop lives in `alknet-core` behind a `tcp` feature as an owned transport. See [ADR-083](../../decisions/083-endpoint-as-accept-loop-runner.md).
- **OQ-61**: Dissolved — the multi-owner shutdown problem does not arise; the endpoint owns all its accept loops. See [ADR-083](../../decisions/083-endpoint-as-accept-loop-runner.md).