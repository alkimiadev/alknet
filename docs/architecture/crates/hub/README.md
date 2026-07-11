---
status: draft
last_updated: 2026-07-09
---

# alknet-hub

The hub pattern as a reusable crate: peer lifecycle management, connection
supervision, operation aggregation, and channel management across connected
peers. One QUIC connection per peer; channel 0 carries the call protocol as
the universal control plane; channels 1..N carry any ALPN as data planes.

## What

`alknet-hub` is a thin crate that provides the head/worker (hub-spoke) pattern
as a reusable library. It depends on `alknet-call` and `alknet-core`. It does
not introduce new types — it wires the existing `PeerCompositeEnv`,
`CallClient`, `CallAdapter`, `from_call`, and `Dispatcher` types into a
coherent hub runtime.

### The channel model

A hub-worker connection is a single QUIC connection. QUIC multiplexes
multiple bidirectional streams over one connection. The hub uses this
multiplexing to carry multiple protocols over one connection:

- **Channel 0 — call protocol (control plane).** Always `alknet/call`. This is
  the bootstrap channel. It handles operation discovery (`from_call`),
  operation routing (`invoke_peer`), service discovery (`services/list`,
  `services/list-peers`), and channel negotiation (`channel/open`,
  `channel/close`). Every hub-worker connection has channel 0.

- **Channels 1..N — any ALPN (data planes).** Opened via `channel/open` on
  the call protocol. Each channel is a bidirectional QUIC stream, wrapped as
  a `Connection` via `Connection::from_bidi` (ADR-065), and handed to the
  same `ProtocolHandler::handle(connection, auth)` that handles dedicated
  connections. The handler does not know it's on a multiplexed channel — it
  sees a `Connection` and dispatches normally.

Channel negotiation is symmetric — either side can open a channel. A hub can
open a tty channel to a worker (hub wants a terminal on the worker), or a
worker can open a tty channel to the hub (worker exposes a terminal to the
hub). Same pattern as `from_call`: bidirectional, symmetric, negotiated over
the call protocol.

The hub's role for channels 1..N is **channel management** — tracking which
channels are open on which peer, answering `channel/list`, and rejecting
duplicate `channel/open` requests. The hub does not interpret the protocol
running on the channel and does not proxy streams. What the caller does with
the resulting channel is the assembly layer's business.

### What the hub provides

1. **Peer lifecycle** — dial, accept, disconnect, reconnect with backoff.
   Identity resolution via `IdentityProvider`. One connection per peer.

2. **Aggregated operation env** — a shared `PeerCompositeEnv` across all
   calls (ADR-067). Operations discovered via `from_call` on channel 0 are
   registered in each peer's connection overlay and aggregated into the
   shared env. `invoke_peer` routes operation calls to the right peer.

3. **Channel management** — tracking which channels are open on which peer.
   `channel/open` and `channel/close` operations on the call protocol.
   Channel tracking for channels 1..N.

4. **Service discovery** — `services/list-peers` returns each connected
   worker's operation list via `PeerCompositeEnv::peer_operations` (ADR-068).

## Why

The alkapi project identified that the hub pattern requires wiring that
alknet-call does not provide out of the box:

- `Dispatcher::compose_root_env` builds a fresh `PeerCompositeEnv` per call
  with only the current connection — multi-worker aggregation is not wired
  (ADR-067).
- `PeerCompositeEnv::peer_operations` is not overridden — `services/list-peers`
  returns empty operation lists for non-local peers (ADR-068).
- `from_call` is a free function, not wired into `CallClient::connect` — the
  assembly layer must call it manually after every connect (ADR-069).
- There is no worker supervision loop — reconnection, backoff, and
  re-discovery are assembly-layer concerns.

These are not design flaws; they are the correct separation of concerns.
`alknet-call` provides the types and the routing logic; the hub wiring is a
consumer concern. But it is a concern *every* hub consumer shares. Rather than
each downstream project (alkapi, future hubs) building the same wiring
independently, `alknet-hub` provides it once, as a reusable crate.

## Architecture

### Hub struct

The `Hub` is the central type. It owns the aggregated `PeerCompositeEnv`, the
`OperationRegistry`, the `Dispatcher`, and channel tracking:

