# ADR-053: TtyBackend Trait and TtyHandle — the Backend Inversion Point

## Status

Accepted

## Context

alknet-tty's wire format (ADR-052) is backend-agnostic — the chunk codec
pumps bytes and JSON control messages without knowing whether the backend
is a docker container, an SSH session channel, or a local process. The
question this ADR answers: **what is the seam between the wire-format
adapter and the backends?**

The alknet-tty research (`docs/research/alknet-tty/phase-0-findings.md`)
identified the `TtyBackend` trait as the inversion point. The guiding
insight:

> A terminal session is not an SSH concern, or a Docker concern — it is a
> terminal concern. SSH and Docker are just two backends that can allocate
> a PTY.

The trait is what makes that insight load-bearing: alknet-tty defines the
trait and the wire-format adapter; the backend crates (alknet-docker,
alknet-ssh, alknet-tty-local) implement the trait. This preserves ADR-003's
no-handler-depends-on-another-handler rule (amended by ADR-003 Amendment 1
for protocol-foundation crates): alknet-tty depends on alknet-core;
backend crates depend on alknet-tty for the trait; alknet-tty does not
depend on any backend.

### What the local-PTY POC discovered about the trait shape

The Phase 0 POC (`/workspace/alknet-tty-poc`) was built *before* this ADR
specifically to discover constraints the trait sketch would have missed by
reading docs alone. Two requirements fell out of it (recorded as
REQ-TTY-01 and REQ-TTY-02 in the findings doc):

- **REQ-TTY-01: backends are not required to be natively async.**
  `portable_pty` is a blocking `std::io::{Read, Write}` API with a blocking
  `Child::wait()`. The POC bridges it to async via three dedicated std
  threads (reader, writer, waiter) feeding tokio mpsc/oneshot channels —
  the same pattern wezterm (portable_pty's primary consumer) uses. The
  trait's adapter-facing types (`AsyncWrite`, `Stream<Item = Bytes>`,
  `BoxFuture`) are the *adapter's* contract; a backend may expose blocking
  handles internally and bridge them. The bridging pattern is a
  documented, supported implementation strategy, not a workaround.

- **`exit_code` is a `Future` the adapter awaits, not a method on
  `TtyHandle`.** A `oneshot::Receiver<i32>` (or any
  `BoxFuture<'static, i32>`) lets the adapter `select` between exit and
  stream-close without coupling to the handle's other fields. The local
  backend's waiter thread produces exactly this shape for free.

The POC's `LocalPty` struct (`src/local_pty.rs`) is the reference
implementation of what a backend produces: `stdout: mpsc::Receiver<Bytes>`,
`stdin: mpsc::Sender<StdinCmd>`, `control: PtyControl` (Clone), `exit_code:
oneshot::Receiver<i32>`. The trait shape below generalizes this.

### What the local-PTY POC did not resolve

The POC used a separate cloneable `PtyControl` struct for resize/signal,
not a `Box<dyn TtyControl + Send + Unpin>` trait object. The research
noted this worked cleanly because the control-chunk dispatcher needs to be
`Clone` to hand off to the spawned pump task. Phase 1 confirms the `control`
field as a separate `Clone` trait object — see OQ-43 for the confirmation
and the `Clone` constraint rationale.

## Decision

### 1. `TtyBackend` trait

```rust
#[async_trait]
pub trait TtyBackend: Send + Sync {
    /// Allocate a terminal/process session and return the handles the
    /// adapter pumps. The `backend` field of the negotiation frame
    /// (ADR-052) selects which registered backend's `allocate` is called.
    async fn allocate(&self, params: &TtyParams) -> Result<TtyHandle, TtyError>;
}
```

The adapter holds a `HashMap<String, Arc<dyn TtyBackend>>` keyed by the
negotiation frame's `backend` string (`"local"`, `"docker"`, `"ssh"`).
The assembly layer registers backends at startup; the adapter dispatches
by the `backend` field. A backend is the *thing that allocates a session*;
the wire-format pump is backend-agnostic.

