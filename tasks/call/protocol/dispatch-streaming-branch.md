---
id: call/protocol/dispatch-streaming-branch
name: Wire Dispatcher::handle_stream streaming branch (Subscription → invoke_streaming → write each → call.completed)
status: pending
depends_on: [call/registry/invoke-streaming]
scope: narrow
risk: medium
impact: component
level: implementation
---

## Description

Wire the server-side streaming dispatch branch in
`Dispatcher::handle_stream` / `dispatch_requested`. When a `call.requested`
arrives for a `Subscription` op, the dispatcher must call
`OperationRegistry::invoke_streaming()` and pump the resulting
`ResponseStream` to the wire: each `Ok(value)` → `call.responded` frame,
`Err` → `call.error` frame (terminal), natural stream end → `call.completed`
frame. This is the server-side path that makes `Subscription` operations work
end-to-end — without it, a `StreamingHandler`-registered op had no server-side
dispatch path.

This task depends on `call/registry/invoke-streaming` (which provides
`invoke_streaming()`). It adds the `op_type` branch to the dispatch path and
the stream-to-wire pump.

### The branch

`dispatch_requested` currently unconditionally calls `registry.invoke()` and
returns one `ResponseEnvelope`. It needs to know the `op_type` to branch. Two
options:

1. **Look up the registration to get `op_type` before dispatching.** The
   `build_root_context` already looks up the registration; expose `op_type`
   from it. Then branch: `Subscription` → streaming path, `Query`/`Mutation` →
   existing `invoke()` path.
2. **Return an enum from `dispatch_requested`** (`DispatchResult::Once(ResponseEnvelope)`
   | `DispatchResult::Stream(ResponseStream)`) and let `handle_stream` match on
   it for the wire-writing loop.

Pick the cleaner option. Option 1 keeps `dispatch_requested` returning a
`ResponseEnvelope` for the Once path but needs a separate streaming entry point
(e.g., `dispatch_requested_streaming` returning `ResponseStream`). Option 2
unifies the dispatch entry but changes the return type. The spec frames it as
"branches on `op_type`" in `handle_stream`, suggesting the branch lives in the
dispatch layer. Document the choice.

### Streaming dispatch path

For a `Subscription` op:

```rust
// In dispatch_requested (or a new dispatch_requested_streaming):
let context = self.build_root_context(...);
// deadline: None for subscriptions (unbounded — ADR-049 §6, call-protocol Timeouts)
// The build_root_context sets a 30s deadline; for the streaming path, set
// deadline to None AFTER construction (or pass a flag). The spec says
// "deadline: None for subscriptions (unbounded)".
let stream = self.registry.invoke_streaming(&operation_name, input, context);
stream  // ResponseStream — pumped by handle_stream
```

### handle_stream streaming pump

In `handle_stream`, after dispatching, if the result is a stream:

```rust
// Read the ResponseStream, write each envelope as an EventEnvelope frame
let mut stream = tokio_stream_into_response_stream(...);  // or use StreamExt
while let Some(envelope) = stream.next().await {
    let event: EventEnvelope = envelope.into();
    if let Err(err) = writer.write_frame(&event).await {
        warn!(error = %err, "failed to write streaming frame; closing stream");
        break;
    }
    // If the envelope was an error (Err result), the stream ends after it
    // (the StreamingHandler's contract: Err is terminal). The stream's own
    // end (None) triggers call.completed below.
}
// Natural stream end → write call.completed
let completed = EventEnvelope::completed(&request_id);
if let Err(err) = writer.write_frame(&completed).await {
    warn!(error = %err, "failed to write call.completed");
}
```

The `ResponseEnvelope → EventEnvelope` conversion (`into()`) already exists and
produces `call.responded` for `Ok` and `call.error` for `Err`. The
`call.completed` frame is written once when the stream ends naturally (not on
error — an `Err` envelope is terminal, the stream ends after it, and we do NOT
write `call.completed` after a `call.error`; the stream's `None` after an error
is not a "natural end"). Track whether the last envelope was an error to decide
whether to write `call.completed`. Alternatively, the `StreamingHandler`'s
contract is: `Err` ends the stream (the handler's stream yields the error then
`None`), so after the loop, only write `call.completed` if the stream did not
end on an error. Simplest correct approach: write `call.completed` only on
natural end (the stream returned `None` without the last item being an `Err`).
Track a `last_was_error` flag.

