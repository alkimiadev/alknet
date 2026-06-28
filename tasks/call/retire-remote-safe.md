---
id: call/retire-remote-safe
name: Retire remote_safe/trusted_peer/RemoteFilter — peer authorization via AccessControl (ADR-029 §3)
status: pending
depends_on: [core/review-core-sync]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Remove the ADR-028 peer-authorization machinery from alknet-call. Per ADR-029
§3, peer authorization now flows through the existing `AccessControl::check
(peer_identity)` — the same mechanism that gates every other call. No
`remote_safe` flag, no `trusted_peer` bypass, no `RemoteFilter` gate.

This task is the "remove the old" step before the "build the new" (PeerCompositeEnv,
invoke_peer). Removing the ADR-028 machinery first means the new
`AccessControl`-based authorization replaces the old model rather than
coexisting.

### What to remove

1. **`HandlerRegistration.remote_safe: bool`** (`registry/registration.rs`):
   - Remove the field
   - Remove `HandlerRegistration::remote_safe()` setter
   - Remove `OperationRegistryBuilder::remote_safe()` method
   - Remove all tests asserting `remote_safe` defaults/setter

2. **`OperationRegistry::list_operations_peer_scoped`** (`registry/registration.rs`):
   - Remove the method (replaced by AccessControl-filtered `services/list` in
     `call/services-list-accesscontrol-filtered`)

3. **`services_list_handler_peer_scoped`** (`registry/discovery.rs`):
   - Remove the function (replaced by single AccessControl-filtered handler in
     `call/services-list-accesscontrol-filtered`)

4. **`RemoteFilter`** (`protocol/dispatch.rs`):
   - Remove the `RemoteFilter` struct and its `default_deny()`/`trusted()`/
     `allows()` methods
   - Remove the `remote_filter` field from `Dispatcher`
   - Remove the `RemoteFilter` parameter from `Dispatcher::new()`
   - Remove the `remote_filter.allows(registration.remote_safe)` gate in
     `dispatch_requested` (the AccessControl gate in `OperationRegistry::invoke`
     already handles authorization — this task removes the *parallel* gate)

5. **`CallClient::trusted_peer`** (`client/call_client.rs`):
   - Remove the `trusted_peer: bool` field
   - Remove `CallClient::trusted_peer()` constructor
   - Remove `CallClient::is_trusted_peer()` method
   - Remove the `RemoteFilter::trusted()`/`default_deny()` selection in
     `spawn_dispatch`
   - `CallClient::new()` stays; `spawn_dispatch` constructs `Dispatcher::new`
     without `RemoteFilter`

6. **All ADR-028 tests**:
   - Remove tests asserting `remote_safe` behavior, `trusted_peer` mode,
     `RemoteFilter` filtering, `list_operations_peer_scoped`,
     `services_list_handler_peer_scoped`
   - These tests verify the old model; the new model's tests land in the
     consuming tasks (`call/services-list-accesscontrol-filtered`,
     `call/dispatch-peer-identity`)

### Transient state

After this task, the dispatch path authorizes via `AccessControl::check` (which
`OperationRegistry::invoke` already runs) — no parallel gate. The
`PeerCompositeEnv` and `invoke_peer` are not yet built (those are
`call/peer-composite-env` and `call/operation-env-invoke-peer`), so the
composition env is still `CompositeOperationEnv` (singular connection). The
`services/list` handler is the unfiltered `services_list_handler` until
`call/services-list-accesscontrol-filtered` adds the AccessControl filter.

This transient state compiles and is correct — it's just the ADR-028 model
removed without the ADR-029 peer-keyed routing yet added. The
`AccessControl::check` gate in `OperationRegistry::invoke` is the authorization
mechanism throughout.

### ADR-029 §3 mapping (the three `remote_safe` cases)

| `remote_safe` case | Replacement (already in place via AccessControl) |
|---|---|
| Op callable by any peer (was `remote_safe: true`) | `AccessControl::default()` — no restrictions |
| Op callable only by some peers | `AccessControl { required_scopes: [...] }` — peer's `Identity.scopes` must satisfy |
| Op never callable from wire | `Visibility::Internal` — `NOT_FOUND` before ACL |

## Acceptance Criteria

- [ ] `HandlerRegistration.remote_safe` field removed
- [ ] `HandlerRegistration::remote_safe()` setter removed
- [ ] `OperationRegistryBuilder::remote_safe()` removed
- [ ] `OperationRegistry::list_operations_peer_scoped` removed
- [ ] `services_list_handler_peer_scoped` removed
- [ ] `RemoteFilter` struct removed from `protocol/dispatch.rs`
- [ ] `Dispatcher.remote_filter` field removed
- [ ] `Dispatcher::new()` no longer takes `RemoteFilter`
- [ ] `CallClient.trusted_peer` field removed
- [ ] `CallClient::trusted_peer()` constructor removed
- [ ] `CallClient::is_trusted_peer()` removed
- [ ] `dispatch_requested` no longer has the `remote_filter.allows` gate
- [ ] All ADR-028 tests removed
- [ ] No `remote_safe`/`trusted_peer`/`RemoteFilter` references remain in the crate
- [ ] `cargo test -p alknet-call` succeeds (remaining tests pass — the AccessControl gate in invoke still works)
- [ ] `cargo clippy -p alknet-call` succeeds with no warnings

## References

- docs/architecture/decisions/029-peer-graph-routing-model.md — ADR-029 §3 (retire remote_safe/trusted_peer)
- docs/architecture/crates/call/operation-registry.md — HandlerRegistration (remote_safe removed)
- docs/architecture/crates/call/client-and-adapters.md — CallClient (trusted_peer removed)

## Notes

> This is the "remove the old" step. The new model (PeerCompositeEnv,
> invoke_peer, AccessControl-filtered services/list) lands in subsequent tasks.
> The transient state after this task compiles and is correct —
> `AccessControl::check` in `OperationRegistry::invoke` is the authorization
> mechanism throughout. The ADR-028 tests are removed because they verify the
> old model; the new model's tests land in the consuming tasks.

## Summary

> To be filled on completion