### 2. `TtyParams` — the allocation request

```rust
pub struct TtyParams {
    /// Terminal parameters. `None` = pipe mode (no PTY — the runner case,
    /// ADR-054). `Some` = allocate a PTY with these dimensions.
    pub terminal: Option<TerminalParams>,
    /// Command vector (argv[0] + args).
    pub cmd: Vec<String>,
    /// Working directory (backend-specific; None = inherit/default).
    pub cwd: Option<PathBuf>,
    /// Environment variables (backend-specific; empty = inherit).
    pub env: HashMap<String, String>,
    /// Backend-specific selector fields from the negotiation frame
    /// (e.g., `container` for docker, `SshChannelRef` for ssh). The
    /// adapter parses the negotiation frame's backend-specific fields
    /// and passes them here. The exact shape is backend-defined; the
    /// adapter does not interpret it.
    pub backend_params: BackendParams,
}

pub struct TerminalParams {
    pub term: Option<String>,      // e.g., "xterm-256color"; None = backend default
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
    pub modes: serde_json::Value,  // reserved — see OQ-44
}

pub enum BackendParams {
    Local,
    Docker { container: String },
    Ssh { channel: SshChannelRef },
    // Extensible: a backend crate may add variants. (See Consequences.)
}
```

`terminal: None` is the pipe/runner case — no PTY, separate stdout/stderr
(ADR-054). `terminal: Some` is the PTY case — stdout/stderr merged into
the single stdout stream (`TtyHandle.stderr` is `None`), real terminal
semantics (resize, signal delivery to process group).

### 3. `TtyHandle` — what a backend produces

```rust
pub struct TtyHandle {
    /// Stdin writer — bytes the adapter pumps from client stdin chunks.
    pub stdin: Box<dyn AsyncWrite + Send + Unpin>,
    /// Stdout stream — bytes the adapter pumps to client stdout chunks.
    /// Ends when the backend's stdout reaches EOF (process exited,
    /// container output stream ended, SSH channel closed).
    pub stdout: Pin<Box<dyn Stream<Item = Bytes> + Send>>,
    /// Stderr stream — `None` for PTY backends (stdout/stderr merged
    /// into `stdout`). `Some` for pipe backends (separate streams).
    pub stderr: Option<Pin<Box<dyn Stream<Item = Bytes> + Send>>>,
    /// Exit code — a `Future` the adapter awaits. Resolves when the
    /// process/container/SSH exec exits. The adapter sends the result
    /// as the `{"type":"exit","code":N}` control chunk (ADR-055) and
    /// closes the stream. This is `BoxFuture`, not a method on
    /// `TtyHandle`, so the adapter can `select` between exit and
    /// stream-close without coupling to the other fields. (REQ-TTY-01.)
    pub exit_code: BoxFuture<'static, Result<i32, TtyError>>,
    /// Control handle (resize, signal) — `Clone` so the adapter can
    /// hand it to the spawned control-chunk dispatcher. `None` only
    /// when the backend genuinely has no control path (e.g., a pipe
    /// backend with no PTY — signal still works via `kill(pid, sig)`,
    /// but resize is a no-op). See OQ-43 for the trait-object-vs-methods
    /// confirmation.
    pub control: Option<Box<dyn TtyControl + Send + Unpin + Clone>>,
}

pub trait TtyControl: Send {
    /// Resize the terminal. Maps to SSH `window-change`, docker exec
    /// resize, or `ioctl(TIOCSWINSZ)` on a local PTY. No-op for pipe
    /// backends without a PTY (the adapter still calls it; the backend
    /// ignores).
    fn resize(&self, cols: u16, rows: u16, pixel_width: u16, pixel_height: u16);

    /// Forward a signal by name. Best-effort delivery to the foreground
    /// process group (see tty-local.md REQ-TTY-02). Unknown names fall
    /// back to the backend's default kill.
    fn signal(&self, name: &str);
}
```

