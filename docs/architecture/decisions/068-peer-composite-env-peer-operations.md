# ADR-068: PeerCompositeEnv::peer_operations Override

## Status

Proposed

## Context

`OperationEnv::peer_operations` (defined in `registry/env.rs:63-65`) has a
default implementation returning `Vec::new()`. `PeerCompositeEnv` overrides
`invoke_with_policy`, `contains`, `invoke_peer`, `peer_contains`, and
`peer_ids` — but does **not** override `peer_operations`. This means
`peer_operations` on a `PeerCompositeEnv` always returns an empty `Vec`.

The `services/list-peers` handler (`registry/discovery.rs:245-296`) calls
`ctx.env.peer_operations(&peer_id)` to discover what operations each peer
serves. Since `PeerCompositeEnv` does not override this, non-local peers
always show empty operation lists in the `list-peers` response. The
`peer_ids()` method correctly returns the peer IDs, but the operations for
each peer are always empty.

This is a pure gap — the `services/list-peers` handler is specced to enumerate
each peer's operations (ADR-029 §6), and the `PeerCompositeEnv` type has all
the data needed to implement it (each peer's `OverlayOperationEnv` holds a
`HashMap<String, HandlerRegistration>`). The override is one method collecting
each peer overlay's registered op names.

The alkapi project identified this as gap G.6: a hub consumer calling
`services/list-peers` gets `peers: [{peer_id: "dev1", operations: []}]` until
this is fixed.

## Decision

`PeerCompositeEnv` overrides `peer_operations` to collect the operation names
from each peer's connection overlay:

```rust
fn peer_operations(&self, peer: &PeerId) -> Vec<String> {
    match self.connections.get(peer) {
        Some(overlay) => {
            // The overlay is an OverlayOperationEnv wrapping a
            // HashMap<String, HandlerRegistration>. We need the op names.
            // Rather than adding a method to OperationEnv (which would
            // require every impl to add it), we use the existing `contains`
            // method — but that requires knowing the name to check.
            //
            // The correct approach: iterate the overlay's known names.
            // OverlayOperationEnv already has the data (the HashMap keys).
            // We add a `list_operation_names(&self) -> Vec<String>` method
            // to OperationEnv with a default returning Vec::new(), and
            // OverlayOperationEnv overrides it to return the keys.
            overlay.list_operation_names()
        }
        None => Vec::new(),
    }
}
```

### 1. `OperationEnv` gains `list_operation_names` with a default impl

```rust
fn list_operation_names(&self) -> Vec<String> {
    Vec::new()
}
```

The default returns empty — existing impls (`LocalOperationEnv`, test-only
envs) don't need to change. Only `OverlayOperationEnv` overrides it.

### 2. `OverlayOperationEnv` overrides `list_operation_names`

```rust
impl OperationEnv for OverlayOperationEnv {
    fn list_operation_names(&self) -> Vec<String> {
        self.overlay.read().keys().cloned().collect()
    }
    // ... existing impl unchanged
}
```

### 3. `PeerCompositeEnv::peer_operations` uses `list_operation_names`

The override delegates to each peer's overlay:

```rust
fn peer_operations(&self, peer: &PeerId) -> Vec<String> {
    self.connections
        .get(peer)
        .map(|overlay| overlay.list_operation_names())
        .unwrap_or_default()
}
```

### Why a new trait method instead of a different approach

Alternatives considered:

- **Add `fn operations(&self) -> Vec<String>` to `OperationEnv`**: Same
  concept, different name. `list_operation_names` is chosen to match the
  existing `list_operations` naming on `OperationRegistry`.
- **Make `peer_operations` on `PeerCompositeEnv` reach into
  `OverlayOperationEnv`'s internals**: Requires `OverlayOperationEnv` to
  expose its `HashMap` or a method. The trait method is cleaner — it keeps
  the abstraction boundary intact.
- **Have `services/list-peers` iterate `ctx.env.peer_ids()` and call
  `contains` for every known op name**: Requires knowing all possible op
  names (from the registry), which is a cross-layer coupling. The trait
  method keeps the data where it lives.

The trait method is the smallest surface change: one new method with a
default impl, one override on `OverlayOperationEnv`, one override on
`PeerCompositeEnv`. No existing code changes.

## Consequences

**Positive:**
- `services/list-peers` returns actual operation lists for each peer. A hub
  consumer calling `services/list-peers` gets `peers: [{peer_id: "dev1",
  operations: [{name: "docker/container/exec", ...}, ...]}]` — the specced
  behavior.
- The fix is small: one trait method, two overrides. No existing code paths
  change.
- The `list_operation_names` method is generally useful — any future code
  that needs to enumerate an env's operations can use it.

**Negative:**
- `OperationEnv` gains a method. The default impl preserves back-compat for
  all existing implementors. Only `OverlayOperationEnv` and
  `PeerCompositeEnv` override it.
- The `OverlayOperationEnv` override holds the `RwLock` read for the
  duration of the `keys().cloned().collect()`. This is a `Vec<String>`
  allocation — cheap for typical peer operation counts (tens, not thousands).

## Assumptions

1. **`OverlayOperationEnv`'s `RwLock<HashMap<String, HandlerRegistration>>`
   read is cheap.** The lock is held only for the `keys()` iteration and
   `collect()`. Typical peer operation counts are small (tens of ops).
2. **`list_operation_names` is the right name.** It matches the existing
   `list_operations` naming on `OperationRegistry` and avoids confusion with
   `peer_operations` (which takes a `PeerId` parameter).

## References

- ADR-029 §6: `services/list-peers` opt-in peer-attributed re-export listing
- ADR-067: Aggregated Peer-Environment Wiring (sibling hub-wiring decision)
- ADR-069: from_call Is a Manual Free Function (sibling hub-wiring decision)
- `crates/alknet-call/src/registry/env.rs:63-65` — default `peer_operations`
- `crates/alknet-call/src/registry/env.rs:155-301` — `PeerCompositeEnv`
- `crates/alknet-call/src/protocol/connection.rs:305-397` —
  `OverlayOperationEnv`
- `crates/alknet-call/src/registry/discovery.rs:245-296` —
  `services_list_peers_handler`
- alkapi gap G.6: `PeerCompositeEnv::peer_operations` unimplemented
