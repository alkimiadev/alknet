---
id: call/client/from-call
name: Implement from_call adapter (discover remote ops via services/list + services/schema, register FromCall leaves)
status: completed
depends_on: [call/client/call-client]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Implement `from_call` in `src/client/from_call.rs`. This is the #2 gap — it
discovers the remote peer's `External` operations and registers them in the
connection's Layer 2 overlay as `FromCall`-provenance leaves with forwarding
handlers. The discovery mechanism (`services/list` + `services/schema`) is
already implemented in `registry/discovery.rs`; `from_call` is the
client-side consumer of that API.

### Flow (ADR-017 §3)

1. Call `services/list` on the remote → list of `External` operations.
2. Call `services/schema` for each → input/output JSON Schemas and declared
   `error_schemas` (ADR-023).
3. For each discovered op, construct a `HandlerRegistration`:
   - `spec` mirrors the remote op's name (with optional prefix), namespace,
     type, schemas, access control.
   - `handler` is a forwarding handler: sends `call.requested` through the
     `CallConnection`, awaits `call.responded` (or streams for subscriptions).
   - `provenance: FromCall`, `composition_authority: None`, `scoped_env: None`
     (leaf — ADR-022).
4. The caller registers the bundles via
   `CallConnection::register_imported_all()`.

### API

```rust
pub struct FromCallConfig {
    /// Namespace prefix applied to imported operation names. Optional —
    /// default no prefix. Collision on import is an error (DC-3, OQ-28),
    /// not last-wins.
    pub namespace_prefix: Option<String>,
    /// Optional filter — import only operations whose names match. None
    /// imports all External ops discovered via services/list.
    pub operation_filter: Option<HashSet<String>>,
}

/// Discover the remote peer's External ops and construct HandlerRegistration
/// bundles with FromCall provenance and forwarding handlers. The caller
/// registers the bundles in the connection's overlay via
/// CallConnection::register_imported_all().
pub async fn from_call(
    connection: &CallConnection,
    config: FromCallConfig,
) -> Result<Vec<HandlerRegistration>, AdapterError>;
```

### Forwarding handler

The handler captures a handle to the `CallConnection` and, on invocation:

- For a `Query`/`Mutation` op: calls `connection.call(imported_name, input)`,
  returns the `ResponseEnvelope`.
- For a `Subscription` op: calls `connection.subscribe(imported_name, input)`,
  yields each `call.responded` until `call.completed`/`call.aborted`.
- The handler's `parent_request_id` participates in the abort cascade
  (ADR-016 §6) — if the parent is aborted, the cascade reaches this handler,
  which sends `call.aborted` to the remote node; the remote node cascades to
  its own descendants. Cross-node abort is transparent.

### Re-import on reconnection (DC-2, OQ-27)

v1 default: `from_call` runs **automatically on connection establishment**.
The overlay is per-connection (Layer 2, ADR-024), so a stale overlay dies with
the connection; re-import on reconnect is naturally scoped to the new
connection. This is the right default for the runner pattern (a worker
reconnects → the hub re-discovers the worker's ops automatically). Wire the
auto-re-import into the `CallClient::connect` path (or document that the
assembly layer calls `from_call` immediately after `connect()` — pick the
cleaner integration; the auto-on-reconnect behavior is the v1 contract).

Explicit re-import via a future `CallConnection::refresh()` is additive
(OQ-27); do not implement `refresh()` in this task unless the auto-import
wiring naturally produces it.

### Namespace collision (DC-3, OQ-28)

v1 default: **optional prefix, default no prefix, collision = error**. A node
importing from two remotes that both expose `/container/exec` without
prefixes should fail loudly (return `AdapterError`) rather than silently
overwrite. The operator adds prefixes when importing from multiple sources.
Implement collision detection: if applying the (possibly empty) prefix
produces a name that already exists in the target overlay, return an error.
This matches the default-deny, explicit-allow posture (ADR-015, ADR-028).

### Provenance and visibility

`from_call`-registered operations are `Internal` by default (ADR-015) —
composition material, not directly callable from the wire. The handler that
composes them is `External`. Set `remote_safe: false` on FromCall leaves
(they're leaves — they don't expose to *their* peers; the composition
authority is `None`).

### Trust is transitive

A `from_call`-imported operation executes the remote node's code, not yours.
The scoped env (ADR-015) bounds *which* operations are reachable, not *what*
they do. `from_call` means "I trust the remote node as much as my own
handlers." This is inherent to remote composition; the spec records it, the
implementation doesn't need to enforce it beyond the scoped-env reachability
that already exists.

## Acceptance Criteria

- [ ] `src/client/from_call.rs` exists with `FromCallConfig` and `from_call`
- [ ] `from_call` calls `services/list` then `services/schema` for each op
- [ ] Each discovered op becomes a `HandlerRegistration` with `provenance: FromCall`
- [ ] Forwarding handler sends `call.requested` via `CallConnection::call`/`subscribe`
- [ ] Subscription forwarding yields until `call.completed`/`call.aborted`
- [ ] `composition_authority: None`, `scoped_env: None` for FromCall leaves
- [ ] `remote_safe: false` on FromCall leaves
- [ ] Namespace prefix applied when `config.namespace_prefix` is Some
- [ ] Collision on import (same prefixed name) returns `AdapterError`, not silent overwrite
- [ ] `operation_filter` limits which ops are imported
- [ ] Re-import runs on connection establishment (auto-on-reconnect, v1 default)
- [ ] Cross-node abort: parent abort cascades to from_call handler → sends call.aborted remote
- [ ] `from_call` returns `Result<_, AdapterError>` (the error type from OQ-26)
- [ ] Integration test: from_call populates Layer 2 overlay with remote External ops
- [ ] Integration test: forwarding handler invokes remote op and returns result
- [ ] Integration test: subscription forwarding streams remote events
- [ ] Integration test: namespace collision returns error
- [ ] Integration test: operation_filter limits imports
- [ ] `cargo test -p alknet-call` succeeds
- [ ] `cargo clippy -p alknet-call --all-targets` succeeds with no warnings

## References

- docs/architecture/crates/call/client-and-adapters.md — from_call §, re-import §, namespace collision §
- docs/architecture/decisions/017-call-protocol-client-and-adapter-contract.md — ADR-017 §3 (from_call flow), §6 (cross-node abort)
- docs/architecture/decisions/022-handler-registration-provenance-and-composition-authority.md — ADR-022 (leaf provenance, None authority/env)
- docs/architecture/decisions/024-operation-registry-layering.md — ADR-024 (Layer 2 overlay)
- docs/architecture/decisions/016-abort-cascade-for-nested-calls.md — ADR-016 §6 (cross-node cascade)
- docs/architecture/decisions/023-operation-error-schemas.md — ADR-023 (error_schemas mirrored)
- docs/research/alknet-call-completion/gap-analysis.md — DC-2, DC-3, DC-5, implementation priority #2

## Notes

> from_call is the client-side consumer of the already-implemented
> services/list + services/schema discovery API. The v1 defaults are
> auto-on-reconnect (DC-2/OQ-27) and error-on-collision (DC-3/OQ-28) — both
> two-way doors, recorded in client-and-adapters.md, revisitable without an
> ADR. The AdapterError type (DC-4/OQ-26) is shared with the
> operation-adapter-trait task — coordinate the enum shape. Cross-node abort
> is transparent via the forwarding handler's parent_request_id (ADR-016 §6).