---
id: call/protocol/call-connection
name: Implement CallConnection with imported-ops overlay (Layer 2) and call/subscribe/abort methods
status: completed
depends_on: [call/protocol/pending-request-map, call/registry/operation-env]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Implement `CallConnection` in `src/protocol/connection.rs`. This represents an
established `alknet/call` connection, regardless of which side opened it
(ADR-017). It holds the connection's imported-ops overlay (Layer 2, ADR-024).

### CallConnection

```rust
pub struct CallConnection {
    connection: Connection,
    imported_operations: Arc<RwLock<HashMap<String, HandlerRegistration>>>,
}
```

An established alknet/call connection (either direction — accepted or opened).
Holds the Layer 2 overlay (imported ops from `from_call` discovery).

### Layer 2 registration API

```rust
impl CallConnection {
    /// Register an imported operation into this connection's overlay (Layer 2, ADR-024).
    /// Called by from_call after discovery.
    pub fn register_imported(&self, registration: HandlerRegistration) {
        let name = registration.spec.name.clone();
        self.imported_operations.write().insert(name, registration);
    }

    /// Register multiple imported operations (bulk variant for from_call).
    pub fn register_imported_all(&self, registrations: Vec<HandlerRegistration>) {
        let mut overlay = self.imported_operations.write();
        for reg in registrations {
            overlay.insert(reg.spec.name.clone(), reg);
        }
    }
}
```

Layer 0 (curated) is built via `OperationRegistryBuilder` at startup. Layer 2
(per-connection) registration uses `CallConnection::register_imported()` at
runtime. When the connection drops, the overlay (and all imported ops) is
dropped — no explicit deregistration needed.

### Overlay env

```rust
impl CallConnection {
    /// Build an OperationEnv impl for this connection's overlay.
    /// Used by the CallAdapter when composing the root OperationContext.env.
    /// Returns an OperationEnv that dispatches to this connection's imported ops
    /// (and reports contains only for ops in the overlay).
    pub fn overlay_env(&self) -> Arc<dyn OperationEnv + Send + Sync>;
}
```

This is an `OperationEnv` impl that dispatches to the connection's imported ops.
The `contains()` method returns true only for ops in the overlay. The
`invoke_with_policy()` method looks up the op in the overlay and dispatches to
its handler.

This env is composed into the `CompositeOperationEnv` by the CallAdapter as the
`connection` layer (Layer 2).

### Call methods (outgoing)

```rust
impl CallConnection {
    /// Call an operation on the remote peer (sends call.requested).
    pub async fn call(&self, operation_id: &str, input: Value) -> ResponseEnvelope;

    /// Subscribe to a streaming operation on the remote peer.
    pub async fn subscribe(&self, operation_id: &str, input: Value) -> impl Stream<Item = ResponseEnvelope>;

    /// Abort an in-flight request (sends call.aborted, cascades per ADR-016).
    pub async fn abort(&self, request_id: &str);
}
```

These methods:
1. Open a bidirectional stream with `connection.open_bi()`
2. Send `call.requested` on that stream (via FrameFramedWriter)
3. Add the request ID to the PendingRequestMap
4. Read responses from any stream, correlate by ID (via PendingRequestMap)

`call()` resolves on the first `call.responded`. `subscribe()` yields each
`call.responded` until `call.completed` or `call.aborted`.

`abort()` sends `call.aborted` for the given request ID. The abort cascade
(ADR-016) is handled by the abort-cascade task.

### Connection direction independence

Per ADR-017, connection direction is independent of call direction. Both
sides can call each other once connected. The `CallConnection` type is the same
whether the connection was accepted (server side) or opened (client side via
`CallClient`). The `call`/`subscribe`/`abort` methods work the same way.

### from_call integration

The `from_call` adapter (ADR-017) discovers operations on a remote call
protocol endpoint via `services/list` and `services/schema`, then registers
them with `register_imported()` / `register_imported_all()`. This makes
cross-node composition transparent — a handler calling
`env.invoke("worker", "exec", ...)` doesn't know whether the operation is
local or remote.

The `from_call` adapter itself is not implemented in this task — it's a future
task. This task implements the `CallConnection` infrastructure that `from_call`
will use.

## Acceptance Criteria

- [ ] `CallConnection` struct with connection and imported_operations fields
- [ ] `register_imported()` adds to the Layer 2 overlay
- [ ] `register_imported_all()` bulk adds to the overlay
- [ ] `overlay_env()` returns an OperationEnv dispatching to imported ops
- [ ] `overlay_env().contains()` returns true only for ops in the overlay
- [ ] `call()` sends call.requested, resolves on first call.responded
- [ ] `subscribe()` sends call.requested, yields call.responded until completed/aborted
- [ ] `abort()` sends call.aborted for the request ID
- [ ] Outgoing calls open a stream, send request, add to PendingRequestMap
- [ ] Connection drop drops the overlay (no explicit deregistration)
- [ ] Unit test: register_imported adds to overlay, contains returns true
- [ ] Unit test: overlay_env dispatches to imported op
- [ ] Unit test: overlay_env contains returns false for non-imported op
- [ ] `cargo test -p alknet-call` succeeds
- [ ] `cargo clippy -p alknet-call` succeeds with no warnings

## References

- docs/architecture/crates/call/call-protocol.md — CallConnection section
- docs/architecture/decisions/017-call-protocol-client-and-adapter-contract.md — ADR-017
- docs/architecture/decisions/024-operation-registry-layering.md — ADR-024 (Layer 2)

## Notes

> Connection direction is independent of call direction (ADR-017) — both sides
> can call each other. The Layer 2 overlay is per-connection: when the
> connection drops, the overlay drops (no deregistration needed). The
> overlay_env() is composed into CompositeOperationEnv by the CallAdapter as
> the connection layer. The from_call adapter itself is a future task — this
> implements the infrastructure it will use.

## Summary

Implemented `CallConnection` in `protocol/connection.rs` with Layer 2 imported-ops
overlay (`Arc<RwLock<HashMap>>`), `register_imported`/`register_imported_all`,
`overlay_env()` returning an `OperationEnv` that dispatches to imported ops
(`contains()` returns true only for overlay ops), and `call()`/`subscribe()`/`abort()`
methods that open a stream, send `call.requested`, register in `PendingRequestMap`,
spawn a stream reader, and correlate responses by ID. Connection drop drops the
overlay. Exposed `MockConnection` + `Connection::from_mock` in alknet-core for
cross-crate testing. Added `parking_lot` dep. 9 new connection tests (115 total in
call crate). Clippy clean. Merged to develop.