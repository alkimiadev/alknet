# ADR-083: Endpoint as Pure Accept-Loop Runner with Public Dispatch

## Status

Proposed

## Context

`AlknetEndpoint` (ADR-010) conflates two concerns:

1. **Transport construction** — building `quinn::Endpoint` and
   `iroh::Endpoint`, reading `static_config.tls_identity`, constructing TLS
   internally (`TlsSetup::new`, `build_rustls_server_config`), wrapping
   rustls into `quinn::ServerConfig`, binding.
2. **Dispatch** — take a connection, extract its ALPN, look up the
   handler, build `AuthContext`, spawn the handler.

The conflation was tolerable while TLS was welded to quinn (one transport,
one cert path). Two things surfaced it:

- **ADR-082 (`alknet-tls` extraction)** pulls quinn's TLS construction
  out of the endpoint into a shareable `TlsServerConfig`. Once the cert
  is built outside the endpoint, the endpoint's transport-construction
  role is the odd one out — it's the only thing still reading
  `static_config` and building transports.
- **A hub holds two `TlsServerConfig`s**, not one (raw key for native
  clients on QUIC + TCP+TLS fallback; X.509/ACME for HTTPS on TCP+TLS).
  ADR-082's original signature `AlknetEndpoint::new(..., Arc<TlsServerConfig>)`
  cannot represent this — a hub has two configs and the endpoint has no
  single "the TLS config" to take. The TLS extraction surfaces this
  because the endpoint can no longer build the one cert it used to own.

### The dispatch logic is the endpoint's real value

Stripped of transport construction, what the endpoint provides is
**dispatch**: take a connection, extract its ALPN, look up the handler,
build `AuthContext`, spawn. This is identical for every transport. The
transport-specific parts are narrow bookends:

- **ALPN extraction** — quinn: `handshake_data().protocol`; iroh:
  `connecting.alpn().await`; TCP+TLS: `tls_stream.alpn()`.
- **Fingerprint extraction** — quinn: `peer_identity()` →
  `CertificateDer`; iroh: `remote_id()` → `ed25519:<hex>`; TCP+TLS:
  `tls_stream.peer_certificates()` → `CertificateDer`.
- **Connection construction** — quinn: `from_quinn_with_alpn`; iroh:
  `from_iroh`; TCP+TLS: `from_bidi(tls_stream, alpn, remote_addr)`.
- **No-handler close** — quinn/iroh: `connection.close()`; TCP+TLS: drop
  the stream (the `Connection::close` API handles both uniformly —
  ADR-065).

Everything between those bookends — the ACME ALPN guard, the handler
lookup, the `build_auth_context` call, the `tokio::spawn` — is identical.
Exposing a public `dispatch` method that takes the already-extracted
ALPN, fingerprint, and `Connection` lets every transport's accept loop
call the same dispatch path. No duplicated `build_auth_context` or
handler-lookup logic at the assembly layer.

### TCP+TLS is a first-class transport, not a sibling afterthought

ADR-010 Amendment 1 made TCP+TLS a "sibling accept loop" outside the
endpoint because the endpoint's dispatch logic was crate-private and
couldn't be shared. The assembly layer had to duplicate
`build_auth_context` and handler lookup, or call `HandlerRegistry::get`
directly. The "sibling" framing was a workaround for the endpoint being
welded to quinn — not a deliberate design.

With a public `dispatch` method, the TCP+TLS accept loop calls into the
endpoint — the same path quinn and iroh use. Amendment 1's *ownership*
model (the TCP+TLS listener owns its own `TcpListener` and `TlsAcceptor`,
living outside the endpoint struct) survives; its *dispatch* workaround
(duplicated `build_auth_context` and handler-lookup logic) is retired.

TCP+TLS is the most common transport after raw-key QUIC, not an edge
case: HTTPS for browsers, worker registration over HTTP, raw-key fallback
for native clients when UDP is blocked.

### The `acme-tls/1` guard

ADR-027 §5 places the `acme-tls/1` early-return guard in
`dispatch_quinn`. The rationale (no handler for `acme-tls/1`; the
challenge is answered at the TLS layer; close gracefully) is correct.
The **location** is wrong once TCP+TLS exists.

ACME TLS-ALPN-01 (RFC 8737) challenges are validated by the CA
connecting **over TCP to port 443**. Let's Encrypt's validator is a TCP
TLS client — it does not speak QUIC. In any real hub deployment, the
ACME challenge arrives on the **TCP+TLS** listener (443), not the QUIC
listener (4433). Today the guard fires only because quinn happens to be
the listener that's bound to the ACME port — an artifact of TCP+TLS not
existing yet, not a property of QUIC.

