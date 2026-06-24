---
id: call/protocol/abort-cascade-wiring
name: Wire AbortCascade into CallAdapter inbound event path (ADR-016)
status: completed
depends_on: [call/protocol/abort-cascade]
scope: narrow
risk: medium
impact: component
level: implementation
---

## Description

`AbortCascade::cascade_abort` is implemented and unit-tested in
`crates/alknet-call/src/protocol/abort.rs` (20 tests cover depth-3 cascades,
mixed Call/Subscribe entries, both `AbortPolicy` variants, and the
"unknown root silently discarded" rule), but no caller invokes it.
`CallAdapter::handle_stream` (adapter.rs:210–213) only dispatches
`EVENT_REQUESTED`; an inbound `call.aborted` event is logged at `debug!`
and dropped. As a result, ADR-016's cascade is a documented property
that does not hold at runtime — when a wire client aborts a parent
request, the parent's composed descendants keep running to completion,
which is exactly the wasted-work / unwanted-side-effects case ADR-016
was written to prevent.

This task adds the missing bolt between the two halves.

### Wire visibility (read before implementing)

ADR-016 Decision 2 (decisions/016-...:220–224) and the abort-cascade
task (tasks/call/protocol/abort-cascade.md:68–74) are explicit:
**composed child request_ids are internal**. The client only sees
`call.aborted` for the root ID it sent; the server cascades internally.
So the wiring should:

1. Receive the inbound `call.aborted` for `root_request_id`.
2. Call `AbortCascade::cascade_abort(root_request_id, AbortPolicy::AbortDependents)`
   — the wire caller does not choose the policy (ADR-016 Decision 6:
   the root gets the default `AbortDependents`; `ContinueRunning` is a
   handler-level opt-in for children, not a wire field).
3. Drop each descendant's pending entry (which cancels its future via
   Rust's async drop semantics — no `call.aborted` is sent on the wire
   for descendants).
4. Drop the root's pending entry (the trigger event already came from
   the wire; the server mirrors the abort locally).

Do **not** send `call.aborted` frames back to the client for descendant
IDs — that would leak internal composition structure to the wire.

### Implementation sketch

In `CallAdapter::handle_stream` (adapter.rs:200–228), replace the
`continue` branch with a match on event type:

```rust
match envelope.r#type.as_str() {
    EVENT_REQUESTED => { /* existing dispatch path */ }
    EVENT_ABORTED => {
        let request_id = envelope.id.clone();
        let mut pending = connection.pending().lock();
        let mut cascade = AbortCascade::new(&mut pending);
        let aborted = cascade.cascade_abort(&request_id, AbortPolicy::AbortDependents);
        // also abort the root itself (cascade_abort does not touch the root)
        pending.handle_aborted(&request_id);
        if !aborted.is_empty() {
            tracing::debug!(count = aborted.len(), "abort cascade evicted descendants");
        }
    }
    other => {
        debug!(event_type = %other, id = %envelope.id, "ignoring non-requested/non-aborted event on inbound stream");
    }
}
```

`AbortCascade` already takes `&mut PendingRequestMap`; `CallConnection::pending()`
returns `&Arc<Mutex<PendingRequestMap>>` (connection.rs:53–55). Import
`AbortCascade` and `AbortPolicy` from `super::abort` and
`crate::registry::context` respectively.

### Integration test (the test that would have caught the gap)

Add an integration test in `adapter.rs`'s `tests` module that exercises
the full path:

1. Build a `CallAdapter` with a registry containing one parent op
   (`parent/run`) whose handler calls `env.invoke("child", "run", ...)`.
2. Register `child/run` in the same registry.
3. Open a stub `Connection`, construct a `CallConnection`, manually
   register a pending entry for the parent's request_id, and simulate
   a composed child by registering a second pending entry with
   `parent_request_id: Some(parent_id)`.
4. Send an `EventEnvelope::aborted(parent_id)` frame through
   `handle_stream`.
5. Assert both the parent and child entries are gone from
   `PendingRequestMap`.

The existing unit tests on `AbortCascade` cover the tree-walking logic;
this test only needs to confirm the wiring — that an inbound abort
frame actually reaches `cascade_abort`.

## Acceptance Criteria

- [ ] `CallAdapter::handle_stream` handles `EVENT_ABORTED` (not just `EVENT_REQUESTED`)
- [ ] Inbound `call.aborted` triggers `AbortCascade::cascade_abort` with `AbortPolicy::AbortDependents`
- [ ] Root request's pending entry is also removed (cascade_abort skips the root)
- [ ] No `call.aborted` frames are sent on the wire for descendant IDs (internal-only cascade)
- [ ] Non-requested, non-aborted events still log at `debug!` and continue
- [ ] Integration test: parent abort removes parent + child from `PendingRequestMap`
- [ ] Integration test: abort for unknown request_id is a no-op (no panic, no removal)
- [ ] `cargo test -p alknet-call` succeeds
- [ ] `cargo clippy -p alknet-call --all-targets` succeeds with no warnings

## References

- docs/reviews/004-post-implementation-sanity-check.md — W1 (full finding)
- docs/architecture/decisions/016-abort-cascade-for-nested-calls.md — ADR-016 (Decision 2: server-side cascade; Decision 6: policy is handler-set, not wire-set)
- docs/architecture/crates/call/call-protocol.md:497 — spec requiring the CallAdapter to walk the tree
- tasks/call/protocol/abort-cascade.md — completed task that built `AbortCascade` in isolation
- crates/alknet-call/src/protocol/abort.rs — existing `AbortCascade` impl
- crates/alknet-call/src/protocol/adapter.rs:200–228 — `handle_stream` (the site to modify)

## Notes

> The abort-cascade task (call/protocol/abort-cascade) built and tested
> `AbortCascade` but its acceptance criteria did not include wiring it
> into `CallAdapter::handle_stream`. The call-adapter task's acceptance
> criteria likewise omitted "inbound `call.aborted` triggers cascade."
> This task closes that integration gap — all the hard logic already
> exists and is tested; this task adds the ~30-line bolt and the one
> integration test that would have caught the gap.

## Summary

`handle_stream` now matches `EVENT_ABORTED` → invokes
`AbortCascade::cascade_abort` with `AbortPolicy::AbortDependents`, then
aborts the root. Non-requested/non-aborted events still log at `debug!`.
No descendant `call.aborted` frames sent on the wire. Two integration
tests: cascade removes parent + child from `PendingRequestMap`; unknown
request_id is a no-op. `cargo test -p alknet-call` (161 tests) and
clippy clean.