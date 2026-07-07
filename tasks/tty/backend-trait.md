---
id: tty/backend-trait
name: Implement TtyBackend trait, TtyHandle, TtyControl, TtyParams, TtyError (ADR-053)
status: pending
depends_on: [tty/crate-init]
scope: moderate
risk: high
impact: phase
level: implementation
---

## Description

Implement the `TtyBackend` trait and its associated types in `src/backend.rs`.
This is the **inversion point** (ADR-053) between the wire-format adapter and
the backends. alknet-tty defines the trait; the backend crates
(`alknet-tty-local`, future `alknet-docker`, `alknet-ssh`) implement it. The
trait shape is a **one-way door** (ADR-053) — changing it after backends exist
is a rewrite across crates. Get it right.

### TtyBackend trait

```rust
#[async_trait]
pub trait TtyBackend: Send + Sync {
    /// Allocate a terminal/process session and return the handles the
    /// adapter pumps. The `backend` field of the negotiation frame
    /// (ADR-052) selects which registered backend's `allocate` is called.
    async fn allocate(&self, params: &TtyParams) -> Result<TtyHandle, TtyError>;

    /// The pre-existing resource this session targets, for ownership
    /// checks (ADR-050). `None` = no pre-existing resource (the session
    /// creates its own — local process, SSH channel). `Some((kind, id))`
    /// = the session targets an existing resource the caller must own
    /// (e.g., DockerTtyBackend returns `Some(("container", id))`). The
    /// adapter calls this at negotiation to gate access; the backend
    /// extracts the id from its own `backend_params`. Default `None`.
    fn resource_id(&self, _params: &TtyParams) -> Option<(&'static str, String)> { None }
}
```

The adapter holds `HashMap<String, Arc<dyn TtyBackend>>` populated at
construction. A backend is the *thing that allocates a session*; the
wire-format pump is backend-agnostic.

### TtyError

`#[non_exhaustive]` so new variants are additive (two-way-door extension
within the one-way trait shape — ADR-053).

```rust
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum TtyError {
    #[error("allocate failed: {message}")]
    AllocFailed { message: String },
    #[error("wait failed: {message}")]
    WaitFailed { message: String },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("backend-specific: {message}")]
    Backend { message: String },
}
```

- `AllocFailed` — PTY couldn't be allocated, docker exec failed, SSH channel
  rejected. Returned by `allocate()`; adapter sends `allocate_failed` error.
- `WaitFailed` — backend couldn't reap the child / determine exit code.
  Returned by the `exit_code` future; adapter sends `{"type":"exit","code":-1}`.
- `Io` — I/O error from a backend's stream/handle.
- `Backend` — backend-specific error not covered by the above.

### TtyParams — the allocation request

```rust
pub struct TtyParams {
    /// Terminal parameters. `None` = pipe mode (no PTY — the runner case,
    /// ADR-054). `Some` = allocate a PTY with these dimensions.
    pub terminal: Option<TerminalParams>,
    /// Command vector (argv[0] + args). Non-empty.
    pub cmd: Vec<String>,
    /// Working directory (None = inherit/default).
    pub cwd: Option<PathBuf>,
    /// Environment variables (empty = inherit).
    pub env: HashMap<String, String>,
    /// Backend-specific selector fields from the negotiation frame,
    /// unparsed. The adapter passes the JSON object through verbatim; the
    /// backend deserializes its own strongly-typed params struct from it.
    /// alknet-tty has zero knowledge of any backend's params shape.
    pub backend_params: serde_json::Map<String, serde_json::Value>,
}

pub struct TerminalParams {
    pub term: Option<String>,      // e.g., "xterm-256color"; None = backend default
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
    pub modes: serde_json::Value,  // reserved — OQ-44; backends MUST ignore content in v1
}
```

Provide a conversion `From<NegotiateRequest> for TtyParams` (or a constructor)
that the adapter uses — mapping `NegotiateRequest.tty: Option<TerminalParamsWire>`
to `TtyParams.terminal: Option<TerminalParams>`, and passing `backend_params`
through verbatim. This conversion lives here (in `backend.rs` or
`negotiation.rs`) so the adapter doesn't hand-roll it.

