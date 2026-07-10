---
status: draft
last_updated: 2026-07-09
---

# alknet-hub

The hub pattern as a reusable crate: peer lifecycle management, connection
supervision, and service discovery across connected peers. The call-protocol
strategy (operation import via `from_call`, routing via `PeerCompositeEnv`)
is the first and primary strategy; the core peer lifecycle is
protocol-agnostic.

## What

`alknet-hub` is a thin crate that provides the head/worker (hub-spoke) pattern
as a reusable library. It depends on `alknet-call` and `alknet-core`. It does
not introduce new types — it wires the existing `PeerCompositeEnv`,
`CallClient`, `CallAdapter`, `from_call`, and `Dispatcher` types into a
coherent hub runtime.

The hub has two layers:

1. **Core peer lifecycle** — protocol-agnostic. The hub knows which peers are
   connected, manages their connection lifecycle (dial, accept, disconnect,
   reconnect with backoff), and resolves their identities. This layer does not
   know what a peer *does* — it only knows that a peer is connected and has an
   identity.

2. **Call-protocol strategy** — the first strategy. When a call-protocol peer
   connects, the hub discovers its operations via `from_call`, registers them
   in the aggregated `PeerCompositeEnv`, and routes operation calls to the
   right peer via `PeerRef`. This is the strategy that handles the majority of
   real-world hub use cases (docker workers, agent tool dispatch, service
   composition).

Future strategies (tty, blobs, custom protocols) plug into the same core peer
lifecycle. A QUIC connection can multiplex multiple protocols over separate
streams — a single peer connection could carry both call-protocol operations
and tty sessions. The hub's core lifecycle manages the connection; each
strategy manages its own protocol-specific state on top.

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

### Core: Hub struct and peer lifecycle

The `Hub` is the central type. It owns the aggregated `PeerCompositeEnv`
(used by the call-protocol strategy), the `OperationRegistry`, and the
`Dispatcher`:

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

The `Hub` exposes builder methods for optional hooks (`with_session_source`,
`with_ownership_provider`, `with_timeout`) that delegate to the `Dispatcher`.

### Call-protocol strategy

The call-protocol strategy is what the hub *does* when a call-protocol peer
connects: discover its operations, register them, and make them routable.

#### Inbound workers (workers dial the hub)

The hub's `CallAdapter` accepts inbound connections. The hub provides an
`on_worker_connected` hook that the assembly layer wires into the accept path:

```rust
impl Hub {
    /// Called by the assembly layer when a call-protocol worker connects
    /// inbound (via CallAdapter::handle). Runs from_call, registers the
    /// bundles in the connection's overlay, and attaches the peer to the
    /// aggregated env.
    pub async fn on_worker_connected(
        &self,
        connection: &CallConnection,
        config: FromCallConfig,
    ) -> Result<PeerId, HubError> {
        let peer_id = connection.identity()
            .map(|id| id.id.clone())
            .ok_or(HubError::NoPeerIdentity)?;

        let bundles = from_call(connection, config).await?;
        connection.register_imported_all(bundles);

        // Acquire write lock on the aggregated env and attach this peer's
        // connection overlay. The lock is held only for the HashMap insert
        // (connection-rate, not call-rate).
        self.aggregated_env
            .write()
            .expect("aggregated env lock poisoned")
            .attach_peer(peer_id.clone(), connection.overlay_env());

        Ok(peer_id)
    }

    /// Called by the assembly layer when a worker disconnects (run_loop exits).
    pub fn on_worker_disconnected(&self, peer_id: &PeerId) {
        self.aggregated_env
            .write()
            .expect("aggregated env lock poisoned")
            .detach_peer(peer_id);
    }
}
```

> **Note on `expect`**: The `RwLock` is held only for the duration of the
> `HashMap` insert/remove. A poisoned lock indicates a panic in another thread
> while holding the write lock — an unrecoverable state. The `expect` message
> documents this invariant. The implementation may use a different concurrency
> primitive (e.g. `ArcSwap`) that avoids poisoning entirely; the spec
> describes the intent (acquire exclusive access, insert/remove, release),
> not the mechanism.

#### Outbound workers (hub dials workers)

The hub provides a `dial_worker` method that combines `CallClient::connect`,
`from_call`, and `attach_peer` into a single operation. It also provides a
`supervise_worker` method that wraps `dial_worker` in a reconnect loop with
configurable backoff:

