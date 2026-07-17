---
status: draft
last_updated: 2026-07-17
---

# alknet-endpoint

The server-side establishment crate — the multi-transport accept-loop
runner that dispatches incoming connections by ALPN. The server-side
analogue of `alknet-client` (ADR-089): `alknet-endpoint` accepts and
dispatches; `alknet-client` dials and produces a `Connection`. Both
depend on `alknet-core` for shared types; neither depends on the other.

## What

`AlknetEndpoint` is the central runtime type for any node that accepts
inbound connections. It takes pre-built transport endpoints (quinn,
iroh, TCP+TLS) via builder methods, runs their accept loops inside a
single `run()` method, and dispatches each accepted connection to the
registered `ProtocolHandler` by the ALPN the TLS handshake negotiated.
It does not build transports and does not build TLS configs — the
assembly layer does both (transports from `alknet-tls`'s
`TlsServerConfig`, per ADR-082).

`alknet-endpoint` is a leaf consumer of `alknet-core`'s shared types
(it imports `auth`, `config`, `types`; nothing in core imports from
it), depended on by the assembly layer — a different audience than the
shared types (every handler crate). No handler crate imports
`AlknetEndpoint` or `HandlerRegistry` — they depend on `alknet-core`
for `ProtocolHandler`, `Connection`, `AuthContext`, and types only.
This keeps the heavy transport deps (quinn, iroh, tokio-rustls) out of
the handler crates' dep closure. (`EndpointError` is removed — see
below.)

## Why

Separating the endpoint from the shared-types crate lets `alknet-core`
be the lightweight types+auth+config crate that every handler crate
wants, while the accept-loop runner (which only the assembly layer
depends on) carries the heavy transport deps. See
[ADR-083](../../decisions/083-endpoint-as-accept-loop-runner.md)
§"Amendment 2026-07-15 — crate extraction" for the full rationale,
including the dependency data and the symmetry with `alknet-client`.

## Architecture

### `AlknetEndpoint`

```rust
pub struct AlknetEndpoint {
    quinn: Option<quinn::Endpoint>,
    iroh: Option<iroh::Endpoint>,
    #[cfg(feature = "tcp")]
    tcp_tls: Option<TcpTlsListener>,       // (TcpListener, TlsAcceptor)
    handlers: Arc<HandlerRegistry>,
    dynamic: Arc<ArcSwap<DynamicConfig>>,
    identity_provider: Arc<dyn IdentityProvider>,
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

    #[cfg(feature = "tcp")]
    pub fn with_tcp_tls(
        mut self,
        listener: tokio::net::TcpListener,
        acceptor: tokio_rustls::TlsAcceptor,
    ) -> Self;

    pub fn shutdown_sender(&self) -> watch::Sender<bool>;

    pub fn dispatch(
        &self,
        connection: Connection,
        alpn: Vec<u8>,
        fingerprint: Option<String>,
        remote_addr: Option<SocketAddr>,
    );

    pub async fn run(self: Arc<Self>);

    /// Signal all owned accept loops to stop and drain in-flight handlers
    /// for `drain_timeout`. Infallible — the accept loops are owned by
    /// the endpoint (quinn, iroh, TCP+TLS), so there is no external
    /// coordination and no bind failure path (the assembly layer binds
    /// before handing pre-built transports to the endpoint via
    /// `with_quinn` / `with_iroh` / `with_tcp_tls`). No-handler matches
    /// are swallowed by `dispatch` (ADR-083: close + log, not an error).
    /// One owner, one shutdown — no external loop coordination needed.
    pub async fn shutdown(&self);
}
```

`new` takes **no `StaticConfig`** and **no TLS config** — the assembly
layer reads `StaticConfig` (in `alknet-core`), builds the transports
(via `alknet-tls`'s `TlsServerConfig` + the transport's own builder),
and hands them to the endpoint via `with_quinn` / `with_iroh` /
`with_tcp_tls`. The endpoint's job is to run accept loops and dispatch;
transport construction is not its concern. See
[ADR-083](../../decisions/083-endpoint-as-accept-loop-runner.md) for
the full design.

### `HandlerRegistry`

