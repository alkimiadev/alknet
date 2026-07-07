---
id: tty-local/pty-mode
name: Implement PTY mode (three-thread bridge, PtyControl, REQ-TTY-02, ADR-056 kill guard)
status: pending
depends_on: [tty-local/crate-init]
scope: broad
risk: high
impact: component
level: implementation
---

## Description

Implement the PTY mode in `src/pty.rs`: allocate a real PTY via
`portable_pty::native_pty_system().openpty()`, spawn the command into the
slave side, and return a `TtyHandle` with merged stdout (stderr is `None` —
kernel PTY property) and a real `TtyControl` (resize via `MasterPty::resize`,
signal via `libc::kill(-pgid, sig)`).

This is the reference implementation of REQ-TTY-01 (backends need not be
natively async) and carries REQ-TTY-02 (signal forwarding to the process
group). It is a port + generalization of the POC's
`/workspace/alknet-tty-poc/src/local_pty.rs`, adapted to produce a `TtyHandle`
(the trait's handle type) instead of the POC's `LocalPty` struct.

### The blocking→async bridge (REQ-TTY-01)

`portable_pty` is a blocking `std::io` API. `MasterPty::try_clone_reader()`
returns `Box<dyn std::io::Read + Send>`; `take_writer()` returns
`Box<dyn std::io::Write + Send>`; `Child::wait()` blocks. Bridge to async via
**three dedicated std threads** feeding tokio mpsc/oneshot channels:

1. **Reader thread** — blocking reads from `MasterPty::try_clone_reader()` →
   `mpsc::Sender<Bytes>`. Reads into an 8 KiB buffer, copies each chunk to
   `Bytes`, `blocking_send`s to the mpsc. On EOF (master reader returns EOF
   when the slave closes — child exited, PTY buffer drained), sends a
   zero-length `Bytes` sentinel and exits. The async-facing `TtyHandle.stdout`
   is the `mpsc::Receiver<Bytes>` wrapped as `Pin<Box<dyn Stream<Item = Bytes> + Send>>`.
2. **Writer thread** — drains an `mpsc::Receiver<StdinCmd>` → blocking writes
   to `MasterPty::take_writer()`. `StdinCmd::Bytes(bytes)` writes and flushes;
   `StdinCmd::Eof` drops the writer (EOF to slave stdin) and exits. The
   async-facing `TtyHandle.stdin` is the `mpsc::Sender<StdinCmd>` wrapped as
   `Box<dyn AsyncWrite + Send + Unpin>` (an `AsyncWrite` impl that wraps each
   `write` as a `StdinCmd::Bytes` and `flush` as a no-op).
3. **Waiter thread** — blocking `Child::wait()` → `oneshot::Sender<i32>` with
   the exit code. The async-facing `TtyHandle.exit_code` is a `Future` wrapping
   this `oneshot::Receiver<i32>` PLUS a kill guard holding the
   `portable_pty::ChildKiller` (see ADR-056 below).

`TtyHandle.stderr` is `None` (PTY backends merge stdout/stderr).

### StdinCmd

```rust
pub enum StdinCmd {
    Bytes(Vec<u8>),  // write these bytes to the master writer
    Eof,            // close the master writer (EOF to the slave's stdin)
}
```

### AsyncWrite wrapper for stdin

Implement an `AsyncWrite` impl over `mpsc::Sender<StdinCmd>`: `poll_write`
sends `StdinCmd::Bytes(bytes.to_vec())` (awaiting the send via `poll`), `poll_flush`
is a no-op (the writer thread flushes), `poll_close` sends `StdinCmd::Eof`. This
wraps the tokio mpsc sender as the `Box<dyn AsyncWrite + Send + Unpin>` the
`TtyHandle.stdin` field requires.

### PtyControl (implements TtyControl)

```rust
pub struct PtyControl {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    killer: Arc<Mutex<Box<dyn portable_pty::ChildKiller + Send + Sync>>>,
    pid: Option<u32>,
}

impl TtyControl for PtyControl {
    fn resize(&self, cols: u16, rows: u16, pixel_width: u16, pixel_height: u16);
    fn signal(&self, name: &str);
}
```

- `resize()` locks the master and calls `MasterPty::resize(PtySize)` —
  non-blocking (issues an `ioctl`).
- `signal()` — see REQ-TTY-02 below.

### REQ-TTY-02: Signal Forwarding Must Target the Process Group

`libc::kill(pid, sig)` on the spawned child's pid alone is **insufficient** for
terminal semantics: a shell under a PTY will have spawned children (a
`find | grep` pipeline, a `make` with sub-makes), and those children won't
receive the signal. A real terminal forwards Ctrl-C to the **foreground
process group**.

`portable_pty` makes the child a session leader (when `controlling_tty = true`,
the default — `CommandBuilder::set_controlling_tty(true)`), so the child's pid
*is* its process-group id, and `libc::kill(-pid, sig)` (the negative pid)
reaches the whole group. The POC's `PtyControl::signal` uses exactly this —
`kill(-pgid, sig)` with a fallback to `kill(pid, sig)` if the group signal
fails (e.g., the child already exited).

The spec records (tty-local.md §"REQ-TTY-02"):
1. The local backend MUST forward signals to the child's process group, not
   just the child pid. Using `kill(-pgid, sig)` when the child is a session
   leader.
2. The local backend MUST spawn the child as a session leader with a
   controlling tty (`CommandBuilder::set_controlling_tty(true)` — the default).
3. The `TtyControl::signal` contract is "best-effort delivery to the
   foreground process group." Unknown signal names fall back to the backend's
   default kill (`portable_pty`'s `ChildKiller::kill` sends SIGHUP); known
   names map to `libc` signal numbers via `alknet_tty::signal_from_name` and
   are sent to the group.

Use `alknet_tty::signal_from_name` (from `alknet-tty`'s `control` module) for
the name→number mapping. On non-Unix, fall back to `ChildKiller::kill`.

### Cancel-Cleanup (ADR-056)

The `exit_code` future's `Drop`-on-cancel MUST kill the child. Implement a
`LocalExitFuture` wrapping the `oneshot::Receiver<i32>` plus a kill guard:

```rust
struct LocalExitFuture {
    rx: oneshot::Receiver<i32>,
    killer: Option<portable_pty::ChildKiller>,  // None after resolve (disarmed)
}

impl Future for LocalExitFuture {
    // poll delegates to rx; on Ready, take killer (disarm)
}

impl Drop for LocalExitFuture {
    fn drop(&mut self) {
        if let Some(killer) = self.killer.take() {
            let _ = killer.kill(SIGHUP);  // best-effort; child may already be exiting
        }
    }
}
```

On cancel: `Drop` kills the child (SIGHUP); the child exits; the waiter
thread's `wait()` reaps it and exits (its `oneshot::send` fails silently —
the receiver was dropped with the future, which is expected); the
reader/writer threads exit on channel close. The child is reaped (no zombie)
by the waiter thread's `wait()` returning after the kill.

On happy path: the future resolves, the guard is disarmed (`Option::take()`
in `poll`'s `Ready` branch), and the subsequent `Drop` is a no-op. The
contract is "kill on cancel; no-op on resolve."

**Critical**: the POC's `LocalPty::exit_code` was a bare `oneshot::Receiver<i32>`
with no kill guard. An implementer who copies the POC's shape without the
guard violates the contract and will orphan processes on cancel. This task
MUST add the guard.

### allocate_pty function

```rust
pub fn allocate_pty(
    terminal: TerminalParams,
    cmd: Vec<String>,
    cwd: Option<PathBuf>,
    env: HashMap<String, String>,
) -> Result<TtyHandle, TtyError>;
```

This is called by `LocalTtyBackend::allocate()` (task `tty-local/backend-impl`)
when `TtyParams.terminal` is `Some`. It returns a fully-wired `TtyHandle`.

### Tests

- **Happy path**: spawn `echo hello` into a PTY, read stdout until the
  zero-length sentinel, await exit_code → 0.
- **Stdin round-trip**: spawn `cat` into a PTY, write bytes via the
  `AsyncWrite` stdin, read them back via stdout, send `Eof`, await exit 0.
- **Resize**: call `PtyControl::resize`, assert no error (the ioctl succeeds).
- **Signal (Unix)**: spawn `sleep 60`, send `signal("INT")`, assert the child
  exits with a signal-terminated status (negative exit code or 130 for SIGINT).
  Test process-group targeting: spawn `bash -c "sleep 60"`, send INT, assert
  the `sleep` child also receives it (the group signal).
- **Cancel cleanup (ADR-056)**: spawn `sleep 60`, drop the `TtyHandle` (and
  thus the `exit_code` future) without awaiting it, assert the child is
  killed (reaped by the waiter thread; no orphan). Use a short delay then
  check the process is gone.
- **Unknown signal name**: send `signal("NOSUCH")`, assert it falls back to
  `ChildKiller::kill` (SIGHUP); the child exits.

## Acceptance Criteria

- [ ] `allocate_pty` function spawns via `portable_pty::native_pty_system().openpty()`
- [ ] `CommandBuilder::set_controlling_tty(true)` (the default; session leader)
- [ ] Three std threads (reader, writer, waiter) feeding tokio mpsc/oneshot
- [ ] `TtyHandle.stdout` is the reader mpsc wrapped as `Pin<Box<dyn Stream<Item = Bytes> + Send>>`
- [ ] `TtyHandle.stdin` is the writer mpsc wrapped as `Box<dyn AsyncWrite + Send + Unpin>`
- [ ] `TtyHandle.stderr` is `None` (PTY merges stdout/stderr)
- [ ] `TtyHandle.exit_code` is `LocalExitFuture` (oneshot + kill guard), `BoxFuture<Result<i32, TtyError>>`
- [ ] `TtyHandle.control` is `Some(TtyControlHandle::new(Arc::new(PtyControl)))`
- [ ] `PtyControl` implements `alknet_tty::TtyControl` (resize, signal)
- [ ] `PtyControl::signal` uses `kill(-pgid, sig)` with `kill(pid, sig)` fallback (Unix); `ChildKiller::kill` (non-Unix)
- [ ] `PtyControl::signal` uses `alknet_tty::signal_from_name` for name→number
- [ ] Unknown signal names fall back to `ChildKiller::kill` (SIGHUP)
- [ ] `LocalExitFuture::Drop` calls `ChildKiller::kill(SIGHUP)` when dropped without resolving (ADR-056)
- [ ] `LocalExitFuture` disarms the guard on resolve (no-op Drop on happy path)
- [ ] Reader thread sends zero-length `Bytes` sentinel on EOF
- [ ] Writer thread drops the writer on `StdinCmd::Eof`
- [ ] Integration tests: happy path, stdin round-trip, resize, signal (process group), cancel cleanup, unknown signal
- [ ] `cargo test -p alknet-tty-local` succeeds
- [ ] `cargo clippy -p alknet-tty-local` succeeds with no warnings

## References

- docs/architecture/crates/tty/tty-local.md — §"PTY Mode", §"REQ-TTY-02", §"Cancel-Cleanup (ADR-056)"
- docs/architecture/crates/tty/tty-backend.md — `TtyHandle`, `TtyControl` (the types this produces)
- docs/architecture/decisions/053-ttybackend-trait-and-ttyhandle.md — ADR-053 (REQ-TTY-01)
- docs/architecture/decisions/056-backend-cleanup-on-session-cancel.md — ADR-056 (kill-on-Drop)
- /workspace/alknet-tty-poc/src/local_pty.rs — the reference implementation (port + add kill guard)
- /workspace/alknet-tty-poc/tests/signal.rs — the SIGINT-forwarding integration test (validates REQ-TTY-02)

## Notes

> This is the highest-risk task in the tty-local crate: the three-thread
> bridge, the process-group signal targeting, and the ADR-056 kill guard are
> all subtle. The POC is the reference but its `exit_code` was a bare
> `oneshot::Receiver<i32>` — the kill guard is NEW and required. Do not copy
> the POC's `exit_code` shape without the guard. The `AsyncWrite` wrapper over
> the mpsc sender is also new (the POC exposed the sender directly); the
> `TtyHandle.stdin` field requires `Box<dyn AsyncWrite + Send + Unpin>`.

## Summary

> To be filled on completion