# ADR-055: Exit Code on a Control Chunk (the Last Chunk Before Stream Close)

## Status

Accepted

## Context

The alknet-docker POC validated exit-code propagation for the JSON
carriage path: exec with an exit code rides on a final `call.responded`
frame `{ "exitCode": N }` before `call.completed`. That works because
the JSON carriage path is the call protocol — `call.responded` and
`call.completed` exist and carry the result.

The raw-carriage path (ADR-052) has no `call.responded` and no
`call.completed` — after the negotiation frame, the stream is raw chunks.
The exit code must ride on the chunk format itself. Two options:

- **(a) Control chunk**: `{"type":"exit","code":N}` as the last control
  chunk (stream_type 3) before stream close. Clean, explicit, carries the
  code as structured data on the channel that already exists for control
  metadata.
- **(b) Final data chunk with exit code**: a special stdout chunk with an
  exit-code payload. Overloads the data channel for metadata — a client
  parsing stdout chunks would have to special-case "this stdout chunk is
  actually an exit code," conflating data and control.

The local-PTY POC (`/workspace/alknet-tty-poc`, built 2026-07-05)
validated option (a) end-to-end: the `{"type":"exit","code":N}` chunk
fires after the child is reaped (the waiter thread's
`oneshot::Receiver<i32>` resolves) and is the last control chunk before
the stream closes. The POC's `session.rs` `pump_exit` task awaits
`pty.exit_code`, serializes the result as `ControlMessage::Exit { code }`,
enqueues it as a control chunk, and the drainer writes it to the client
before the writer closes.

### Why this is a one-way door

Clients will depend on the **"exit chunk is last"** invariant: after the
exit control chunk, no more data chunks follow, and the stream closes.
This is the deterministic completion notification the docker POC
identified as the stopgap coordination property — a coordinator spawns a
process, streams its output, and gets a reliable "it exited with code N"
signal without polling or plugin state. Changing the ordering after
clients exist would break every consumer that reads stdout until the exit
chunk and then stops.

## Decision

### 1. The exit code rides on a control chunk, not a data chunk

