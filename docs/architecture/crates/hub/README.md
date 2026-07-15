---
status: draft
last_updated: 2026-07-15
---

# alknet-hub

The hub pattern as a reusable crate: a multi-transport endpoint that
composes a subset of three endpoint types (web, native, iroh —
ADR-086), accepts workers and browsers over whichever transports the
subset implies, relays channels between legs (ADR-079), manages peer
lifecycle, aggregates operations, and exposes service discovery. One
channels connection per peer; multiple endpoint types coexisting.

## What

`alknet-hub` is the crate that wires the hub role (ADR-029, ADR-034,
ADR-079) into a reusable library. A hub is the central node in a
hub-and-spoke (head/worker) topology: it accepts inbound connections from
workers and browsers, relays channels between legs, aggregates the
workers' operations into a shared environment, and serves the discovery
API. It depends on `alknet-channels` (the substrate), `alknet-call`
(the protocol), and `alknet-core`. It does not introduce new protocol
types — it wires the existing `ChannelsAdapter`, `ChannelClient`,
`CallAdapter`, `PeerCompositeEnv`, `from_call`, and `Dispatcher` types
into a coherent hub runtime.

### The hub composes endpoint types (ADR-086)

A hub composes a **subset** of three endpoint types, each an independent
listener with its own identity model, auth model, and transport(s). The
subset determines which transports the hub runs and which ALPNs each
`TlsServerConfig` advertises. See [ADR-086](../../decisions/086-endpoint-types-and-entry-points.md)
for the full model.

| Endpoint type | Identity | Auth model | Transport(s) | Client class |
|---------------|----------|------------|--------------|--------------|
| **web** | X.509 (ACME or manual) | token-based (Bearer) | TCP+TLS (HTTP, WebSocket), QUIC (WebTransport — deferred per ADR-044) | browsers, curl, registration, HTTP API consumers |
| **native** | RFC 7250 raw key (Ed25519) | key-based (fingerprint) | QUIC (primary), TCP+TLS (fallback when UDP blocked) | alknet-native clients, workers (fingerprint auth) |
| **iroh** | RFC 7250 raw key (NodeId) | key-based (fingerprint) | iroh (relay-assisted QUIC) | p2p peers, NAT'd nodes, minimal-hub deployments |

The hub shapes that make sense:

| Hub shape | Endpoint types | Public IP required? | Example |
|-----------|---------------|---------------------|---------|
| **full hub** | web + native + iroh | yes (web, native) | the general case — browsers, native clients, p2p |
| **web + native** | web + native | yes | the first real use case — public domain, native clients |
| **native + iroh** | native + iroh | yes (native only) | a hub without browser-facing services |
| **minimal hub** | iroh only | no | a p2p-only hub behind NAT, relay-assisted |

The first real use case is **web + native** (public domain with X.509
for browsers/registration + raw-key QUIC for native clients). Iroh is a
hard requirement for the project (the p2p, no-public-IP case) but is not
in the first deployed subset. All three are hard requirements for the
project as a whole — a full hub runs all three.

### Entry points vs. endpoints (ADR-086 §2)

Within each endpoint type, ALPNs fall into two categories:

- **Entry points** — connections accepted without an established peer
  identity. Per-request auth may apply (registration token, Bearer
  header, or `auth_token` on channel 0), but the connection itself is
  not identity-gated at the TLS layer. Examples: `h2`/`http/1.1`
  (HTTP registration, browser API, stealth decoy, WebSocket upgrade),
  the future `alknet/register` ALPN (worker registration over
  QUIC/TCP without HTTP). Entry points exist to bootstrap a peer
  relationship or serve non-peer clients (browsers).
- **Endpoints** (narrow sense) — connections that require identity
  resolution before the handler runs. No identity → rejected.
  Examples: `alknet/channels`, `alknet/call` (top-level),
  `alknet/ssh` (future).

The distinction determines which `TlsServerConfig` advertises which
ALPNs — see "ALPN lists" below. All ALPNs (entry-point and endpoint)
are registered on the same `HandlerRegistry`; the difference is which
listener accepted them and whether identity was required at the TLS
layer.

### The hub is multi-transport

A hub runs whichever transports its endpoint-type subset implies. A
full hub runs TCP+TLS (web), QUIC (native), and iroh (iroh)
simultaneously; a minimal hub runs iroh alone. The channels protocol is
transport-agnostic (ADR-071; ADR-065
`Connection::from_stream`/`from_bidi`). The hub's accept and dial paths
inherit that property — they take a `Connection`, not a `SocketAddr`
welded to a dial. See "Transport" below.

### What the hub provides

1. **Multi-transport endpoint** — composes a subset of three endpoint
   types (web, native, iroh — ADR-086), each wired as an owned transport
   on `AlknetEndpoint` via `with_quinn` / `with_iroh` / `with_tcp_tls`
   (ADR-083). All feed the same dispatch path. A web+native hub runs
   TCP+TLS (web, X.509) + QUIC (native, raw key); a minimal hub runs
   iroh alone; a full hub runs all three. The endpoint owns all accept
   loops and `shutdown()` stops them all.

