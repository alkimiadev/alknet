---
id: tty/adapter
name: Implement TtyAdapter (ProtocolHandler) and three-pump session driver
status: pending
depends_on: [tty/wire-codec, tty/control-messages, tty/negotiation, tty/backend-trait]
scope: broad
risk: high
impact: phase
level: implementation
---

## Description

Implement the `TtyAdapter` (`ProtocolHandler` on `alknet/tty`) and the
`drive_session` three-pump bidirectional driver in `src/adapter.rs`. This is
where the wire format (ADR-052), the backend trait (ADR-053), and the
exit-chunk ordering (ADR-055) come together. The adapter is backend-agnostic;
the backends are wire-format-agnostic. The inversion is the `TtyBackend` trait.

This is a generalization of the POC's `/workspace/alknet-tty-poc/src/session.rs`,
which hardcoded the local PTY backend; the adapter dispatches to any
`TtyBackend`.

### TtyAdapter struct

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

impl TtyAdapter {
    pub fn new(backends: HashMap<String, Arc<dyn TtyBackend>>) -> Self;
    pub fn with_ownership(backends: HashMap<String, Arc<dyn TtyBackend>>, ownership: Arc<dyn OwnershipProvider>) -> Self;
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

### Session lifecycle (drive_session)

A `alknet/tty` session on one bidi stream proceeds in three phases:

1. **Negotiation.** Read the single length-prefixed JSON negotiation frame
   (task `tty/negotiation`), parse into `NegotiateRequest`, extract the
   `backend` string, look up the `TtyBackend`, construct `TtyParams`. If the
   backend is not registered → `unknown_backend` error. If the frame is
   malformed or `carriage != "raw"` or `cmd` is empty → `malformed_negotiation`
   error. Send the JSON error response in negotiation framing and close the
   stream (do NOT enter raw mode).

2. **Allocation.** Call `backend.allocate(&params)`. If it fails →
   `allocate_failed` error response, close. Before allocating, run the access
   control checks (see below).

3. **Raw carriage — the bidirectional pump.** Switch to the raw chunk format
   and pump three concurrent tasks:

   - **A. stdout → client**: backend stdout (`TtyHandle.stdout`) → stdout
     chunks (stream_type 1). If `TtyHandle.stderr` is `Some`, a concurrent
     stderr pump emits stderr chunks (stream_type 2). On backend stdout EOF,
     emit a zero-length stdout sentinel.
   - **B. client → backend**: client chunks → backend. stdin chunks
     (stream_type 0) → `TtyHandle.stdin` (via `AsyncWrite`). Control chunks
     (stream_type 3) → `ControlMessage` dispatch: `Resize` →
     `TtyControl::resize`, `Signal` → `TtyControl::signal`, `Eof` → close
     stdin. `Exit` from the client is ignored (server→client only). On client
     read-half close or a zero-length stdin chunk, signal EOF to the backend's
     stdin. Unknown control `type` values are ignored (logged at debug), not
     errors.
   - **C. exit → exit chunk**: await `TtyHandle.exit_code`; on resolve, enqueue
     `{"type":"exit","code":N}` as a control chunk (stream_type 3). On
     `TtyError`, send `{"type":"exit","code":-1}` (ADR-055 §4).

