---
status: draft
last_updated: 2026-07-07
---

# alknet-tty — TtyBackend Trait and TtyHandle

The `TtyBackend` trait is the inversion point that keeps alknet-tty
decoupled from its backends. alknet-tty defines the trait, the
`TtyParams` allocation request, the `TtyHandle` a backend produces, and
the `TtyControl` trait; the backend crates (alknet-tty-local,
alknet-docker, alknet-ssh) implement `TtyBackend`. This document
specifies what an implementer builds against. The trait shape is decided
in [ADR-053](../../decisions/053-ttybackend-trait-and-ttyhandle.md).

## What

The `TtyBackend` trait is what the `TtyAdapter` calls to allocate a
terminal/process session. The adapter holds a
`HashMap<String, Arc<dyn TtyBackend>>` keyed by the negotiation frame's
`backend` string (`"local"`, `"docker"`, `"ssh"`). On a new session, the
adapter reads the negotiation frame, selects the backend by the
`backend` field, calls `allocate()`, and pumps the resulting
`TtyHandle`'s fields bidirectionally using the chunk format
(ADR-052). The backend does not write to the wire — it produces handles;
the adapter pumps.

Three implementations are contemplated, each in its own crate (the
no-handler-depends-on-another-handler rule from ADR-003 is preserved —
backends depend on alknet-tty for the trait, alknet-tty doesn't depend on
them):

- **`LocalTtyBackend`** (in `alknet-tty-local`, ADR-054) — wraps
  `portable_pty` for the PTY case and `std::process::Command` with
  `Stdio::piped()` for the pipe/runner case. See
  [tty-local.md](tty-local.md).
- **`DockerTtyBackend`** (in `alknet-docker` or a sibling adapter crate —
  future, out of scope here) — wraps `bollard::attach_container()` for
  interactive attach or `bollard::exec::start_exec` with `tty: true` for
  exec-with-PTY. `control.resize()` calls `bollard::exec::resize_exec`
  or `bollard::container::resize_container`. stdout/stderr are merged
  when `tty: true` (bollard's `LogOutput` on a TTY exec returns
  `StdOut` only), so `TtyHandle.stderr` is `None` for the PTY case.
- **`SshTtyBackend`** (in `alknet-ssh` — future, out of scope here) —
  wraps russh's `pty_request` + `shell_request` (or `exec_request` with
  a PTY) on a session channel. `channel.into_stream()` gives
  `(AsyncRead, AsyncWrite)` — the stream *is* the PTY; russh handles
  kernel PTY allocation on the server side. `control.resize()` sends a
  `window_change` channel request; `control.signal()` sends a `signal`
  channel request. stdout and stderr are merged (PTY property), so
  `TtyHandle.stderr` is `None`.

The docker and SSH backend crates are future work; this spec set commits
the trait shape they will implement, so they can be built against it
without re-spec'ing the seam.

## Why

The guiding insight: **a terminal session is not an SSH concern, or a
Docker concern — it is a terminal concern. SSH and Docker are just two
backends that can allocate a PTY.** The `TtyBackend` trait is what makes
that insight load-bearing — alknet-tty owns the wire format and session
lifecycle; the backends own PTY allocation. The full rationale (the
inversion point, why the trait is the seam) is in
[ADR-053](../../decisions/053-ttybackend-trait-and-ttyhandle.md) §Context.

The Phase 0 local-PTY POC (`/workspace/alknet-tty-poc`) was built
*before* this spec specifically to discover constraints the trait sketch
would have missed by reading docs alone. Two requirements fell out,
recorded as REQ-TTY-01 and REQ-TTY-02 in the research findings; this
spec carries REQ-TTY-01 here (backends need not be natively async) and
[tty-local.md](tty-local.md) carries REQ-TTY-02 (signal forwarding to
the process group).

## Architecture

### `TtyBackend` trait

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
construction. The assembly layer (the CLI binary) constructs backends
with their dependencies and registers them. A backend is the *thing
that allocates a session*; the wire-format pump is backend-agnostic.

### `TtyError`

The error type for `allocate()` and `exit_code`. `#[non_exhaustive]` so
new variants are additive (two-way-door extension within the one-way
trait shape — ADR-053).

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

- `AllocFailed` — the PTY couldn't be allocated, the docker exec failed
  to start, the SSH channel request was rejected. Returned by
  `allocate()`; the adapter sends `{"error":"allocate_failed",...}` and
  closes (tty-adapter.md §"Negotiation errors").
- `WaitFailed` — the backend couldn't reap the child / determine the
  exit code. Returned by the `exit_code` future; the adapter sends
  `{"type":"exit","code":-1}` (ADR-055 §4).