2. **Peer lifecycle** — accept, dial, disconnect, reconnect with
   backoff. Identity resolution via `IdentityProvider` (fingerprint or
   bearer token, depending on transport — see "Identity over
   transports"). One channels connection per peer.

3. **Aggregated operation env** — a shared `PeerCompositeEnv` across
   all connected workers (ADR-067). Operations discovered via
   `from_call` are registered in each peer's connection overlay and
   aggregated into the shared env. `invoke_peer` routes operation calls
   to the right peer.

4. **Service discovery** — `services/list-peers` returns each
   connected worker's operation list via
   `PeerCompositeEnv::peer_operations` (ADR-068).

5. **Channel relay** — translates `channel/open` on channel 0 and
   byte-forwards data channels with `channel_id` rewrite (ADR-079).
   Lets a browser reach a spoke's channels through the hub without the
   hub parsing any protocol-specific framing.

6. **Worker registration** (in scope of the hub) — the HTTP endpoint
   that lets a freshly-provisioned worker enroll its key with a
   one-time registration token. The registration flow is what makes
   worker provisioning over TCP+TLS a hard requirement, not an
   option. See "Worker registration" below and OQ-58.

## Why

The hub pattern requires wiring that `alknet-channels` and
`alknet-call` do not provide out of the box:

- The hub composes **multiple endpoint types** (ADR-086). The channels
  crate is transport-agnostic (ADR-071, ADR-065); the hub composes the
  multi-transport endpoint (`AlknetEndpoint` with `with_quinn` /
  `with_iroh` / `with_tcp_tls`, ADR-083) and filters ALPN lists per
  endpoint type. The accept loops themselves live in `alknet-core`; the
  hub provides the handlers and wiring.
- The hub relays channels between legs (ADR-079) — terminating
  channel 0 on each leg, translating `channel/open`, byte-forwarding
  data channels. The channels crate is ALPN-blind and does not know it
  is being relayed; the relay is a hub-crate concern.
- `Dispatcher::compose_root_env` builds a fresh `PeerCompositeEnv` per
  call with only the current connection — multi-worker aggregation is
  not wired (ADR-067).
- `PeerCompositeEnv::peer_operations` is not overridden —
  `services/list-peers` returns empty operation lists for non-local
  peers (ADR-068).
- `from_call` is a free function, not wired into `ChannelClient` —
  the assembly layer must call it manually after every connect
  (ADR-069).
- There is no worker supervision loop — reconnection, backoff, and
  re-discovery are assembly-layer concerns.
- There is no registration endpoint — a freshly-provisioned worker
  has no way to enroll its key with the hub before establishing a
  channels connection.

These are not design flaws; they are the correct separation of
concerns. The channels and call crates provide the types and the
routing logic; the hub wiring is a consumer concern. But it is a
concern *every* hub consumer shares. Rather than each downstream
project (alkapi, future hubs) building the same wiring independently,
`alknet-hub` provides it once, as a reusable crate.

## Architecture

### The hub is built on channels, not call-directly

The hub's substrate is `alknet/channels`, not `alknet/call` directly.
Each peer (worker or browser) holds one channels connection to the
hub. Channel 0 on each connection is `alknet/call` (ADR-072); the
hub's `CallAdapter` runs on channel 0 for the hub's own operations,
for `channel/open` translation (ADR-079), and for `from_call`
discovery. Data channels carry the actual protocol work (TTY, SSH,
tunnels) and are relayed byte-for-byte across legs.

This is the post-channels hub model. The pre-channels model (one QUIC
connection per peer carrying `alknet/call` directly) is replaced: the
connection is `alknet/channels`, and `alknet/call` rides channel 0.
The hub's `CallClient`-direct dial path is replaced by
`ChannelClient::from_connection` (ADR-080).

### Hub struct

The `Hub` owns the aggregated `PeerCompositeEnv`, the
`OperationRegistry`, and the `Dispatcher`:

```rust
pub struct Hub {
    registry: Arc<OperationRegistry>,
    aggregated_env: Arc<RwLock<PeerCompositeEnv>>,
    dispatcher: Dispatcher,
    identity_provider: Arc<dyn IdentityProvider>,
}
```

Construction:

```rust
impl Hub {
    pub fn new(
        registry: Arc<OperationRegistry>,
        identity_provider: Arc<dyn IdentityProvider>,
    ) -> Self {
        let base: Arc<dyn OperationEnv + Send + Sync> =
            Arc::new(LocalOperationEnv::new(Arc::clone(&registry)));
        let aggregated_env = Arc::new(RwLock::new(PeerCompositeEnv::new(base)));
        let dispatcher = Dispatcher::new(Arc::clone(&registry), Arc::clone(&identity_provider))
            .with_aggregated_env(Arc::clone(&aggregated_env));

        Self {
            registry,
            aggregated_env,
            dispatcher,
            identity_provider,
        }
    }

    /// The shared aggregated PeerCompositeEnv. The assembly layer wires
    /// this into CallAdapter::with_aggregated_env so every call's
    /// compose_root_env sees all connected workers.
    pub fn aggregated_env(&self) -> &Arc<RwLock<PeerCompositeEnv>> {
        &self.aggregated_env
    }
}
```

The `Hub` exposes builder methods for optional hooks
(`with_session_source`, `with_ownership_provider`,
`with_timeout`) that delegate to the `Dispatcher`.

### Transport

The hub accepts and dials channels connections over any transport
the channels protocol supports (ADR-071). A hub composes a subset of
three endpoint types (ADR-086); the subset determines which transports
run. All are owned by `AlknetEndpoint` via builder methods (ADR-083):

| Builder method | Endpoint type | Transport | What it carries |
|----------------|---------------|-----------|-----------------|
| `with_quinn` (raw-key config) | native | QUIC (quinn) | `alknet/channels`, `alknet/call`, `alknet/ssh` (future) — native clients with QUIC reachability |
| `with_quinn` (X.509/ACME config) | web | QUIC (quinn) | `h2`, `http/1.1`, `h3` (WebTransport — deferred per ADR-044) — browsers over QUIC |
| `with_iroh` | iroh | QUIC (iroh, relay-assisted) | `alknet/channels`, `alknet/call` — p2p peers, NAT'd nodes |
| `with_tcp_tls` (X.509/ACME config) | web | TCP + TLS | `h2`/`http/1.1` (registration, browser), `alknet/channels` (WebSocket-carrying-channels — OQ-65), `acme-tls/1` (appended automatically) |
| `with_tcp_tls` (raw-key config) | native | TCP + TLS | `alknet/channels`, `alknet/call` — native clients using TCP+TLS when UDP is blocked |
| (Future) WebTransport | web | WebTransport | Browser bidirectional path (deferred per ADR-044; WebSocket is the v1 browser path) |

A single `AlknetEndpoint` may hold two quinn listeners (one per
`TlsServerConfig` — raw-key for native, X.509 for web), one iroh
endpoint, and one or two TCP+TLS listeners (X.509 for web, raw-key
for native fallback). The endpoint owns all of them; `shutdown()`
stops them all.

### ALPN lists (ADR-086 §3)

Each `TlsServerConfig` advertises only the ALPNs its endpoint type's
client class can negotiate. The assembly layer filters
`registry.alpn_strings()` by endpoint type — see
[ADR-086](../../decisions/086-endpoint-types-and-entry-points.md) §3
for the full table. The split makes the advertisement honest (no `h2`
on a raw-key listener that browsers cannot reach) and the
assembly-layer wiring pattern guessable (not a same-list-vs-split
guess).

All accept loops run inside `endpoint.run()` and feed the same dispatch
path. The hub's `HttpAdapter` serves `h2`/`http/1.1` over the web
endpoint's TCP+TLS path for registration and browser access; the
`ChannelsAdapter` (registered on the hub's `HandlerRegistry`) serves
`alknet/channels` over the native endpoint's QUIC path (and over
TCP+TLS when a worker dials channels-over-TCP). Both ALPNs are
registered on the same registry; the TLS handshake on each connection
negotiates the ALPN and dispatches to the right adapter.

The TCP+TLS accept loop constructs a `Connection` per accepted
`TlsStream<TcpStream>` via `Connection::from_bidi` (ADR-065) and hands
it to the same dispatch path the quinn endpoint uses. After ADR-083,
TCP+TLS is a first-class owned transport on `AlknetEndpoint` — the hub
hands a `TcpListener` + `TlsAcceptor` to the endpoint via
`with_tcp_tls(listener, acceptor)`, and the endpoint runs the accept
loop inside `run()` alongside the quinn/iroh loops. No external sibling
loop; the endpoint owns all its accept loops and `shutdown()` stops them
all.

#### Dial (outbound workers) — transport-agnostic

The hub dials outbound workers via `ChannelClient`, not `CallClient`.
The hub is a client when it dials out — a hub (B) that connects to
another hub (A) is a client from A's perspective. The dial needs a
client-side TLS config (`TlsClientConfig`, ADR-087) for the outbound
connection's `rustls::ClientConfig` (verifier selection per ADR-034:
fingerprint pin for the worker's known key). The dial path mirrors the
`from_connection` / `connect_quic` split (ADR-080):

```rust
impl Hub {
    /// Take over a pre-established channels `Connection` as a worker
    /// connection. Transport-agnostic — the caller (or a transport
    /// helper) produces the `Connection`. This is the primary path;
    /// `connect_quic_worker` and future `connect_tcp_tls_worker` are
    /// conveniences over it.
    pub async fn dial_worker_connection(
        &self,
        connection: Connection,
        config: FromCallConfig,
    ) -> Result<(PeerId, ChannelClient), HubError>;

    /// QUIC convenience: dial a worker over QUIC, then
    /// `dial_worker_connection`. Builds a `TlsClientConfig` (ADR-087)
    /// with the worker's fingerprint pinned (ADR-034), dials QUIC,
    /// wraps as a channels `Connection`, calls `dial_worker_connection`.
    pub async fn connect_quic_worker(
        &self,
        addr: SocketAddr,
        credentials: CallCredentials,
        config: FromCallConfig,
    ) -> Result<(PeerId, ChannelClient), HubError>;
}
```

`dial_worker_connection` is the transport-agnostic primary. After
taking over the `Connection`, it runs `from_call` on channel 0 to
discover the worker's operations, registers the discovered bundles in
the connection's Layer 2 overlay, and attaches the peer to the
aggregated env. `connect_quic_worker` is the "I just want QUIC"
convenience — it dials QUIC (via `AlknetClient::dial_quic`, ADR-089,
which builds the `TlsClientConfig` with the worker's fingerprint
pinned), calls `ChannelClient::from_connection`, then
`dial_worker_connection`. A future `connect_tcp_tls_worker` dials
TCP+TLS via `AlknetClient::dial_tcp_tls` the same way. The
one-way-door surface is `dial_worker_connection`; the dial helpers are
two-way-door conveniences over `AlknetClient` (ADR-089). The hub's
`supervise_worker` (below) takes a `dial` closure that can call
`AlknetClient` internally — the hub does not need to know
`AlknetClient` exists; the closure seam is preserved.

#### Accept (inbound workers and browsers) — transport-agnostic

Inbound connections arrive over whatever transport the accept loop
yielded them from. The `ChannelsAdapter` (registered on
`alknet/channels` in the `HandlerRegistry`) receives a `Connection` and runs the demux loop
(ADR-075); the `CallAdapter` (running on channel 0) handles
hub-level operations and `channel/open` translation (ADR-079).

The `WorkerConnectedCallback` fires inside `ChannelsAdapter::handle`
between channel-0 establishment and dispatch start:

```rust
/// Callback invoked by ChannelsAdapter::handle when a peer connects
/// inbound. Fires after identity resolution and before the dispatch
/// loop starts. Carries both on_connected (runs from_call, registers
/// discovered ops, attaches peer to aggregated env) and on_disconnected
/// (detaches peer on run_loop exit).
pub struct WorkerConnectedCallback {
    hub: Arc<Hub>,
    config: FromCallConfig,
}

impl WorkerConnectedCallback {
    pub fn new(hub: Arc<Hub>, config: FromCallConfig) -> Self {
        Self { hub, config }
    }

    pub(crate) async fn on_connected(
        &self,
        connection: &CallConnection,
    ) -> Result<PeerId, HubError> {
        let peer_id = connection.identity()
            .map(|id| id.id.clone())
            .ok_or(HubError::NoPeerIdentity)?;

        let bundles = from_call(connection, self.config.clone()).await?;
        connection.register_imported_all(bundles);

        self.hub.aggregated_env
            .write()
            .expect("aggregated env lock poisoned")
            .attach_peer(peer_id.clone(), connection.overlay_env());

        Ok(peer_id)
    }

    pub(crate) fn on_disconnected(&self, peer_id: &PeerId) {
        self.hub.on_worker_disconnected(peer_id);
    }
}
```

The callback is wired into `ChannelsAdapter` (the channels substrate)
via a builder method. The `ChannelsAdapter::handle` flow becomes:

1. Channel 0 (`alknet/call`) is preinstalled (ADR-072). The channel-0
   `Connection` is handed to the `CallAdapter`.
2. Identity resolution — two paths depending on transport (see
   "Identity over transports"):
   - **Fingerprint path** (QUIC + raw key, or a transport with a
     client cert): the `AuthContext` carried into
     `ChannelsAdapter::handle` carries the peer's TLS fingerprint. The
     `CallAdapter` resolves it via `resolve_from_fingerprint`.
   - **Bearer-token path** (TCP+TLS with no client cert, WebTransport,
     WebSocket): the `CallAdapter` on channel 0 extracts `auth_token`
     from the first call frame (auth.md) and resolves it via
     `resolve_from_token`. The transport carries no identity of its
     own — the call protocol's first frame does.
3. The `CallConnection` is constructed with the resolved identity.
4. Invoke `on_connected` — runs `from_call`, registers bundles,
   attaches peer to aggregated env.
5. Run the call-protocol dispatch loop on channel 0; the demux loop
   continues on the other channels.
6. On disconnect, invoke `on_disconnected` — detaches peer from
   aggregated env.

The assembly layer constructs the callback and passes it to
`ChannelsAdapter`:

```rust
let callback = WorkerConnectedCallback::new(Arc::clone(&hub), FromCallConfig::new());
let channels_adapter = ChannelsAdapter::new(Arc::clone(&registry), /* ... */)
    .with_worker_connected_callback(callback);
// Register channels_adapter on alknet/channels in the HandlerRegistry.
// The endpoint dispatches alknet/channels connections to it — whether
// they arrived over quinn, iroh, or TCP+TLS (all owned by the endpoint).
```

### Identity over transports

A peer's identity is resolved differently depending on the transport
the connection arrived over. This is not a hub invention — it is the
existing identity model (ADR-030, ADR-034, auth.md) applied to the
channels substrate.

| Transport | Identity source | Resolution path |
|-----------|----------------|-----------------|
| QUIC + RFC 7250 raw key | TLS fingerprint (automatic from handshake) | `resolve_from_fingerprint` — both sides present raw keys |
| QUIC + X.509 (client cert) | TLS fingerprint (from client cert) | `resolve_from_fingerprint` — worker presents a client cert |
| TCP+TLS (client cert) | TLS fingerprint (from client cert) | `resolve_from_fingerprint` — worker presents a client cert |
| TCP+TLS (no client cert) | Bearer token (call-protocol `auth_token` in first frame on channel 0) | `resolve_from_token` |
| WebTransport / WebSocket | Bearer token | `resolve_from_token` — browsers have no fingerprint (ADR-034 §4) |

The fingerprint path is the QUIC+raw-key optimization — identity is
"free" because the TLS handshake carries it. The X.509-client-cert
rows (QUIC and TCP+TLS) are the same path with a different cert
format — the hub matches the client cert's fingerprint via
`resolve_from_fingerprint` against the `SHA256:<hex>` entry in the
peer's `PeerEntry` (the mixed-fingerprint case from ADR-034 §3). A
worker may present an X.509 client cert over QUIC or TCP+TLS when the
hub's deployment uses X.509 rather than raw keys; the identity model
handles both identically.

The token path is the transport-agnostic fallback — it works over any
transport that can carry a call-protocol first frame, which is all of
them (channel 0 is `alknet/call`). `resolve_from_token` matches the
token against `PeerEntry.auth_token_hash` and returns the same
`PeerId` the fingerprint path would (ADR-030).

This is why channels-over-TCP is not a special case. The channels
protocol runs `alknet/call` on channel 0 (ADR-072); the call
protocol's `auth_token` path resolves identity from the first frame;
the transport carries no identity burden of its own. A worker that
registered its key over HTTP (TCP+TLS, no client cert) and then
connects via channels-over-TCP authenticates with the bearer token it
received at registration. A worker that connects via channels-over-QUIC
authenticates with its TLS fingerprint. Both resolve to the same
`PeerEntry` and the same `PeerId`.

### Worker registration

The registration flow is an **entry point** (ADR-086 §2) — a connection
accepted without an established peer identity, authenticated per-request
by the registration token. This is why the registration endpoint is an
HTTP route on `h2`/`http/1.1` (an entry-point ALPN), not a
call-protocol operation: the worker has no `CallConnection` yet at
registration time. A future `alknet/register` ALPN would serve the same
entry-point role over QUIC/TCP without the HTTP layer (not yet specced).

The registration flow is what makes the web endpoint (TCP+TLS, X.509)
a hard requirement for a hub that provisions workers. A freshly-provisioned
worker (container, vast.ai, runpod) does not yet have an established
peer relationship with the hub — it has a one-time registration token
supplied via the provisioning config. The flow:

1. The hub provisions a worker instance (docker, vast.ai, runpod —
   platform-specific) and supplies a registration token via
   `onStartCMD` (or equivalent).
2. The instance downloads the worker binary.
3. The instance generates an Ed25519 key pair (its future identity).
4. The instance POSTs to the hub's HTTP registration endpoint over
   TCP+TLS, sending its public key and the registration token.
5. The hub validates the token, creates a `PeerEntry` for the worker
   recording *both* the fingerprint (from the public key) and an
   `auth_token_hash` (from a session token the hub issues), and returns
   the session credential. The `PeerEntry` is the mixed-fingerprint
   shape from ADR-034 §3 — fingerprint for the QUIC path,
   `auth_token_hash` for the TCP+TLS path.
6. The instance connects to the hub via channels — over QUIC
   (fingerprint identity) or over TCP+TLS (bearer-token identity) —
   and both resolve to the same `PeerEntry` and the same `PeerId`.
   The ongoing session begins.

Step 4 is HTTP over TCP+TLS. Step 6 is channels over QUIC or TCP+TLS.
Both happen; they are not alternatives. The registration endpoint is
an HTTP route on the `HttpAdapter` (registered on the hub's
`HandlerRegistry`, served on `h2`/`http/1.1`
over TCP+TLS), not a call-protocol operation — the worker has no
`CallConnection` yet at step 4.

The registration endpoint and the enrollment-token model are a
one-way-door API (the endpoint shape, the token semantics). That
decision is tracked as OQ-58 — it is decision-ready in shape (HTTP
POST, token in, `PeerEntry` out, session credential returned) but
the exact token model (one-time vs. refresh, single-use vs.
multi-use, rotation) and the endpoint path need a dedicated ADR
before the hub crate stabilizes.

### Supervision

The hub provides a `supervise_worker` method that wraps
`dial_worker_connection` (or a transport-specific helper) in a
reconnect loop with configurable backoff:

```rust
impl Hub {
    /// Supervise an outbound worker: dial, discover, attach. On
    /// disconnect, detach and retry with backoff. Runs until the Hub
    /// is dropped (the returned JoinHandle can be aborted).
    ///
    /// `dial` is a closure that produces a channels `Connection` —
    /// the hub does not bake a transport into the supervision loop.
    /// The caller provides e.g. `|| async { Ok(Connection::from_quinn(quinn_endpoint.connect(addr, "alknet/channels")?.await?)) }`
    /// for QUIC, or `|| async { Ok(Connection::from_bidi(tls_connector.connect(host, TcpStream::connect(addr).await?).await?, b"alknet/channels".to_vec(), Some(addr))) }`
    /// for TCP+TLS. The supervision loop is transport-agnostic.
    pub fn supervise_worker<F, Fut>(
        self: &Arc<Self>,
        dial: F,
        config: FromCallConfig,
        backoff: BackoffConfig,
    ) -> tokio::task::JoinHandle<()>
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Connection, HubError>> + Send + 'static;
}
```

The supervision loop takes a `dial` closure rather than a `SocketAddr`
+ `CallCredentials` pair. This keeps the loop transport-agnostic —
the caller decides the transport by what the closure does. The
backoff and re-discovery logic is the same regardless of transport.

Disconnect is detected via the OQ-52 interim — the loop polls
`connection().accept_bi()` until `ConnectionClosed` — pending a
`CallConnection::closed()` method (OQ-52 target resolution). On
disconnect, the peer is detached from the aggregated env and the
loop retries with backoff.

### Backoff configuration

```rust
pub struct BackoffConfig {
    pub initial: Duration,
    pub max: Duration,
    pub multiplier: f64,
}

impl BackoffConfig {
    pub fn delay_for(&self, retries: u32) -> Duration {
        let delay = self.initial.as_millis() as f64
            * self.multiplier.powi(retries as i32);
        let delay_ms = delay.min(self.max.as_millis() as f64) as u64;
        Duration::from_millis(delay_ms)
    }
}

impl Default for BackoffConfig {
    fn default() -> Self {
        Self {
            initial: Duration::from_secs(1),
            max: Duration::from_secs(60),
            multiplier: 2.0,
        }
    }
}
```

### Channel relay (ADR-079)

When a browser (or any non-peer client) connects to the hub and opens
a channel to a spoke, the hub translates `channel/open` on channel 0
(terminate on both legs, re-issue with `forwarded_for` — ADR-032) and
byte-forwards data channels with `channel_id` rewrite. `channel/control`
operations on channel 0 carry `channel_id` in their JSON payload; the
hub's `CallAdapter` translates these too, rewriting `channel_id` to
the other leg's id (ADR-079). The hub does not run protocol-specific
handlers (`alknet/tty`, `alknet/ssh`, `alknet/tunnel`) — it runs
`alknet/channels` (the relay) and `alknet/call` (for its own ops +
translation). The full relay contract is in ADR-079; the relay
implementation lives in `alknet-hub`.

### Service discovery

The hub registers the built-in service discovery operations
(`services/list`, `services/schema`, `services/list-peers`)
automatically. The `services/list-peers` handler returns each
connected worker's operation list via
`PeerCompositeEnv::peer_operations` (ADR-068).

### HubError

```rust
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HubError {
    #[error("worker connection has no resolved peer identity")]
    NoPeerIdentity,
    #[error("from_call discovery failed: {0}")]
    Discovery(#[from] AdapterError),
    #[error("call client error: {0}")]
    Client(#[from] ClientError),
    #[error("channel error (client or adapter path): {0}")]
    Channel(#[from] ChannelError),
    #[error("registration failed: {0}")]
    Registration(#[from] RegistrationError),
}
```

`RegistrationError` lives in `alknet-hub` (the registration endpoint is
a hub-crate surface). Its variants are defined alongside the OQ-58
resolution — the likely shape is `InvalidToken`, `ExpiredToken`,
`AlreadyEnrolled`, `Store(StoreError)`, but the exact set is not
fixed until the enrollment-token model is decided.

## What the hub does NOT do

- **Worker authentication policy.** The hub resolves the worker's
  identity via `IdentityProvider` (the existing mechanism). Whether a
  given identity is *allowed* to connect as a worker is an
  `AccessControl` decision on the assembly layer's curated ops — the
  hub does not add a separate worker-auth layer.
- **Worker-specific routing policy.** `PeerRef::Any` uses
  insertion-order first-match (ADR-029 §2). A richer
  `RoutingPolicy` (round-robin, least-loaded) is a future extension
  behind the same `PeerRef` enum.
- **Multi-hop federation.** The hub is one-hop: workers connect to
  the hub, the hub composes their ops. Worker A does not transitively
  see worker B's ops through the hub unless the hub explicitly
  re-exports them (ADR-029 Assumption 5).
- **Worker provisioning.** The hub does not spawn workers, configure
  them, or manage their lifecycle beyond connection supervision. The
  registration endpoint enrolls a key; it does not provision the
  instance. Worker provisioning (docker, vast.ai, runpod) is an
  assembly-layer concern that calls the hub's registration endpoint
  after provisioning.

## Crate dependencies

```
alknet-hub (Hub struct deps)
├── alknet-channels-call (ChannelClient, ChannelsAdapter, ChannelManager,
│                         ChannelBidiStreamSource)
├── alknet-call (CallAdapter, Dispatcher, PeerCompositeEnv,
│                from_call, FromCallConfig, AdapterError, ClientError)
├── alknet-http (HttpAdapter — for the registration endpoint and browser access)
├── alknet-core (IdentityProvider, Connection, OperationRegistry,
│                AuthContext)
├── tokio (spawn, time::sleep)
└── tracing (logging)

  (assembly-layer deps — used by the hub's composition code, not by
   the Hub struct itself)
  ├── alknet-endpoint (AlknetEndpoint with quinn + iroh + tcp features,
  │   │               HandlerRegistry — the accept-loop runner)
  ├── alknet-client (AlknetClient — outbound worker dials, ADR-089)
  ├── alknet-tls (TlsServerConfig — builds the raw-key + X.509/ACME
  │               configs handed to the endpoint's transports)
  └── alknet-core [quinn/iroh features] (Connection::from_quinn/from_iroh)
```

`alknet-hub` depends on `alknet-channels-call`, `alknet-call`, and
`alknet-http`, all of which depend on `alknet-core`. The hub is a
consumer of the channels substrate and the call protocol, not a new
protocol handler. The `alknet-http` dependency is for the
registration endpoint and browser HTTP access — the hub wires
`HttpAdapter` into the same `HandlerRegistry` as
`ChannelsAdapter`.

`alknet-tls` is an **assembly-layer** dependency, not a `Hub` struct
dependency: the `Hub` struct holds no `TlsServerConfig`, but the hub's
composition code (the assembly-layer wiring shown in "Assembly layer
integration" below) builds the `TlsServerConfig`s and hands them to the
endpoint's transports via `for_quinn()` / `for_tcp_tls()`. This
distinction matters: a consumer that uses `Hub` with externally-built
transports does not pull `alknet-tls` through the `Hub` type, only
through the assembly wiring.

## Assembly layer integration

A downstream hub (alkapi) uses `alknet-hub` like this:

```rust
// 1. Build the curated registry (Layer 0)
let registry = OperationRegistryBuilder::new()
    .with_local(agent_chat_spec(), agent_chat_handler, ...)
    .with_local(services_list_spec(), services_list_handler, ...)
    // ... other curated ops
    .build();
let registry = Arc::new(registry);

// 2. Create the hub
let hub = Arc::new(Hub::new(
    Arc::clone(&registry),
    Arc::clone(&identity_provider),
).with_ownership_provider(ownership_provider));

// 3. Register the ChannelsAdapter on alknet/channels. The endpoint
//    dispatches alknet/channels connections to this adapter — whether
//    they arrived over quinn, iroh, or TCP+TLS.
let callback = WorkerConnectedCallback::new(Arc::clone(&hub), FromCallConfig::new());
let channels_adapter = ChannelsAdapter::new(/* ... */)
    .with_worker_connected_callback(callback);
registry.register(b"alknet/channels", Arc::new(channels_adapter));

// 4. Register the HttpAdapter on h2/http1.1. The endpoint dispatches
//    h2/http1.1 connections (arriving over TCP+TLS) to this adapter
//    (registration endpoint, browser access, stealth decoy).
let http_adapter = HttpAdapter::new(/* ... */);
registry.register(b"h2", Arc::new(http_adapter.clone()));
registry.register(b"http/1.1", Arc::new(http_adapter));

// 5. Build the quinn endpoint (raw-key config for native clients)
let quinn_endpoint = raw_key_tls.for_quinn()?.into_endpoint(listen_addr)?;

// 6. Build the TCP+TLS listener (X.509/ACME config for HTTPS)
let tcp_listener = TcpListener::bind(registration_addr).await?;
let tls_acceptor = x509_tls.for_tcp_tls();

// 7. Construct the endpoint with all owned transports, then run.
//    TCP+TLS is a first-class owned transport (ADR-083) — the endpoint
//    runs its accept loop inside run() alongside quinn/iroh. No external
//    sibling loop; shutdown() stops them all.
let endpoint = Arc::new(
    AlknetEndpoint::new(registry, dynamic, identity_provider, drain_timeout)
        .with_quinn(quinn_endpoint)
        .with_tcp_tls(tcp_listener, tls_acceptor),
);
endpoint.clone().run().await;

// 7. Dial outbound workers (hub dials workers). The closure produces a
//    channels Connection — the hub's supervise_worker calls
//    dial_worker_connection internally. Both transports are shown; a
//    real deployment picks one per worker.

// QUIC dial:
hub.supervise_worker(
    || async move {
        // Dial QUIC, wrap the quinn connection as a channels Connection.
        let quinn_conn = quinn_endpoint
            .connect(worker_addr, "alknet/channels")?
            .await?;
        Ok(Connection::from_quinn(quinn_conn))
    },
    FromCallConfig::new(),
    BackoffConfig::default(),
);

// TCP+TLS dial (e.g., for a worker that can't reach the hub over QUIC):
hub.supervise_worker(
    || async move {
        let tcp = TcpStream::connect(worker_addr).await?;
        let tls = tls_connector.connect("worker.example.com", tcp).await?;
        // from_bidi wraps the TlsStream as a channels Connection
        // (ADR-065). The ALPN must be alknet/channels.
        Ok(Connection::from_bidi(tls, b"alknet/channels".to_vec(), Some(worker_addr)))
    },
    FromCallConfig::new(),
    BackoffConfig::default(),
);
```

The hub's `aggregated_env()` accessor returns the shared
`Arc<RwLock<PeerCompositeEnv>>` so the assembly layer can wire it
into `CallAdapter::with_aggregated_env`.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| Hub relay — translate, not transparently forward | [ADR-079](../../decisions/079-hub-relay-translate-not-forward.md) | Translate `channel/open` on channel 0 with `forwarded_for`; byte-forward data channels with `channel_id` rewrite |
| Aggregated peer-env wiring | [ADR-067](../../decisions/067-aggregated-peer-env-wiring.md) | `Dispatcher::with_aggregated_env` hook; `compose_root_env` reads shared env |
| PeerCompositeEnv::peer_operations | [ADR-068](../../decisions/068-peer-composite-env-peer-operations.md) | `list_operation_names` trait method; `PeerCompositeEnv` override |
| from_call is manual | [ADR-069](../../decisions/069-from-call-manual-free-function.md) | `from_call` is a free function; the hub calls it after connect |
| Peer-graph routing model | [ADR-029](../../decisions/029-peer-graph-routing-model.md) | Peer-keyed overlays, `PeerRef` routing, `AccessControl`-based peer auth |
| PeerEntry and Identity.id | [ADR-030](../../decisions/030-peerentry-and-identity-id-decoupling.md) | `PeerId` = `Identity.id` = `PeerEntry.peer_id` (stable) |
| Three peer roles | [ADR-034](../../decisions/034-outgoing-only-x509-and-three-peer-roles.md) | Hub = role-3 `PeerEntry` (mixed fingerprints); browsers not peers; bearer-token identity over TCP/WebTransport |
| ChannelClient — transport-agnostic | [ADR-080](../../decisions/080-channelclient.md) | `from_connection` primary, `connect_quic` convenience; the dial path the hub uses |
| Channels transport-agnostic | [ADR-071](../../decisions/071-channels-wire-format.md) | Substrate modes; `Connection::from_stream`/`from_bidi` (ADR-065) — the substrate the hub relays |
| TCP+TLS as first-class owned transport | [ADR-083](../../decisions/083-endpoint-as-accept-loop-runner.md) | `with_tcp_tls(listener, acceptor)` — TCP+TLS is owned by the endpoint, not a sibling loop; supersedes ADR-010 Am. 1 |
| Channel 0 pre-negotiated | [ADR-072](../../decisions/072-channel-0-pre-negotiated-call.md) | Channel 0 = `alknet/call`; the `CallAdapter` runs here |
| Channel lifecycle operations | [ADR-073](../../decisions/073-channel-lifecycle-operations.md) | `channel/open`/`close`/`control`/`resources/subscribe` — what the hub translates |
| Endpoint types and entry points | [ADR-086](../../decisions/086-endpoint-types-and-entry-points.md) | Three endpoint types (web/native/iroh); entry-point vs. endpoint ALPN distinction; split ALPN lists per endpoint type |
| `TlsClientConfig` for outbound dials | [ADR-087](../../decisions/087-tlsclientconfig-not-blocked-on-dial.md) | `alknet-tls` provides client-side TLS config; hub-as-client is a first-class use case; not blocked on the dial-seam extraction (OQ-55) |
| `AlknetClient` native dial seam | [ADR-089](../../decisions/089-alknetclient-native-dial-seam.md) | New crate `alknet-client`; the hub's outbound worker dials use `AlknetClient` (via the `supervise_worker` closure or the `connect_quic_worker` convenience); resolves OQ-55 |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-58** (open): Worker registration flow — the enrollment-token
  model, the HTTP registration endpoint shape, and the
  `register_worker` API. Decision-ready in shape (HTTP POST, token in,
  `PeerEntry` created, session credential returned); the exact token
  model (one-time vs. refresh, rotation) and endpoint path need a
  dedicated ADR before the hub crate stabilizes. The registration
  endpoint is an entry point (ADR-086 §2) — a future `alknet/register`
  ALPN would serve the same role over QUIC/TCP without HTTP.
- **OQ-65** (open): WebSocket carrying channels — whether the browser
  path extends from call-protocol-only (ADR-048) to full channels
  (the 9-byte chunk format over WebSocket binary frames). If chosen,
  the browser is a first-class channels participant and the hub relay
  works unchanged for browser legs. The web endpoint advertises
  `alknet/channels` by default (ADR-086 §3 — the advertisement is
  settled; OQ-65 governs whether the browser path uses it).
- **OQ-52** (open): `CallConnection::wait_for_close()` — the
  supervision loop needs a way to await connection close. The
  committed interim is polling `connection().accept_bi()` until
  `ConnectionClosed`. A `closed()` method on `CallConnection` is the
  target resolution.
- **OQ-53** (open): `BackoffConfig` defaults — the committed policy
  is 1s initial, 60s max, 2x multiplier. OQ-53 tracks whether
  operational experience warrants a change before the first release.
- **OQ-54** (resolved): Inbound worker hook placement — the callback
  fires inside `ChannelsAdapter::handle()` between identity resolution
  and dispatch start. `handle()` blocks until disconnect, so there is
  no "after handle accepts" point for the assembly layer to hook into.
  The `WorkerConnectedCallback` is the committed design.

## References

- [channel-client.md](../channels/channel-client.md) — `ChannelClient`
  (`from_connection` / `connect_quic` — the dial path)
- [channels-adapter.md](../channels/channels-adapter.md) —
  `ChannelsAdapter`, `ChannelManager`, the accept path
- [channel-operations.md](../channels/channel-operations.md) —
  `channel/open`/`close`/`control`, the hub relay contract
- [client-and-adapters.md](../call/client-and-adapters.md) — `CallClient`,
  `from_call`, `OperationAdapter`
- [call-protocol.md](../call/call-protocol.md) — `CallAdapter`,
  `Dispatcher`, `CallConnection`
- [operation-registry.md](../call/operation-registry.md) —
  `OperationRegistry`, `OperationRegistryBuilder`, `OperationEnv`
- [http-server.md](../http/http-server.md) — `HttpAdapter` (the
  registration endpoint and browser HTTP access)
- [auth.md](../core/auth.md) — `resolve_from_token`,
  `resolve_from_fingerprint` (the identity paths over transports)
- ADR-029: Peer-Graph Routing Model
- ADR-034: Three Peer Roles (hub = role-3, bearer-token identity)
- ADR-065: `Connection::from_stream`/`from_bidi` (TCP+TLS path)
- ADR-067: Aggregated Peer-Environment Wiring
- ADR-068: PeerCompositeEnv::peer_operations Override
- ADR-069: from_call Is a Manual Free Function
- ADR-079: Hub Relay — Translate, Not Transparently Forward
- ADR-080: ChannelClient (transport-agnostic `from_connection`)
- ADR-082: alknet-tls extraction (`TlsServerConfig` — shared across quinn + TCP+TLS)
- ADR-083: Endpoint as multi-transport accept-loop runner (`with_tcp_tls` — TCP+TLS owned by the endpoint; the hub composes transports and handlers)
- ADR-086: Endpoint types and entry points (web/native/iroh; entry-point vs. endpoint; split ALPN lists per endpoint type)
- ADR-087: `TlsClientConfig` not blocked on dial seam (client-side TLS config; hub-as-client requirement)
- alkapi [hub.md](/workspace/@alkdev/alkapi/docs/architecture/hub.md) —
  the first hub consumer, the concrete use case that informed this
  crate
- alkapi [ADR-011](/workspace/@alkdev/alkapi/docs/architecture/decisions/011-aggregated-peer-env.md) —
  the downstream aggregation decision