   A drainer task writes chunks to the client in arrival order. After the exit
   chunk is written (task C resolves AND stdout/stderr pumps complete AND the
   exit chunk drains), the adapter closes the write half — the session ends.

### Exit-chunk ordering (ADR-055)

The "exit chunk is last" invariant is enforced here, in the adapter's session
driver, not in the backend:

1. The stdout pump (task A) drains the backend's stdout to EOF.
2. The exit task (task C) awaits `TtyHandle.exit_code`.
3. **The adapter waits for *both* the stdout pump to complete (EOF) *and*
   `exit_code` to resolve** before enqueueing the exit chunk. If `stderr` is
   `Some`, it also drains before the exit chunk. (ADR-055 assumption 2.)
4. After both resolve, the exit chunk is enqueued on the writer channel.
5. The drainer writes the exit chunk to the client.
6. The adapter closes the write half — the session ends.

Use a coordination primitive (e.g., `tokio::join!` on the stdout pump and the
exit future, or a barrier) to enforce the "both done before exit chunk" rule.
The POC's `session.rs` uses an mpsc writer channel where the exit task sends
the exit chunk and the drainer writes it last; the adapter must additionally
ensure the stdout pump has finished before the exit chunk is sent. A clean
pattern: `join!(stdout_pump, exit_future)` then send the exit chunk.

### Access control

Terminal sessions are runtime-spawned resources per ADR-050:

- **Scope-gate at negotiation.** Check the caller's `identity.scopes` for the
  `tty:open` scope (or a deployment-configured scope) before allocating. A
  caller without the scope gets `{"error":"forbidden"}` and the stream closes.
  The scope name is a two-way-door choice (reversible, not a wire-format
  constant).
- **Resource ownership for backend-specific resources.** Call
  `backend.resource_id(&params)`. If `Some((kind, id))` and an ownership
  provider is wired, check `OwnershipProvider::owns(identity, kind, id, "tty")`.
  If the caller doesn't own it → `{"error":"forbidden"}`. If `None` (the
  session creates its own resource — local process, SSH channel), no
  ownership check.
- **`forwarded_for`** for proxied sessions (ADR-032) is the hub's concern, not
  the adapter's — the worker authorizes the hub (its direct caller).

### Negotiation errors

Send the JSON error response in negotiation framing (task `tty/negotiation`),
then close the write half. The framing-disambiguation trick (first byte `0x00`
= error frame, first byte `1`/`2`/`3` = raw chunk) is sound because error frames
are under 16 MiB.

### Connection and stream lifecycle

- **Connection drop**: all in-flight sessions are cancelled. Pump tasks drop;
  `TtyHandle` drops; `exit_code` future drops without completion → backend's
  cancel-cleanup kills the session target (ADR-056).
- **Stream reset**: `ChunkReader` returns `RawError` (ConnectionClosed or Io).
  Pump tasks exit; `TtyHandle` drops; cancel-cleanup runs. No exit chunk sent.
- **Client cancel (write-half close / eof / zero-length stdin)**: signal EOF
  to backend stdin, keep pumping stdout until exit resolves. The session
  completes normally (exit chunk sent). This is NOT a cancel — ADR-056 is not
  triggered. Cancel-cleanup is triggered only when the *adapter* drops the
  handle (connection drop, stream reset, panic).

### Tests

Use the `MockBackend` from task `tty/backend-trait` (in-memory tokio mpsc
channels for stdin/stdout, a oneshot for exit_code, a mock `TtyControl`).
Drive `drive_session` over a `tokio::io::duplex`:

- **Happy path**: send a negotiation frame, send stdin chunks, mock backend
  echoes stdout, resolve exit_code 0, assert the client receives stdout
  chunks then the exit chunk last, then stream close.
- **Exit-chunk-is-last**: assert no stdout chunk arrives after the exit chunk.
- **Stdin EOF**: send a zero-length stdin chunk (or `eof` control), assert the
  backend's stdin closes, stdout continues, exit chunk still sent.
- **Resize/signal control**: send resize and signal control chunks, assert
  the mock `TtyControl` receives them.
- **Unknown control type**: send `{"type":"unknown"}`, assert it's ignored
  (no error, session continues).
- **unknown_backend error**: negotiation frame with unregistered backend →
  error response, stream closes, no raw mode.
- **malformed_negotiation error**: bad JSON or `carriage != "raw"` or empty
  `cmd` → error response, stream closes.
- **allocate_failed error**: mock backend returns `TtyError::AllocFailed` →
  error response, stream closes.
- **Exit error**: mock backend's `exit_code` resolves with `TtyError` →
  `{"type":"exit","code":-1}`.