### 4. Backends are not required to be natively async (REQ-TTY-01)

The trait's adapter-facing types (`AsyncWrite`, `Stream<Item = Bytes>`,
`BoxFuture`, the `TtyControl` trait object) are the **adapter's contract**.
A backend may expose blocking handles internally and bridge them to these
async-facing types. The bridging pattern — blocking `std::io` on dedicated
std threads or `tokio::task::spawn_blocking`, feeding tokio mpsc/oneshot
channels — is a **documented, supported implementation strategy**, not a
workaround.

The local backend (ADR-054) uses this pattern: `portable_pty` is a blocking
API, and the backend's `allocate()` spawns reader/writer/waiter threads
that feed `mpsc::Receiver<Bytes>` (stdout), `mpsc::Sender<StdinCmd>` (stdin
wrapped as `AsyncWrite`), and `oneshot::Receiver<i32>` (exit). The
adapter consumes the bridged async-facing types and is unaware of the
threading. See `tty-local.md` for the bridge details.

### 5. Backend registration and the assembly layer

The `TtyAdapter` does not know the set of backends at compile time — it
holds a `HashMap<String, Arc<dyn TtyBackend>>` populated at construction.
The assembly layer (the CLI binary) constructs backends with their
dependencies (a `DockerTtyBackend` wraps a `bollard::Docker` client; an
`SshTtyBackend` wraps an SSH session; a `LocalTtyBackend` takes no
deps) and registers them:

```rust
let mut backends = HashMap::new();
backends.insert("local".into(), Arc::new(LocalTtyBackend::new()) as Arc<dyn TtyBackend>);
backends.insert("docker".into(), Arc::new(DockerTtyBackend::new(docker_client)) as _);
backends.insert("ssh".into(), Arc::new(SshTtyBackend::new(ssh_session)) as _);
let tty_adapter = TtyAdapter::new(Arc::new(backends));
```

A deployment that doesn't want docker registers only `local`. A browser
terminal endpoint that proxies to remote docker/ssh registers `docker`
and/or `ssh` backends. The adapter is backend-agnostic; the assembly
layer chooses what's available.

## Consequences

**Positive:**

- The wire-format adapter is backend-agnostic and testable with a mock
  backend (in-memory pipes). The build order (ADR-052 wire format + mock
  backend first, real backends last) follows directly.
- alknet-tty stays dependency-light: no bollard, no russh, no
  `portable_pty` in the core crate. The heavy deps live in the backend
  crates. This is the same inversion as `OperationAdapter` (ADR-017):
  the trait lives where the types live; the implementations live where
  their transport dependencies live.
- Blocking-API backends (portable_pty) are first-class — the trait
  accommodates them by making the adapter-facing types the contract and
  the bridging pattern a documented strategy. No re-spec required when a
  future backend is also blocking.
- `exit_code` as a `Future` (not a method on the handle) lets the adapter
  `select` between exit and stream-close — the load-bearing composition
  the session lifecycle needs (the exit chunk is sent after the child is
  reaped, then the stream closes — ADR-055).

**Negative:**

- `BackendParams` is an enum, which means a new backend crate adding a
  variant modifies the enum. This is a two-way door (the enum is
  `#[non_exhaustive]`; a new variant is additive, and the adapter's
  `match` has a `_ => unsupported backend` arm), but it means
  `BackendParams` lives in alknet-tty and backend crates extend it rather
  than defining their own. An alternative — backend-specific params as
  `serde_json::Value` — was rejected: it loses type safety and forces the
  adapter to parse backend-specific JSON it shouldn't interpret. The
  enum-with-`#[non_exhaustive]` trade is the right one: the adapter
  dispatches by string key, the params enum carries the typed shape, and
  new backends add variants additively.