The guard's job is transport-agnostic: "if the TLS handshake negotiated
`acme-tls/1`, this is a challenge connection — the cert was already
served at the TLS layer via `ResolvesServerCertAcme`, close it, no
handler." `Connection::close` is transport-abstracted (ADR-065), so
the guard works uniformly for quinn, iroh, and TCP+TLS. The right home is
the **shared `dispatch` method**, not `dispatch_quinn`. In practice only
the TCP+TLS listener on 443 receives challenges (CAs validate via TCP);
advertising `acme-tls/1` on a QUIC listener that shares the ACME config
is harmless — no QUIC client negotiates it.

### `StaticConfig`'s role shifts

`StaticConfig` today holds `listen_addr`, `tls_identity`, `iroh_relay`,
`drain_timeout`. After this ADR, the endpoint reads only `drain_timeout`
(passed directly to `new`); the assembly layer reads the rest to build
transports. `StaticConfig` becomes the canonical deployment config the
**assembly layer** reads, not "the thing the endpoint takes." It stays
in `alknet-core` (it's a config type) and stays extensible (future
transport config fields land here as the assembly layer needs them).

This is a conceptual shift from ADR-010's framing ("the CLI binary
constructs a `HandlerRegistry` and passes it, with `StaticConfig`, to
`AlknetEndpoint::new()`"). The endpoint no longer takes `StaticConfig`
at all. The endpoint takes `drain_timeout` and pre-built transport
endpoints; the assembly layer is the `StaticConfig` consumer.

## Decision

### The endpoint is a pure accept-loop runner + a public dispatch method

```rust
pub struct AlknetEndpoint {
    quinn: Option<quinn::Endpoint>,
    iroh: Option<iroh::Endpoint>,
    handlers: Arc<HandlerRegistry>,                       // ADR-010
    dynamic: Arc<ArcSwap<DynamicConfig>>,                 // ADR-010, unchanged
    identity_provider: Arc<dyn IdentityProvider>,         // ADR-010, unchanged
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
    drain_timeout: Duration,
}

impl AlknetEndpoint {
    pub fn new(
        handlers: HandlerRegistry,
        dynamic: Arc<ArcSwap<DynamicConfig>>,
        identity_provider: Arc<dyn IdentityProvider>,
        drain_timeout: Duration,
    ) -> Self;

    pub fn with_quinn(mut self, endpoint: quinn::Endpoint) -> Self;
    pub fn with_iroh(mut self, endpoint: iroh::Endpoint) -> Self;

    /// Clone of the shutdown watch sender. The assembly layer uses this
    /// to signal its own accept loops (TCP+TLS) to stop accepting — one
    /// signal, all loops stop. See OQ-61 for the full coordination model.
    pub fn shutdown_sender(&self) -> watch::Sender<bool>;

    /// Dispatch a pre-established connection by ALPN. Called by the
    /// endpoint's own accept loops (quinn, iroh) after transport-
    /// specific extraction, and by external accept loops (TCP+TLS,
    /// future SSH, future WebTransport) after their own extraction.
    ///
    /// Synchronous (non-async): performs the ACME guard, handler
    /// lookup, `build_auth_context`, and `tokio::spawn`s the handler.
    /// Returns immediately after spawning — the handler runs on its own
    /// task. The caller's accept loop is not blocked by handler
    /// execution.
    ///
    /// No-handler match (ALPN not in the registry): closes the
    /// connection and logs a warning, per ADR-010's existing dispatch
    /// behavior. Does not panic, does not return an error — the
    /// connection is dropped, the accept loop continues.
    pub fn dispatch(
        &self,
        connection: Connection,
        alpn: Vec<u8>,
        fingerprint: Option<String>,
        remote_addr: Option<SocketAddr>,
    );

    /// Run the endpoint's owned accept loops. Returns when shutdown is
    /// signaled. The caller drives shutdown via `shutdown_sender()`.
    pub async fn run(self: Arc<Self>);
}
```

The consumer pattern: the assembly layer holds `Arc<AlknetEndpoint>`,
clones the `Arc` for `run` (which consumes one clone), and passes
`&endpoint` to its TCP+TLS accept loops so they can call `dispatch`.
After `run` returns, the assembly layer drops its `Arc`; in-flight
dispatched handlers (spawned by `dispatch` via `tokio::spawn`) are
owned by the endpoint's runtime and drain per `shutdown` / OQ-61.

`AlknetEndpoint::new` takes **no `StaticConfig`** and **no TLS config**.
The assembly layer (the deployment binary that composes crates — see
ADR-014) reads `StaticConfig`, builds the transports, and hands the
quinn/iroh endpoints to `AlknetEndpoint` via builder methods. The
`handlers`, `dynamic`, and `identity_provider` fields are unchanged
passthroughs from ADR-010 — this ADR does not change auth or dynamic
config; it changes who builds transports and where dispatch lives.

