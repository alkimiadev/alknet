---
id: tty-local/review-tty-local
name: Review alknet-tty-local for REQ-TTY-01/02 and ADR-056 contract conformance
status: completed
depends_on: [tty-local/backend-impl]
scope: moderate
risk: low
impact: phase
level: review
---

## Description

Review the `alknet-tty-local` implementation for spec conformance, with
particular attention to the three load-bearing requirements: REQ-TTY-01
(backends need not be natively async), REQ-TTY-02 (signal forwarding to the
process group), and ADR-056 (kill-on-Drop cancel-cleanup). This is the
quality checkpoint at the end of the local backend phase, before the
integration tests and the feature re-export wire the two crates together.

### Review Checklist

1. **PTY mode conformance** (tty-local.md §"PTY Mode"):
   - `allocate_pty` uses `portable_pty::native_pty_system().openpty()`
   - `CommandBuilder::set_controlling_tty(true)` (the default; session leader)
   - Three std threads (reader, writer, waiter) feeding tokio mpsc/oneshot
   - `TtyHandle.stdout` is the reader mpsc as `Pin<Box<dyn Stream<Item = Bytes> + Send>>`
   - `TtyHandle.stdin` is the writer mpsc as `Box<dyn AsyncWrite + Send + Unpin>`
   - `TtyHandle.stderr` is `None` (PTY merges stdout/stderr)
   - `TtyHandle.exit_code` is `LocalExitFuture` (oneshot + kill guard)
   - `TtyHandle.control` is `Some(TtyControlHandle::new(Arc::new(PtyControl)))`
   - Reader thread sends zero-length `Bytes` sentinel on EOF
   - Writer thread drops the writer on `StdinCmd::Eof`
   - `AsyncWrite` wrapper over the mpsc sender (poll_write, poll_flush, poll_close)

2. **REQ-TTY-02: Signal forwarding to the process group** (tty-local.md §"REQ-TTY-02"):
   - `PtyControl::signal` uses `kill(-pgid, sig)` (negative pid = process group)
   - Fallback to `kill(pid, sig)` if the group signal fails
   - `portable_pty` child is a session leader (`controlling_tty = true`)
   - `signal_from_name` from `alknet-tty` for the 9 supported names
   - Unknown names fall back to `ChildKiller::kill` (SIGHUP)
   - Contract is "best-effort delivery to the foreground process group"
   - Integration test validates process-group targeting (a child of the shell receives the signal)

3. **Pipe mode conformance** (tty-local.md §"Pipe Mode"):
   - `allocate_pipe` uses `tokio::process::Command` with `Stdio::piped()`
   - `TtyHandle.stdin` is `ChildStdin` as `Box<dyn AsyncWrite + Send + Unpin>`
   - `TtyHandle.stdout`/`stderr` wrapped as `Pin<Box<dyn Stream<Item = Bytes> + Send>>`
   - `TtyHandle.stderr` is `Some` (separate streams)
   - `TtyHandle.exit_code` is `PipeExitFuture` (wait + kill guard)
   - `TtyHandle.control` is `Some(TtyControlHandle::new(Arc::new(PipeControl)))`
   - `PipeControl::resize` is a no-op
   - `PipeControl::signal` uses `kill(pid, sig)` (NOT process group; documented limitation)
   - Unknown signal names fall back to `start_kill()` (SIGKILL)

