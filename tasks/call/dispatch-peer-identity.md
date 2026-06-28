---
id: call/dispatch-peer-identity
name: Wire dispatch_requested to resolve peer Identity and run AccessControl::check (ADR-029 §3, ADR-030 §5)
status: pending
depends_on: [call/peer-composite-env, call/retire-remote-safe]
scope: narrow
risk: medium
impact: component
level: implementation
---

## Description

Wire `dispatch_requested` to resolve the peer's `Identity` and run
`AccessControl::check(peer_identity)` as the authorization mechanism, and
ensure the `PeerId` for a connection comes from `connection.identity().id`
(IdentityProvider resolution). Per ADR-029 §3 (AccessControl-based peer
authorization) and ADR-030 §5 (PeerId from IdentityProvider resolution).

### Current state

After `call/retire-remote-safe`, the `RemoteFilter` gate is removed.
`dispatch_requested` resolves identity (connection-level + auth_token override)
and `OperationRegistry::invoke` runs `AccessControl::check`. The remaining gap
is ensuring the `PeerId` for the connection comes from `IdentityProvider`
resolution (not a UUID), and that connections with no resolved identity get no
`PeerId` (not added to `PeerCompositeEnv`).

### Target state

#### 1. PeerId from IdentityProvider resolution (ADR-030 §5)

The `PeerId` for a connection is `connection.identity().id` — the resolved
`Identity.id` from `IdentityProvider` (= `PeerEntry.peer_id`, stable). The UUID
workaround is removed (done in `call/peer-composite-env`). This task verifies
the dispatch path reads `connection.identity().id` for the peer-keyed overlay
and that a connection with no resolved identity is handled correctly.

#### 2. AccessControl::check(peer_identity) is the authorization gate

`dispatch_requested` resolves the peer's `Identity` (from the connection's TLS
fingerprint or the `auth_token` payload, via the existing `IdentityProvider`)
and `OperationRegistry::invoke` runs `AccessControl::check(peer_identity)`
against the op's `AccessControl`:

- If the op's `AccessControl` is satisfied → dispatch (capabilities populated
  from the bundle).
- If not → `FORBIDDEN` before the handler runs (capabilities never populated —
  the security property).
- If the op is `Visibility::Internal` → `NOT_FOUND` before ACL (existing
  behavior).

This is the existing `OperationRegistry::invoke` path — the `RemoteFilter` gate
(removed in `call/retire-remote-safe`) was a *parallel* gate. This task
verifies the `AccessControl::check` path is the sole authorization mechanism
and that no `remote_safe`/`trusted_peer` remnants remain.

#### 3. Connections with no resolved identity

A connection with no resolved identity (no client cert, unrecognized
fingerprint) has no `PeerId` and is not added to `PeerCompositeEnv`
(ADR-030 §5). The handler either rejects the connection or falls back to a
connection-without-peer-identity path. The dispatch path must handle this case:
- `connection.identity()` returns `None` → no `PeerId`
- The connection's ops (if any discovered via `from_call`) are invoked through
  the `CallConnection` handle directly, not via `PeerRef::Specific`

### dispatch_requested flow (post-sync)

```rust
async fn dispatch_requested(&self, connection: &Arc<CallConnection>,
    request_id: String, payload: Value) -> ResponseEnvelope {
    let operation_id = payload.get("operationId").and_then(|v| v.as_str()).unwrap_or("");
    let operation_name = Self::strip_leading_slash(operation_id).to_string();

    // No RemoteFilter gate (removed). AccessControl::check in invoke is the gate.

    let connection_identity = connection.connection().identity().cloned();
    let identity = self.resolve_identity(connection_identity, &payload);
    let forwarded_for = payload.get("forwarded_for")
        .and_then(|v| serde_json::from_value::<Identity>(v.clone()).ok());

    let input = payload.get("input").cloned().unwrap_or(Value::Null);
    let context = self.build_root_context(
        request_id.clone(), &operation_name, identity, forwarded_for, connection);

    // OperationRegistry::invoke runs AccessControl::check(identity) —
    // the sole authorization mechanism (ADR-029 §3).
    self.registry.invoke(&operation_name, input, context).await
}
```

## Acceptance Criteria

- [ ] `dispatch_requested` resolves peer `Identity` (connection-level + auth_token override)
- [ ] `OperationRegistry::invoke` runs `AccessControl::check(peer_identity)` as the sole authorization gate
- [ ] No `RemoteFilter`/`remote_safe`/`trusted_peer` remnants in dispatch
- [ ] `PeerId` for connection comes from `connection.identity().id` (not UUID)
- [ ] Connection with no resolved identity → no `PeerId`, not added to `PeerCompositeEnv`
- [ ] Op with `AccessControl::default()` dispatches to any peer
- [ ] Op with `required_scopes` → `FORBIDDEN` for unauthorized peers (capabilities never populated)
- [ ] Op with `Visibility::Internal` → `NOT_FOUND` before ACL
- [ ] `forwarded_for` extracted from payload and passed to `build_root_context`
- [ ] Unit test: authorized peer → dispatch (capabilities populated)
- [ ] Unit test: unauthorized peer → FORBIDDEN (capabilities never populated)
- [ ] Unit test: Internal op → NOT_FOUND from wire
- [ ] Unit test: connection with no identity → no PeerId
- [ ] `cargo test -p alknet-call` succeeds
- [ ] `cargo clippy -p alknet-call` succeeds with no warnings

## References

- docs/architecture/crates/call/call-protocol.md — dispatch_requested, AuthContext and Identity Resolution
- docs/architecture/crates/call/client-and-adapters.md — peer authorization via AccessControl
- docs/architecture/decisions/029-peer-graph-routing-model.md — ADR-029 §3 (AccessControl-based peer auth)
- docs/architecture/decisions/030-peerentry-and-identity-id-decoupling.md — ADR-030 §5 (PeerId from IdentityProvider)

## Notes

> The `RemoteFilter` gate (removed in `call/retire-remote-safe`) was a parallel
> authorization system. This task verifies `AccessControl::check` in
> `OperationRegistry::invoke` is the sole gate and that the `PeerId` comes from
> `IdentityProvider` resolution (not UUID). Connections with no resolved
> identity get no `PeerId` and are not in the peer-keyed overlay — their ops
> are invoked through the `CallConnection` handle directly.

## Summary

> To be filled on completion