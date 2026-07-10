# ADR-067: Aggregated Peer-Environment Wiring for Hub Deployments

## Status

Proposed

## Context

`Dispatcher::compose_root_env` (`protocol/dispatch.rs:134-154`) constructs a
**fresh** `PeerCompositeEnv` per call and attaches **only the current call's
own connection** as a peer overlay. It does not aggregate the hub's other live
worker connections into that call's environment.

Consequence: on a hub with N connected workers, a handler composing
`env.invoke_peer(&PeerRef::Specific("dev1"), "docker", "container/exec",
input, &ctx, policy)` from a call that arrived on an HTTP connection (or on a
*different* worker's connection) will **not** find dev1's overlay — the routing
falls through to the curated base and returns `NOT_FOUND`.

The `PeerCompositeEnv` *type* and the `invoke_peer` routing logic are built for
multi-peer aggregation (`attach_peer`/`detach_peer` with insertion-order
preservation, `PeerRef::Specific`/`Any` routing — `registry/env.rs:155-301`).
The per-call `compose_root_env` does not use that capability. The ADR-029
*model* is committed; the implementation is incomplete for the head→N-workers
case.

This is the single highest-impact gap a first hub consumer (alkapi) surfaces.
A hub is *defined* by composing ops across its connected workers. Without an
aggregated env shared across all calls, the hub pattern does not work: a call
arriving on one transport cannot reach a worker connected on another.

The alkapi project identified this as OQ-08 and committed to the aggregation
decision in their ADR-011. The decision to aggregate is made; the question is
where the wiring lives — alknet (reusable by any hub) or a hub-side wrapper
(alkapi-only). This ADR resolves that question: the wiring lives in alknet.

## Decision

### 1. `Dispatcher` gains a `with_aggregated_env` builder method

A new optional field on `Dispatcher` holds a shared aggregated
`PeerCompositeEnv`:

```rust
pub struct Dispatcher {
    pub registry: Arc<OperationRegistry>,
    pub identity_provider: Arc<dyn IdentityProvider>,
    pub session_source: Option<Arc<dyn SessionOverlaySource + Send + Sync>>,
    pub ownership_provider: Option<Arc<dyn OwnershipProvider>>,
    pub aggregated_env: Option<Arc<std::sync::RwLock<PeerCompositeEnv>>>,
    pub default_timeout: Duration,
}

impl Dispatcher {
    pub fn with_aggregated_env(
        mut self,
        env: Arc<std::sync::RwLock<PeerCompositeEnv>>,
    ) -> Self {
        self.aggregated_env = Some(env);
        self
    }
}
```

The builder method mirrors `with_session_source` and `with_ownership_provider`
— an optional hook the assembly layer wires at construction time. A deployment
that does not set an aggregated env gets today's `compose_root_env` behavior
unchanged.

### 2. `CallAdapter` gains a matching `with_aggregated_env` builder method

`CallAdapter` delegates to `Dispatcher`:

```rust
impl CallAdapter {
    pub fn with_aggregated_env(
        mut self,
        env: Arc<std::sync::RwLock<PeerCompositeEnv>>,
    ) -> Self {
        self.dispatcher = self.dispatcher.with_aggregated_env(env);
        self
    }
}
```

### 3. `compose_root_env` reads the aggregated env when set

When `aggregated_env` is `Some`, `compose_root_env` reads the shared env,
attaches the current connection's overlay as an override for the current call
only, and returns the result. When `None`, the existing per-call behavior is
preserved:

```rust
pub fn compose_root_env(
    &self,
    connection: &CallConnection,
    context: &OperationContext,
) -> Arc<dyn OperationEnv + Send + Sync> {
    let base: Arc<dyn OperationEnv + Send + Sync> =
        Arc::new(LocalOperationEnv::new(Arc::clone(&self.registry)));
    let session = self
        .session_source
        .as_ref()
        .and_then(|s| s.overlay_for(context));

    if let Some(aggregated) = &self.aggregated_env {
        // Acquire read lock on the shared aggregated env, clone it (cheap —
        // all fields are Arc), and release the lock. The clone is the
        // per-call snapshot; the lock is not held for the call duration.
        let mut env = aggregated
            .read()
            .expect("aggregated env lock poisoned")
            .clone();
        // Attach the current connection's overlay as an override for this
        // call only. The current connection's overlay is the authoritative
        // view of *that* peer; the aggregated env is the authoritative view
        // of *all other* peers. This avoids a race where the aggregated env
        // has not yet picked up a new op the current peer just registered.
        if let Some(peer_id) = connection.identity().map(|identity| identity.id.clone()) {
            env.attach_peer(peer_id, connection.overlay_env());
        }
        Arc::new(env)
    } else {
        let mut env = PeerCompositeEnv::new(base);
        if let Some(session) = session {
            env = env.with_session(session);
        }
        if let Some(peer_id) = connection.identity().map(|identity| identity.id.clone()) {
            env.attach_peer(peer_id, connection.overlay_env());
        }
        Arc::new(env)
    }
}
```

The clone of the aggregated env is cheap: `PeerCompositeEnv`'s fields are all
`Arc` (the `HashMap` values are `Arc<dyn OperationEnv>`, the `Vec` is
`Vec<PeerId>` which is a `String` clone). The `RwLock::read()` is held only
for the clone, not for the duration of the call.

### 4. The hub owns the aggregated env lifecycle

The hub (assembly layer) constructs the aggregated env once at startup:

```rust
let base: Arc<dyn OperationEnv + Send + Sync> =
    Arc::new(LocalOperationEnv::new(Arc::clone(&registry)));
let aggregated = Arc::new(RwLock::new(PeerCompositeEnv::new(base)));

let adapter = CallAdapter::new(registry, identity_provider)
    .with_aggregated_env(Arc::clone(&aggregated));
```

The hub calls `attach_peer(peer_id, overlay)` on the aggregated env on every
worker connection-establish (after `from_call` populates the overlay) and
`detach_peer(&peer_id)` on every disconnect. The write lock is held only for
the `HashMap` insert/remove — connection-rate, not call-rate.

### 5. `PeerCompositeEnv` gains `Clone`

`PeerCompositeEnv` is made `Clone` (all fields are `Arc` or `Clone` already).
This is a one-line derive addition.

## Consequences

**Positive:**
- The hub pattern works. A call arriving on any transport can reach any
  connected worker's ops via `PeerRef::Specific` or `PeerRef::Any`.
- The existing single-connection behavior is preserved. A deployment that does
  not set an aggregated env gets today's `compose_root_env` unchanged.
- The hook is additive — a new optional field, a new builder method, a branch
  in `compose_root_env`. No existing code path changes.
- The capability is reusable by any future hub, not just alkapi. The alkapi
  project's ADR-011 fallback (hub-side wrapper) is no longer needed.

**Negative:**
- A `RwLock<PeerCompositeEnv>` on the read hot path of every dispatch. The
  lock is held only for a clone (all `Arc` fields — cheap). An `ArcSwap`
  copy-on-write variant could avoid the lock on reads entirely, at the cost
  of a clone on `attach_peer`/`detach_peer` (infrequent). The `RwLock` is the
  simpler starting point; `ArcSwap` is an additive optimization.
- `PeerCompositeEnv` gains `Clone`. The derive is mechanical; all fields are
  already `Clone`.
- The hub must manage the aggregated env lifecycle (`attach_peer`/`detach_peer`
  on connection events). This is assembly-layer code, not alknet-call code.
  The hooks exist; the hub wires them.

## Assumptions

1. **`PeerCompositeEnv` clone is cheap.** All fields are `Arc` or `Clone` of
   small types (`String`, `Vec<String>`). The clone does not copy the
   operation registries or the connection overlays — it copies `Arc` pointers.
2. **The current connection's overlay is authoritative for that peer.** A call
   arriving on worker-a uses worker-a's live overlay as the view of worker-a
   (not the aggregated env's possibly-stale snapshot), and the aggregated env
   for all other peers. This avoids a race where the aggregated env has not
   yet picked up a new op worker-a just registered.
3. **The `RwLock` is not a contention point.** Reads (clones) are call-rate
   but the lock is held only for the clone duration (microseconds). Writes
   (`attach_peer`/`detach_peer`) are connection-rate (seconds to minutes). If
   profiling shows contention, `ArcSwap` is the additive optimization.

## References

- ADR-029: Peer-Graph Routing Model (the model this wiring completes)
- ADR-030: PeerEntry and Identity.id Decoupling (the `PeerId` source)
- ADR-068: PeerCompositeEnv::peer_operations Override (sibling hub-wiring decision)
- ADR-069: from_call Is a Manual Free Function (sibling hub-wiring decision)
- alkapi ADR-011: Aggregated Peer Environment (the downstream commitment)
- alkapi OQ-08: alknet aggregated peer-env wiring (the blocking question)
- `crates/alknet-call/src/protocol/dispatch.rs:134-154` — current
  `compose_root_env`
- `crates/alknet-call/src/registry/env.rs:155-301` — `PeerCompositeEnv`