- **Cancel cleanup**: drop the connection mid-session, assert the mock
  backend's `exit_code` future is dropped (the kill-on-Drop guard fires).
- **Scope gate**: identity without `tty:open` scope → `forbidden` error.
- **Ownership check**: backend returns `resource_id Some`, ownership provider
  returns false → `forbidden`; returns true → session proceeds.

## Acceptance Criteria

- [ ] `TtyAdapter` struct with `backends` and `ownership` fields
- [ ] `TtyAdapter::new` and `with_ownership` constructors
- [ ] `impl ProtocolHandler for TtyAdapter` with `alpn()` returning `b"alknet/tty"`
- [ ] `handle()` loops `accept_bi`, spawns `drive_session` per stream
- [ ] `drive_session` reads negotiation frame, parses, selects backend, constructs `TtyParams`
- [ ] Negotiation errors (`unknown_backend`, `malformed_negotiation`, `allocate_failed`) sent as JSON in negotiation framing, stream closed
- [ ] Three-pump driver: stdout→client, client→backend (stdin + control dispatch), exit→exit-chunk
- [ ] stderr pump concurrent with stdout when `TtyHandle.stderr` is `Some`
- [ ] Exit-chunk-is-last invariant: stdout/stderr pumps complete AND exit_code resolves before exit chunk enqueued
- [ ] Exit error → `{"type":"exit","code":-1}`
- [ ] Unknown control `type` ignored (not an error)
- [ ] `Exit` control from client ignored (server→client only)
- [ ] Zero-length stdin chunk and `eof` control both close backend stdin
- [ ] Scope-gate at negotiation (`tty:open` scope, or deployment-configured)
- [ ] Ownership check via `backend.resource_id()` + `OwnershipProvider::owns()` when wired
- [ ] Connection drop / stream reset drops `TtyHandle` (triggers ADR-056 cancel-cleanup)
- [ ] Client write-half close does NOT trigger cancel-cleanup (session runs to completion)
- [ ] Integration tests with `MockBackend` over `tokio::io::duplex` for all the scenarios above
- [ ] `cargo test -p alknet-tty` succeeds
- [ ] `cargo clippy -p alknet-tty` succeeds with no warnings

## References

- docs/architecture/crates/tty/tty-adapter.md — the authoritative session lifecycle spec
- docs/architecture/crates/tty/tty-wire.md — the wire format the adapter pumps
- docs/architecture/crates/tty/tty-backend.md — the backend trait the adapter dispatches to
- docs/architecture/decisions/052-alknet-tty-wire-format-and-two-carriage.md — ADR-052
- docs/architecture/decisions/053-ttybackend-trait-and-ttyhandle.md — ADR-053
- docs/architecture/decisions/055-exit-code-on-control-chunk.md — ADR-055 (exit-chunk ordering)
- docs/architecture/decisions/056-backend-cleanup-on-session-cancel.md — ADR-056 (cancel-cleanup)
- docs/architecture/decisions/050-dynamic-resource-ownership-for-runtime-spawned-resources.md — ADR-050 (access control)
- docs/architecture/decisions/032-forwarded-for-identity.md — ADR-032 (forwarded_for)
- /workspace/alknet-tty-poc/src/session.rs — the reference three-pump driver (hardcoded to local PTY; generalize to the trait)

## Notes

> This is the integration task where all the invariants come together. The
> exit-chunk-is-last ordering (ADR-055) is the subtle part: the adapter must
> wait for BOTH the stdout pump to complete AND exit_code to resolve before
> sending the exit chunk. The POC's `session.rs` is the reference but it does
> not enforce this ordering strictly (the exit task sends the chunk
> independently); the crate's adapter must add the coordination. The
> `MockBackend` from `tty/backend-trait` is the test fixture. The scope name
> `tty:open` is a two-way-door choice — make it a constant or configurable,
> not a wire-format constant.

## Summary

> To be filled on completion