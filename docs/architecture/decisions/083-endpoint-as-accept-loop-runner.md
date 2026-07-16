# ADR-083: Endpoint as Multi-Transport Accept-Loop Runner with Public Dispatch

## Status

Accepted (revised 2026-07-14: TCP+TLS moved inside the endpoint as a
third owned transport, not an external loop. This dissolves the
multi-owner shutdown problem — the endpoint owns all its accept loops.
`dispatch` stays public for genuinely external shapes: SSH channels,
WebTransport streams. See §"TCP+TLS is a first-class owned transport".
All TLS-scope OQs resolved; advanced to Accepted ahead of the
task-decomposition session.)

Accepted (amended 2026-07-15: the endpoint is extracted from
`alknet-core` into a new crate `alknet-endpoint`. The ADR's shape
(`new` + builder methods + public `dispatch` + `run` / `shutdown`) is
unchanged; the *location* changes. See §"Amendment 2026-07-15 — crate
extraction".)

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

### TCP+TLS is a first-class owned transport

TCP+TLS is the most common transport after raw-key QUIC, not an edge
case: HTTPS for browsers, worker registration over HTTP, raw-key
fallback for native clients when UDP is blocked. A hub — and any
hub-worker — needs it.

ADR-010 Amendment 1 made TCP+TLS a "sibling accept loop" outside the
endpoint because the endpoint's dispatch logic was crate-private and
couldn't be shared. The reason TCP+TLS was *structurally* excluded —
the endpoint built transports internally, and TCP+TLS couldn't fit that
shape — is gone after this ADR: the endpoint no longer builds
transports. It runs accept loops on whatever it's given.

TCP+TLS is a listener transport, same shape as quinn and iroh:

| Transport | What `with_*` takes | Accept loop |
|-----------|---------------------|-------------|
| quinn | `quinn::Endpoint` | `quinn.accept()` → handshake → `Connection::from_quinn_with_alpn` |
| iroh | `iroh::Endpoint` | `iroh.accept()` → alpn+handshake → `Connection::from_iroh` |
| TCP+TLS | `TcpListener` + `TlsAcceptor` | `tcp.accept()` → `tls.accept()` → `Connection::from_bidi` |

All three are: accept → extract ALPN + fingerprint → construct
`Connection` → dispatch. The endpoint already runs the first two; the
third is the same pattern with a different accept call. Making TCP+TLS
an owned transport (via `with_tcp_tls(listener, acceptor)`) instead of
an external loop gives the endpoint a single, uniform ownership model:
it owns all its accept loops, `shutdown()` stops them all. No
coordination between the endpoint and external loops.

This dissolves the multi-owner shutdown problem (the endpoint owns all
loops; one owner, one shutdown). The `dispatch` method stays public —
but for genuinely external shapes that the endpoint can't own: SSH
channels (one connection, many channels with different ALPNs — a
multiplexing shape, not a listener shape) and future WebTransport
streams (one QUIC connection, many WT streams). These are not listener
transports; they're connection-internal multiplexing. The endpoint
can't own them the way it owns a `TcpListener`.

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

The "assembly layer" is, in practice, the deployment binary — today,
that's primarily the hub. A pure worker (no inbound endpoints) has a
trivial assembly layer; a hub-worker combines both. Putting
hub-specific composition in the hub crate is appropriate; putting
transport loops that any node might need in the hub crate is not. The
TCP+TLS loop belongs in `alknet-endpoint` (behind a `tcp` feature),
where any node that wants it can enable the feature and call
`with_tcp_tls` — no hub dependency.

## Decision

### The endpoint is a multi-transport accept-loop runner + a public dispatch method

```rust
pub struct AlknetEndpoint {
    quinn: Option<quinn::Endpoint>,
    iroh: Option<iroh::Endpoint>,
    #[cfg(feature = "tcp")]
    tcp_tls: Option<TcpTlsListener>,       // (TcpListener, TlsAcceptor)
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

    /// Take ownership of a TCP+TLS listener. The endpoint runs the
    /// accept loop (`tcp.accept()` → `tls.accept()` → extract ALPN +
    /// fingerprint → `Connection::from_bidi` → `dispatch`). Feature-gated
    /// on `tcp` (pulls `tokio-rustls`). A hub serving HTTPS and a
    /// hub-worker serving TCP+TLS both use this; a pure worker doesn't.
    #[cfg(feature = "tcp")]
    pub fn with_tcp_tls(
        mut self,
        listener: tokio::net::TcpListener,
        acceptor: tokio_rustls::TlsAcceptor,
    ) -> Self;

    /// Clone of the shutdown watch sender. The assembly layer uses this
    /// to signal any external dispatch callers (SSH, future WT) to stop.
    /// The endpoint's own loops (quinn, iroh, TCP+TLS) are signaled
    /// internally by `shutdown()`.
    pub fn shutdown_sender(&self) -> watch::Sender<bool>;

    /// Dispatch a pre-established connection by ALPN. Called by the
    /// endpoint's own accept loops (quinn, iroh, TCP+TLS) after
    /// transport-specific extraction, and by external dispatch callers
    /// (SSH channels, future WebTransport streams) after their own
    /// extraction.
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

    /// Run the endpoint's owned accept loops (quinn, iroh, TCP+TLS —
    /// whichever were configured). Returns when shutdown is signaled.
    /// The caller drives shutdown via `shutdown()` or `shutdown_sender()`.
    pub async fn run(self: Arc<Self>);

    /// Signal all owned accept loops to stop, drain in-flight handlers
    /// for `drain_timeout`, then close. Infallible — the endpoint owns
    /// all its accept loops (quinn, iroh, TCP+TLS) and takes pre-built
    /// pre-bound transports, so there is no bind failure path and no
    /// external loop coordination. No-handler matches are swallowed by
    /// `dispatch` (close + log). One owner, one shutdown.
    pub async fn shutdown(&self);
}
```

The consumer pattern: the assembly layer (the deployment binary — today
primarily the hub) holds `Arc<AlknetEndpoint>`, clones the `Arc` for
`run` (which consumes one clone), and — for external dispatch callers
like SSH — passes `&endpoint` to them so they can call `dispatch`. After
`run` returns (on shutdown), in-flight dispatched handlers drain per
`shutdown()`.

`AlknetEndpoint::new` takes **no `StaticConfig`** and **no TLS config**.
The assembly layer (the deployment binary that composes crates — see
ADR-014) reads `StaticConfig`, builds the transports, and hands the
quinn/iroh/TCP+TLS endpoints to `AlknetEndpoint` via builder methods.
The `handlers`, `dynamic`, and `identity_provider` fields are unchanged
passthroughs from ADR-010 — this ADR does not change auth or dynamic
config; it changes who builds transports and where dispatch lives.

### TCP+TLS is owned, not external

The TCP+TLS accept loop runs inside `run()` alongside the quinn and
iroh loops. It is not an external sibling. This means:

- **Shutdown is single-owner.** `endpoint.shutdown()` stops all owned
  loops (quinn, iroh, TCP+TLS) and drains all dispatched handlers. No
  coordination between the endpoint and external accept loops. The
  multi-owner shutdown problem (formerly OQ-61) does not arise.
- **Any node can use TCP+TLS.** A hub, a hub-worker, or any node that
  accepts inbound TCP+TLS enables the `tcp` feature and calls
  `with_tcp_tls(listener, acceptor)`. No hub dependency. The loop isn't
  duplicated per binary.
- **The TCP+TLS loop's home is `alknet-endpoint`** (behind a `tcp`
  feature), not the hub crate or the assembly layer. The endpoint
  crate has `quinn` and `iroh` as feature-gated transport deps; adding
  `tcp` (pulling `tokio-rustls`) is the same pattern. A deployment that
  doesn't use TCP+TLS doesn't enable the feature.