### TtyHandle — what a backend produces

```rust
pub struct TtyHandle {
    /// Stdin writer — bytes the adapter pumps from client stdin chunks.
    /// `tokio::io::AsyncWrite` (the tokio flavor, not `futures::io`).
    pub stdin: Box<dyn tokio::io::AsyncWrite + Send + Unpin>,
    /// Stdout stream — bytes the adapter pumps to client stdout chunks.
    /// Ends when the backend's stdout reaches EOF.
    pub stdout: Pin<Box<dyn futures_core::Stream<Item = bytes::Bytes> + Send>>,
    /// Stderr stream — `None` for PTY backends (stdout/stderr merged
    /// into `stdout`). `Some` for pipe backends (separate streams).
    pub stderr: Option<Pin<Box<dyn futures_core::Stream<Item = bytes::Bytes> + Send>>>,
    /// Exit code — a `Future` the adapter awaits. Resolves when the
    /// process/container/SSH exec exits. The adapter sends the result
    /// as the `{"type":"exit","code":N}` control chunk (ADR-055) and
    /// closes the stream. This is `BoxFuture`, not a method on
    /// `TtyHandle`, so the adapter can `select` between exit and
    /// stream-close without coupling to the other fields. (REQ-TTY-01.)
    pub exit_code: BoxFuture<'static, Result<i32, TtyError>>,
    /// Control handle (resize, signal) — `Clone` so the adapter can
    /// hand it to the spawned control-chunk dispatcher. `None` only
    /// when the backend genuinely has no control path. See OQ-43.
    pub control: Option<TtyControlHandle>,
}
```

### TtyControl trait and TtyControlHandle

```rust
pub trait TtyControl: Send + Sync {
    /// Resize the terminal. Maps to SSH `window-change`, docker exec
    /// resize, or `ioctl(TIOCSWINSZ)` on a local PTY. No-op for pipe
    /// backends without a PTY.
    fn resize(&self, cols: u16, rows: u16, pixel_width: u16, pixel_height: u16);

    /// Forward a signal by name. Best-effort delivery to the foreground
    /// process group (see tty-local.md REQ-TTY-02). Unknown names fall
    /// back to the backend's default kill.
    fn signal(&self, name: &str);
}

/// The `Clone`-able handle to a backend's control path. The `TtyControl`
/// trait is NOT `Clone` (`Clone` is not object-safe); the `Clone`-ability
/// lives on this concrete newtype, which holds the trait object behind an
/// `Arc`. See OQ-43.
#[derive(Clone)]
pub struct TtyControlHandle(Arc<dyn TtyControl + Send + Sync>);

impl TtyControlHandle {
    pub fn new(control: Arc<dyn TtyControl + Send + Sync>) -> Self { Self(control) }
    pub fn resize(&self, c: u16, r: u16, pw: u16, ph: u16) { self.0.resize(c, r, pw, ph) }
    pub fn signal(&self, name: &str) { self.0.signal(name) }
}
```

The trait is kept object-safe by NOT putting `Clone` on it; the `Clone` newtype
(`TtyControlHandle`) holds the trait object behind an `Arc`. A backend produces
its own control type via `TtyControlHandle::new(Arc::new(MyControl))` without
the adapter knowing the concrete shape (OQ-43).

### REQ-TTY-01: backends are not required to be natively async

The trait's adapter-facing types (`AsyncWrite`, `Stream<Item = Bytes>`,
`BoxFuture`, `TtyControl`) are the **adapter's contract**. A backend may expose
blocking handles internally and bridge them to these async-facing types via
std threads + tokio mpsc/oneshot (the pattern `portable_pty` requires). This is
a documented, supported implementation strategy, not a workaround. The local
backend (task `tty-local/pty-mode`) is the reference implementation.

### ADR-056: kill-on-Drop contract

