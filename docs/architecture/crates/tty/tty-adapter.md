---
status: draft
last_updated: 2026-07-18
---

# alknet-tty — TtyAdapter and Session Lifecycle

The `TtyAdapter` is the `ProtocolHandler` for `alknet/tty`: it receives a
`Connection`, accepts bidi streams, reads the negotiation frame, selects
a `TtyBackend` (ADR-053), and pumps bytes bidirectionally for the life of
the session using the wire format (ADR-052). This document specifies the
session lifecycle, the three-pump driver, negotiation errors, the
exit-chunk ordering (ADR-055), session-cancel cleanup (ADR-056), and
access control.

## What

`TtyAdapter` implements `ProtocolHandler` (ADR-002, revised by ADR-007 to
receive a `Connection`) on ALPN `alknet/tty` (ADR-006). It holds a
`HashMap<String, Arc<dyn TtyBackend>>` populated at construction (ADR-053
§5). Its `handle()` method accepts the connection and loops
`connection.accept_bi()`, dispatching each bidi stream to a session. One
`alknet/tty` connection hosts multiple terminal sessions — one session
per bidi stream (DP-6, decided in the research; matches the call
protocol's one-operation-per-stream model).

```rust
pub struct TtyAdapter {
    /// Backends keyed by the negotiation frame's `backend` string
    /// ("local", "docker", "ssh"). Populated at construction.
    backends: Arc<HashMap<String, Arc<dyn TtyBackend>>>,
    /// Optional ownership provider (ADR-050) for terminal sessions as
    /// runtime-spawned resources. None = no resource-level ACL (scope-
    /// gate only). Wired by the assembly layer.
    ownership: Option<Arc<dyn OwnershipProvider>>,
}

#[async_trait]
impl ProtocolHandler for TtyAdapter {
    fn alpn(&self) -> &'static [u8] { b"alknet/tty" }

    async fn handle(&self, connection: Connection, auth: &AuthContext)
        -> Result<(), HandlerError>
    {
        // One connection → many sessions (one bidi stream each).
        while let Ok((send, recv)) = connection.accept_bi().await {
            let backends = self.backends.clone();
            let ownership = self.ownership.clone();
            let identity = auth.identity.clone();
            tokio::spawn(async move {
                let _ = drive_session(send, recv, backends, ownership, identity).await;
            });
        }
        Ok(())
    }
}
```

The `drive_session` function is the per-stream session driver — the
counterpart to the POC's `session::drive_session`
(`/workspace/alknet-tty-poc/src/session.rs`), generalized from the local
PTY backend to the `TtyBackend` trait.

## Why

The adapter is the place where the wire format (ADR-052), the backend
trait (ADR-053), and the exit-chunk ordering (ADR-055) come together.
Keeping these in one place — the adapter — is what makes the invariants
enforceable: the wire format's "exit chunk is last" invariant is enforced
here, not in the backends (which produce handles, not wire bytes); the
backend's `exit_code` future is awaited here, not in the backend; the
negotiation frame is parsed here, not in the backend. The adapter is
backend-agnostic; the backends are wire-format-agnostic. The inversion is
the `TtyBackend` trait.

## Architecture

### Session Lifecycle

A `alknet/tty` session on one bidi stream proceeds in three phases:

1. **Negotiation.** The adapter reads the single length-prefixed JSON
   negotiation frame from the client (ADR-052 §"Negotiation Frame"),
   parses it into `NegotiateRequest`, extracts the `backend` string,
   looks up the `TtyBackend`, and constructs `TtyParams`. If the backend
   is not registered, or the negotiation frame is malformed, the adapter
   sends a JSON error response and closes the stream (see §"Negotiation
   errors" below).

2. **Allocation.** The adapter calls `backend.allocate(&params)`, which
   returns a `TtyHandle` (ADR-053). If allocation fails (PTY couldn't be
   allocated, docker exec failed, SSH channel request rejected), the
   adapter sends a JSON error response and closes the stream.

3. **Raw carriage — the bidirectional pump.** The adapter switches to
   the raw chunk format and pumps three concurrent tasks:

   - **A. stdout → client**: backend stdout (`TtyHandle.stdout`) → stdout
     chunks (stream_type 1) to the client. If `TtyHandle.stderr` is
     `Some`, a concurrent stderr pump emits stderr chunks (stream_type 2).
     On backend stdout EOF, emit a zero-length stdout sentinel.
   - **B. client → backend**: client chunks → backend. stdin chunks
     (stream_type 0) → `TtyHandle.stdin` (via `AsyncWrite`).
     Client→server control chunks (`STREAM_CTRL_IN`, stream_type 3) →
     `ControlMessage` dispatch: `Resize` → `TtyControl::resize`, `Signal`
     → `TtyControl::signal`, `Eof` → close stdin. `Exit` on
     `STREAM_CTRL_IN` is a protocol violation (it's server→client only)
     and is ignored. `STREAM_CTRL_OUT` (stream_type 4) from the client is
     a protocol violation (it's the server→client half) and is ignored.
     On client read-half close or a zero-length stdin chunk, signal EOF
     to the backend's stdin.
   - **C. exit → exit chunk**: await `TtyHandle.exit_code`; on resolve,
     enqueue `{"type":"exit","code":N}` as a server→client control
     chunk (`STREAM_CTRL_OUT`, stream_type 4).

   A drainer task writes chunks to the client in arrival order. After the
   exit chunk is written (task C resolves and the exit chunk drains),
   the adapter closes the write half — the session ends.

This is the POC's `session::drive_session` pattern, generalized: the POC
hardcoded the local PTY backend; the adapter dispatches to any
`TtyBackend`. See `/workspace/alknet-tty-poc/src/session.rs` for the
reference implementation of the three-pump driver.

### Bidirectional Control Channel (Phase 7)

The control channel is split into two halves so it is genuinely
bidirectional on the wire:

- **`STREAM_CTRL_IN = 3`** — client→server control (`Resize`, `Signal`,
  `Eof`).
- **`STREAM_CTRL_OUT = 4`** — server→client control (`Exit`).

The adapter enforces the direction:

- An `Exit` arriving on `STREAM_CTRL_IN` is a protocol violation
  (server→client message on the client→server half) — the adapter
  ignores it (the previous single `STREAM_CONTROL = 3` could not
  distinguish the two directions, so `Exit` from the client was always
  ignored; the split makes the rejection explicit).
- A `Resize`/`Signal`/`Eof` arriving on `STREAM_CTRL_OUT` is a protocol
  violation (client→server message on the server→client half) — the
  adapter ignores it (the server never dispatches control messages it
  receives on the server→client half).
- `STREAM_CTRL_OUT` (stream_type 4) chunks written by the client are a
  protocol violation (the client should not write on the server→client
  half) — the adapter ignores them.

The exit chunk (`Exit`) is emitted on `STREAM_CTRL_OUT` (stream_type
4), not on the previous `STREAM_CONTROL = 3`. A client distinguishing
the two halves can route exit vs. control without parsing the JSON
`type` tag first. See `docs/research/alknet-crate-extraction/findings.md`
Phase 7 and `tty-wire.md` §"Control Channel".

### Negotiation Errors

If the server cannot allocate the session, it sends a JSON error response
in the same length-prefixed framing as the negotiation frame (the JSON
carriage, not the raw chunk format) and closes the stream without
entering raw mode. The error response shape:

```json
{ "error": "unknown_backend", "backend": "kubernetes" }
```

| Error | When | Shape |
|-------|------|------|
| `unknown_backend` | the `backend` string is not in the adapter's backend map | `{"error":"unknown_backend","backend":"..."}` |
| `malformed_negotiation` | the negotiation frame failed to parse as JSON or failed `NegotiateRequest` validation | `{"error":"malformed_negotiation","message":"..."}` |
| `allocate_failed` | `backend.allocate()` returned a `TtyError` | `{"error":"allocate_failed","message":"..."}` |

After sending the error response, the adapter closes the write half of
the bidi stream. The client reads the error frame and treats stream close
as the failure signal. There is no `call.error` — this is not the call
protocol; the error is a JSON response in the negotiation framing.
**Framing disambiguation (success vs error).** Both a successful
allocation (raw chunks) and a failed allocation (JSON error frame) begin
with bytes the client must read before knowing which framing applies.
The disambiguation is by the first byte: a JSON error frame's 4-byte
big-endian length prefix always starts with `0x00` (error frames MUST
be under 16 MiB — `MAX_CHUNK_LEN` — so the high byte is zero; this is
a wire-format invariant, not an assumption), while a raw chunk's first
byte is a `stream_type`. The server never sends `0` (stdin —
client→server only) or `3` (`STREAM_CTRL_IN` — client→server only), so
the server-sent set is `{1, 2, 4}` (stdout, stderr, `STREAM_CTRL_OUT`);
`0x00` is unambiguous. The client distinguishes: read the first byte; if
it is `0x00`, interpret the next 4 bytes as a big-endian length prefix
and read that many bytes as a JSON error frame; otherwise interpret it
as a `stream_type` byte and continue reading the raw chunk header. This
is a one-way-door wire-format invariant (ADR-052): error frames use the
negotiation framing (length prefix) and MUST be under 16 MiB; success
uses the raw chunk framing (stream_type byte first); the
`0x00`-as-length-prefix vs `0x00`-as-invalid-stream_type disambiguation
is what makes the two distinguishable on the wire.

### Exit-Chunk Ordering (ADR-055)

The "exit chunk is last" invariant (ADR-055) is enforced here, in the
adapter's session driver, not in the backend. The ordering:

1. The stdout pump (task A) drains the backend's stdout to EOF. The
   backend's stdout ends when the process exits and the PTY/pipe buffer
   drains (Unix `Child::wait()` blocks until the child is reaped, which
   happens after the child exits and its stdout drains — ADR-055
   assumption 1).
2. The exit task (task C) awaits `TtyHandle.exit_code`. The exit resolves
   after the child is reaped (the local backend's waiter thread calls
   `Child::wait()`; docker's `inspect_exec` after the output stream ends;
   SSH's channel close after the process exits).
3. **The adapter waits for *both* the stdout pump to complete (EOF)
   *and* `exit_code` to resolve** before enqueueing the exit chunk. If a
   backend's stdout outlives the exit resolve (a hypothetical backend
   where the process exits but a buffer flush is still in flight), the
   adapter waits for the stdout pump; the `TtyHandle.stderr` (if `Some`)
   is pumped concurrently and also drains before the exit chunk. (ADR-055
   assumption 2.)
4. After both resolve, the exit chunk (`{"type":"exit","code":N}`) is
   enqueued on the writer channel.
5. The drainer writes the exit chunk to the client.
6. The adapter closes the write half — the session ends.

A client reads stdout/stderr/control chunks until it sees the exit
chunk, records the exit code, and treats subsequent stream close as the
session end. The exit chunk is the deterministic completion signal —
the same stopgap property the docker POC validated for logs subscriptions,
now for any backend.

If `exit_code` resolves with a `TtyError` (the backend couldn't determine
the exit code), the adapter sends `{"type":"exit","code":-1}` (ADR-055
§4). The client treats `-1` as "the backend reported an exit error, not
a real exit code."

### Access Control

Terminal sessions are runtime-spawned resources per ADR-050. A
`alknet/tty` session is a resource the caller owns: the caller that
opened the session owns it; proxy to share; teardown (stream close)
revokes. The adapter's access control declares against the ADR-050 model:

- **Scope-gate at negotiation.** The adapter checks the caller's
  `identity.scopes` for the `tty:open` scope (or a deployment-configured
  scope) before allocating the session. A caller without the scope gets
  a negotiation error (`{"error":"forbidden"}`) and the stream closes.
- **Resource ownership for backend-specific resources.** Some
  backends target a pre-existing resource (a docker backend targets a
  specific container); others create their own (a local backend's
  process, an SSH backend's channel). The adapter delegates the
  resource-id extraction to the backend via
  `TtyBackend::resource_id(&params)` (ADR-053), which returns
  `None` (no pre-existing resource — the session creates its own) or
  `Some((kind, id))` (the caller must own this resource). The adapter
  checks `OwnershipProvider::owns(identity, kind, id, "tty")` if an
  ownership provider is wired and the backend returns `Some`. The
  adapter does not parse backend-specific JSON itself — the extraction
  is backend-driven, so adding a backend with a new resource shape (e.g.,
  a Kubernetes backend targeting a pod) requires no adapter change.
- **`forwarded_for` for proxied sessions.** A hub that proxies a
  terminal session to a worker carries the end user's identity as
  `forwarded_for` (ADR-032); the worker authorizes the hub (its direct
  caller), not the end user. The hub's end-user ACL is its own layer.

The tty adapter is a `ProtocolHandler`, not an `OperationSpec`-registered
operation — it doesn't go through the call protocol's
`OperationRegistry::invoke()`. The access-control shape is the adapter's
own (scope-gate + backend-driven ownership check at negotiation),
declaring against the ADR-050 model but not consuming
`OperationSpec.resource_id_path` (that field is for call-protocol
operations; the tty adapter is its own ALPN, and the resource-id
extraction is delegated to the backend via `resource_id()` rather than
a path expression). See ADR-050 §"Specifics" for the model this declares
against.

The concrete choice — the scope name (`tty:open`) and the
check-at-negotiation timing — is a **two-way-door** choice within the
one-way `TtyAdapter` shape. The scope name can be renamed (a
deployment-configured scope, not a wire-format constant). The
resource-id extraction is backend-driven via `resource_id()`, so new
backends with new resource shapes require no adapter change — the
generalization is already in place (ADR-053). No ADR is warranted for
the scope name; it is a reversible implementation choice, not an
architectural commitment.

### Connection and Stream Lifecycle

- **Connection drop**: when the QUIC connection closes, all in-flight
  sessions on that connection are cancelled. Each session's pump tasks
  are dropped (Rust `Drop`); the `TtyHandle` is dropped; the
  `exit_code` future is dropped without being driven to completion,
  which triggers the backend's cancel-cleanup — the session target is
  killed (ADR-056). For the local backend, the `exit_code` future's
  `Drop` calls `ChildKiller::kill(SIGHUP)`, the child exits, the
  waiter thread's `wait()` reaps it and exits, and the reader/writer
  threads exit on channel close. For docker/SSH backends (future), the
  `Drop` issues the backend's kill (container kill / channel close).
  See ADR-056 for the contract and the mechanism.
- **Stream reset**: when a bidi stream is reset mid-session, the
  `ChunkReader` returns a `RawError` (ConnectionClosed or Io). The pump
  tasks exit; the `TtyHandle` is dropped; the cancel-cleanup runs
  (ADR-056). No exit chunk is sent — the stream is gone, the client
  that reset it already knows.
- **Client cancel**: when the client closes the write half (or sends a
  zero-length stdin chunk / `eof` control chunk), the adapter signals
  EOF to the backend's stdin and keeps pumping stdout until the backend's
  stdout ends and the exit resolves. The session completes normally —
  the exit chunk is sent — the client just stopped sending input. This
  is NOT a cancel from the adapter's perspective (the session runs to
  completion); the cancel-cleanup (ADR-056) is not triggered. The
  cancel-cleanup is triggered only when the *adapter* drops the handle
  (connection drop, stream reset, panic), not when the client closes
  the write half.

## Constraints

- **The adapter, not the backend, owns the wire format.** Backends
  produce handles; the adapter pumps. A backend that wrote to the wire
  directly would break the "exit chunk is last" invariant (ADR-055) and
  the negotiation-error framing.
- **One session per bidi stream, multiple streams per connection.** A
  connection hosts multiple sessions (one stream each); the adapter
  spawns a `drive_session` task per accepted stream. Sessions are
  independent — one session's exit doesn't affect another.
- **Negotiation errors are JSON, not raw chunks.** The error response
  uses the negotiation framing (length-prefixed JSON), not the raw chunk
  format. The stream enters raw mode only after a successful allocation.
- **The exit chunk is the deterministic completion signal.** A client
  reading to completion sees the exit chunk and knows the process exited
  with code N. A client that cancels mid-stream (closes the write half)
  won't see the exit chunk — that's correct; a cancelled stream doesn't
  have a deterministic exit.
- **The adapter triggers backend cleanup by dropping the `TtyHandle`
  (ADR-056).** On connection drop, stream reset, or pump-task panic, the
  adapter's pump tasks are dropped, which drops the `TtyHandle`, which
  drops the `exit_code` future without driving it to completion. The
  `exit_code` future's `Drop` is the backend's cancel-cleanup path
  (kill the session target). The adapter has no separate kill method to
  call; the cleanup is wired into the `exit_code` future's `Drop` by the
  backend. A backend that returns an `exit_code` future without a
  kill-on-`Drop` guard violates the contract and will orphan processes
  on cancel. See ADR-056.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| Wire format and two-carriage model | [ADR-052](../../decisions/052-alknet-tty-wire-format-and-two-carriage.md) | The chunk codec + control channel the adapter pumps |
| `TtyBackend` trait and `TtyHandle` | [ADR-053](../../decisions/053-ttybackend-trait-and-ttyhandle.md) | The backend the adapter dispatches to; the handles the adapter pumps |
| Exit code on a control chunk | [ADR-055](../../decisions/055-exit-code-on-control-chunk.md) | The "exit chunk is last" invariant the adapter enforces |
| Backend cleanup on session cancel | [ADR-056](../../decisions/056-backend-cleanup-on-session-cancel.md) | Dropping `exit_code` future kills the session target; the adapter triggers it by dropping the `TtyHandle` on cancel |
| ALPN-based protocol dispatch | [ADR-001](../../decisions/001-alpn-protocol-dispatch.md) | `TtyAdapter` registers on `alknet/tty` |
| ProtocolHandler receives `Connection` | [ADR-007](../../decisions/007-bistream-type-definition.md) | `TtyAdapter` accepts the connection, loops `accept_bi` |
| Dynamic resource ownership | [ADR-050](../../decisions/050-dynamic-resource-ownership-for-runtime-spawned-resources.md) | Terminal sessions as runtime-spawned resources; the adapter's access-control shape |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-45** (resolved): Flow control for high-throughput stdout — no application-level windowing; QUIC per-stream flow control is the backpressure mechanism.

## References

- [ADR-052](../../decisions/052-alknet-tty-wire-format-and-two-carriage.md)
  — the wire format the adapter pumps
- [ADR-053](../../decisions/053-ttybackend-trait-and-ttyhandle.md) — the
  backend trait the adapter dispatches to
- [ADR-055](../../decisions/055-exit-code-on-control-chunk.md) — the
  exit-chunk ordering the adapter enforces
- [ADR-056](../../decisions/056-backend-cleanup-on-session-cancel.md) —
  the cancel-cleanup contract the adapter triggers by dropping the
  `TtyHandle` on session cancel
- [ADR-050](../../decisions/050-dynamic-resource-ownership-for-runtime-spawned-resources.md)
  — the ownership model the adapter's access control declares against
- [ADR-007](../../decisions/007-bistream-type-definition.md) — `Connection`,
  `accept_bi`, the handler-receives-Connection pattern
- `/workspace/alknet-tty-poc/src/session.rs` — the reference
  implementation of the three-pump session driver (hardcoded to the
  local PTY backend; the adapter generalizes to the trait)
- [tty-wire.md](tty-wire.md) — the wire format details
- [tty-backend.md](tty-backend.md) — the backend trait details