### `dispatch` is public — for genuinely external shapes

The public `dispatch` method takes the already-extracted ALPN,
fingerprint (if available), remote address (if available), and a
`Connection`. It performs the ACME guard, the handler lookup, the
`build_auth_context` call, and the `tokio::spawn`. `build_auth_context`
becomes a private helper called by `dispatch`, not a standalone
function — external callers call `dispatch`, which calls
`build_auth_context` internally.

`dispatch` is for transports the endpoint **can't own** — shapes that
aren't listener-based:

- **SSH channels** (ADR-065): one SSH connection carries multiple
  channels with different ALPNs. The SSH handler accepts one connection,
  then dispatches each channel via `dispatch` with the channel's
  ALPN. This is connection-internal multiplexing, not a listener loop.
- **Future WebTransport streams** (parked per ADR-044): one QUIC
  connection, many WT streams. Same multiplexing shape.

TCP+TLS is **not** an external dispatch caller — it's a listener
transport the endpoint owns. The distinction: listener transports
(quinn, iroh, TCP+TLS) produce connections from an accept loop the
endpoint runs; multiplexing transports (SSH, WT) produce connections
from within an existing connection, and the endpoint can't own their
accept loop.

Transport-specific extraction (`extract_quinn_alpn`,
`extract_quinn_client_fingerprint`, `extract_iroh_client_fingerprint`,
`extract_tcp_tls_alpn`, `extract_tcp_tls_client_fingerprint`) stays
private in the endpoint — the accept loops call them, then call
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

