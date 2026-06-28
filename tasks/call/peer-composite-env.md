---
id: call/peer-composite-env
name: Replace CompositeOperationEnv with PeerCompositeEnv (peer-keyed overlays) and PeerId from Identity.id (ADR-029/030)
status: completed
depends_on: [call/retire-remote-safe]
scope: broad
risk: high
impact: phase
level: implementation
---

## Description

Replace `CompositeOperationEnv` (singular `connection: Option<Arc<dyn
OperationEnv>>`) with `PeerCompositeEnv` (peer-keyed `HashMap<PeerId,
connection_overlay>`), and change `PeerId` from a connection-assigned UUID to
`Identity.id` from `IdentityProvider` resolution (= `PeerEntry.peer_id`,
stable across key rotation). Per ADR-029 §1 and ADR-030 §4-5.

This is the highest-risk call task — a structural rewrite of the composition
env that aggregates multiple connections. The singular-connection case (one
peer) is the degenerate case with a single-entry map.

### PeerCompositeEnv struct

```rust
pub struct PeerCompositeEnv {
    pub base: Arc<dyn OperationEnv + Send + Sync>,       // Layer 0 curated
    pub session: Option<Arc<dyn OperationEnv + Send + Sync>>,  // Layer 1
    pub connections: HashMap<PeerId, Arc<dyn OperationEnv + Send + Sync>>,  // Layer 2, peer-keyed
    connection_order: Vec<PeerId>,  // insertion order for PeerRef::Any first-match
}

pub type PeerId = String;  // = Identity.id from IdentityProvider resolution
                           // = PeerEntry.peer_id (stable, not crypto material — ADR-030)
```

### PeerCompositeEnv methods

```rust
impl PeerCompositeEnv {
    pub fn new(base: Arc<dyn OperationEnv + Send + Sync>) -> Self;

    pub fn with_session(mut self, session: Arc<dyn OperationEnv + Send + Sync>) -> Self;

    /// Attach a peer's connection overlay. The `peer_id` comes from
    /// `connection.identity().id` (IdentityProvider resolution). A connection
    /// with no resolved identity has no PeerId and is NOT attached (ADR-030 §5).
    pub fn attach_peer(&mut self, peer_id: PeerId, overlay: Arc<dyn OperationEnv + Send + Sync>);

    /// Detach a peer's overlay (on disconnect). The peer's sub-overlay drops;
    /// in-flight PeerRef::Specific(that_peer) gets NOT_FOUND.
    pub fn detach_peer(&mut self, peer_id: &PeerId);
}
```

### PeerCompositeEnv::invoke_with_policy

```rust
async fn invoke_with_policy(&self, namespace: &str, operation: &str, input: Value,
    parent: &OperationContext, policy: AbortPolicy) -> ResponseEnvelope {
    let name = format!("{namespace}/{operation}");
    if !parent.scoped_env.allows(&name) {
        return ResponseEnvelope::not_found(parent.request_id.clone(), &name);
    }
    // PeerRef::Any routing (ADR-029 §2): session → peers in insertion
    // order → curated base. First overlay that contains the op wins.
    if let Some(session) = &self.session {
        if session.contains(&name) {
            return session.invoke_with_policy(namespace, operation, input, parent, policy).await;
        }
    }
    for peer_id in &self.connection_order {
        if let Some(conn_env) = self.connections.get(peer_id) {
            if conn_env.contains(&name) {
                return conn_env.invoke_with_policy(namespace, operation, input, parent, policy).await;
            }
        }
    }
    self.base.invoke_with_policy(namespace, operation, input, parent, policy).await
}

fn contains(&self, name: &str) -> bool {
    self.session.as_ref().map_or(false, |s| s.contains(name))
        || self.connections.values().any(|c| c.contains(name))
        || self.base.contains(name)
}
```

The `invoke_peer` / `peer_contains` methods (PeerRef routing) are
`call/operation-env-invoke-peer` — this task builds the struct and the
`invoke_with_policy` / `contains` methods; the peer-routing methods are the
next task.

### compose_root_env rewrite

`Dispatcher::compose_root_env` builds a `PeerCompositeEnv` per incoming call:

```rust
fn compose_root_env(&self, connection: &CallConnection, context: &OperationContext)
    -> Arc<dyn OperationEnv + Send + Sync>
{
    let base = Arc::new(LocalOperationEnv::new(Arc::clone(&self.registry)));
    let session = self.session_source.as_ref()
        .and_then(|s| s.overlay_for(context));

    let mut env = PeerCompositeEnv::new(base);
    if let Some(session) = session {
        env = env.with_session(session);
    }
    // Attach this connection's overlay, keyed by the peer's PeerId.
    // PeerId = connection.identity().id (IdentityProvider resolution).
    // A connection with no resolved identity is NOT attached to the
    // peer-keyed overlay (ADR-030 §5) — its ops are invoked through the
    // CallConnection handle directly, not via PeerRef::Specific.
    if let Some(peer_id) = connection.connection().identity().map(|id| id.id.clone()) {
        env.attach_peer(peer_id, connection.overlay_env());
    }
    Arc::new(env)
}
```