- `Io` — an I/O error from a backend's stream/handle.
- `Backend` — backend-specific error not covered by the above (e.g., a
  bollard API error, a russh protocol error).

### `TtyParams` — the allocation request

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
    /// See "Backend params are opaque" below.
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

`terminal: None` is the pipe/runner case — no PTY, separate
stdout/stderr. `terminal: Some` is the PTY case — stdout/stderr merged
into the single stdout stream (`TtyHandle.stderr` is `None`), real
terminal semantics (resize, signal delivery to process group). The
per-session choice is the backend's branch in `allocate()`, not a
per-deployment choice — see ADR-054.

### Backend params are opaque

`backend_params` is a `serde_json::Map<String, serde_json::Value>`, not
a typed enum. The adapter passes the negotiation frame's
backend-specific fields through verbatim; the backend deserializes its
own strongly-typed params struct. alknet-tty has zero knowledge of any
backend's params shape — not docker's `container`, not an SSH host
selector, not anything. Each backend defines its own params struct and
deserializes from `params.backend_params` inside `allocate()`:

```rust
// in alknet-docker
#[derive(Deserialize)]
struct DockerBackendParams { container: String }

impl TtyBackend for DockerTtyBackend {
    async fn allocate(&self, params: &TtyParams) -> Result<TtyHandle, TtyError> {
        let p: DockerBackendParams = serde_json::from_value(
            serde_json::Value::Object(params.backend_params.clone())
        ).map_err(|e| TtyError::Backend { message: e.to_string() })?;
        // use p.container ...
    }
    fn resource_id(&self, params: &TtyParams) -> Option<(&'static str, String)> {
        // extract for the ownership check — backend-driven, not adapter-hardcoded
        let p: DockerBackendParams = serde_json::from_value(
            serde_json::Value::Object(params.backend_params.clone())
        ).ok()?;
        Some(("container", p.container))
    }
}
```