```rust
impl Hub {
    /// Dial a call-protocol worker, discover its operations, and attach it
    /// to the aggregated env. Returns the worker's PeerId and the live
    /// CallConnection.
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

    /// Supervise an outbound call-protocol worker: dial, discover, attach.
    /// On disconnect, detach and retry with backoff. Runs until the
    /// Hub is dropped (the returned JoinHandle can be aborted).
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

### Extension points for future strategies

The call-protocol strategy is the first strategy. The hub's core peer
lifecycle is protocol-agnostic — it manages connections and identities
regardless of what protocol runs over them. Future strategies plug into the
same lifecycle:

- **TTY strategy**: When a peer connects that supports `alknet/tty`, the hub
  registers the peer as a terminal session provider. A tty request routes to
  the right peer based on identity, not operation name. The strategy manages
  its own routing table (peer → tty backend), separate from
  `PeerCompositeEnv`.

- **Blobs strategy**: When a peer connects that supports `alknet/blobs`, the
  hub registers the peer as a blob store provider. Blob requests route by
  content hash or peer identity.

Each strategy is additive — it registers its own connection-establish and
connection-drop hooks on the hub's core lifecycle. The strategies are
independent; a single QUIC connection can carry multiple protocols over
separate streams (QUIC multiplexing). The hub's core manages the connection;
each strategy manages its own protocol-specific state.

These strategies are not specced here. The architecture keeps the door open
by separating core lifecycle from protocol-specific strategy. The
call-protocol strategy is the first and primary strategy because it handles
the majority of real-world hub use cases.

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
- **Non-call-protocol strategies.** The tty, blobs, and other protocol
  strategies are not specced or implemented. The architecture keeps the door
  open by separating core peer lifecycle from protocol-specific strategy;
  each strategy is additive.

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
    // ... other curated ops
    .build();
let registry = Arc::new(registry);

// 2. Create the hub
let hub = Arc::new(Hub::new(
    Arc::clone(&registry),
    Arc::clone(&identity_provider),
).with_ownership_provider(ownership_provider));

// 3. Start the inbound call-protocol listener (workers dial the hub)
let adapter = CallAdapter::new(Arc::clone(&registry), Arc::clone(&identity_provider))
    .with_aggregated_env(hub.aggregated_env().clone());
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

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-52** (open): `CallConnection::wait_for_close()` — the supervision loop
  needs a way to await connection close. Today `CallConnection` exposes
  `connection()` (the underlying `Connection`) but not a "wait for run_loop
  exit" future. The committed interim is polling `connection().accept_bi()`
  in a loop until it returns `ConnectionClosed`. A `closed()` method on
  `CallConnection` (signaled by the dispatcher on `run_loop` exit) is the
  target resolution. See OQ-52.
- **OQ-53** (open): `BackoffConfig` defaults — the committed policy is 1s
  initial, 60s max, 2x multiplier. OQ-53 tracks whether operational
  experience from the alkapi deployment warrants a change before the first
  release. The `BackoffConfig` struct shape is committed; the defaults are
  the starting point.
- **OQ-54** (open): Inbound worker `on_worker_connected` hook placement —
  the committed design is the explicit approach: the assembly layer calls
  `hub.on_worker_connected()` after `CallAdapter::handle` accepts the
  connection. A `HubCallAdapter` wrapper that calls the hook automatically
  is additive if needed. See OQ-54.

## References

- [client-and-adapters.md](../call/client-and-adapters.md) — `CallClient`,
  `from_call`, `OperationAdapter`
- [call-protocol.md](../call/call-protocol.md) — `CallAdapter`, `Dispatcher`,
  `CallConnection`
- [operation-registry.md](../call/operation-registry.md) — `OperationRegistry`,
  `OperationRegistryBuilder`, `OperationEnv`
- ADR-029: Peer-Graph Routing Model
- ADR-067: Aggregated Peer-Environment Wiring
- ADR-068: PeerCompositeEnv::peer_operations Override
- ADR-069: from_call Is a Manual Free Function
- alkapi [hub.md](/workspace/@alkdev/alkapi/docs/architecture/hub.md) — the
  first hub consumer, the concrete use case that informed this crate
- alkapi [ADR-011](/workspace/@alkdev/alkapi/docs/architecture/decisions/011-aggregated-peer-env.md) —
  the downstream aggregation decision