`build_iroh_endpoint` moves out of `alknet-core/endpoint.rs`. Its
destination is an open question (see OQ-60) — the assembly layer, an
`alknet-tls` convenience helper, or a transport-construction
module/crate. This ADR commits to the **boundary** (construction is
not in the endpoint); the *where* is tracked separately so it doesn't
block the endpoint-shape decision. (`build_quinn_server_config_from_rustls`
is different: it's a thin wrapper that converts a `rustls::ServerConfig`
into a `quinn::ServerConfig`, which is exactly `TlsServerConfig::for_quinn()`
— its destination is `alknet-tls`, decided in ADR-082. Only
`build_iroh_endpoint`, which reads `StaticConfig` and builds an
`iroh::Endpoint` without a rustls config, is genuinely undecided.)

### `dispatch` is public and transport-agnostic

The public `dispatch` method takes the already-extracted ALPN,
fingerprint (if available), remote address (if available), and a
`Connection`. It performs the ACME guard, the handler lookup, the
`build_auth_context` call, and the `tokio::spawn`. `build_auth_context`
becomes a private helper called by `dispatch`, not a standalone
function — the assembly layer calls `dispatch`, which calls
`build_auth_context` internally.

Transport-specific extraction (`extract_quinn_alpn`,
`extract_quinn_client_fingerprint`, `extract_iroh_client_fingerprint`)
stays private in the endpoint — the accept loops call them, then call
`dispatch` with the results.

### The `acme-tls/1` guard moves to `dispatch`

```rust
pub fn dispatch(&self, connection: Connection, alpn: Vec<u8>, ...) {
    if alpn == b"acme-tls/1" {
        debug!("acme-tls/1 challenge connection completed at TLS layer; closing");
        connection.close(0, "acme done");
        return;
    }
    // ... handler lookup, build_auth_context, spawn
}
```

The guard is **not** feature-gated on `acme`: if the `acme` feature is
off, no transport advertises `acme-tls/1`, the ALPN never negotiates, the
guard never fires. It is harmless dead code without the feature.

This supersedes ADR-027 §5's "in `dispatch_quinn`" location. The
rationale (no handler for `acme-tls/1`, silent close) holds; only the
location changes. The `acme-tls/1` ALPN append still happens in
`TlsServerConfig::new`'s ACME branch (ADR-082), not per-transport — every
transport using the ACME `TlsServerConfig` advertises it, but in
practice only the TCP+TLS listener on 443 receives challenges.

### `StaticConfig` stays in core; the endpoint drops it

`StaticConfig` remains in `alknet-core/config.rs` as the canonical
deployment config the assembly layer reads. The endpoint no longer takes
`&StaticConfig` — `new()` takes `drain_timeout: Duration` directly.
Future transport config fields (addresses, relay URLs, identity) land in
`StaticConfig` as the assembly layer needs them; the endpoint's `new`
signature does not change when they're added, because the endpoint
doesn't read them.

### What moves out of `alknet-core/endpoint.rs`

| Code | Destination |
|------|-------------|
| `build_rustls_server_config()` | `alknet-tls` (ADR-082) |
| `TlsSetup` / ACME state machine | `alknet-tls` (ADR-082) |
| `RawKeyCertResolver` | `alknet-tls` (ADR-082) |
| `Ed25519SigningKey` | `alknet-tls` (ADR-082) |
| `AcceptAnyCertVerifier` | `alknet-tls` (ADR-082) |
| `SelfSignedCert` / `generate_self_signed_cert()` | `alknet-tls` (ADR-082) |
| `load_cert_chain()` / `load_private_key()` | `alknet-tls` (ADR-082) |
| `build_quinn_server_config_from_rustls()` | `alknet-tls` (`for_quinn()`, ADR-082) |
| `build_iroh_endpoint()` | Out of core (destination: OQ-60) |

### What stays in `alknet-core/endpoint.rs`

- `AlknetEndpoint` struct (accept-loop runner + public `dispatch`)
- `HandlerRegistry`
- `dispatch` (public — ACME guard, handler lookup, `build_auth_context`,
  spawn)
- `dispatch_quinn` / `dispatch_iroh` (private — transport-specific
  extraction, then call `dispatch`)
- `run_quinn_accept_loop` / `run_iroh_accept_loop`
- `extract_quinn_alpn` / `extract_quinn_client_fingerprint` /
  `extract_iroh_client_fingerprint`
- `build_auth_context` (private helper, called by `dispatch`)

### ADR-082 amendment