### PeerId source: Identity.id (remove UUID workaround)

`PeerId` is `Identity.id` from `IdentityProvider` resolution — the stable
`PeerEntry.peer_id` (ADR-030 §4). The UUID workaround (ADR-029 Assumption 1's
connection-assigned UUID) is removed. A connection with no resolved identity
has no `PeerId` and is not added to `PeerCompositeEnv` (ADR-030 §5).

### Migration: CompositeOperationEnv → PeerCompositeEnv

All call sites that construct `CompositeOperationEnv::new(base, Some(conn),
session)` migrate to `PeerCompositeEnv::new(base).with_session(session).
attach_peer(peer_id, conn)`. The singular-connection case (one peer) is the
degenerate case (`connections` with one entry).

### What this task does NOT do

- Does NOT add `invoke_peer` / `peer_contains` / `PeerRef` — that's
  `call/operation-env-invoke-peer`. This task builds the struct and the
  `invoke_with_policy` (PeerRef::Any equivalent) + `contains` methods.
- Does NOT change `from_call` registration — that's
  `call/from-call-forwarded-for` (peer-keyed registration, forwarded_for).
- Does NOT change `services/list` — that's
  `call/services-list-accesscontrol-filtered`.

## Acceptance Criteria

- [ ] `PeerCompositeEnv` struct with `base`, `session`, `connections: HashMap<PeerId, ...>`, `connection_order: Vec<PeerId>`
- [ ] `PeerId = String` (= `Identity.id`, not UUID)
- [ ] `PeerCompositeEnv::new(base)`, `with_session(session)`, `attach_peer(peer_id, overlay)`, `detach_peer(peer_id)`
- [ ] `invoke_with_policy` routes: session → peers in insertion order → base (first `contains` wins)
- [ ] `contains` checks session + all connections + base
- [ ] `compose_root_env` builds `PeerCompositeEnv` per call, attaches this connection's overlay keyed by `connection.identity().id`
- [ ] Connection with no resolved identity → not attached to peer-keyed overlay (no PeerId)
- [ ] `CompositeOperationEnv` removed (all call sites migrated)
- [ ] UUID workaround removed (no connection-assigned UUID for PeerId)
- [ ] Singular-connection case works (degenerate single-entry map)
- [ ] Unit test: PeerCompositeEnv routes to session when it contains the op
- [ ] Unit test: PeerCompositeEnv routes to first peer (insertion order) that contains the op
- [ ] Unit test: PeerCompositeEnv falls through to base when no overlay contains the op
- [ ] Unit test: attach_peer + detach_peer (detach → NOT_FOUND for that peer)
- [ ] Unit test: connection with no identity → not attached
- [ ] Unit test: reachability check (scoped_env.allows) still gates before routing
- [ ] `cargo test -p alknet-call` succeeds
- [ ] `cargo clippy -p alknet-call` succeeds with no warnings

## References

- docs/architecture/crates/call/operation-registry.md — PeerCompositeEnv, OperationEnv
- docs/architecture/crates/call/call-protocol.md — compose_root_env, build_root_context
- docs/architecture/decisions/029-peer-graph-routing-model.md — ADR-029 §1 (peer-keyed overlays)
- docs/architecture/decisions/030-peerentry-and-identity-id-decoupling.md — ADR-030 §4-5 (PeerId source)

## Notes

> Highest-risk call task — structural rewrite of the composition env. The
> singular-connection case is the degenerate case (one-entry map). PeerId is
> `Identity.id` (stable `peer_id`), not a UUID — the UUID workaround is removed.
> A connection with no resolved identity gets no PeerId and is not attached
> (ADR-030 §5). The `invoke_peer` / `PeerRef` routing methods are the next
> task (`call/operation-env-invoke-peer`); this task builds the struct and the
> PeerRef::Any-equivalent routing (`invoke_with_policy`).

## Summary

Replaced CompositeOperationEnv with PeerCompositeEnv (peer-keyed HashMap<PeerId, overlay> + connection_order for insertion-order first-match routing). Added PeerId = String (= Identity.id from IdentityProvider resolution, ADR-030). Implemented new/with_session/attach_peer/detach_peer; invoke_with_policy routes session → peers in insertion order → base (first contains wins); contains aggregates session + all connections + base. Rewrote Dispatcher::compose_root_env to build PeerCompositeEnv per call, attaching this connection's overlay keyed by connection.identity().id — connections with no resolved identity are NOT attached (ADR-030 §5). Migrated all CompositeOperationEnv call sites and tests. 199 lib + 2 integration tests pass, clippy clean, fmt clean.