```rust
pub struct Hub {
    registry: Arc<OperationRegistry>,
    aggregated_env: Arc<RwLock<PeerCompositeEnv>>,
    dispatcher: Dispatcher,
    identity_provider: Arc<dyn IdentityProvider>,
    /// Open channels per peer: PeerId → set of channel numbers.
    /// Channel 0 (call protocol) is tracked implicitly via the
    /// CallConnection; channels 1..N are tracked here. The hub tracks
    /// which channels exist so it can answer channel/list and reject
    /// duplicate channel/open requests. The hub does not hold the
    /// channel's Connection — the ProtocolHandler owns it.
    channels: Arc<RwLock<HashMap<PeerId, HashSet<u32>>>>,
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
            channels: Arc::new(RwLock::new(HashMap::new())),
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

The `Hub` exposes builder methods for optional hooks (`with_session_source`,
`with_ownership_provider`, `with_timeout`) that delegate to the `Dispatcher`.

### Peer lifecycle (channel 0 bootstrap)

Every hub-worker connection starts with channel 0 — the call protocol. The
hub establishes channel 0, runs `from_call` to discover the peer's operations,
and attaches the peer to the aggregated env. After channel 0 is established,
either side can open additional channels via `channel/open`.

#### Inbound workers (workers dial the hub)

The hub's `CallAdapter` accepts inbound connections. The hub provides a
`WorkerConnectedCallback` that fires inside the accept path, between
connection-establish and dispatch-start. The callback runs `from_call`,
registers the bundles, and attaches the peer to the aggregated env:

```rust
/// Callback invoked by CallAdapter::handle when a worker connects inbound.
/// Fires after identity resolution and before the dispatch loop starts.
/// Carries both on_connected (runs from_call, registers discovered ops,
/// attaches peer to aggregated env) and on_disconnected (detaches peer,
/// drops channel tracking on run_loop exit).
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

The callback is wired into `CallAdapter` via a new builder method:

```rust
impl CallAdapter {
    /// Register a callback invoked when a worker connects inbound.
    /// Fires after identity resolution and before the dispatch loop.
    /// The callback runs from_call, registers discovered operations,
    /// and attaches the peer to the aggregated env.
    pub fn with_worker_connected_callback(
        mut self,
        callback: WorkerConnectedCallback,
    ) -> Self {
        self.worker_connected_callback = Some(callback);
        self
    }
}
```

The `CallAdapter::handle` flow becomes:

1. Resolve identity from the connection.
2. Create `CallConnection`, set identity.
3. Invoke `on_connected` callback (if set) — runs `from_call`, registers
   bundles, attaches peer to aggregated env.
4. Run the dispatch loop (`run_loop`).
5. On `run_loop` exit, invoke `on_disconnected` on the callback — detaches
   peer from aggregated env, drops channel tracking.

The assembly layer constructs the callback and passes it to `CallAdapter`:

```rust
let callback = WorkerConnectedCallback::new(Arc::clone(&hub), FromCallConfig::new());
let adapter = CallAdapter::new(registry, identity_provider)
    .with_aggregated_env(hub.aggregated_env().clone())
    .with_worker_connected_callback(callback);
```

> **Note on OQ-54**: The callback approach replaces the earlier "explicit
> post-hoc call" design. The callback fires inside `handle()` because
> `handle()` blocks until disconnect — there is no "after handle accepts"
> point for the assembly layer to hook into. The callback is the committed
> design; OQ-54 is resolved.

#### Outbound workers (hub dials workers)

The hub provides a `dial_worker` method that combines `CallClient::connect`,
`from_call`, and `attach_peer` into a single operation. It also provides a
`supervise_worker` method that wraps `dial_worker` in a reconnect loop with
configurable backoff:

