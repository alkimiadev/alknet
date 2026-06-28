---
id: call/from-call-forwarded-for
name: Wire from_call forwarding handler to populate forwarded_for and use peer-keyed registration (ADR-029 §5, ADR-032)
status: pending
depends_on: [call/operation-context-forwarded-for, call/peer-composite-env]
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Update the `from_call` forwarding handler to populate `forwarded_for` on the
`call.requested` payload it constructs, and update `from_call` registration to
use the peer-keyed overlay model. Per ADR-029 §5 (peer-keyed registration,
collision rule change) and ADR-032 §3 (from_call populates forwarded_for).

### forwarded_for population (ADR-032 §3)

The hub's `from_call` forwarding handler constructs the `call.requested` payload
to send to the spoke. It populates `forwarded_for` with the end user's identity
— read from the hub's `OperationContext.identity` (the caller the hub
authenticated) when the hub forwards the call.

```rust
// In the from_call forwarding handler:
let mut payload = serde_json::json!({
    "operationId": operation_id,
    "input": input,
});
// Populate forwarded_for from the hub's context.identity (ADR-032)
if let Some(originator) = &context.identity {
    payload["forwarded_for"] = serde_json::to_value(originator).ok().unwrap_or(Value::Null);
}
// The hub authenticates as itself (its own auth_token)
if let Some(token) = &credentials.auth_token {
    payload["auth_token"] = serde_json::Value::String(token);
}
```

The hub may set `forwarded_for: None` (omit the field) if it doesn't want to
disclose the originator. The spoke receives it as metadata on its
`OperationContext` — available for logging, auditing, per-user rate limiting,
but never used by `AccessControl::check` (the spoke authorizes the hub, its
direct caller).

### Peer-keyed registration (ADR-029 §5)

`from_call` registers into the specific peer's sub-overlay (via
`CallConnection::register_imported`), not a flat overlay. Cross-peer collision
dissolves: same name on different peers is fine (separate sub-overlays, no
collision, no prefix needed). Same-peer collision stays an error (a peer
shouldn't expose two ops with the same name).

`FromCallConfig::namespace_prefix` becomes optional local-naming sugar for
when the importing node wants to expose a peer's ops under a different name
*locally* — a local-naming concern, not a disambiguation concern. It defaults
to `None`.

### Collision rule change

- **Same-peer collision** = error (a peer shouldn't expose two ops with the
  same name). `AdapterError::SamePeerCollision`.
- **Cross-peer collision** dissolves (same name on different peers is fine —
  separate sub-overlays, ADR-029 §5).

The existing `from_call` namespace-collision check (which was flat-namespace)
changes to same-peer-only. The `AdapterError::Conflict` variant (if it exists)
renames to `SamePeerCollision` (OQ-26).

### What this task does NOT do

- Does NOT build `PeerCompositeEnv` (that's `call/peer-composite-env`, a
  dependency) — `from_call` registers into the connection's overlay, which
  `PeerCompositeEnv` aggregates by peer.
- Does NOT add `forwarded_for` to `OperationContext` (that's
  `call/operation-context-forwarded-for`, a dependency) — this task populates
  the wire field.

## Acceptance Criteria

- [ ] `from_call` forwarding handler populates `forwarded_for` on the `call.requested` payload from `context.identity`
- [ ] `forwarded_for` omitted (None) when the hub chooses not to disclose the originator
- [ ] `from_call` registers into the connection's overlay (peer-keyed via `PeerCompositeEnv`)
- [ ] Same-peer collision = error (`AdapterError::SamePeerCollision`)
- [ ] Cross-peer collision dissolves (same name on different peers is fine)
- [ ] `FromCallConfig::namespace_prefix` defaults to `None` (optional local-naming sugar)
- [ ] `AdapterError::Conflict` renamed to `SamePeerCollision` (OQ-26)
- [ ] Unit test: `forwarded_for` populated from `context.identity` on forwarding
- [ ] Unit test: `forwarded_for` omitted when context.identity is None
- [ ] Unit test: same-peer collision returns `SamePeerCollision` error
- [ ] Unit test: cross-peer same name does not collide (separate sub-overlays)
- [ ] `cargo test -p alknet-call` succeeds
- [ ] `cargo clippy -p alknet-call` succeeds with no warnings

## References

- docs/architecture/crates/call/client-and-adapters.md — from_call, forwarded_for, namespace collision
- docs/architecture/decisions/029-peer-graph-routing-model.md — ADR-029 §5 (peer-keyed registration, collision rule)
- docs/architecture/decisions/032-forwarded-for-identity.md — ADR-032 §3 (from_call populates forwarded_for)

## Notes

> The `from_call` handler is the hub's forwarding path. It populates
> `forwarded_for` from the hub's `context.identity` (the end user) so the spoke
> has the originator as metadata. The spoke authorizes the hub (its direct
> caller), not the end user — `AccessControl::check` never reads
> `forwarded_for`. Cross-peer collision dissolves under the peer-keyed model
> (separate sub-overlays); same-peer collision stays an error.

## Summary

> To be filled on completion