Dropping the `exit_code` future MUST kill the session target. This is a
behavioral contract on the `TtyBackend` trait — the adapter triggers it by
dropping the `TtyHandle` on session cancel (connection drop, stream reset); the
backend wires the kill into the `exit_code` future's `Drop`. A backend that
returns a bare `oneshot::Receiver<i32>` (or any future without a kill-on-`Drop`
guard) as `exit_code` violates the contract and will orphan processes on cancel.
Document this contract in the `TtyHandle.exit_code` doc comment.

### Tests

This task defines types and traits; the concrete implementations are in the
local backend tasks. Tests here are structural:
- A mock backend (in-memory pipes) implementing `TtyBackend` for the adapter's
  tests is built in task `tty/adapter`. Here, write a minimal compile-time
  check: a `MockBackend` struct that implements `TtyBackend` returning a
  `TtyHandle` with tokio mpsc channels, to verify the trait is implementable
  and the types compose. This mock is reused by the adapter task.
- `TtyControlHandle::new` + `Clone` + `resize`/`signal` delegation test with a
  mock `TtyControl`.

## Acceptance Criteria

- [ ] `TtyBackend` trait with `allocate()` (async) and `resource_id()` (default `None`)
- [ ] `TtyError` enum, `#[non_exhaustive]`, with `AllocFailed`, `WaitFailed`, `Io`, `Backend`
- [ ] `TtyParams` struct with `terminal`, `cmd`, `cwd`, `env`, `backend_params`
- [ ] `TerminalParams` struct with `term`, `cols`, `rows`, `pixel_width`, `pixel_height`, `modes`
- [ ] `TtyHandle` struct with `stdin` (Box<dyn AsyncWrite + Send + Unpin>), `stdout` (Pin<Box<dyn Stream<Item=Bytes> + Send>>), `stderr` (Option), `exit_code` (BoxFuture<Result<i32, TtyError>>), `control` (Option<TtyControlHandle>)
- [ ] `TtyControl` trait with `resize` and `signal` (object-safe, NOT `Clone`)
- [ ] `TtyControlHandle` newtype, `#[derive(Clone)]`, wraps `Arc<dyn TtyControl + Send + Sync>`, with `new`/`resize`/`signal`
- [ ] `From<NegotiateRequest> for TtyParams` (or constructor) mapping wire types to params
- [ ] `TtyHandle.exit_code` doc comment documents the ADR-056 kill-on-Drop contract
- [ ] `TtyBackend` trait doc comment documents REQ-TTY-01 (backends need not be natively async)
- [ ] A `MockBackend` (in-memory pipes) implements `TtyBackend` and compiles
- [ ] Unit test: `TtyControlHandle::new` + clone + resize/signal delegation
- [ ] `cargo test -p alknet-tty` succeeds
- [ ] `cargo clippy -p alknet-tty` succeeds with no warnings

## References

- docs/architecture/crates/tty/tty-backend.md — full trait spec (the authoritative source)
- docs/architecture/decisions/053-ttybackend-trait-and-ttyhandle.md — ADR-053 (the trait decision)
- docs/architecture/decisions/056-backend-cleanup-on-session-cancel.md — ADR-056 (kill-on-Drop contract)
- docs/architecture/decisions/050-dynamic-resource-ownership-for-runtime-spawned-resources.md — ADR-050 (`resource_id`)
- /workspace/alknet-tty-poc/src/local_pty.rs — `LocalPty` (the reference shape a backend produces)

## Notes

> This is the one-way-door task (ADR-053). The trait shape, `TtyHandle` field
> set, and `TtyControl` trait are the API surface every backend crate
> implements and the adapter consumes. Review carefully before the local
> backend begins. The `MockBackend` built here is reused by the adapter task
> for the three-pump driver tests. The `BoxFuture` type comes from
> `futures::future::BoxFuture` (or `std::pin::Pin<Box<dyn Future + Send>>`);
> the `Stream` type from `futures_core::Stream`. Both are re-exported by
> `tokio_stream` for extension methods.

## Summary

> To be filled on completion