```rust
impl Hub {
    /// Dial a worker, establish channel 0 (call protocol), discover its
    /// operations, and attach it to the aggregated env. Returns the
    /// worker's PeerId and the live CallConnection (channel 0).
    pub async fn dial_worker(
        &self,
        addr: SocketAddr,
        credentials: CallCredentials,
        config: FromCallConfig,
    ) -> Result<(PeerId, CallConnection), HubError> {
        let client = CallClient::new(
            Arc::clone(&self.registry),
            Arc::clone(&self.identity_provider),
        );
        let connection = client.connect(addr, credentials).await?;

        let peer_id = connection.identity()
            .map(|id| id.id.clone())
            .ok_or(HubError::NoPeerIdentity)?;

        let bundles = from_call(&connection, config).await?;
        connection.register_imported_all(bundles);

        self.aggregated_env
            .write()
            .expect("aggregated env lock poisoned")
            .attach_peer(peer_id.clone(), connection.overlay_env());

        Ok((peer_id, connection))
    }

    /// Detach a peer from the aggregated env and drop all channel tracking.
    /// Called on disconnect (both inbound and outbound).
    fn on_worker_disconnected(&self, peer_id: &PeerId) {
        self.aggregated_env
            .write()
            .expect("aggregated env lock poisoned")
            .detach_peer(peer_id);
        self.channels
            .write()
            .expect("channels lock poisoned")
            .remove(peer_id);
    }

    /// Supervise an outbound worker: dial, discover, attach. On disconnect,
    /// detach and retry with backoff. Runs until the Hub is dropped (the
    /// returned JoinHandle can be aborted).
    pub fn supervise_worker(
        self: &Arc<Self>,
        addr: SocketAddr,
        credentials: CallCredentials,
        config: FromCallConfig,
        backoff: BackoffConfig,
    ) -> tokio::task::JoinHandle<()> {
        let hub = Arc::clone(self);
        tokio::spawn(async move {
            let mut retries = 0;
            loop {
                match hub.dial_worker(addr, credentials.clone(), config.clone()).await {
                    Ok((peer_id, connection)) => {
                        retries = 0;
                        // Wait for the connection to drop (run_loop exits).
                        // Committed interim (OQ-52): poll the underlying
                        // Connection's accept_bi() until it returns
                        // ConnectionClosed. A CallConnection::closed()
                        // method is the target resolution.
                        loop {
                            match connection.connection() {
                                Some(conn) => {
                                    match conn.accept_bi().await {
                                        Err(StreamError::ConnectionClosed)
                                        | Err(StreamError::StreamClosed)
                                        | Err(StreamError::Timeout) => break,
                                        _ => continue,
                                    }
                                }
                                None => break,
                            }
                        }
                        hub.on_worker_disconnected(&peer_id);
                    }
                    Err(e) => {
                        tracing::warn!(?e, retries, "worker dial failed; retrying");
                    }
                }
                let delay = backoff.delay_for(retries);
                tokio::time::sleep(delay).await;
                retries += 1;
            }
        })
    }
}
```

### Channel negotiation (channels 1..N)

After channel 0 is established, either side can open additional channels via
the call protocol. The hub registers `channel/open`, `channel/close`, and
`channel/list` as built-in operations on its curated registry.

#### channel/open

Opens a new bidirectional stream on the QUIC connection, wraps it as a
`Connection` via `Connection::from_bidi`, and hands it to the
`HandlerRegistry` for the requested ALPN. The handler runs on a spawned task;
the channel is tracked until closed.

```
Request:  { "alpn": "alknet/tty", "channel": 1 }
Response: { "channel": 1, "status": "open" }
```

The `channel` field is a hint — the caller suggests a channel number. The
receiver may accept it or assign a different number. Channel 0 is reserved
for the call protocol.

The operation is symmetric: either side can call it. When the hub calls
`channel/open` on a worker, the worker opens the stream and dispatches to its
handler. When a worker calls `channel/open` on the hub, the hub opens the
stream and dispatches to its handler.

#### channel/close

Closes a previously opened channel. Drops the `Connection`, which cancels the
handler task and releases resources.

```
Request:  { "channel": 1 }
Response: { "channel": 1, "status": "closed" }
```

#### channel/list

Lists open channels on this connection. Channel 0 (call protocol) is always
present and always listed.

```
Response: { "channels": [{ "channel": 0, "alpn": "alknet/call" }, { "channel": 1, "alpn": "alknet/tty" }] }
```

#### Channel usage