This is a complete inversion: the *trait* is inverted (backends
implement, alknet-tty doesn't depend on them) and the *params* are
inverted (backends define their own typed shape, alknet-tty doesn't
carry it). A new backend crate requires zero changes to alknet-tty — no
enum variant to add, no forward-reference type to place, no dependency
edge. See ADR-053 §"Backend params are opaque" for the full rationale
and why the typed-enum alternative (with `SshChannelRef`) was rejected.

### `TtyHandle` — what a backend produces

```rust
pub struct TtyHandle {
    /// Stdin writer — bytes the adapter pumps from client stdin chunks.
    /// `tokio::io::AsyncWrite` (the tokio flavor, not the `futures::io`
    /// one — they are incompatible traits; the tokio stack is the
    /// adapter's runtime).
    pub stdin: Box<dyn tokio::io::AsyncWrite + Send + Unpin>,
    /// Stdout stream — bytes the adapter pumps to client stdout chunks.
    /// Ends when the backend's stdout reaches EOF.
    /// `futures_core::Stream<Item = bytes::Bytes>` (re-exported by
    /// `tokio_stream::StreamExt` for extension methods).
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

### `TtyControl` trait and `TtyControlHandle`

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
/// trait is NOT `Clone` (`Clone` is not object-safe — `fn clone(&self) ->
/// Self` returns `Self`, which forbids `dyn` dispatch); the `Clone`-ability
/// lives on this concrete newtype, which holds the trait object behind an
/// `Arc`. The adapter clones the `Arc` to hand a handle to the spawned
/// control-chunk dispatcher. See OQ-43.
#[derive(Clone)]
pub struct TtyControlHandle(Arc<dyn TtyControl + Send + Sync>);

impl TtyControlHandle {
    pub fn new(control: Arc<dyn TtyControl + Send + Sync>) -> Self { Self(control) }
    pub fn resize(&self, c: u16, r: u16, pw: u16, ph: u16) { self.0.resize(c, r, pw, ph) }
    pub fn signal(&self, name: &str) { self.0.signal(name) }
}
```

The trait is kept object-safe by NOT putting `Clone` on it; the `Clone`
newtype (`TtyControlHandle`) holds the trait object behind an `Arc`. The
POC used a concrete `PtyControl` struct (inherently `Clone` — it held
`Arc<Mutex<...>>` fields); this newtype generalizes the POC's shape so a
backend produces its own control type via `TtyControlHandle::new(Arc::new(MyControl))`
without the adapter knowing the concrete shape. See OQ-43 for the
confirmation and the rationale for why `Clone` cannot live on the trait
itself.

### REQ-TTY-01: backends are not required to be natively async

`portable_pty`'s API is blocking `std::io::{Read, Write}` and a blocking
`Child::wait()` — there is no async variant. The local-PTY POC bridges
this with three dedicated std threads (reader, writer, waiter) feeding
tokio mpsc/oneshot channels; the async-facing `LocalPty` then exposes
`mpsc::Receiver<Bytes>` for stdout, `mpsc::Sender<StdinCmd>` for stdin,
and `oneshot::Receiver<i32>` for exit. This is the same pattern wezterm
(portable_pty's primary consumer) uses.

The trait's adapter-facing types (`AsyncWrite`, `Stream<Item = Bytes>`,
`BoxFuture`, `TtyControl`) are the **adapter's contract**. A backend may
expose blocking handles internally and bridge them to these async-facing
types. The bridging pattern — blocking `std::io` on dedicated std threads
or `tokio::task::spawn_blocking`, feeding tokio mpsc/oneshot channels —
is a **documented, supported implementation strategy**, not a workaround.

This resolves the first half of OQ-TTY-01 (the research's open question
on the trait shape): `exit_code` is a `Future` the adapter awaits; a
`oneshot::Receiver<i32>` (or any `BoxFuture<'static, i32>`) lets the
adapter `select` between exit and stream-close without coupling to the
handle's other fields. The local backend's waiter thread produces exactly
this shape for free. See [tty-local.md](tty-local.md) for the bridge
details.

### Backend registration and the assembly layer

```rust
let mut backends = HashMap::new();
backends.insert("local".into(),
    Arc::new(LocalTtyBackend::new()) as Arc<dyn TtyBackend>);
backends.insert("docker".into(),
    Arc::new(DockerTtyBackend::new(docker_client)) as Arc<dyn TtyBackend>);
backends.insert("ssh".into(),
    Arc::new(SshTtyBackend::new(ssh_session)) as Arc<dyn TtyBackend>);
let tty_adapter = TtyAdapter::new(Arc::new(backends));
```

A deployment that doesn't want docker registers only `local`. A browser
terminal endpoint that proxies to remote docker/ssh registers `docker`
and/or `ssh` backends. The adapter is backend-agnostic; the assembly
layer chooses what's available.

### Backend implementations (where they live)

| Backend | Crate | Status | Notes |
|---------|-------|--------|-------|
| `LocalTtyBackend` | `alknet-tty-local` (sibling, behind `local` feature) | in scope ([tty-local.md](tty-local.md)) | `portable_pty` (PTY) + `std::process` (pipe); the runner pattern |
| `DockerTtyBackend` | `alknet-docker` or sibling adapter | future, out of scope here | wraps `bollard::attach_container` / `exec` with `tty: true` |
| `SshTtyBackend` | `alknet-ssh` | future, out of scope here | wraps russh `pty_request` + `shell_request`/`exec_request`; dissolves alknet-ssh DP-5 PTY hedge |

The docker and SSH backend crates are future work; this spec commits the
trait shape they implement so they can be built against it without
re-spec'ing the seam. The `DockerTtyBackend` is the natural extension of
the alknet-docker POC's `drive_attach_raw` (`/workspace/alknet-docker-poc/src/ops.rs`)
— with the trait, it becomes `impl TtyBackend for DockerTtyBackend`. The
`SshTtyBackend` dissolves the alknet-ssh research's PTY hedge (DP-5):
alknet-ssh's session channel still does `exec` (structured, JSON carriage,
exit code on completion) but *delegates* PTY to alknet-tty via the
`SshTtyBackend`. alknet-ssh's "default-reject" stance stays for the SSH
channel policy (it rejects `pty_request` on its own session channels),
but the PTY capability is provided by a separate crate via a separate
ALPN (`alknet/tty`), not hedged inside alknet-ssh.

## Constraints

- **The trait shape is one-way (ADR-053).** The `TtyBackend` trait
  method `allocate()`, the `TtyHandle` field set, and the `TtyControl`
  trait are the API surface every backend crate implements and the
  adapter consumes. Changing them after backends exist is a rewrite
  across crates.
- **Backend params are opaque (`serde_json::Map`), not a typed enum.**
  The carrier type is one-way (part of `TtyParams`), but the *contents*
  are backend-defined: each backend deserializes its own
  strongly-typed params struct, and a new backend crate requires zero
  changes to alknet-tty. See "Backend params are opaque" above and
  ADR-053 §"Backend params are opaque."
- **The adapter, not the backend, owns the wire format.** Backends
  produce handles; the adapter pumps. A backend that wrote to the wire
  directly would break the wire-format invariants (the exit-chunk
  ordering, ADR-055). The backend's `exit_code` future resolves and the
  adapter sends the exit chunk — the backend does not serialize
  `ControlMessage::Exit`.
