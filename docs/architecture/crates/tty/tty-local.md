---
status: draft
last_updated: 2026-07-07
---

# alknet-tty — Local TTY Backend (`alknet-tty-local`)

The local backend: a `TtyBackend` implementation that wraps
`portable_pty` for the PTY case (terminal semantics — resize, signal
delivery, escape-sequence handling) and `std::process::Command` with
`Stdio::piped()` for the pipe/runner case (process-streaming without
terminal semantics). This document specifies the `LocalTtyBackend`, the
blocking→async bridge pattern (REQ-TTY-01's reference implementation), and
the signal-delivery contract (REQ-TTY-02). The crate placement is decided
in [ADR-054](../../decisions/054-local-tty-backend-sibling-crate.md);
the trait it implements is in [tty-backend.md](tty-backend.md).

## What

`alknet-tty-local` is a sibling crate (ADR-054) that implements
`TtyBackend` for `LocalTtyBackend`. The backend's `allocate()` branches on
`TtyParams.terminal`:

- **`terminal: Some(TerminalParams { ... })`** — allocate a real PTY via
  `portable_pty::native_pty_system().openpty()`, spawn the command into
  the slave side, return a `TtyHandle` with merged stdout (stderr is
  `None` — kernel PTY property) and a real `TtyControl` (resize via
  `MasterPty::resize`, signal via `libc::kill(-pgid, sig)`).
- **`terminal: None`** — pipe mode, the runner case. Spawn the command
  with `Stdio::piped()` for stdin/stdout/stderr, return a `TtyHandle`
  with separate stdout and stderr (stderr is `Some`) and a `TtyControl`
  whose `resize` is a no-op (no PTY) and `signal` calls
  `libc::kill(pid, sig)` (still works for signal forwarding without a
  PTY).

The backend is the reference implementation of REQ-TTY-01 (backends need
not be natively async) and carries REQ-TTY-02 (signal forwarding to the
process group).

## Why

The local backend is the simplest backend and the one that enables the
runner pattern: a process whose stdin/stdout/stderr/exit-code stream over
a framed bidi connection — the same shape as GitHub/Gitea Actions runners,
just over alknet's transport instead of HTTP polling. With
`LocalTtyBackend`, the dispatch project (`/workspace/@alkdev/dispatch/`, a
reverse runner that currently requires SSH on the remote end) works
without SSH — the endpoint runs the process directly and streams its I/O
back. SSH becomes one transport option (for reaching hosts that don't run
alknet), not a requirement.

The PTY case is what makes a terminal a terminal: real resize (via
`ioctl(TIOCSWINSZ)`), signal delivery to the foreground process group
(via `libc::kill(-pgid, sig)`, REQ-TTY-02), and escape-sequence handling
(the kernel PTY's line discipline). Without a PTY, it's a runner (piped
process); with a PTY, it's a terminal. The per-session choice
(`TtyParams.terminal`) lets one `LocalTtyBackend` serve both — see
ADR-054.

The wrinkle that drove the Phase 0 POC: `portable_pty` is a **blocking
`std::io` API**, not async. `MasterPty::try_clone_reader()` returns
`Box<dyn std::io::Read + Send>`; `take_writer()` returns
`Box<dyn std::io::Write + Send>`; `Child::wait()` blocks. The POC was
built to discover how that constraint shapes the `TtyBackend` trait
(REQ-TTY-01) and the signal-delivery contract (REQ-TTY-02). This spec
records both as requirements, not open questions — the POC turned them
into grounded requirements.

## Architecture

### PTY Mode (`terminal: Some`)

`allocate()` calls `portable_pty::native_pty_system().openpty(PtySize)`
with the terminal dimensions, spawns the command into the slave side
via `SlavePty::spawn_command(CommandBuilder)`, drops the slave (so the
child sees EOF on its stdin when the master writer closes), and returns
a `TtyHandle`.

The blocking→async bridge (REQ-TTY-01's reference implementation):
**three dedicated std threads** feed tokio mpsc/oneshot channels. The
writer thread consumes an mpsc of `StdinCmd`:

```rust
pub enum StdinCmd {
    Bytes(Vec<u8>),  // write these bytes to the master writer
    Eof,            // close the master writer (EOF to the slave's stdin)
}
```

1. **Reader thread** — blocking reads from `MasterPty::try_clone_reader()`
   → `mpsc::Sender<Bytes>`. The reader loop reads into an 8 KiB buffer,
   copies each chunk to `Bytes`, and `blocking_send`s to the mpsc. On EOF
   (the master reader returns EOF when the slave closes — the child has
   exited and the OS has drained the PTY buffer), the thread sends a
   zero-length `Bytes` sentinel (the "drained" signal) and exits. The
   async-facing `TtyHandle.stdout` is the `mpsc::Receiver<Bytes>`,
   wrapped as `Pin<Box<dyn Stream<Item = Bytes> + Send>>`.
2. **Writer thread** — drains an `mpsc::Receiver<StdinCmd>` → blocking
   writes to `MasterPty::take_writer()`. `StdinCmd::Bytes(bytes)` writes
   and flushes; `StdinCmd::Eof` drops the writer (sends EOF to the
   slave's stdin) and exits. The async-facing `TtyHandle.stdin` is the
   `mpsc::Sender<StdinCmd>`, wrapped as `Box<dyn AsyncWrite + Send +
   Unpin>` (an `AsyncWrite` impl that wraps each `write` as a
   `StdinCmd::Bytes` and `flush` as a no-op; the `mpsc::Sender` is the
   sink).
3. **Waiter thread** — blocking `Child::wait()` → `oneshot::Sender<i32>`
   with the exit code. The async-facing `TtyHandle.exit_code` is a
   `Future` wrapping this `oneshot::Receiver<i32>` PLUS a kill guard
   holding the `portable_pty::ChildKiller` (see "Cancel-Cleanup
   (ADR-056)" below). This is the `Future` the adapter awaits (ADR-053
   REQ-TTY-01; ADR-055); its `Drop`-on-cancel kills the child
   (ADR-056).

`TtyHandle.stderr` is `None` (PTY backends merge stdout/stderr — kernel
PTY property, one output stream from the slave).

`TtyHandle.control` is a `PtyControl` struct (the POC's concrete type;
the trait-object form per ADR-053 is the `Arc`-backed `Clone` newtype,
OQ-43):

```rust
#[derive(Clone)]
pub struct PtyControl {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    killer: Arc<Mutex<Box<dyn portable_pty::ChildKiller + Send + Sync>>>,
    pid: Option<u32>,
}
```

`resize()` locks the master and calls `MasterPty::resize(PtySize)` —
non-blocking (it issues an `ioctl`). `signal()` — see REQ-TTY-02 below.

### REQ-TTY-02: Signal Forwarding Must Target the Process Group

`libc::kill(pid, sig)` on the spawned child's pid alone is **insufficient**
for terminal semantics: a shell running under a PTY will have spawned
children (a `find | grep` pipeline, a `make` with sub-makes), and those
children will not receive the signal. A real terminal forwards Ctrl-C to
the **foreground process group**, which (under job-control shells) is the
process group the shell most recently spawned for the foreground job.

`portable_pty` makes the child a session leader (when
`controlling_tty = true`, the default — `CommandBuilder::set_controlling_tty(true)`),
so the child's pid *is* its process-group id, and `libc::kill(-pid, sig)`
(the negative pid) reaches the whole group. The POC's `PtyControl::signal`
uses exactly this — `kill(-pgid, sig)` with a fallback to `kill(pid, sig)`
if the group signal fails (e.g., the child already exited).

The spec records:

1. **The local backend MUST forward signals to the child's process
   group, not just the child pid.** Using `kill(-pgid, sig)` when the
   child is a session leader (the `portable_pty` default).
2. **The local backend MUST spawn the child as a session leader with a
   controlling tty.** This is `portable_pty`'s default
   (`CommandBuilder::set_controlling_tty(true)`); disabling it (e.g.,
   for container-boundary workarounds) breaks signal forwarding and is
   therefore not supported for the terminal use case.
3. **The `TtyControl::signal` contract is "best-effort delivery to the
   foreground process group,"** not "the child pid receives the signal."
   Unknown signal names fall back to the backend's default kill
   (`portable_pty`'s `ChildKiller::kill` sends SIGHUP); known names map
   to `libc` signal numbers (`HUP`, `INT`, `QUIT`, `TERM`, `KILL`,
   `USR1`, `USR2`, `TSTP`, `CONT`) and are sent to the group.

This pre-empts a class of "Ctrl-C doesn't kill my `cargo build`" bugs
that would otherwise surface in Phase 2/3.

### Cancel-Cleanup (ADR-056)

The `TtyBackend` cleanup contract (ADR-056): **dropping the `exit_code`
future kills the session target.** The local backend implements this for
both PTY and pipe modes.

**PTY mode.** `allocate()` obtains a `portable_pty::Child` (with
`wait()`) and a `portable_pty::ChildKiller` (with `kill()`) — the two
handles `portable_pty` exposes alongside each other. The `Child` moves
into the waiter thread (which blocks on `wait()`). The `ChildKiller`
moves into the `exit_code` future's `Drop` guard, alongside the
`oneshot::Receiver<i32>` from the waiter thread. The future's `poll`
delegates to the oneshot receiver (resolves on natural exit); the
future's `Drop` (runs on cancel only — on resolve, the guard is
disarmed) calls `ChildKiller::kill(SIGHUP)`:

```rust
struct LocalExitFuture {
    rx: oneshot::Receiver<i32>,
    killer: Option<portable_pty::ChildKiller>,  // None after resolve (disarmed)
}

impl Future for LocalExitFuture { /* poll delegates to rx; on Ready, take killer */ }
impl Drop for LocalExitFuture {
    fn drop(&mut self) {
        if let Some(killer) = self.killer.take() {
            let _ = killer.kill(SIGHUP);  // best-effort; child may already be exiting
        }
    }
}
```

On cancel: the `Drop` kills the child (SIGHUP); the child exits; the
waiter thread's `wait()` reaps it and exits (its `oneshot::send` fails
silently — the receiver was dropped with the future, which is expected);
the reader/writer threads exit on channel close. The child is reaped
(no zombie) by the waiter thread's `wait()` returning after the kill.

**Pipe mode.** The same pattern with `tokio::process::Child` instead of
`portable_pty::Child`. The `exit_code` future's `Drop` guard holds the
`Child` handle (or a `Child`-kill wrapper) and calls
`Child::start_kill()` on cancel. The waiter task (`Child::wait()`)
reaps the killed child.

**The happy path is unaffected.** When the adapter drives `exit_code`
to completion (the child exits naturally), the future resolves, the
guard is disarmed (the `Option::take()` in `poll`'s `Ready` branch),
and the subsequent `Drop` is a no-op. The contract is "kill on cancel;
no-op on resolve."

This closes the orphaned-process gap the local-PTY POC surfaced: a
child that ignores stdin EOF (a daemon, a long-lived process with no
stdin reader) is killed when the session is cancelled, not left
running. The POC's `LocalPty::exit_code` was a bare
`oneshot::Receiver<i32>` with no kill guard — an implementer who
copies the POC's shape without the guard violates the contract. See
ADR-056 for the contract and the trait-level rationale.

### Pipe Mode (`terminal: None`)

`allocate()` spawns the command with `std::process::Command` and
`Stdio::piped()` for stdin, stdout, and stderr. The async bridge is
simpler than the PTY case — tokio's `Child` provides `AsyncRead` for
stdout/stderr and `AsyncWrite` for stdin directly (no std-thread
bridge needed). `TtyHandle.stderr` is `Some` (separate streams). The
`exit_code` future is `Child::wait()` (async on tokio's `Child`).

`TtyHandle.control` is a `PipeControl` whose `resize()` is a no-op
(no PTY — resize doesn't apply) and `signal()` calls `libc::kill(pid, sig)`
on the child's pid. Signal forwarding to the process group is not
applicable in pipe mode (there's no session leader / controlling tty);
`kill(pid, sig)` reaches the direct child only. If the child has
spawned its own children, they won't receive the signal — this is a
known limitation of the runner case (a runner that needs
process-group signal delivery uses the PTY case, not the pipe case).

### The Threading/Deadlock Caveat (DP-4, Acknowledged Constraint)

`std::process::Command` with piped stdio can deadlock if stdin writes
block while stdout/stderr buffers fill — the classic pipe-buffer deadlock.
The fix is concurrent reads on stdout/stderr alongside stdin writes,
which is exactly what the bidirectional pump does (the POC's
`drive_attach_raw` runs the two directions as concurrent
`tokio::spawn` tasks). The same pattern works for `LocalTtyBackend`:
spawn one task pumping stdin→process, one task pumping process→stdout-chunks,
one for stderr if piped. This is a known constraint with a known solution
(POC-validated); no design decision needed.

### Crate Placement (ADR-054)

`alknet-tty-local` is a sibling crate. `alknet-tty` re-exports
`LocalTtyBackend` behind a `local` feature:

```toml
# alknet-tty Cargo.toml
[features]
default = []
local = ["dep:alknet-tty-local"]
```

A consumer that wants the local backend enables `features = ["local"]`
and gets `alknet_tty::local::LocalTtyBackend`. A consumer that only wants
docker/ssh uses the default features and depends on the backend crate
directly — no `portable_pty` in the dependency tree. See ADR-054.

### Dependencies

```
alknet-tty-local
├── alknet-tty     (TtyBackend trait, TtyHandle, TtyControl, wire types)
├── alknet-core     (via alknet-tty's re-export; not direct)
├── portable_pty    (PTY allocation — the heavy dep, Unix openpty + Windows ConPTY)
├── libc            (signal forwarding — REQ-TTY-02, Unix only)
└── tokio           (mpsc, oneshot, AsyncRead/AsyncWrite for the pipe case)
```

## The Runner Pattern

The pipe mode (`terminal: None`) is the "runner" generalization the
research identified. A coordinator sends a negotiation frame with
`{ "backend": "local", "tty": null, "cmd": ["cargo", "test"] }`; the
endpoint runs `cargo test` with piped stdio, streams stdout/stderr chunks
back, sends `{"type":"exit","code":N}` when it finishes (ADR-055). The
coordinator gets reliable completion notification (the exit control
chunk + stream close) — no polling, no plugin state.

This is functionally identical to GitHub/Gitea Actions runners, just over
alknet's transport instead of HTTP polling. The dispatch project
(`/workspace/@alkdev/dispatch/`) is a reverse runner that currently
requires SSH on the remote end; with `LocalTtyBackend`, the same pattern
works without SSH — the endpoint runs the process directly. SSH becomes
one transport option (for reaching hosts that don't run alknet), not a
requirement.

The runner-specific API surface (job management, log persistence, task
graph integration) is **out of scope for alknet-tty** (OQ-46). alknet-tty
provides the *mechanism* (a framed byte stream for a process + exit
code); the runner *policy* is a downstream crate's job. This spec
commits to preserving the option (`terminal: None` → pipe mode) and not
building runner policy into alknet-tty.

## Constraints

- **PTY mode requires `portable_pty`'s native PTY (Unix `openpty` /
  Windows ConPTY).** The blocking→async bridge (three std threads) is
  the documented pattern for any blocking-API backend (REQ-TTY-01).
- **Signal forwarding in PTY mode targets the process group (REQ-TTY-02).**
  `kill(-pgid, sig)` when the child is a session leader
  (`controlling_tty = true`, the default). Disabling the controlling tty
  breaks signal forwarding and is not supported for the terminal use
  case.
- **Pipe mode does not forward signals to the process group.** `kill(pid,
  sig)` reaches the direct child only; grandchildren don't receive it.
  A runner that needs process-group signal delivery uses the PTY case.
- **The pipe-buffer deadlock is handled by the concurrent pump.** The
  adapter's three-pump driver (`tty-adapter.md`) reads stdout/stderr
  concurrently with writing stdin — the POC-validated pattern. No design
  decision needed; the spec notes it as a known constraint with a known
  solution.
- **`LocalTtyBackend` takes no constructor dependencies.** Unlike
  `DockerTtyBackend` (wraps a `bollard::Docker` client) or
  `SshTtyBackend` (wraps an SSH session), the local backend is
  dependency-free at construction — the `portable_pty` system is
  process-global. The assembly layer constructs one `LocalTtyBackend`
  and registers it as `"local"`.
- **The `exit_code` future's `Drop`-on-cancel kills the child
  (ADR-056).** The local backend MUST NOT return a bare
  `oneshot::Receiver<i32>` as `TtyHandle.exit_code` — it must wrap it
  in a `Future` whose `Drop` calls `ChildKiller::kill(SIGHUP)` (PTY) or
  `Child::start_kill()` (pipe) when dropped without resolving. An
  implementer who copies the POC's bare `oneshot::Receiver<i32>` shape
  without the kill guard violates the contract and will orphan
  processes on session cancel. See ADR-056.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| Local backend as a sibling crate | [ADR-054](../../decisions/054-local-tty-backend-sibling-crate.md) | `alknet-tty-local` behind a `local` feature re-export; PTY vs pipe per-session |
| `TtyBackend` trait and `TtyHandle` | [ADR-053](../../decisions/053-ttybackend-trait-and-ttyhandle.md) | The trait this backend implements; REQ-TTY-01 (backends need not be natively async) |
| Wire format | [ADR-052](../../decisions/052-alknet-tty-wire-format-and-two-carriage.md) | The chunk codec + control channel the adapter pumps to/from this backend |
| Exit code on a control chunk | [ADR-055](../../decisions/055-exit-code-on-control-chunk.md) | The waiter thread's `oneshot::Receiver<i32>` feeds the exit chunk |
| Backend cleanup on session cancel | [ADR-056](../../decisions/056-backend-cleanup-on-session-cancel.md) | The `exit_code` future's `Drop`-on-cancel kills the child via `ChildKiller` (PTY) / `start_kill` (pipe); the waiter thread reaps |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-46** (deferred(scope)): Runner API surface.

## References

- [ADR-054](../../decisions/054-local-tty-backend-sibling-crate.md) —
  the crate placement decision
- [ADR-053](../../decisions/053-ttybackend-trait-and-ttyhandle.md) — the
  trait this backend implements; REQ-TTY-01 (the blocking-backend
  accommodation)
- [ADR-055](../../decisions/055-exit-code-on-control-chunk.md) — the
  waiter thread's `oneshot::Receiver<i32>` feeds the exit chunk
- [ADR-056](../../decisions/056-backend-cleanup-on-session-cancel.md) —
  the cancel-cleanup contract this backend implements (the `exit_code`
  future's `Drop`-on-cancel kills the child via `ChildKiller` /
  `start_kill`)
- `docs/research/alknet-tty/phase-0-findings.md` — §"Requirements from
  the local-PTY POC" (REQ-TTY-01 and REQ-TTY-02, the load-bearing
  constraints this spec records)
- `/workspace/alknet-tty-poc/src/local_pty.rs` — the reference
  implementation of the PTY backend (the three-thread bridge, the
  `PtyControl` struct, the `kill(-pgid, sig)` signal forwarding)
- `/workspace/alknet-tty-poc/src/session.rs` — the session driver that
  consumes this backend's handles (the reference for the adapter's
  three-pump driver)
- `/workspace/alknet-tty-poc/tests/signal.rs` — the SIGINT-forwarding
  integration test (validates REQ-TTY-02)
- `/workspace/@alkdev/dispatch/` — the reverse-runner prior art
  (currently requires SSH; this backend removes that requirement)
- `portable-pty` 0.9 source — the blocking-API constraint and the
  `CommandBuilder::set_controlling_tty` default REQ-TTY-02 depends on
- [tty-backend.md](tty-backend.md) — the trait this backend implements
- [tty-adapter.md](tty-adapter.md) — the session driver that consumes
  this backend's handles