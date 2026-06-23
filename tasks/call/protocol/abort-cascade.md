---
id: call/protocol/abort-cascade
name: Implement abort cascade logic for nested calls (ADR-016)
status: pending
depends_on: [call/protocol/call-adapter]
scope: moderate
risk: high
impact: component
level: implementation
---

## Description

Implement the abort cascade logic in `src/protocol/abort.rs`. When a handler
composes other operations via `OperationEnv::invoke()`, it creates a call tree:
a parent request (r1) spawns children (r1-a, r1-b), which may spawn their own
children. When `call.aborted` arrives for a parent, the protocol cascades the
abort to all non-terminal descendants.

**Read ADR-016 before starting this task.**

### Call tree

The call tree is indexed by `parent_request_id` in the `PendingRequestMap`. The
root request has `parent_request_id: None`. Each composed call has
`parent_request_id: Some(parent.request_id)`.

```
r1 (root, wire call)
├── r1-a (composed by r1's handler)
│   ├── r1-a-1 (composed by r1-a's handler)
│   └── r1-a-2
└── r1-b
    └── r1-b-1
```

### Abort cascade

When `call.aborted` arrives for a parent request:

1. Find all non-terminal descendants in the tree (walk by `parent_request_id`)
2. Send `call.aborted` for each descendant
3. Cancel each descendant's future (Drop releases resources)

The CallAdapter walks the tree indexed by `parent_request_id` in
`PendingRequestMap` and sends `call.aborted` for each descendant.

### AbortPolicy

The abort policy is set on `OperationContext` and propagated through
`OperationEnv::invoke()` — the composing handler decides the child's policy,
not the wire caller.

**`AbortDependents` (default)**: aborting a request aborts everything
downstream, regardless of branch. This is the correct default because aborted
parent work has no consumer waiting for results — continuing is wasted work at
best and unwanted side effects at worst (e.g., a `bash/exec` that keeps running
after the caller stopped caring).

**`ContinueRunning` (opt-in)**: descendants that have already started continue
to completion; descendants that haven't started yet are aborted; no new
descendants start. Use for long-running work that should survive a parent's
abort (e.g., a subscription that should keep streaming).

### Wire visibility

Composed child `request_id`s are **internal** — they appear in
`PendingRequestMap` for abort-cascade indexing but are not sent as
`call.requested` to any peer. The client only sees `call.aborted` for the root
ID it sent; the server cascades internally to descendants.

The exception is `from_call` ops, which generate their own wire ID when
forwarding to the remote node (the remote node's `PendingRequestMap` indexes
it).

### Implementation

The abort cascade needs access to the `PendingRequestMap` to walk the tree.
The `CallAdapter` holds the `PendingRequestMap` (or a reference to it). The
cascade logic:

```rust
pub struct AbortCascade {
    // Access to PendingRequestMap for tree walking
    // The map indexes entries by request_id, and each entry knows its parent_request_id
    // (from OperationContext, stored when the entry was registered)
}

impl AbortCascade {
    /// Cascade an abort from the given request ID to all non-terminal descendants.
    /// Returns the list of request IDs that were aborted (for logging/auditing).
    pub fn cascade_abort(&self, root_request_id: &str, policy: AbortPolicy) -> Vec<String>;

    /// Find all descendants of a request ID in the call tree.
    fn find_descendants(&self, parent_id: &str) -> Vec<String>;
}
```

### Storing parent_request_id in PendingRequestMap

The `PendingRequestMap` needs to know the `parent_request_id` for each entry to
walk the tree. This means `PendingEntry` needs to store the parent ID (or the
full `OperationContext`):

```rust
enum PendingEntry {
    Call {
        tx: oneshot::Sender<Result<Value, CallError>>,
        timeout: Instant,
        parent_request_id: Option<String>,  // for abort cascade tree
    },
    Subscribe {
        tx: mpsc::Sender<Result<Value, CallError>>,
        timeout: Option<Instant>,
        parent_request_id: Option<String>,  // for abort cascade tree
    },
}
```

Update the `PendingRequestMap` (from the pending-request-map task) to store
`parent_request_id` when registering entries. The `register_call` and
`register_subscribe` methods take an optional `parent_request_id` parameter.

### AbortPolicy propagation

The abort policy is propagated through `OperationEnv::invoke()`:

- `invoke()` uses the default impl, which delegates to `invoke_with_policy()`
  with `parent.abort_policy.clone()`
- `invoke_with_policy()` takes an explicit policy — use
  `AbortPolicy::ContinueRunning` for long-running work

When cascading:
- `AbortDependents`: abort ALL descendants (started and unstarted)
- `ContinueRunning`: abort only unstarted descendants; started ones continue to
  completion; no new descendants start

Determining "started" vs "unstarted" is tricky. A practical approach:
- A descendant is "started" if its handler has begun executing (the future has
  been polled at least once)
- A descendant is "unstarted" if it's queued but not yet dispatched

This may require tracking dispatch state in `PendingEntry`. A simpler
approximation: under `ContinueRunning`, abort all descendants that haven't sent
a `call.responded` yet (they're still pending). This is conservative but safe.

### Handler cleanup

Handlers clean up resources when their call is cancelled. In Rust, the future
is dropped and `Drop` guards release resources (HTTP streams, file handles,
locks). This is a handler-level concern; the protocol's job is to cascade the
abort. See ADR-016.

## Acceptance Criteria

- [ ] `PendingEntry` stores `parent_request_id` (Call and Subscribe variants)
- [ ] `register_call` and `register_subscribe` accept optional `parent_request_id`
- [ ] `AbortCascade` struct with `cascade_abort()` method
- [ ] `cascade_abort` walks the tree by `parent_request_id`
- [ ] `AbortDependents`: aborts ALL descendants (started and unstarted)
- [ ] `ContinueRunning`: aborts unstarted descendants, started ones continue
- [ ] `cascade_abort` returns list of aborted request IDs
- [ ] `call.aborted` for unknown request_id is silently discarded
- [ ] Composed child request_ids are internal (not sent as call.requested to peer)
- [ ] Client only sees call.aborted for the root ID it sent
- [ ] AbortPolicy propagated through OperationEnv::invoke()
- [ ] Unit test: cascade aborts all descendants under AbortDependents
- [ ] Unit test: cascade aborts only unstarted under ContinueRunning
- [ ] Unit test: unknown request_id → no-op (silently discarded)
- [ ] Unit test: tree with depth 3, abort root → all descendants aborted
- [ ] `cargo test -p alknet-call` succeeds
- [ ] `cargo clippy -p alknet-call` succeeds with no warnings

## References

- docs/architecture/decisions/016-abort-cascade-for-nested-calls.md — ADR-016 (full rationale)
- docs/architecture/crates/call/call-protocol.md — Abort Cascade and Nested Calls section
- docs/architecture/crates/call/operation-registry.md — AbortPolicy, OperationContext.abort_policy

## Notes

> **Read ADR-016 before starting.** The abort cascade walks the call tree
> indexed by parent_request_id in PendingRequestMap. The default policy
> (AbortDependents) aborts everything downstream — this is correct because
> aborted parent work has no consumer. ContinueRunning is the opt-in for
> long-running work. Composed child request_ids are internal — the client only
> sees call.aborted for the root ID. The PendingRequestMap needs to store
> parent_request_id for tree walking — update the pending-request-map task's
> output if needed.

## Summary

> To be filled on completion