### Feature gates

```toml
[features]
quinn = ["dep:quinn"]           # with_quinn — quinn accept loop
iroh = ["dep:iroh"]             # with_iroh — iroh accept loop
tcp = ["dep:tokio-rustls"]      # with_tcp_tls — TCP+TLS accept loop
acme = ["dep:rustls-acme"]      # (used by alknet-tls, not core directly)
```

The `tcp` feature pulls `tokio-rustls`. A deployment enables the
features for the transports it runs. A pure-QUIC node enables `quinn` +
`iroh`; a hub serving HTTPS enables `quinn` + `tcp`; a hub-worker
enables all three. This matches the existing pattern (`quinn` and
`iroh` are already feature-gated transport deps on `alknet-endpoint`).

### `StaticConfig` stays in core; the endpoint drops it

`StaticConfig` remains in `alknet-core/config.rs` as the canonical
deployment config the assembly layer reads. The endpoint no longer takes
`&StaticConfig` — `new()` takes `drain_timeout: Duration` directly.
Future transport config fields (addresses, relay URLs, identity) land in
`StaticConfig` as the assembly layer needs them; the endpoint's `new`
signature does not change when they're added, because the endpoint
doesn't read them.

### Transport construction: inlined by the assembly layer

The builder functions are trivial API calls. The assembly layer (the
deployment binary — today primarily the hub crate's composition code)
inlines them:

- `build_quinn_endpoint`: `tls_config.for_quinn()` → `quinn::Endpoint::server(addr)` — 2 lines
- `build_iroh_endpoint`: iroh builder + key + relay + alpns + bind — 15 lines
- `build_tcp_tls`: `TcpListener::bind(addr)` + `tls_config.for_tcp_tls()` — 2 lines

These are pure configuration, not shared logic. No helper crate or
module — 20 lines total across all three, each binary picks which
transports it wants. If a future binary duplicates the iroh builder and
the pattern drifts, extraction is a two-way door on the function; the
*loop* (the runtime with shutdown + dispatch) is in core, so the
duplication risk is only on the trivial builder, not the real component.

Hub-specific composition (wiring `ChannelsAdapter`, `HttpAdapter`,
`CallAdapter`, the relay, peer lifecycle, worker registration) lives in
the hub crate. That's where multi-transport *composition* happens. The
endpoint provides the accept loops; the hub provides the handlers and
wiring. A `alknet-transport` crate was considered and rejected — it
would contain only trivial builder functions (the real component, the
TCP+TLS loop, is in core). A crate for 20 lines of API calls doesn't
earn its existence. If hub-specific transport helpers accumulate, they
live in the hub crate, not a generic transport crate.

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
| `build_iroh_endpoint()` | Assembly layer (inlined; 15 lines of iroh API calls) |
| `AlknetEndpoint`, `HandlerRegistry`, all dispatch/loop/extraction code | `alknet-endpoint` (this ADR, §"Amendment 2026-07-15") |
| `EndpointError` | **removed** — `BindFailed` and `HandlerNotFound` are both vestigial (endpoint takes pre-bound transports; `dispatch` swallows no-handler matches); `shutdown()` is infallible |

### What stays in `alknet-endpoint` (the new crate)

- `AlknetEndpoint` struct (multi-transport accept-loop runner + public `dispatch`)
- `HandlerRegistry`
- `TcpTlsListener` (the `(TcpListener, TlsAcceptor)` tuple for the TCP+TLS field)
- `dispatch` (public — ACME guard, handler lookup, `build_auth_context`, spawn)
- `dispatch_quinn` / `dispatch_iroh` / `dispatch_tcp_tls` (private — transport-specific extraction, then call `dispatch`)
- `run_quinn_accept_loop` / `run_iroh_accept_loop` / `run_tcp_tls_accept_loop`
- `extract_quinn_alpn` / `extract_quinn_client_fingerprint` /
  `extract_iroh_client_fingerprint` / `extract_tcp_tls_alpn` /
  `extract_tcp_tls_client_fingerprint`
- `build_auth_context` (private helper, called by `dispatch`)

### ADR-082 amendment

ADR-082's `AlknetEndpoint::new(..., Arc<TlsServerConfig>, ...)` signature
is superseded by this ADR. The endpoint takes no TLS config; the
assembly layer builds transports from `TlsServerConfig`s and hands them
to the endpoint via `with_quinn` / `with_iroh` / `with_tcp_tls`. ADR-082
should reference this ADR for the endpoint signature and focus on what
`alknet-tls` provides (`TlsServerConfig` and its accessors).

### Amendment 2026-07-15 — crate extraction (`alknet-endpoint`)

The endpoint is extracted from `alknet-core` into a new crate
`alknet-endpoint`. The ADR's shape — `new(handlers, dynamic,
identity_provider, drain_timeout)` + `with_quinn` / `with_iroh` /
`with_tcp_tls` + public `dispatch` + `run` / `shutdown` — is **unchanged**
by this amendment. Only the *location* changes: `endpoint.rs`,
`HandlerRegistry` move from `alknet-core` to `alknet-endpoint`
(`EndpointError` is removed — see above).

#### Why extract (the dependency data)

`alknet-core` is currently two things welded together with different
audiences:

| Concern | Modules | Depended on by |
|---------|---------|----------------|
| **Shared types + auth + config** | `types.rs`, `auth.rs`, `config.rs`, `fingerprint.rs`, `ownership.rs`, `store.rs` | every handler crate (call, http, tty, tty-local) — 124 import sites |
| **Endpoint (accept-loop runner)** | `endpoint.rs` (1606 LOC) | **zero handler crates** — only the assembly layer (hub/worker) |

The handler crates import `alknet_core::types` (57×), `alknet_core::auth`
(41×), `alknet_core::config` (13×), `alknet_core::ownership` (7×),
`alknet_core::fingerprint` (6×). They import `alknet_core::endpoint`
**zero times**. `endpoint.rs` is a one-way leaf consumer of the shared
types (it imports `auth`, `config`, `types`; nothing in core imports
*from* it). This is the textbook shape for an extraction: a leaf
consumer depended on by a different audience than the shared types.

#### The dependency-weight win

`alknet-core`'s `Cargo.toml` currently carries `quinn`, `iroh`,
`rustls-pemfile`, `rcgen`, `rustls-acme`, `rustls` — heavy transport/TLS
deps — because `endpoint.rs` uses them. Every handler crate transitively
pulls these via `alknet-core`, even though none of them use the
endpoint. `alknet-tty` is a pure wire-format handler — it has no business
linking quinn.

After extraction:

- **`alknet-core`** loses `quinn`, `iroh`, `rcgen`, `rustls-pemfile`,
  `rustls-acme` from its `[dependencies]` (the TLS-setup deps already
  move to `alknet-tls` per ADR-082; the transport deps move with the
  endpoint). It keeps `rustls`/`rustls-pki-types` (narrow — for
  `fingerprint.rs` types, per OQ-59), `ed25519-dalek`, `sha2`, `tokio`,
  `serde`, `arc-swap`. It becomes a **lightweight types+auth+config
  crate** — what the handler crates actually want.
- **`alknet-endpoint`** gains `quinn`, `iroh` (the accept-loop
  transports). It depends on `alknet-core` for `Connection`,
  `ProtocolHandler`, `AuthContext`, `IdentityProvider`, `DynamicConfig`.
  It does **not** depend on `alknet-tls` — the endpoint takes pre-built
  transports (per this ADR), so TLS config construction stays in
  `alknet-tls` at the assembly layer.

A handler crate (call, http, tty) no longer transitively links quinn,
iroh, rcgen, or rustls-acme. A pure worker (client-only, no inbound)
depends on `alknet-client` + handler crates + `alknet-core` and does
**not** pull `alknet-endpoint` at all.

#### The `quinn` feature split

`alknet-core`'s `quinn` feature currently gates two unrelated things:
(1) `endpoint.rs`'s quinn accept loop and (2) `types.rs`'s
`Connection::from_quinn` / `QuinnBidiStreamSource` constructor. After
extraction:

- **`Connection::from_quinn` stays in `alknet-core`** (in `types.rs`).
  It's a shared-type constructor used by both the endpoint's accept
  loop (server) and `alknet-client`'s `dial_quic` (client, ADR-089) and
  `CallClient` tests. The `quinn` feature on `alknet-core` gates this
  constructor — "does `Connection` support quinn-constructed
  connections" — not "does core run a quinn accept loop." Same for
  `from_iroh` and the `iroh` feature.
- **The quinn accept loop moves to `alknet-endpoint`**, which has its
  own `quinn` feature for the accept loop. The heavy quinn accept-loop
  code is in `alknet-endpoint`, not core.

#### Symmetry with `alknet-client` (ADR-089)

The extraction mirrors the client-side extraction (ADR-089):

| Server side | Client side | Audience |
|-------------|-------------|----------|
| `alknet-endpoint` (this ADR) | `alknet-client` (ADR-089) | the assembly layer (hub, worker, hub-worker) |
| `alknet-core` (shared types) | `alknet-core` (shared types) | every handler crate |

A **hub** depends on `alknet-endpoint` + `alknet-client` + handler
crates + `alknet-core` (transitively). A **pure worker** depends on
`alknet-client` + handler crates + `alknet-core` — no `alknet-endpoint`.
A **handler crate** depends on `alknet-core` only — neither endpoint
nor client.

#### Implementation: build new, delete old

The endpoint's `endpoint.rs` is 1606 lines of `#[cfg(feature = "quinn")]`
/ `#[cfg(feature = "iroh")]` / `#[cfg(feature = "acme")]` conditionals —
the TLS-setup code that ADR-082 moves to `alknet-tls`, the
`StaticConfig`-taking constructor that this ADR replaces, the
`acme-tls/1` guard that this ADR moves to shared `dispatch`. Building
`alknet-endpoint` fresh against this ADR's shape — `new()` + builder
methods + public `dispatch` + `run` / `shutdown`, importing
`Connection`/`ProtocolHandler`/`AuthContext` from `alknet-core`, no TLS
setup (that's `alknet-tls`'s job now) — is cleaner than editing the
1606-line conditional file in place. The old `endpoint.rs` becomes a
deletion, not a refactor. This is pruning (cutting the endpoint + the
TLS deps that already move to `alknet-tls`), not a massive inline
refactor.

