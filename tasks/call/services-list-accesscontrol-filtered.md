---
id: call/services-list-accesscontrol-filtered
name: Filter services/list by AccessControl::check(peer_identity) and add services/list-peers opt-in (ADR-029 §6)
status: pending
depends_on: [call/retire-remote-safe]
scope: narrow
risk: medium
impact: component
level: implementation
---

## Description

Change `services/list` to filter by `AccessControl::check(calling_peer_identity)`
— the calling peer sees only ops it is authorized to call. Collapse the
`services_list_handler` / `services_list_handler_peer_scoped` split (the latter
was removed in `call/retire-remote-safe`) into a single AccessControl-filtered
handler. Add the opt-in `services/list-peers` for peer-attributed re-export
listing. Per ADR-029 §6.

### services/list (AccessControl-filtered)

The single `services_list_handler` filters by `AccessControl::check` against
the calling peer's resolved `Identity`:

```rust
pub fn services_list_handler(registry: Arc<OperationRegistry>) -> Handler {
    Arc::new(move |_input, context| {
        let registry = Arc::clone(&registry);
        Box::pin(async move {
            let calling_identity = &context.identity;
            let ops: Vec<Value> = registry.list_operations()
                .into_iter()
                .filter(|spec| {
                    // Only list ops the calling peer is authorized to call.
                    // AccessControl::check returns Allowed/Forbidden.
                    spec.access_control.check(calling_identity.as_ref()).is_allowed()
                })
                .map(|spec| serde_json::json!({
                    "name": spec.name,
                    "namespace": spec.namespace,
                    "op_type": spec.op_type,
                }))
                .collect();
            ResponseEnvelope::ok(context.request_id, serde_json::json!({ "operations": ops }))
        })
    })
}
```

- An op with `AccessControl::default()` (no restrictions) is listed to any
  peer — implicitly callable by any authenticated peer.
- An op with `required_scopes` is listed only to peers whose `Identity.scopes`
  satisfy them.
- An op with `Visibility::Internal` is never listed (excluded from
  `list_operations()` which already filters to `External`).

### services/list-peers (opt-in)

The opt-in for peer-attributed re-export listing — each peer's sub-overlay
listed with attribution, filtered by the calling peer's authorization:

```rust
pub fn services_list_peers_handler(/* ... */) -> Handler {
    // Lists ops from each peer's sub-overlay in PeerCompositeEnv,
    // attributed by peer_id, filtered by AccessControl::check(calling_identity).
    // Opt-in: the calling peer invokes this operation name explicitly.
}
```

This operation is registered alongside `services/list` and `services/schema`.
It iterates the peer-keyed overlays (via `context.env`), lists each peer's ops
with `peer_id` attribution, and filters by the calling peer's authorization.

### What this task does NOT do

- Does NOT change `services/schema` (unchanged — returns full spec for a
  named op).
- Does NOT build the `PeerCompositeEnv` (that's `call/peer-composite-env`) —
  but `services/list-peers` consumes it via `context.env`. If
  `PeerCompositeEnv` is not yet built, `services/list-peers` can be registered
  with a stub that returns empty until the env is ready, or this task can
  depend on `call/peer-composite-env`. **This task depends only on
  `call/retire-remote-safe`** so `services/list` (the AccessControl filter)
  can land independently; `services/list-peers` is implemented to consume
  `PeerCompositeEnv` via `context.env.peer_contains` (which has a default-impl
  that works even before `PeerCompositeEnv` is wired).

## Acceptance Criteria

- [ ] `services_list_handler` filters by `AccessControl::check(context.identity)`
- [ ] Op with `AccessControl::default()` listed to any peer
- [ ] Op with `required_scopes` listed only to authorized peers
- [ ] Op with `Visibility::Internal` never listed (unchanged — `list_operations` filters to External)
- [ ] `services_list_handler_peer_scoped` removed (was removed in `call/retire-remote-safe`; verify gone)
- [ ] `services/list-peers` opt-in operation added (peer-attributed, AccessControl-filtered)
- [ ] `services/schema` unchanged
- [ ] Unit test: `services/list` lists only ops the calling peer is authorized for
- [ ] Unit test: op with no restrictions listed to any peer
- [ ] Unit test: op with required_scopes hidden from unauthorized peer
- [ ] Unit test: `services/list-peers` attributes ops by peer_id
- [ ] `cargo test -p alknet-call` succeeds
- [ ] `cargo clippy -p alknet-call` succeeds with no warnings

## References

- docs/architecture/crates/call/operation-registry.md — Service Discovery
- docs/architecture/crates/call/client-and-adapters.md — services/list AccessControl-filtered
- docs/architecture/decisions/029-peer-graph-routing-model.md — ADR-029 §6

## Notes

> `services/list` semantics change: the filter is `AccessControl`-based, not
> `remote_safe`-based. An op with `AccessControl::default()` is now listed to
> any peer — this is correct (it's implicitly callable by any authenticated
> peer). Operators who relied on `remote_safe: false` to hide ops from peers
> must instead set `required_scopes` or `Visibility::Internal`. The
> `services/list-peers` opt-in is for peer-attributed re-export listing.

## Summary

> To be filled on completion