- `Box<dyn TtyControl + Send + Unpin + Clone>` is an unusual trait-object
  bound (`Clone` on a trait object requires a workaround — typically a
  custom clone-via-`Arc` or a `Box<dyn TtyControl>` wrapped in a small
  `Clone` newtype). This is the cost of the POC-discovered constraint
  that the control-chunk dispatcher needs to be `Clone` to hand off to
  the spawned pump task. See OQ-43 for the confirmation and the concrete
  `Clone` newtype approach.
- A backend that produces neither a PTY nor a process (a hypothetical
  "recorded session replay" backend) would have a no-op `TtyControl` and
  a synthetic `exit_code`. The trait accommodates it but the `TtyParams`
  shape (`cmd` is `Vec<String>`, `terminal` is `Option`) assumes
  command-spawning. A non-command backend would need a different
  `BackendParams` variant and a backend-internal `cmd` synthesis. Not a
  current use case; the trait shape doesn't preclude it but doesn't
  optimize for it.

## Door type

**One-way.** The `TtyBackend` trait, `TtyHandle` field set, and
`TtyControl` trait are the API surface every backend crate implements and
the adapter consumes. Changing the trait shape after backends exist is a
rewrite across crates. The `BackendParams` enum is `#[non_exhaustive]`
(additive — two-way for new variants), but the trait and handle shapes are
one-way.

## Assumptions

1. **The `TtyControl` trait object can be made `Clone` via a small
   newtype.** The POC used a concrete `PtyControl` struct (inherently
   `Clone`). The trait-object form needs a `Clone` newtype wrapping the
   `Box` (e.g., a struct holding `Arc<dyn TtyControl>` so `Clone` is an
   `Arc` clone). This is a known Rust pattern; OQ-43 confirms the
   approach.

2. **Backends produce a single session per `allocate()` call.** The
   adapter calls `allocate()` once per accepted bidi stream (one session
   per stream — ADR-052). A backend that multiplexed multiple sessions
   over one `allocate()` would not fit the trait; no such backend is
   contemplated.

3. **The adapter, not the backend, owns the exit-chunk ordering.** The
   backend resolves `exit_code`; the adapter awaits it, sends the exit
   control chunk, and closes the stream (ADR-055). The backend does not
   write to the wire — it produces handles; the adapter pumps. This keeps
   the wire-format logic in one place (the adapter) and the backend
   focused on its allocation target (docker, ssh, local process).

## References

- `docs/research/alknet-tty/phase-0-findings.md` — Phase 0 research; §"The
  Backend Trait" is the seed of this ADR; §"Requirements from the
  local-PTY POC" (REQ-TTY-01) is the load-bearing constraint
- `/workspace/alknet-tty-poc/src/local_pty.rs` — the reference
  implementation of what a backend produces (`LocalPty`: stdout mpsc,
  stdin mpsc, control `PtyControl` (Clone), exit oneshot)
- `/workspace/alknet-tty-poc/src/session.rs` — the adapter-side pump that
  consumes `TtyHandle`-shaped fields (the reference for how the adapter
  uses the trait)
- [ADR-003](003-crate-decomposition.md) + Amendment 1 — no-handler-depends-
  on-another-handler; alknet-tty depends on alknet-core; backends depend
  on alknet-tty for the trait
- [ADR-007](007-bistream-type-definition.md) — `Connection`, `SendStream`,
  `RecvStream` (the adapter receives a `Connection`, accepts bidi streams,
  pumps per-session)
- [ADR-052](052-alknet-tty-wire-format-and-two-carriage.md) — the wire
  format this trait's backends feed
- [ADR-054](054-local-tty-backend-sibling-crate.md) — the local backend's
  crate placement (sibling crate, behind a feature re-export)
- [ADR-055](055-exit-code-on-control-chunk.md) — the exit chunk ordering
  this trait's `exit_code` field feeds into
- OQ-43 — `TtyControl` as `Clone` trait object (resolved: confirmed)
- OQ-44 — terminal modes (deferred(scope): not needed for current scope)
- Spec: [crates/tty/tty-backend.md](../crates/tty/tty-backend.md)