#### What `alknet-core` looks like after

`alknet-core` after the extraction + the ADR-082 TLS pruning:

| Module | LOC | Stays? |
|--------|-----|--------|
| `types.rs` (`Connection`, `ProtocolHandler`, `BiStream`, `BidiStreamSource`, `StreamError`, `Capabilities`) | ~1100 | yes — shared by every crate |
| `auth.rs` (`AuthContext`, `Identity`, `IdentityProvider`, `IdentityStore`, `AuthToken`) | ~640 | yes — shared by every crate |
| `config.rs` (`StaticConfig`, `DynamicConfig`, `TlsIdentity`, `Ed25519SecretKey`) | ~710 | yes — shared config types |
| `fingerprint.rs` | ~260 | yes — shared by server + client (OQ-59) |
| `ownership.rs` (`OwnershipStore`, `OwnershipProvider`) | ~280 | yes — shared |
| `store.rs` (`CredentialStore`, `EncryptedData`) | ~200 | yes — shared |
| `endpoint.rs` (`AlknetEndpoint`, `HandlerRegistry`) | ~1600 | **no** — moves to `alknet-endpoint` (`EndpointError` is removed, not moved) |

Core loses ~1600 LOC (the endpoint) and 5 heavy deps (`quinn`, `iroh`,
`rcgen`, `rustls-pemfile`, `rustls-acme`). The remaining ~3200 LOC is
the lightweight types+auth+config+ownership+store+fingerprint surface
that every handler crate depends on.