Maps ALPN byte strings to `ProtocolHandler` instances. Registered
statically at startup by the assembly layer; the endpoint dispatches by
looking up the negotiated ALPN.

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
- `alpn_strings()`: Return all registered ALPN strings. Used by the
  assembly layer to build the TLS `ServerConfig`'s ALPN list (via
  `alknet-tls`, filtered by endpoint type per ADR-086).

Registration is static at startup (ADR-010, OQ-04). The assembly layer
builds a `HandlerRegistry`, inserts all handlers, and passes it to
`AlknetEndpoint::new()`.

### `EndpointError` — removed

The endpoint previously had an `EndpointError { BindFailed(io::Error),
HandlerNotFound(Vec<u8>) }` enum. Both variants are vestigial after
ADR-083:

- `BindFailed` — the endpoint takes pre-built, pre-bound transports
  (the assembly layer does the binding); the endpoint performs no bind,
  so it cannot produce a bind error.
- `HandlerNotFound` — `dispatch` swallows no-handler matches (close +
  log per ADR-083), so this variant is never returned.

The enum is removed. `shutdown()` is infallible (`async fn shutdown(&self)`,
no `Result`). If a future requirement adds a real failure path to
shutdown or dispatch, a fresh error type is cleaner than retrofitting
this one. The `EndpointError` type, its `TlsConfig` variant (already
removed by ADR-083), and the `BindFailed`/`HandlerNotFound` variants all
move out of the codebase with the endpoint extraction — none survives
into `alknet-endpoint`.

### `TcpTlsListener`

The type held by the endpoint's `tcp_tls` field — a tuple of the TCP
listener and the TLS acceptor:

```rust
type TcpTlsListener = (tokio::net::TcpListener, tokio_rustls::TlsAcceptor);
```

The endpoint owns both halves: `tcp.accept()` produces a `TcpStream`,
`tls.accept()` wraps it, then ALPN + fingerprint extraction →
`Connection::from_bidi` → `dispatch`. Feature-gated on `tcp`.

### Accept loops

Each active transport runs its own accept loop inside `run()`:

- **Quinn** — `quinn.accept()` → TLS handshake → extract ALPN +
  fingerprint → `Connection::from_quinn_with_alpn` → `dispatch`.
- **Iroh** — `iroh.accept()` → `accepting.alpn().await` → extract
  fingerprint → `Connection::from_iroh` → `dispatch`.
- **TCP+TLS** (behind `tcp` feature) — `tcp.accept()` →
  `tls.accept()` → extract ALPN + fingerprint →
  `Connection::from_bidi` → `dispatch`.

All three feed the same `dispatch` method. The transport-specific
extraction (ALPN, fingerprint, remote address) is private to the
endpoint; `dispatch` receives the extracted values. See
[ADR-083](../../decisions/083-endpoint-as-accept-loop-runner.md)
§"Accept Loops" for the loop pseudocode.

### `dispatch` (public)

The shared dispatch path for every transport — the endpoint's own
accept loops call it after transport-specific extraction, and external
dispatch callers (SSH channels, future WebTransport streams) call it
after their own extraction. Synchronous (non-async): performs the
ACME guard, handler lookup, `build_auth_context`, and `tokio::spawn`s
the handler. Returns immediately after spawning.

`dispatch` is public for **connection-internal multiplexing shapes**
that the endpoint can't own (SSH channels: one connection, many
channels with different ALPNs; future WT streams: one QUIC connection,
many WT streams). Listener transports (quinn, iroh, TCP+TLS) are owned
by the endpoint and call `dispatch` internally; they are not external
dispatch callers. See
[ADR-083](../../decisions/083-endpoint-as-accept-loop-runner.md)
§"`dispatch` is public — for genuinely external shapes".

### Shutdown

`shutdown()` signals all owned accept loops (quinn, iroh, TCP+TLS) to
stop, waits for in-flight dispatched handlers with a drain timeout,
then forcefully closes remaining connections. One owner, one shutdown
— no external loop coordination. SIGTERM/SIGINT are wired to the
shutdown channel by the assembly layer (the deployment binary).

### What `alknet-endpoint` does NOT do

- **No transport construction.** The endpoint takes pre-built
  transports via builder methods. The assembly layer builds them
  (`TlsServerConfig::for_quinn()` → `quinn::Endpoint::server()`, etc.).
- **No TLS config.** The endpoint does not depend on `alknet-tls`. TLS
  configs are built by the assembly layer; the endpoint receives the
  resulting transport endpoints.