ADR-082's `AlknetEndpoint::new(..., Arc<TlsServerConfig>, ...)` signature
is superseded by this ADR. The endpoint takes no TLS config; the
assembly layer builds transports from `TlsServerConfig`s and hands them
to the endpoint via `with_quinn` / `with_iroh`. ADR-082 should reference
this ADR for the endpoint signature and focus on what `alknet-tls`
provides (`TlsServerConfig` and its accessors).

## Consequences

**Positive:**
- The endpoint has one job: dispatch. Transport construction lives
  outside it, where the multi-`TlsServerConfig` hub case is natural.
- TCP+TLS dispatch is first-class — same `dispatch` path as quinn/iroh,
  no duplicated `build_auth_context` or handler-lookup logic at the
  assembly layer. ADR-010 Amendment 1's second-class-dispatch workaround
  is retired.
- The `acme-tls/1` guard is transport-agnostic — it works for TCP+TLS
  (where challenges actually arrive) and any future transport. ADR-027
  §5's quinn-specific location is corrected.
- `StaticConfig`'s role is clear: it's the assembly-layer config, not
  the endpoint's. Adding transport config fields doesn't churn the
  endpoint's `new` signature.
- The hub (the first multi-transport consumer) is unblocked: build two
  `TlsServerConfig`s, build quinn/iroh/TCP+TLS listeners, hand the
  endpoint the quinn/iroh ones, spawn the TCP+TLS loops calling
  `endpoint.dispatch`.

**Negative:**
- `AlknetEndpoint::new` signature changes (breaking). Pre-1.0, in-repo
  consumers only — the assembly layer and tests must update. Expected;
  this is the point of the refactor.
- `build_iroh_endpoint` leaves core. Its destination is an open question
  (OQ-60), not decided here. The boundary (not in the endpoint) is
  committed; the *where* is not, so that the endpoint-shape decision
  isn't blocked on the transport-construction-location decision.
  (`build_quinn_server_config_from_rustls` is decided — it moves to
  `alknet-tls` as `for_quinn()` per ADR-082; only `build_iroh_endpoint`
  is open.)
- Multi-owner shutdown: the endpoint owns shutdown of *dispatched
  handlers* (it spawned them in `dispatch`); the assembly layer owns
  shutdown of the *accept loops it spawned* (TCP+TLS listeners). The
  coordination mechanism (shared `shutdown_sender`, drain semantics) is
  a follow-up design point, not resolved by this ADR — see OQ-61.

## Door type

**One-way.** The endpoint's `new` signature, the public `dispatch`
contract, and the "endpoint owns dispatch, not construction" boundary
are structural. Reversing would mean re-welding transport construction
to the endpoint and re-privatizing `dispatch` — breaking every
multi-transport consumer (hub, future HTTP, future SSH).

The `dispatch` signature (`connection, alpn, fingerprint, remote_addr`)
is one-way — changing it after consumers exist is a rewrite. The
internal implementation (how extraction is factored, how `run` spawns
tasks) is two-way.

## References

- ADR-010: ALPN router and endpoint (amended — the endpoint no longer
  constructs transports; Amendment 1's second-class-dispatch workaround
  is retired by the public `dispatch` method)
- ADR-014: Secret material flow and capability injection (defines the
  "assembly layer" term — the deployment binary that composes crates)
- ADR-010 Amendment 1: TCP+TLS as sibling (superseded — Amendment 1's
  *ownership* model (sibling listener outside the endpoint) survives;
  its *dispatch* workaround (duplicated logic) is retired by this ADR's
  public `dispatch`)
- ADR-027 §5: ACME ALPN challenge handling (location amended — guard
  moves from `dispatch_quinn` to shared `dispatch`)
- ADR-065: `Connection::from_stream`/`from_bidi` (the primitive that
  makes TCP+TLS dispatch possible; `Connection::close` is
  transport-abstracted, so the `acme-tls/1` guard works uniformly)
- ADR-082: `alknet-tls` extraction (amended — endpoint takes no
  `Arc<TlsServerConfig>`; the assembly layer builds transports from
  `TlsServerConfig`s)
- ADR-080: `ChannelClient::from_connection` (the transport-agnostic
  client pattern this ADR mirrors on the server side)
- `docs/research/alknet-endpoint-refactor/findings.md` — the analysis
  that surfaced the conflation and the two-config hub case
- `crates/alknet-core/src/endpoint.rs` — the code being refactored
- OQ-60: Where does transport construction live? (assembly layer,
  `alknet-tls` helper, or transport module/crate)
- OQ-61: Multi-owner shutdown coordination (endpoint owns dispatched
  handlers; assembly layer owns spawned accept loops)