## Consequences

**Positive:**
- The endpoint has one job: run accept loops + dispatch. Transport
  construction lives outside it, where the multi-`TlsServerConfig` hub
  case is natural.
- TCP+TLS is a first-class owned transport — same `run()` loop, same
  `shutdown()`, same dispatch path as quinn/iroh. No duplicated
  `build_auth_context` or handler-lookup logic anywhere. ADR-010
  Amendment 1's sibling-dispatch workaround is retired entirely.
- Shutdown is single-owner. The endpoint owns all its loops; one
  `shutdown()` stops them all and drains. The multi-owner shutdown
  problem (formerly OQ-61) does not arise.
- Any node can use TCP+TLS — enable the `tcp` feature, call
  `with_tcp_tls`. No hub dependency. A hub-worker serving TCP+TLS is
  the same code path as a hub serving TCP+TLS.
- The `acme-tls/1` guard is transport-agnostic — it works for TCP+TLS
  (where challenges actually arrive) and any future transport. ADR-027
  §5's quinn-specific location is corrected.
- `StaticConfig`'s role is clear: it's the assembly-layer config, not
  the endpoint's. Adding transport config fields doesn't churn the
  endpoint's `new` signature.
- The hub (the first multi-transport consumer) is unblocked: build two
  `TlsServerConfig`s, build quinn/iroh/TCP+TLS listeners, hand all
  three to the endpoint, `run()`.