- **No protocol logic.** The endpoint dispatches by ALPN; the handler
  runs the protocol. The endpoint does not know what `alknet/call` or
  `alknet/channels` means — it looks up the ALPN in the registry and
  spawns the handler.

### Feature gates

```toml
[features]
default = []
quinn = ["dep:quinn", "alknet-core/quinn"]      # with_quinn — quinn accept loop
iroh = ["dep:iroh", "alknet-core/iroh"]          # with_iroh — iroh accept loop
tcp = ["dep:tokio-rustls"]                       # with_tcp_tls — TCP+TLS accept loop
```

The `quinn`/`iroh` features pull the corresponding features on
`alknet-core` (for `Connection::from_quinn` / `from_iroh` — the
constructors stay in core; see ADR-083 §"The `quinn` feature split"). A
deployment enables the features for the transports it runs. A
pure-QUIC node enables `quinn` + `iroh`; a hub serving HTTPS enables
`quinn` + `tcp`; a hub-worker enables all three.

### Dependencies

```
alknet-endpoint
├── alknet-core       (Connection, ProtocolHandler, AuthContext,
│                     IdentityProvider, DynamicConfig)
├── quinn             (optional — quinn accept loop)
├── iroh              (optional — iroh accept loop)
├── tokio-rustls      (optional — TCP+TLS accept loop, tcp feature)
├── tokio             (spawn, watch, TcpListener)
├── arc-swap          (DynamicConfig)
└── tracing           (logging)
```

`alknet-endpoint` depends on `alknet-core` (for `Connection`,
`ProtocolHandler`, `AuthContext`, `IdentityProvider`, `DynamicConfig`).
`HandlerRegistry` lives in `alknet-endpoint` (it moves with the
endpoint from core). `EndpointError` is removed (both variants were
vestigial — see "`EndpointError` — removed" above). The endpoint does
**not** depend on `alknet-tls` — it takes pre-built transports, so TLS
config
construction stays at the assembly layer.

### Crate dependencies (in the dep graph)

```
alknet-endpoint
└── alknet-core (Connection, ProtocolHandler, AuthContext, IdentityProvider, DynamicConfig)

alknet-hub (uses AlknetEndpoint for inbound)
├── alknet-endpoint (the accept-loop runner)
├── alknet-client (the dial — for outbound worker dials, ADR-089)
├── alknet-channels-call (ChannelsAdapter — registered on the HandlerRegistry)
├── alknet-call (CallAdapter, Dispatcher)
├── alknet-http (HttpAdapter)
└── alknet-core (shared types)

alknet-worker (uses AlknetEndpoint if it accepts inbound)
├── alknet-endpoint (if the worker accepts inbound — a hub-worker)
├── alknet-client (the dial — to reach the hub, ADR-089)
└── alknet-core (shared types)
```

`alknet-call`, `alknet-http`, `alknet-tty`, and other handler crates do
**not** depend on `alknet-endpoint`. They depend on `alknet-core` for
`ProtocolHandler` and `Connection`; the endpoint dispatches to them via
the trait, without a dependency edge. This is the dep-weight win: a
handler crate no longer transitively links quinn, iroh, rcgen, or
rustls-acme.

## Assembly layer integration

A downstream hub uses `alknet-endpoint` like this:

```rust
// 1. Build the HandlerRegistry — register all handlers by ALPN.
let mut registry = HandlerRegistry::new();
registry.register(Arc::new(channels_adapter));   // alknet/channels
registry.register(Arc::new(http_adapter));        // h2, http/1.1

// 2. Build the TlsServerConfig(s) via alknet-tls (assembly layer).
let raw_key_tls = TlsServerConfig::new(&raw_key_identity, &native_alpns).await?;
let x509_tls = TlsServerConfig::new(&x509_identity, &web_alpns).await?;

// 3. Build the transport endpoints from the TLS configs.
let quinn_endpoint = raw_key_tls.for_quinn()?.into_endpoint(listen_addr)?;
let tcp_listener = TcpListener::bind(web_addr).await?;
let tls_acceptor = x509_tls.for_tcp_tls();

// 4. Construct the endpoint with all owned transports.
let endpoint = Arc::new(
    AlknetEndpoint::new(registry, dynamic, identity_provider, drain_timeout)
        .with_quinn(quinn_endpoint)
        .with_tcp_tls(tcp_listener, tls_acceptor),
);

// 5. Run — all accept loops run inside run(), shutdown() stops them all.
endpoint.clone().run().await;
```