### deadline: None for subscriptions

`build_root_context` sets `deadline: Some(now + 30s)`. For the streaming path,
the spec says `deadline: None` (unbounded — subscriptions are long-running). Set
`context.deadline = None` after `build_root_context` for the streaming branch,
or add a parameter to `build_root_context`. The deadline bounds the
request/response call tree; a subscription has no such bound. Document this.

### Abort cascade (ADR-016)

If `call.aborted` arrives for a streaming request ID, the stream is dropped
(Rust `Drop` releases the handler's resources). The existing `handle_abort`
path already removes the pending entry and cascades. For the streaming branch,
the stream future being dropped (when the `handle_stream` task is cancelled or
the `call.aborted` is processed) releases the handler's resources via `Drop`.
No new abort code is needed — the existing `handle_abort` + the stream's `Drop`
handle it. Verify the streaming pump task is cancellable (it's a `tokio::spawn`
task; aborting the connection cancels it).

### What this task does NOT do

- **No client-side changes.** The client `CallConnection::subscribe()` already
  works (it reads `call.responded` events until `call.completed`). This task is
  server-side only.
- **No gateway changes.** `GatewayDispatch::invoke_streaming` is
  `http/gateway/invoke-streaming`.

## Acceptance Criteria

- [ ] `dispatch_requested` (or a new `dispatch_requested_streaming`) branches on
      `op_type`: `Subscription` → `invoke_streaming()`, `Query`/`Mutation` →
      `invoke()` (existing)
- [ ] `handle_stream` pumps the `ResponseStream` for the streaming branch:
      each `ResponseEnvelope` → `EventEnvelope` frame
- [ ] Natural stream end → `call.completed` frame written
- [ ] `Err` envelope → `call.error` frame written, stream ends after it (no
      `call.completed` after an error)
- [ ] `deadline: None` for the streaming branch (unbounded subscriptions)
- [ ] Abort: `call.aborted` for a streaming request drops the stream (Drop
      releases resources; existing `handle_abort` handles the pending entry)
- [ ] Existing `Query`/`Mutation` dispatch path unchanged (one
      `call.responded`/`call.error` frame, no `call.completed`)
- [ ] Unit test: `Subscription` op dispatch → multiple `call.responded` frames
      + `call.completed` on stream end
- [ ] Unit test: `Subscription` op handler yields `Err` → one `call.error`
      frame, no `call.completed` after
- [ ] Unit test: `Query` op dispatch unchanged (one frame, no `call.completed`)
- [ ] Unit test: `call.aborted` for streaming request → stream dropped
- [ ] `cargo test -p alknet-call` succeeds
- [ ] `cargo clippy -p alknet-call --all-targets` succeeds with no warnings
- [ ] `cargo fmt --check -p alknet-call` passes

## References

- docs/architecture/decisions/049-streaming-handler-for-subscriptions.md — ADR-049 §6 (server-side dispatch branches on op_type)
- docs/architecture/crates/call/call-protocol.md — §CallAdapter Stream Handling (streaming branch: invoke_streaming → write each → call.completed; deadline: None; abort cascade)
- docs/architecture/decisions/016-abort-cascade-for-nested-calls.md — ADR-016 (stream drop on abort)

## Notes

> The streaming pump is a straightforward `while let Some(envelope) = stream.next().await`
> loop — not a complex abstraction. The tricky part is the `call.completed`
> semantics: write it on natural stream end, NOT after an `Err` (which is
> terminal). Track whether the last envelope was an error. `deadline: None` for
> subscriptions is a spec requirement — the 30s request/response deadline does
> not bound a long-running subscription. The abort cascade needs no new code:
> dropping the stream future (via task cancellation or `handle_abort`) releases
> the handler's resources through Rust's `Drop`. Pick the dispatch-entry shape
> (separate streaming method vs unified enum return) and document it — the spec
> frames it as a branch in `handle_stream`, so the branch should be visible there.

## Summary

> To be filled on completion