`channel/open` is a call-protocol operation. The hub provides it. What the
caller does with the resulting channel is the caller's business — the hub
crate does not prescribe a specific proxying pattern.

The general mechanism: when `channel/open` is invoked, the receiver opens a
new QUIC stream on the connection, wraps it as `Connection::from_bidi(stream,
alpn, ...)`, and hands it to the `HandlerRegistry` for the requested ALPN.
The handler runs on the receiver side. The caller gets back a channel number
and can interact with the handler through the call protocol (e.g. by invoking
operations that the handler registered, or by opening a reciprocal channel).

A common pattern (but not the only one): a hub client wants a tty session on
a worker. The hub calls `channel/open { alpn: "alknet/tty" }` on the worker.
The worker opens the stream, wraps it, and hands it to `TtyAdapter::handle()`.
The hub now has a tty session running on the worker. The hub can proxy the
raw stream to the client, or expose tty operations through the call protocol,
or both. The hub crate provides the `channel/open` mechanism; the assembly
layer decides the proxying pattern.

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

### Service discovery

The hub registers the built-in service discovery operations (`services/list`,
`services/schema`, `services/list-peers`) automatically. The
`services/list-peers` handler returns each connected worker's operation list
via `PeerCompositeEnv::peer_operations` (ADR-068).

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
    #[error("channel {channel} already open on peer {peer}")]
    ChannelAlreadyOpen { peer: PeerId, channel: u32 },
    #[error("channel {channel} not found on peer {peer}")]
    ChannelNotFound { peer: PeerId, channel: u32 },
    #[error("channel {channel} open failed: {reason}")]
    ChannelOpenFailed { channel: u32, reason: String },
}
```

## What the hub does NOT do

- **Worker authentication policy.** The hub resolves the worker's identity
  via `IdentityProvider` (the existing mechanism). Whether a given identity is
  *allowed* to connect as a worker is an `AccessControl` decision on the
  assembly layer's curated ops — the hub does not add a separate worker-auth
  layer.
- **Worker-specific routing policy.** `PeerRef::Any` uses insertion-order
  first-match (ADR-029 §2). A richer `RoutingPolicy` (round-robin,
  least-loaded) is a future extension behind the same `PeerRef` enum.
- **Multi-hop federation.** The hub is one-hop: workers connect to the hub,
  the hub composes their ops. Worker A does not transitively see worker B's
  ops through the hub unless the hub explicitly re-exports them (ADR-029
  Assumption 5).
- **Worker provisioning.** The hub does not spawn workers, configure them, or
  manage their lifecycle beyond connection supervision. Worker provisioning
  is an assembly-layer concern.
- **Protocol interpretation for channels 1..N.** The hub manages channels
  (tracking, listing, open/close). It does not interpret the protocol
  running on the channel and does not proxy streams. What the caller does
  with the resulting channel is the assembly layer's business.

## Crate dependencies

```
alknet-hub
├── alknet-call (CallClient, CallAdapter, Dispatcher, PeerCompositeEnv,
│                from_call, FromCallConfig, AdapterError, ClientError)
├── alknet-core (IdentityProvider, Connection, OperationRegistry)
├── tokio (spawn, time::sleep)
└── tracing (logging)
```

`alknet-hub` depends on `alknet-call`, which depends on `alknet-core`. No new
dependency edges. The crate is a consumer of the call protocol types, not a
new protocol handler.

## Assembly layer integration

A downstream hub (alkapi) uses `alknet-hub` like this:

```rust
// 1. Build the curated registry (Layer 0)
let registry = OperationRegistryBuilder::new()
    .with_local(agent_chat_spec(), agent_chat_handler, ...)
    .with_local(services_list_spec(), services_list_handler, ...)
    .with_local(channel_open_spec(), channel_open_handler, ...)
    .with_local(channel_close_spec(), channel_close_handler, ...)
    .with_local(channel_list_spec(), channel_list_handler, ...)
    // ... other curated ops
    .build();
let registry = Arc::new(registry);

// 2. Create the hub
let hub = Arc::new(Hub::new(
    Arc::clone(&registry),
    Arc::clone(&identity_provider),
).with_ownership_provider(ownership_provider));

