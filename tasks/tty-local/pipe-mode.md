---
id: tty-local/pipe-mode
name: Implement pipe mode (tokio::process, PipeControl, ADR-056 kill guard)
status: pending
depends_on: [tty-local/crate-init]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Implement the pipe mode in `src/pipe.rs`: the runner case (`terminal: None`,
no PTY). Spawn the command with `tokio::process::Command` (or
`std::process::Command` with `Stdio::piped()`) for stdin/stdout/stderr, return
a `TtyHandle` with separate stdout and stderr (stderr is `Some`) and a
`PipeControl` whose `resize()` is a no-op (no PTY) and `signal()` calls
`libc::kill(pid, sig)` on the child's pid.

The async bridge is simpler than the PTY case — tokio's `Child` provides
`AsyncRead` for stdout/stderr and `AsyncWrite` for stdin directly (no
std-thread bridge needed).

### Pipe mode (`terminal: None`)

`allocate_pipe` spawns the command with `Stdio::piped()` for stdin, stdout,
and stderr. `TtyHandle.stderr` is `Some` (separate streams). The `exit_code`
future is `Child::wait()` (async on tokio's `Child`).

### Types

```rust
pub fn allocate_pipe(
    cmd: Vec<String>,
    cwd: Option<PathBuf>,
    env: HashMap<String, String>,
) -> Result<TtyHandle, TtyError>;
```

- `TtyHandle.stdin` — `Child::stdin` (`ChildStdin` implements `AsyncWrite`),
  boxed as `Box<dyn AsyncWrite + Send + Unpin>`.
- `TtyHandle.stdout` — `Child::stdout` wrapped as a `Stream<Item = Bytes>`.
  Use `tokio_util::io::ReaderStream` or a manual `AsyncRead`→`Stream` adapter
  to convert `ChildStdout` (`AsyncRead`) into `Pin<Box<dyn Stream<Item = Bytes> + Send>>`.
- `TtyHandle.stderr` — `Some`, same wrapping as stdout.
- `TtyHandle.exit_code` — a `Future` wrapping `Child::wait()` PLUS a kill
  guard (ADR-056, see below).
- `TtyHandle.control` — `Some(TtyControlHandle::new(Arc::new(PipeControl)))`.

### PipeControl (implements TtyControl)

```rust
pub struct PipeControl {
    pid: Option<u32>,
}

impl TtyControl for PipeControl {
    fn resize(&self, _cols: u16, _rows: u16, _pw: u16, _ph: u16) {
        // No-op — no PTY in pipe mode.
    }
    fn signal(&self, name: &str) {
        // libc::kill(pid, sig) on the child's pid (NOT the process group —
        // pipe mode has no session leader / controlling tty).
    }
}
```

`resize()` is a no-op (no PTY — resize doesn't apply). `signal()` calls
`libc::kill(pid, sig)` on the child's pid. Signal forwarding to the process
group is **not applicable** in pipe mode (there's no session leader /
controlling tty); `kill(pid, sig)` reaches the direct child only. If the child
has spawned its own children, they won't receive the signal — this is a known
limitation of the runner case (a runner that needs process-group signal
delivery uses the PTY case, not the pipe case). Document this in the
`PipeControl::signal` doc comment.

Use `alknet_tty::signal_from_name` for the name→number mapping. Unknown names
fall back to `Child::start_kill()` (SIGKILL) — there's no `ChildKiller` in
pipe mode; `tokio::process::Child::start_kill()` is the kill path. On non-Unix,
`signal()` calls `start_kill()` directly (no `libc::kill`).

### Cancel-Cleanup (ADR-056)

The `exit_code` future's `Drop`-on-cancel MUST kill the child. Implement a
`PipeExitFuture` wrapping the `Child::wait()` future plus a kill guard holding
the `tokio::process::Child` handle:

```rust
struct PipeExitFuture {
    wait: Pin<Box<dyn Future<Output = Result<ExitStatus, io::Error>> + Send>>,
    child: Option<tokio::process::Child>,  // None after resolve (disarmed)
}

impl Future for PipeExitFuture {
    // poll delegates to wait; on Ready, take child (disarm), map ExitStatus to i32
}

impl Drop for PipeExitFuture {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();  // best-effort; child may already be exiting
        }
    }
}
```

On cancel: `Drop` calls `start_kill()` (SIGKILL); the child exits; the `wait`
future reaps it. On happy path: the future resolves, the guard is disarmed,
the subsequent `Drop` is a no-op.

**Critical**: a bare `Child::wait()` future without the kill guard violates
the ADR-056 contract and will orphan processes on cancel. This task MUST add
the guard. Note: `tokio::process::Child` must be created with
`kill_on_drop(true)` OR the explicit guard — the guard is the spec-compliant
mechanism; `kill_on_drop` alone is insufficient because the `Child` is moved
into the future and the future's `Drop` is the cancel path, not the `Child`'s.
Use the explicit guard.

### The Threading/Deadlock Caveat (DP-4)

`std::process::Command` with piped stdio can deadlock if stdin writes block
while stdout/stderr buffers fill — the classic pipe-buffer deadlock. The fix
is concurrent reads on stdout/stderr alongside stdin writes, which is exactly
what the adapter's three-pump driver does (task `tty/adapter`). No design
decision needed; the spec notes it as a known constraint with a known
(POC-validated) solution.

### Tests

- **Happy path**: spawn `echo hello` in pipe mode, read stdout stream to EOF,
  await exit_code → 0, assert stderr is empty.
- **Stdin round-trip**: spawn `cat`, write via `AsyncWrite` stdin, read back
  via stdout, close stdin, await exit 0.
- **Separate stderr**: spawn a command that writes to stderr (e.g.,
  `sh -c "echo err >&2"`), assert stderr stream receives the bytes, stdout
  is empty.
- **Signal (Unix)**: spawn `sleep 60`, send `signal("TERM")`, assert the child
  exits with signal-terminated status.
- **Cancel cleanup (ADR-056)**: spawn `sleep 60`, drop the `TtyHandle` without
  awaiting exit_code, assert the child is killed (no orphan).
- **Resize no-op**: call `PipeControl::resize`, assert no error (it's a no-op).
- **Unknown signal name**: send `signal("NOSUCH")`, assert it falls back to
  `start_kill()` (SIGKILL).

## Acceptance Criteria

- [ ] `allocate_pipe` function spawns via `tokio::process::Command` with `Stdio::piped()`
- [ ] `TtyHandle.stdin` is `ChildStdin` boxed as `Box<dyn AsyncWrite + Send + Unpin>`
- [ ] `TtyHandle.stdout` is `ChildStdout` wrapped as `Pin<Box<dyn Stream<Item = Bytes> + Send>>`
- [ ] `TtyHandle.stderr` is `Some` (same wrapping as stdout)
- [ ] `TtyHandle.exit_code` is `PipeExitFuture` (wait + kill guard), `BoxFuture<Result<i32, TtyError>>`
- [ ] `TtyHandle.control` is `Some(TtyControlHandle::new(Arc::new(PipeControl)))`
- [ ] `PipeControl` implements `alknet_tty::TtyControl`
- [ ] `PipeControl::resize` is a no-op
- [ ] `PipeControl::signal` uses `libc::kill(pid, sig)` (Unix) / `start_kill()` (non-Unix); NOT process-group targeting
- [ ] `PipeControl::signal` uses `alknet_tty::signal_from_name`; unknown names fall back to `start_kill()`
- [ ] `PipeExitFuture::Drop` calls `Child::start_kill()` when dropped without resolving (ADR-056)
- [ ] `PipeExitFuture` disarms the guard on resolve (no-op Drop on happy path)
- [ ] Doc comment on `PipeControl::signal` notes the no-process-group limitation
- [ ] Integration tests: happy path, stdin round-trip, separate stderr, signal, cancel cleanup, resize no-op, unknown signal
- [ ] `cargo test -p alknet-tty-local` succeeds
- [ ] `cargo clippy -p alknet-tty-local` succeeds with no warnings

## References

- docs/architecture/crates/tty/tty-local.md — §"Pipe Mode (`terminal: None`)", §"Cancel-Cleanup (ADR-056)", §"The Threading/Deadlock Caveat"
- docs/architecture/crates/tty/tty-backend.md — `TtyHandle`, `TtyControl` (the types this produces)
- docs/architecture/decisions/056-backend-cleanup-on-session-cancel.md — ADR-056 (kill-on-Drop)
- docs/architecture/decisions/054-local-tty-backend-sibling-crate.md — ADR-054 (pipe mode = runner case)

## Notes

> Pipe mode is simpler than PTY mode (tokio's `Child` is natively async, no
> std-thread bridge). The two subtleties: (1) the `AsyncRead`→`Stream<Item=Bytes>`
> conversion for stdout/stderr (use `tokio_util::io::ReaderStream` or a manual
> adapter), and (2) the ADR-056 kill guard on the `exit_code` future. Do NOT
> rely on `tokio::process::Child::kill_on_drop(true)` alone — the explicit
> guard in the future's `Drop` is the spec-compliant mechanism. Signal
> forwarding in pipe mode targets the direct child only (no process group);
> document this limitation.

## Summary

> To be filled on completion