4. **ADR-056: kill-on-Drop cancel-cleanup** (tty-local.md §"Cancel-Cleanup"):
   - `LocalExitFuture::Drop` calls `ChildKiller::kill(SIGHUP)` when dropped without resolving
   - `LocalExitFuture` disarms the guard on resolve (no-op Drop on happy path)
   - `PipeExitFuture::Drop` calls `Child::start_kill()` when dropped without resolving
   - `PipeExitFuture` disarms the guard on resolve
   - Neither future is a bare `oneshot::Receiver` / bare `Child::wait()` (the POC's shape — MUST have the guard)
   - Integration test: drop the `TtyHandle` mid-session, assert the child is killed (no orphan)

5. **LocalTtyBackend conformance** (tty-local.md, tty-backend.md):
   - `LocalTtyBackend::new()` takes no deps
   - `allocate()` branches on `params.terminal` (Some → PTY, None → pipe)
   - `resource_id()` returns `None`
   - Empty `cmd` → `TtyError::AllocFailed`
   - `backend_params` ignored (no backend-specific selector fields)

6. **Dependency constraints**:
   - `portable_pty` and `libc` deps present (the heavy deps, here not in alknet-tty)
   - `alknet-tty` dependency (workspace path) for the trait and types
   - No `alknet-core` direct dependency (accessed via alknet-tty if needed)
   - No `bollard`/`russh` (those are future backend crates)

7. **Pattern consistency**:
   - `TtyControl` impls use `alknet_tty::TtyControl` trait
   - `TtyControlHandle::new(Arc::new(...))` wrapping (OQ-43 pattern)
   - `TtyError` for errors (not `anyhow` — the POC used `anyhow`; the crate uses `TtyError`)
   - `tracing` for structured logging
   - `#[cfg(unix)]` gates on `libc::kill` paths

8. **Test coverage**:
   - PTY: happy path, stdin round-trip, resize, signal (process group), cancel cleanup, unknown signal
   - Pipe: happy path, stdin round-trip, separate stderr, signal, cancel cleanup, resize no-op, unknown signal
   - Backend: PTY dispatch, pipe dispatch, empty cmd, resource_id

## Acceptance Criteria

- [ ] PTY mode matches tty-local.md §"PTY Mode" (three-thread bridge, PtyControl)
- [ ] REQ-TTY-02 satisfied: `kill(-pgid, sig)` with fallback, session leader, process-group test passes
- [ ] Pipe mode matches tty-local.md §"Pipe Mode" (tokio::process, PipeControl, stderr Some)
- [ ] ADR-056 satisfied: both `LocalExitFuture` and `PipeExitFuture` have kill-on-Drop guards
- [ ] Neither exit future is a bare `oneshot::Receiver` / bare `Child::wait()` (the POC's shape)
- [ ] `LocalTtyBackend` branches on `terminal`, `resource_id` returns `None`
- [ ] `portable_pty`/`libc` deps present; no `bollard`/`russh`/`alknet-core` direct dep
- [ ] `TtyControl` impls use the `alknet_tty::TtyControl` trait + `TtyControlHandle::new` wrapping
- [ ] `#[cfg(unix)]` gates on `libc::kill` paths
- [ ] `cargo fmt --check -p alknet-tty-local` passes
- [ ] `cargo clippy -p alknet-tty-local` passes with no warnings
- [ ] All tests pass (PTY + pipe + backend)
- [ ] Cancel-cleanup integration tests confirm no orphaned processes

## References

- docs/architecture/crates/tty/tty-local.md — the authoritative spec
- docs/architecture/crates/tty/tty-backend.md — the trait this implements
- docs/architecture/decisions/053-ttybackend-trait-and-ttyhandle.md — ADR-053 (REQ-TTY-01)
- docs/architecture/decisions/054-local-tty-backend-sibling-crate.md — ADR-054
- docs/architecture/decisions/056-backend-cleanup-on-session-cancel.md — ADR-056
- /workspace/alknet-tty-poc/src/local_pty.rs — the reference (note: POC lacks the kill guard)
- /workspace/alknet-tty-poc/tests/signal.rs — the SIGINT-forwarding test (REQ-TTY-02)

## Notes

> This review focuses on the three load-bearing requirements. The ADR-056
> kill guard is the most likely deviation — the POC's `exit_code` was a bare
> `oneshot::Receiver<i32>` without a guard, and an implementer who copies the
> POC's shape verbatim violates the contract. Verify both `LocalExitFuture`
> and `PipeExitFuture` have the guard and the cancel-cleanup tests pass (no
> orphaned processes). REQ-TTY-02's process-group targeting is the other
> subtle requirement — verify the integration test spawns a shell with a
> child and confirms the child receives the signal.

## Summary

> To be filled on completion