// 3. Start the inbound call-protocol listener (workers dial the hub)
let callback = WorkerConnectedCallback::new(Arc::clone(&hub), FromCallConfig::new());
let adapter = CallAdapter::new(Arc::clone(&registry), Arc::clone(&identity_provider))
    .with_aggregated_env(hub.aggregated_env().clone())
    .with_worker_connected_callback(callback);
// ... register adapter on the QUIC endpoint

// 4. Dial outbound workers (hub dials workers)
hub.supervise_worker(
    dev1_addr,
    dev1_credentials,
    FromCallConfig::new(),
    BackoffConfig::default(),
);

// 5. Start the HTTP listener (clients dial the hub)
// ... HttpAdapter with the same registry and identity_provider
```

The hub's `aggregated_env()` accessor returns the shared `Arc<RwLock<PeerCompositeEnv>>`
so the assembly layer can wire it into `CallAdapter::with_aggregated_env`.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| Aggregated peer-env wiring | [ADR-067](../../decisions/067-aggregated-peer-env-wiring.md) | `Dispatcher::with_aggregated_env` hook; `compose_root_env` reads shared env |
| PeerCompositeEnv::peer_operations | [ADR-068](../../decisions/068-peer-composite-env-peer-operations.md) | `list_operation_names` trait method; `PeerCompositeEnv` override |
| from_call is manual | [ADR-069](../../decisions/069-from-call-manual-free-function.md) | `from_call` is a free function; the hub calls it after connect |
| Peer-graph routing model | [ADR-029](../../decisions/029-peer-graph-routing-model.md) | Peer-keyed overlays, `PeerRef` routing, `AccessControl`-based peer auth |
| PeerEntry and Identity.id | [ADR-030](../../decisions/030-peerentry-and-identity-id-decoupling.md) | `PeerId` = `Identity.id` = `PeerEntry.peer_id` (stable) |
| Connection::from_stream / from_bidi | [ADR-065](../../decisions/065-connection-from-stream-generic-single-stream.md) | Channel streams wrap as `Connection`; any `ProtocolHandler` consumes them unchanged |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-52** (open): `CallConnection::wait_for_close()` — the supervision loop
  needs a way to await connection close. The committed interim is polling
  `connection().accept_bi()` until `ConnectionClosed`. A `closed()` method on
  `CallConnection` is the target resolution.
- **OQ-53** (open): `BackoffConfig` defaults — the committed policy is 1s
  initial, 60s max, 2x multiplier. OQ-53 tracks whether operational
  experience warrants a change before the first release.
- **OQ-54** (resolved): Inbound worker hook placement — the callback fires
  inside `CallAdapter::handle()` between identity resolution and dispatch
  start. `handle()` blocks until disconnect, so there is no "after handle
  accepts" point for the assembly layer to hook into. The
  `WorkerConnectedCallback` is the committed design.
- **OQ-55** (open): `channel/open` operation — this is a new call-protocol
  operation. It needs a spec in `alknet-call` (the operation shape, the
  stream-opening mechanism, the handler dispatch). The hub registers it as a
  built-in operation; the implementation lives in `alknet-call`. The channel
  model is committed; the operation spec is deferred to the call-protocol
  implementation phase.

## References

- [client-and-adapters.md](../call/client-and-adapters.md) — `CallClient`,
  `from_call`, `OperationAdapter`
- [call-protocol.md](../call/call-protocol.md) — `CallAdapter`, `Dispatcher`,
  `CallConnection`
- [operation-registry.md](../call/operation-registry.md) — `OperationRegistry`,
  `OperationRegistryBuilder`, `OperationEnv`
- ADR-029: Peer-Graph Routing Model
- ADR-065: `Connection::from_stream` — Generic Single-Stream Connections
- ADR-067: Aggregated Peer-Environment Wiring
- ADR-068: PeerCompositeEnv::peer_operations Override
- ADR-069: from_call Is a Manual Free Function
- alkapi [hub.md](/workspace/@alkdev/alkapi/docs/architecture/hub.md) — the
  first hub consumer, the concrete use case that informed this crate
- alkapi [ADR-011](/workspace/@alkdev/alkapi/docs/architecture/decisions/011-aggregated-peer-env.md) —
  the downstream aggregation decision
