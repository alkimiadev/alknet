---
id: call/operation-env-invoke-peer
name: Add invoke_peer/peer_contains/PeerRef to OperationEnv trait for peer-keyed routing (ADR-029 §2)
status: pending
depends_on: [call/peer-composite-env]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Add the `invoke_peer` and `peer_contains` methods to the `OperationEnv` trait,
with the `PeerRef` selector enum. Per ADR-029 §2. `PeerCompositeEnv` (built in
`call/peer-composite-env`) overrides these with real peer-keyed routing; the
default-impl preserves back-compat for single-layer envs
(`LocalOperationEnv`, connection overlays) that don't route by peer.

### PeerRef enum

```rust
pub enum PeerRef {
    Specific(PeerId),  // route to this peer; NOT_FOUND if it doesn't serve the op
    Any,               // first peer (insertion order) that serves it
}
```

`PeerRef::Specific(PeerId)` routes to the named peer's overlay only — no
fallthrough (explicit routing must be honored or fail loudly, ADR-029 §2).
`PeerRef::Any` reuses `invoke_with_policy` (the insertion-order fan-out built
in `call/peer-composite-env`).

### OperationEnv trait additions

```rust
#[async_trait]
pub trait OperationEnv: Send + Sync {
    // ... existing invoke, invoke_with_policy, contains ...

    /// Peer-routing composition (ADR-029 §2). Routes to a specific peer
    /// (`PeerRef::Specific`) or to the first peer that serves the op
    /// (`PeerRef::Any`). The default impl ignores the peer selector and
    /// delegates to `invoke_with_policy`, preserving back-compat for
    /// single-layer envs that don't route by peer. `PeerCompositeEnv`
    /// overrides with real peer-keyed routing.
    async fn invoke_peer(
        &self,
        peer: &PeerRef,
        namespace: &str,
        operation: &str,
        input: Value,
        parent: &OperationContext,
        policy: AbortPolicy,
    ) -> ResponseEnvelope {
        let _ = peer;  // unused — single-layer envs don't route by peer
        self.invoke_with_policy(namespace, operation, input, parent, policy).await
    }

    /// Does this env contain the named op *on the named peer*? Used by
    /// `PeerCompositeEnv` to probe a specific peer's sub-overlay before
    /// dispatching via `invoke_peer` with `PeerRef::Specific`. Default impl
    /// delegates to `contains` (single-layer envs ignore the peer dimension).
    fn peer_contains(&self, _peer: &PeerId, name: &str) -> bool {
        self.contains(name)
    }
}
```

### PeerCompositeEnv overrides

```rust
#[async_trait]
impl OperationEnv for PeerCompositeEnv {
    // ... invoke_with_policy, contains from call/peer-composite-env ...

    async fn invoke_peer(
        &self,
        peer: &PeerRef,
        namespace: &str,
        operation: &str,
        input: Value,
        parent: &OperationContext,
        policy: AbortPolicy,
    ) -> ResponseEnvelope {
        let name = format!("{namespace}/{operation}");
        if !parent.scoped_env.allows(&name) {
            return ResponseEnvelope::not_found(parent.request_id.clone(), &name);
        }
        match peer {
            PeerRef::Specific(peer_id) => {
                // Route to this peer's sub-overlay only. No fallthrough —
                // explicit routing must be honored or fail loudly (ADR-029 §2).
                match self.connections.get(peer_id) {
                    Some(conn_env) if conn_env.contains(&name) => {
                        conn_env.invoke_with_policy(namespace, operation, input, parent, policy).await
                    }
                    _ => ResponseEnvelope::not_found(parent.request_id.clone(), &name),
                }
            }
            PeerRef::Any => {
                // Same as invoke_with_policy: session → peers in order → base.
                self.invoke_with_policy(namespace, operation, input, parent, policy).await
            }
        }
    }

    fn peer_contains(&self, peer: &PeerId, name: &str) -> bool {
        self.connections.get(peer).map_or(false, |c| c.contains(name))
    }
}
```

### Back-compat

Existing impls (`LocalOperationEnv`, connection overlay envs) use the default
`invoke_peer` (delegates to `invoke_with_policy`, ignores peer selector) and
default `peer_contains` (delegates to `contains`). No changes needed to those
impls — the trait surface grows, the behavior is preserved.

## Acceptance Criteria

- [ ] `PeerRef` enum with `Specific(PeerId)` and `Any` variants
- [ ] `OperationEnv::invoke_peer` method with default-impl (delegates to `invoke_with_policy`)
- [ ] `OperationEnv::peer_contains` method with default-impl (delegates to `contains`)
- [ ] `PeerCompositeEnv` overrides `invoke_peer` with real peer-keyed routing
- [ ] `PeerRef::Specific` routes to named peer only (no fallthrough → NOT_FOUND if peer doesn't serve op)
- [ ] `PeerRef::Any` reuses `invoke_with_policy` (insertion-order fan-out)
- [ ] `PeerCompositeEnv` overrides `peer_contains` (checks specific peer's sub-overlay)
- [ ] Reachability check (`scoped_env.allows`) gates before peer routing
- [ ] `LocalOperationEnv` and overlay envs use default-impls (no changes)
- [ ] Unit test: `PeerRef::Specific` routes to the named peer
- [ ] Unit test: `PeerRef::Specific` → NOT_FOUND when peer doesn't serve the op (no fallthrough)
- [ ] Unit test: `PeerRef::Any` routes to first peer (insertion order) that serves it
- [ ] Unit test: `peer_contains` checks specific peer's overlay
- [ ] Unit test: default-impl `invoke_peer` delegates to `invoke_with_policy` (back-compat)
- [ ] `cargo test -p alknet-call` succeeds
- [ ] `cargo clippy -p alknet-call` succeeds with no warnings

## References

- docs/architecture/crates/call/operation-registry.md — OperationEnv, invoke_peer, PeerRef
- docs/architecture/decisions/029-peer-graph-routing-model.md — ADR-029 §2 (PeerRef routing)

## Notes

> The default-impl preserves back-compat — existing single-layer envs
> (`LocalOperationEnv`, connection overlays) work unchanged. `PeerCompositeEnv`
> overrides with real peer-keyed routing. `PeerRef::Specific` has no
> fallthrough (explicit routing must be honored or fail loudly). `PeerRef::Any`
> reuses the `invoke_with_policy` fan-out. The reachability check
> (`scoped_env.allows`) gates before peer routing, same as before.

## Summary

> To be filled on completion