The endpoint takes the pre-built transports; the assembly layer built
them from `alknet-tls`'s `TlsServerConfig`s. The endpoint does not see
`alknet-tls` — it sees `quinn::Endpoint` and `TlsAcceptor`.

## What `alknet-core` looks like

Core is the lightweight types+auth+config+ownership+store+fingerprint
crate (~3200 LOC, no `quinn`/`iroh`/`rcgen`/`rustls-pemfile`/
`rustls-acme` deps). The endpoint module is not in core; the accept
loops are here. See [ADR-083](../../decisions/083-endpoint-as-accept-
loop-runner.md) §"Amendment 2026-07-15 — crate extraction" §"What
`alknet-core` looks like after" for the module-level table and the
`quinn` feature split (`Connection::from_quinn` stays in core; the
accept loop is here).

## Design Decisions

All design decisions are documented as ADRs in
[decisions/](../../decisions/).

| ADR | Decision | Summary |
|-----|----------|---------|
| [083](../../decisions/083-endpoint-as-accept-loop-runner.md) | Endpoint as multi-transport accept-loop runner + crate extraction | `AlknetEndpoint` takes no TLS config; `with_quinn`/`with_iroh`/`with_tcp_tls` builder methods; public `dispatch` for SSH/WT; extracted from `alknet-core` into `alknet-endpoint` (Amendment 2026-07-15) |
| [086](../../decisions/086-endpoint-types-and-entry-points.md) | Endpoint types and entry points | Three endpoint types (web/native/iroh); entry-point vs. endpoint ALPN distinction; split ALPN lists per endpoint type |
| [010](../../decisions/010-alpn-router-and-endpoint.md) | ALPN Router and Endpoint | `HandlerRegistry`, accept loop, static registration (amended by ADR-083) |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-60** (resolved): Where does transport construction live? The
  TCP+TLS accept loop lives in `alknet-endpoint` behind a `tcp` feature
  as an owned transport. Builder functions are inlined by the assembly
  layer. See [ADR-083](../../decisions/083-endpoint-as-accept-loop-runner.md).
- **OQ-61** (dissolved): Multi-owner shutdown coordination. The problem
  does not arise — the endpoint owns all its accept loops (quinn, iroh,
  TCP+TLS); `shutdown()` stops them all. See ADR-083.

## References

- [ADR-083](../../decisions/083-endpoint-as-accept-loop-runner.md) —
  the decision this spec implements (including the Amendment
  2026-07-15 crate extraction)
- [ADR-082](../../decisions/082-alknet-tls-extraction.md) —
  `TlsServerConfig` (the TLS config the assembly layer builds; the
  endpoint does not see it)
- [ADR-086](../../decisions/086-endpoint-types-and-entry-points.md) —
  endpoint types (web/native/iroh); entry-point vs. endpoint ALPN
- [ADR-089](../../decisions/089-alknetclient-native-dial-seam.md) —
  `alknet-client` (the client-side analogue — symmetric extraction)
- [ADR-065](../../decisions/065-connection-from-stream-generic-single-stream.md)
  — `Connection::from_stream` / `from_bidi` (the TCP+TLS path)
- [ADR-070](../../decisions/070-bidistreamsource-trait.md) —
  `BidiStreamSource` (the `Connection` extension point)
- [`crates/core/endpoint.md`](../core/endpoint.md) — the endpoint
  design (will be updated to reflect the extraction; the endpoint
  semantics stay, the location moves)
- [`crates/core/core-types.md`](../core/core-types.md) —
  `ProtocolHandler`, `Connection`, `AuthContext` (the shared types the
  endpoint imports from core)
- [`crates/client/README.md`](../client/README.md) — `alknet-client`
  (the client-side complement)
- [`crates/tls/README.md`](../tls/README.md) — `TlsServerConfig` (the
  assembly-layer TLS config that produces the transports the endpoint
  takes)
- [`crates/hub/README.md`](../hub/README.md) — the hub (the first
  multi-transport consumer of the endpoint)