- `dispatch` stays public for genuinely external shapes (SSH channels,
  future WT streams) — transports the endpoint can't own because they're
  connection-internal multiplexing, not listener-based.
- **The endpoint is extracted into `alknet-endpoint`** (Amendment
  2026-07-15). Handler crates no longer transitively link quinn, iroh,
  rcgen, or rustls-acme. A pure worker (client-only) does not pull the
  endpoint at all. The dep graph is symmetric with `alknet-client`
  (ADR-089): `alknet-core` is the shared types crate; `alknet-endpoint`
  and `alknet-client` are the server-side and client-side establishment
  crates that depend on it.

**Negative:**
- `AlknetEndpoint::new` signature changes (breaking). Pre-1.0, in-repo
  consumers only — the assembly layer and tests must update. Expected;
  this is the point of the refactor.
- `alknet-endpoint` gains a `tcp` feature (pulls `tokio-rustls`). This is
  the same pattern as the `quinn` and `iroh` features — a transport that
  the endpoint can own. A deployment that doesn't use TCP+TLS doesn't
  enable it. `alknet-core` keeps a narrow `rustls` dep (for
  `fingerprint.rs` types, per OQ-59); `tokio-rustls` is the acceptor
  wrapper over `rustls::ServerConfig`, not a separate TLS stack, and
  lives in `alknet-endpoint`, not core.
- `build_iroh_endpoint` leaves core (inlined by the assembly layer).
  This is a one-way dep-graph change: the binary that uses iroh depends
  on `iroh` directly, not via `alknet-core` or `alknet-endpoint`. This
  is correct — the binary *is* the thing that knows which transports it
  wants. The 15 lines are pure iroh API calls; no shared logic is lost.
- This ADR revises ADR-010's "TCP is not an endpoint struct concern"
  more deeply than the original ADR-083 draft. The reason TCP was
  excluded (the endpoint built transports internally, TCP+TLS couldn't
  fit) is gone; the endpoint is now a multi-transport accept-loop runner
  and TCP+TLS is a listener transport that fits the same shape.

## Door type

**One-way.** The endpoint's `new` signature, the public `dispatch`
contract, the "endpoint owns dispatch, not construction" boundary, the
"endpoint owns TCP+TLS as a first-class transport" decision, and the
extraction of the endpoint into `alknet-endpoint` (a separate crate
from `alknet-core`) are structural. Reversing would mean re-welding
transport construction to the endpoint, re-privatizing `dispatch`,
pushing TCP+TLS back outside, and re-merging the endpoint into core —
breaking every multi-transport consumer (hub, hub-worker, future SSH,
future WT) and re-coupling every handler crate to quinn/iroh/rcgen.

The `dispatch` signature (`connection, alpn, fingerprint, remote_addr`)
is one-way — changing it after consumers exist is a rewrite. The
internal implementation (how extraction is factored, how `run` spawns
tasks, the TCP+TLS loop's internal structure) is two-way.

## References

- ADR-010: ALPN router and endpoint (amended — the endpoint no longer
  constructs transports; "TCP is not an endpoint struct concern" is
  revised: TCP+TLS is now a first-class owned transport via
  `with_tcp_tls`; Amendment 1's sibling-dispatch workaround is retired
  by the public `dispatch` method and the owned TCP+TLS loop)
- ADR-014: Secret material flow and capability injection (defines the
  "assembly layer" term — the deployment binary that composes crates)
- ADR-010 Amendment 1: TCP+TLS as sibling (superseded — TCP+TLS is now
  an owned transport, not a sibling; `dispatch` is public for SSH/WT,
  not for TCP+TLS)
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
- `docs/architecture/crates/endpoint/README.md` — the canonical spec for
  `alknet-endpoint` (this ADR's amendment is the decision; that spec is
  the description)
- `crates/alknet-core/src/endpoint.rs` — the code being extracted into
  `alknet-endpoint` (the old module becomes a deletion after the
  extraction)
- OQ-60: resolved — transport construction is inlined by the assembly
  layer (the deployment binary); the TCP+TLS loop lives in
  `alknet-endpoint` behind a `tcp` feature as an owned transport
- OQ-61: dissolved — the multi-owner shutdown problem does not arise;
  the endpoint owns all its accept loops (quinn, iroh, TCP+TLS)