- **PTY backends merge stdout/stderr.** `TtyHandle.stderr` is `None` for
  the PTY case (kernel PTY property — one output stream from the slave).
  The adapter pumps only stdout chunks (stream_type 1). Pipe backends
  set `stderr: Some` and the adapter pumps both stdout (stream_type 1)
  and stderr (stream_type 2) chunks.
- **`TtyControl::signal` is best-effort.** The contract is "best-effort
  delivery to the foreground process group," not "the child pid receives
  the signal." See [tty-local.md](tty-local.md) REQ-TTY-02 for the
  process-group targeting and the fallback to the backend's default kill.
- **Dropping the `exit_code` future MUST kill the session target
  (ADR-056).** The `exit_code` field is a `BoxFuture<'static,
  Result<i32, TtyError>>` whose `Drop`-on-cancel (i.e., dropped without
  being driven to completion) MUST kill the child/container/SSH process.
  This is a behavioral contract on the `TtyBackend` trait — the adapter
  triggers it by dropping the `TtyHandle` on session cancel (connection
  drop, stream reset); the backend wires the kill into the `exit_code`
  future's `Drop`. A backend that returns a bare `oneshot::Receiver<i32>`
  (or any future without a kill-on-`Drop` guard) as `exit_code`
  violates the contract and will orphan processes on cancel. See
  [ADR-056](../../decisions/056-backend-cleanup-on-session-cancel.md)
  and [tty-local.md](tty-local.md) §"Cancel-Cleanup (ADR-056)" for the
  local backend's mechanism.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| `TtyBackend` trait and `TtyHandle` | [ADR-053](../../decisions/053-ttybackend-trait-and-ttyhandle.md) | The backend inversion point; `exit_code` as `Future`; backends need not be natively async (REQ-TTY-01) |
| Local backend as a sibling crate | [ADR-054](../../decisions/054-local-tty-backend-sibling-crate.md) | `alknet-tty-local` behind a `local` feature re-export |
| Wire format | [ADR-052](../../decisions/052-alknet-tty-wire-format-and-two-carriage.md) | The chunk codec + control channel the adapter pumps to/from these handles |
| Exit code on a control chunk | [ADR-055](../../decisions/055-exit-code-on-control-chunk.md) | The adapter awaits `exit_code`, sends the exit chunk, closes |
| Backend cleanup on session cancel | [ADR-056](../../decisions/056-backend-cleanup-on-session-cancel.md) | Dropping `exit_code` future (cancel) MUST kill the session target; contract on the `TtyBackend` trait |
| Crate decomposition | [ADR-003](../../decisions/003-crate-decomposition.md) Am. 1 | alknet-tty depends on alknet-core; backends depend on alknet-tty for the trait |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-43** (resolved): `TtyControl` as a `Clone` trait object.
- **OQ-44** (deferred(scope)): Terminal modes.

## References

- [ADR-053](../../decisions/053-ttybackend-trait-and-ttyhandle.md) — the
  trait shape decision (this spec is its elaboration)
- [ADR-052](../../decisions/052-alknet-tty-wire-format-and-two-carriage.md)
  — the wire format the adapter pumps to/from these handles
- [ADR-055](../../decisions/055-exit-code-on-control-chunk.md) — the
  exit-chunk ordering the `exit_code` field feeds into
- [ADR-056](../../decisions/056-backend-cleanup-on-session-cancel.md) —
  the cancel-cleanup contract on this trait (`exit_code` future's
  `Drop`-on-cancel kills the session target)
- [ADR-054](../../decisions/054-local-tty-backend-sibling-crate.md) —
  the local backend's crate placement
- `docs/research/alknet-tty/phase-0-findings.md` — §"The Backend Trait"
  (the seed of this spec) and §"Requirements from the local-PTY POC"
  (REQ-TTY-01, the load-bearing constraint)
- `/workspace/alknet-tty-poc/src/local_pty.rs` — the reference
  implementation of what a backend produces (`LocalPty`: stdout mpsc,
  stdin mpsc, control `PtyControl` (Clone), exit oneshot)
- `/workspace/alknet-tty-poc/src/session.rs` — the adapter-side pump that
  consumes `TtyHandle`-shaped fields (the reference for how the adapter
  uses the trait)
- [tty-local.md](tty-local.md) — the `LocalTtyBackend` spec (carries
  REQ-TTY-02: signal forwarding to the process group)
- [tty-adapter.md](tty-adapter.md) — the session driver that consumes
  these handles