The exit code is control metadata (the process's termination status), not
data (process output). It rides on the control channel (stream_type 3,
ADR-052) as:

```json
{"type":"exit","code":0}
```

The `code` is an `i32` (matches `std::process::ExitStatus::code()` and
Unix wait-status convention; negative values are signal-terminated, e.g.,
`-9` for SIGKILL, matching `ExitStatus::code()`'s behavior on Unix). The
chunk is the last control chunk before stream close.

### 2. The "exit chunk is last" invariant

After the `{"type":"exit","code":N}` control chunk:

- The server sends no more data chunks (stdout/stderr) and no more
  control chunks.
- The server closes the write half of the bidi stream.

A client reads stdout/stderr/control chunks until it sees the exit chunk,
records the exit code, and treats subsequent stream close as the session
end. The exit chunk is the deterministic completion signal.

### 3. The adapter owns the exit-chunk ordering, not the backend

Per ADR-053 assumption 3, the backend resolves `exit_code` (a
`BoxFuture<'static, Result<i32, TtyError>>`); the adapter awaits it,
sends the exit control chunk, and closes the stream. The backend does not
write to the wire — it produces handles; the adapter pumps. This keeps
the wire-format logic (including the "exit is last" invariant) in one
place (the adapter's session driver) and the backend focused on its
allocation target.

The adapter's session driver (see `tty-adapter.md`) runs three concurrent
pumps:

1. **stdout → client**: backend stdout → stdout chunks (and stderr chunks
   if `TtyHandle.stderr` is `Some`).
2. **client → backend**: stdin chunks → backend stdin; control chunks →
   `TtyControl::resize`/`signal`/`eof`.
3. **exit → exit chunk**: await `TtyHandle.exit_code`; on resolve, enqueue
   `{"type":"exit","code":N}` as a control chunk; after the drainer writes
   it, close the write half.

The exit-chunk task coordinates with the stdout pump: the stdout pump
completes (backend stdout EOF) before or concurrently with the exit
resolve, and the exit chunk is enqueued only after the exit resolves. The
drainer writes chunks in arrival order; the exit chunk is last because
the exit is the last thing to resolve (the child must exit before its
stdout drains, but the exit chunk is sent only after `exit_code` resolves,
which is after `Child::wait()` returns — i.e., after the child is reaped).

### 4. Error exit codes

A backend `TtyError` during `allocate()` (the PTY couldn't be allocated,
the docker exec failed to start, the SSH channel request was rejected)
is handled before the raw-carriage phase begins — the adapter sends a
JSON error response to the negotiation frame and closes the stream
without entering raw mode. See `tty-adapter.md` §"Negotiation errors".

A `TtyError` from the `exit_code` future (the child couldn't be reaped,
or the backend's wait path failed) is serialized as an exit code of `-1`
(`ControlMessage::Exit { code: -1 }`) and the stream closes. The client
treats `-1` as "the backend reported an exit error, not a real exit
code." This is a best-effort signal; a backend that cannot determine the
exit code still sends the exit chunk so the client gets the completion
notification.

## Consequences

**Positive:**

- The exit code is structured data on the control channel, not a hacky
  overload of the data channel. Clients parse it as a `ControlMessage::Exit`,
  not as a special-cased stdout chunk.
- The "exit chunk is last" invariant gives coordinators deterministic
  completion notification — the same stopgap property the docker POC
  validated for logs subscriptions. No polling, no plugin state; the
  process exiting is the signal.
- The adapter owns the ordering, so the invariant is enforced in one
  place; backends don't have to know the wire format's completion
  semantics.
- The error-exit `-1` fallback keeps the completion notification
  reliable even when the backend can't determine the real code — the
  client still knows the session ended.

**Negative:**

- The "exit chunk is last" invariant is a one-way door — clients depend on
  it. Reversing it (allowing data chunks after the exit chunk, or moving
  the exit code to a data chunk) would break every consumer. This is the
  intended commitment: the invariant is the value.
- A client that doesn't read until the exit chunk (e.g., a runner that
  cancels mid-stream by closing the write half) won't see the exit code.
  That's correct — a cancelled stream doesn't have a deterministic exit;
  the client that cancels already knows it cancelled. The exit chunk is
  for the client that reads to completion.
- The `-1` error-exit code conflates "the backend couldn't determine the
  exit" with "the process exited with code -1" (which doesn't happen on
  Unix — `ExitStatus::code()` returns `None` for signal termination, not
  -1; the POC's waiter thread sends -1 only on `wait()` failure, not on
  signal termination — signal termination sends the negative signal
  number, e.g., -9 for SIGKILL). A client that needs to distinguish
  "real exit -1" from "backend error" can't from the code alone. This is
  a documented edge case; if it becomes load-bearing, a future control
  message type (`{"type":"exit_error","message":"..."}`) can carry the
  distinction additively (the `type`-tagged enum is the extension seam
  per ADR-052).

## Door type

**One-way.** The "exit chunk is last" invariant is what clients depend
on for deterministic completion. Changing it after clients exist breaks
every consumer. The `{"type":"exit","code":N}` shape is also one-way
(clients parse it as a `ControlMessage::Exit`), though the `type`-tagged
enum (ADR-052) makes adding *new* control message types additive.

## Assumptions

1. **The child exits before its stdout fully drains, and the exit chunk
   is sent after `exit_code` resolves.** On Unix, `Child::wait()` blocks
   until the child is reaped, which happens after the child exits and its
   stdout pipe/PTY buffer drains. The POC validated this ordering: the
   reader thread sees EOF (buffer drained), the waiter thread reaps
   (exit code available), and the exit chunk is enqueued after the exit
   resolves. There is no race where stdout chunks arrive after the exit
   chunk.

2. **`exit_code` resolving implies the stdout pump is done or will be
   soon.** The adapter's session driver waits for both the stdout pump
   to complete (backend stdout EOF) and the exit to resolve before
   sending the exit chunk and closing. If a backend's stdout outlives the
   exit resolve (a hypothetical backend where the process exits but a
   buffer flush is still in flight), the adapter waits for the stdout
   pump before the exit chunk. The `TtyHandle.stderr` (if `Some`) is
   pumped concurrently with stdout and also drains before the exit chunk.

## References

- `docs/research/alknet-tty/phase-0-findings.md` DP-5 — the decision
  question this ADR resolves
- `docs/research/alknet-docker/poc-summary.md` — the JSON-carriage exit
  code path (the analog this ADR's raw-carriage path mirrors)
- `/workspace/alknet-tty-poc/src/session.rs` `pump_exit` — the reference
  implementation of the exit-chunk ordering this ADR commits
- [ADR-052](052-alknet-tty-wire-format-and-two-carriage.md) — the wire
  format (control channel, stream_type 3) this ADR's exit chunk rides on
- [ADR-053](053-ttybackend-trait-and-ttyhandle.md) — the `TtyHandle.exit_code`
  field (the `Future` the adapter awaits) this ADR's ordering consumes
- Spec: [crates/tty/tty-adapter.md](../crates/tty/tty-adapter.md) (the
  session driver that enforces the ordering)