---
id: call/protocol/pending-request-map
name: Implement PendingRequestMap for correlating call.requested and call.responded events
status: pending
depends_on: [call/protocol/wire-types]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Implement `PendingRequestMap` in `src/protocol/pending.rs`. This manages
in-flight calls and subscriptions, correlating `call.responded` events back to
the original `call.requested` by request ID.

### PendingRequestMap

```rust
pub struct PendingRequestMap {
    pending: HashMap<String, PendingEntry>,
}

enum PendingEntry {
    Call {
        tx: oneshot::Sender<Result<Value, CallError>>,
        timeout: Instant,
    },
    Subscribe {
        tx: mpsc::Sender<Result<Value, CallError>>,
        timeout: Option<Instant>,
    },
}
```

### Behavior

When a `call.responded` event arrives:
- If `PendingEntry::Call` → resolve the oneshot, delete entry
- If `PendingEntry::Subscribe` → push to the mpsc channel, keep entry alive

When `call.completed` arrives on a subscription → close the mpsc channel, delete entry.

When `call.aborted` arrives → cancel/drop whichever side initiated it. A
`call.aborted` for an unknown `requestId` is silently discarded.

When `call.error` arrives → resolve the oneshot (Call) or push to channel
(Subscribe) with the error, delete entry.

### Timeouts

Timeouts prevent dangling entries. A background task sweeps expired entries
periodically (every 10 seconds per call-protocol.md).

- `Call` entries have a timeout (default 30s from CallAdapter.default_timeout)
- `Subscribe` entries may have `timeout: None` (unbounded — long-running
  subscriptions)

When the sweeper finds an expired entry:
- `Call`: resolve oneshot with `CallError { code: "TIMEOUT", retryable: true }`, delete
- `Subscribe`: close mpsc channel with a timeout error, delete

### Methods

```rust
impl PendingRequestMap {
    pub fn new() -> Self;

    /// Register a pending call. Returns a oneshot receiver for the result.
    pub fn register_call(&mut self, request_id: String, timeout: Instant) -> oneshot::Receiver<Result<Value, CallError>>;

    /// Register a pending subscription. Returns an mpsc receiver for the stream.
    pub fn register_subscribe(&mut self, request_id: String, timeout: Option<Instant>) -> mpsc::Receiver<Result<Value, CallError>>;

    /// Handle an incoming call.responded event.
    /// Returns true if the entry was found and handled.
    pub fn handle_responded(&mut self, request_id: &str, output: Value) -> bool;

    /// Handle an incoming call.completed event (subscriptions only).
    /// Closes the mpsc channel, deletes entry.
    pub fn handle_completed(&mut self, request_id: &str) -> bool;

    /// Handle an incoming call.aborted event.
    /// Cancels the pending request, deletes entry.
    pub fn handle_aborted(&mut self, request_id: &str) -> bool;

    /// Handle an incoming call.error event.
    /// Resolves with the error, deletes entry.
    pub fn handle_error(&mut self, request_id: &str, error: CallError) -> bool;

    /// Sweep expired entries. Called periodically by a background task.
    pub fn evict_expired(&mut self) -> Vec<String>;  // returns evicted request IDs

    /// Fail all pending requests (connection closed). Returns the request IDs that were failed.
    pub fn fail_all(&mut self, error: CallError) -> Vec<String>;

    /// Check if a request ID is pending.
    pub fn contains(&self, request_id: &str) -> bool;

    /// Number of pending entries.
    pub fn len(&self) -> usize;
}
```

### Connection drop handling

When the QUIC connection closes, all pending requests are failed with
`call.error` code `INTERNAL` and message `"connection closed"`. All
subscription channels are closed. This is `fail_all()`.

### Stream reset handling

When a QUIC stream is reset mid-operation, the `FrameFramedReader` returns an
error. If the stream was carrying a subscription, the PendingRequestMap entry
is removed and the mpsc channel is closed. If the stream was carrying a call,
the oneshot is resolved with an error. No `call.aborted` is sent — the stream
is gone.

### Correlation is by ID, not by stream

A response arriving on stream N can fulfill a request sent on stream M. The
`PendingRequestMap` is keyed by ID, not by stream. This is the stream-agnostic
correlation property from ADR-012.

## Acceptance Criteria

- [ ] `PendingRequestMap` struct with pending HashMap
- [ ] `PendingEntry::Call` with oneshot::Sender and timeout
- [ ] `PendingEntry::Subscribe` with mpsc::Sender and optional timeout
- [ ] `register_call` returns oneshot::Receiver
- [ ] `register_subscribe` returns mpsc::Receiver
- [ ] `handle_responded` resolves Call oneshot, pushes to Subscribe channel
- [ ] `handle_completed` closes Subscribe mpsc, deletes entry
- [ ] `handle_aborted` cancels pending, deletes entry
- [ ] `handle_error` resolves with error, deletes entry
- [ ] Unknown request_id in handle_* is silently discarded (returns false)
- [ ] `evict_expired` removes timed-out entries, resolves with TIMEOUT error
- [ ] `fail_all` fails all pending with given error (connection close)
- [ ] Correlation is by request ID, not by stream
- [ ] Unit test: register call, handle_responded → oneshot resolves
- [ ] Unit test: register subscribe, handle multiple responded, handle_completed → stream ends
- [ ] Unit test: expired call → evict_expired resolves with TIMEOUT
- [ ] Unit test: fail_all resolves all pending with INTERNAL error
- [ ] Unit test: unknown request_id handle_responded → false (silently discarded)
- [ ] `cargo test -p alknet-call` succeeds
- [ ] `cargo clippy -p alknet-call` succeeds with no warnings

## References

- docs/architecture/crates/call/call-protocol.md — PendingRequestMap section
- docs/architecture/decisions/012-call-protocol-stream-model.md — ADR-012 (ID-based correlation)

## Notes

> Correlation is by request ID, not by stream — a response on stream N can
> fulfill a request sent on stream M. This is the stream-agnostic property from
> ADR-012. The sweeper runs every 10 seconds to evict expired entries. Unknown
> request IDs in handle_* are silently discarded (not an error — the entry may
> have already been resolved/cleaned up).

## Summary

> To be filled on completion