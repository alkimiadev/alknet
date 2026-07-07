//! Pipe mode for `LocalTtyBackend`: `tokio::process`-backed runner sessions.
//!
//! Spawns the command with `Stdio::piped()` for stdin/stdout/stderr, exposes
//! tokio-native `AsyncRead`/`AsyncWrite` (no std-thread bridge needed),
//! `PipeControl` (no-op resize, `libc::kill(pid, sig)` signal), and the
//! ADR-056 kill guard on the `exit_code` future.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::ExitStatus;
use std::sync::Arc;
use std::task::{Context, Poll};

use alknet_tty::backend::{BoxFuture, TtyControl, TtyControlHandle, TtyError, TtyHandle};
use alknet_tty::control::signal_from_name;
use bytes::Bytes;
use futures_core::Stream;
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio_util::io::ReaderStream;

/// Allocate a pipe-mode session: spawn `cmd` with `Stdio::piped()` for
/// stdin/stdout/stderr and return a `TtyHandle` whose `stderr` is `Some`
/// (separate streams). The `exit_code` future carries the ADR-056 kill
/// guard (see [`PipeExitFuture`]). `control` is a [`PipeControl`]
/// (no-op resize, `libc::kill(pid, sig)` signal).
pub fn allocate_pipe(
    cmd: Vec<String>,
    cwd: Option<PathBuf>,
    env: HashMap<String, String>,
) -> Result<TtyHandle, TtyError> {
    if cmd.is_empty() {
        return Err(TtyError::AllocFailed {
            message: "empty command vector".to_string(),
        });
    }

    let mut command = Command::new(&cmd[0]);
    command.args(&cmd[1..]);
    command.stdin(std::process::Stdio::piped());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    command.kill_on_drop(true);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    for (k, v) in env {
        command.env(k, v);
    }

    let mut child = command.spawn().map_err(|e| TtyError::AllocFailed {
        message: format!("spawn failed: {e}"),
    })?;

    let pid = child.id();

    let stdin: Box<dyn tokio::io::AsyncWrite + Send + Unpin> = child
        .stdin
        .take()
        .map(|s: ChildStdin| Box::new(s) as Box<dyn tokio::io::AsyncWrite + Send + Unpin>)
        .ok_or_else(|| TtyError::AllocFailed {
            message: "child stdin not piped".to_string(),
        })?;

    let stdout: ChildStdout = child.stdout.take().ok_or_else(|| TtyError::AllocFailed {
        message: "child stdout not piped".to_string(),
    })?;
    let stderr: ChildStderr = child.stderr.take().ok_or_else(|| TtyError::AllocFailed {
        message: "child stderr not piped".to_string(),
    })?;

    let stdout: Pin<Box<dyn Stream<Item = Bytes> + Send>> =
        Box::pin(BytesStream::wrap(ReaderStream::new(stdout)));
    let stderr: Pin<Box<dyn Stream<Item = Bytes> + Send>> =
        Box::pin(BytesStream::wrap(ReaderStream::new(stderr)));

    let control = Some(TtyControlHandle::new(Arc::new(PipeControl::new(pid))));

    let exit_code: BoxFuture<Result<i32, TtyError>> = Box::pin(PipeExitFuture::new(child));

    Ok(TtyHandle {
        stdin,
        stdout,
        stderr: Some(stderr),
        exit_code,
        control,
    })
}

/// Adapter wrapping `tokio_util::io::ReaderStream<R>` (which yields
/// `Result<Bytes, io::Error>`) into a `Stream<Item = Bytes>`, dropping the
/// `io::Error` info by ending the stream on read error. The adapter pump
/// treats a stream end as EOF; a read error is indistinguishable from EOF
/// at the wire level (the `exit_code` future surfaces the failure if the
/// child died abnormally).
struct BytesStream<S> {
    inner: S,
}

impl<S> BytesStream<S> {
    fn wrap(inner: S) -> Self {
        Self { inner }
    }
}

impl<S, E> Stream for BytesStream<S>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
{
    type Item = Bytes;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Ready(Some(Ok(bytes))) => Poll::Ready(Some(bytes)),
            // Read error: end the stream (EOF to the adapter).
            Poll::Ready(Some(Err(_))) => Poll::Ready(None),
        }
    }
}

/// Control handle for pipe mode: no-op resize and `libc::kill(pid, sig)`
/// signal forwarding (the runner case — ADR-054).
///
/// # Signal scope — no process group (documented limitation)
///
/// Pipe mode has no session leader / controlling tty, so `signal()` targets
/// the direct child pid only via `libc::kill(pid, sig)`. If the child has
/// spawned its own children, they will NOT receive the signal — this is a
/// known limitation of the runner case. A runner that needs process-group
/// signal delivery uses the PTY case, not the pipe case. See
/// `tty-local.md` §"Pipe Mode".
pub struct PipeControl {
    pid: Option<u32>,
}

impl PipeControl {
    fn new(pid: Option<u32>) -> Self {
        Self { pid }
    }

    #[cfg(test)]
    pub(crate) fn pid(&self) -> Option<u32> {
        self.pid
    }
}

impl TtyControl for PipeControl {
    fn resize(&self, _cols: u16, _rows: u16, _pixel_width: u16, _pixel_height: u16) {
        // No-op — no PTY in pipe mode.
    }

    fn signal(&self, name: &str) {
        // Targets the direct child pid only (no process group — see the
        // type-level doc comment on `PipeControl`).
        #[cfg(unix)]
        {
            if let Some(pid) = self.pid {
                let pid = pid as libc::pid_t;
                match signal_from_name(name) {
                    Some(sig) => {
                        // SAFETY: libc::kill on a known pid with a valid
                        // signal number is the documented signal-forwarding
                        // path. Best-effort: ignore errors (child may be gone).
                        let _ = unsafe { libc::kill(pid, sig) };
                    }
                    None => {
                        // Unknown name → SIGKILL. On Unix, `libc::kill(pid,
                        // SIGKILL)` is exactly what
                        // `tokio::process::Child::start_kill()` does
                        // internally; the `Child` handle lives in the
                        // `PipeExitFuture` and isn't accessible here, so we
                        // kill by pid instead.
                        let _ = unsafe { libc::kill(pid, libc::SIGKILL) };
                    }
                }
            } else {
                tracing::warn!(name = name, "PipeControl::signal: no pid; cannot signal");
            }
        }
        #[cfg(not(unix))]
        {
            // Non-Unix: no `libc::kill` available, and the `Child` handle
            // lives in the `PipeExitFuture` (not accessible from here).
            // Dropping the `TtyHandle` triggers the ADR-056 kill guard.
            let _ = name;
            tracing::warn!(
                name = name,
                "PipeControl::signal: non-Unix; drop TtyHandle to kill"
            );
        }
    }
}

/// The `exit_code` future for pipe mode: wraps `tokio::process::Child::wait`
/// plus the ADR-056 kill guard.
///
/// # ADR-056 — kill-on-Drop contract
///
/// On cancel (the future is dropped without being driven to completion),
/// `Drop` calls `Child::start_kill()` (SIGKILL); the child exits and the
/// inner `wait` future reaps it. On the happy path, `poll` resolves the
/// `wait` future, then **disarms** the guard by taking the `Child` out of
/// `self.child` — so the subsequent `Drop` is a no-op. The guard is the
/// spec-compliant mechanism; `kill_on_drop(true)` on the `Command` is a
/// defense-in-depth backstop, not the primary path (the `Child` is moved
/// into the future and the future's `Drop` is the cancel path, not the
/// `Child`'s).
///
/// # Implementation note
///
/// `tokio::process::Child::wait()` borrows `&mut self`, so the `wait` future
/// cannot be held alongside the `Child` in the same struct without a
/// self-referential borrow. Instead, the `Child` lives in an `Option<Child>`
/// and is polled in place: on each `poll`, we `as_mut()` the `Option`,
/// construct a fresh `Child::wait()` borrow-future, pin it locally, and drive
/// it. The borrow ends when the local future is dropped at the end of the
/// `poll` call, so the `Child` remains free to be `take`n (disarmed or
/// killed) outside the borrow. This is the same pattern the standard
/// library's `tokio::process` examples use for "wait with a kill guard."
struct PipeExitFuture {
    child: Option<Child>,
}

impl PipeExitFuture {
    fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }
}

impl Future for PipeExitFuture {
    type Output = Result<i32, TtyError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Poll the wait future in an inner scope so the borrow of
        // `self.child` ends before we `take()` the child to disarm/kill.
        enum Step {
            Pending,
            Exited(ExitStatus),
            Failed(String),
            Taken,
        }
        let step = match self.child.as_mut() {
            None => Step::Taken,
            Some(child) => {
                let wait = child.wait();
                tokio::pin!(wait);
                match Future::poll(Pin::new(&mut wait), cx) {
                    Poll::Pending => Step::Pending,
                    Poll::Ready(Ok(status)) => Step::Exited(status),
                    Poll::Ready(Err(e)) => Step::Failed(format!("wait failed: {e}")),
                }
            }
        };
        // The borrow of `self.child` ended with `step`; now we can `take()`.
        match step {
            Step::Pending => Poll::Pending,
            Step::Exited(status) => {
                self.child.take();
                Poll::Ready(Ok(exit_code_from(status)))
            }
            Step::Failed(msg) => {
                self.child.take();
                Poll::Ready(Err(TtyError::WaitFailed { message: msg }))
            }
            Step::Taken => Poll::Ready(Err(TtyError::WaitFailed {
                message: "child already taken".to_string(),
            })),
        }
    }
}

impl Drop for PipeExitFuture {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // Best-effort kill; the child may already be exiting.
            let _ = child.start_kill();
        }
    }
}

/// Map an `ExitStatus` to the adapter's `i32` exit code: `code()` on normal
/// exit; on Unix, the negative signal number on signal-terminated exit
/// (matches `std` convention and ADR-055 §4). Falls back to `-1` (the
/// adapter's "could not determine" sentinel — ADR-055 §4).
fn exit_code_from(status: ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return -sig;
        }
    }
    -1
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;
    use tokio_stream::StreamExt;

    fn sh_cmd(argv: &[&str]) -> Vec<String> {
        argv.iter().map(|s| s.to_string()).collect()
    }

    /// Drain a `Stream<Item = Bytes>` into a `Vec<u8>`.
    async fn drain(mut s: Pin<Box<dyn Stream<Item = Bytes> + Send>>) -> Vec<u8> {
        let mut out = Vec::new();
        while let Some(chunk) = s.next().await {
            out.extend_from_slice(&chunk);
        }
        out
    }

    /// Swap a `Pin<Box<dyn Stream<Item = Bytes> + Send>>` out for an empty
    /// stream, returning the original. (`TtyHandle.stdout` is not `Option`,
    /// so `.take()` isn't available; this is the equivalent.)
    fn swap_stdout(
        slot: &mut Pin<Box<dyn Stream<Item = Bytes> + Send>>,
    ) -> Pin<Box<dyn Stream<Item = Bytes> + Send>> {
        std::mem::replace(slot, Box::pin(tokio_stream::empty()))
    }

    #[tokio::test]
    async fn happy_path_echo() {
        let mut handle =
            allocate_pipe(sh_cmd(&["echo", "hello"]), None, HashMap::new()).expect("allocate");
        let stderr = handle.stderr.take().expect("stderr present");
        let stdout = swap_stdout(&mut handle.stdout);
        let out = drain(stdout).await;
        let err = drain(stderr).await;
        assert!(err.is_empty(), "stderr empty");
        let code = handle.exit_code.await.expect("exit");
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8_lossy(&out), "hello\n");
    }

    #[tokio::test]
    async fn stdin_round_trip_cat() {
        let mut handle = allocate_pipe(sh_cmd(&["cat"]), None, HashMap::new()).expect("allocate");
        handle
            .stdin
            .write_all(b"round-trip\n")
            .await
            .expect("write stdin");
        // Close stdin so `cat` sees EOF and exits. Replace with a sink
        // (`TtyHandle.stdin` is not `Option`; this drops the child stdin).
        handle.stdin = Box::new(tokio::io::sink());
        let stdout = swap_stdout(&mut handle.stdout);
        let out = drain(stdout).await;
        let code = handle.exit_code.await.expect("exit");
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8_lossy(&out), "round-trip\n");
    }

    #[tokio::test]
    async fn separate_stderr() {
        let mut handle = allocate_pipe(sh_cmd(&["sh", "-c", "echo err >&2"]), None, HashMap::new())
            .expect("allocate");
        let stderr = handle.stderr.take().expect("stderr present");
        let stdout = swap_stdout(&mut handle.stdout);
        let out = drain(stdout).await;
        assert!(out.is_empty(), "stdout empty");
        let err = drain(stderr).await;
        let code = handle.exit_code.await.expect("exit");
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8_lossy(&err), "err\n");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn signal_term_kills_child() {
        let handle =
            allocate_pipe(sh_cmd(&["sleep", "60"]), None, HashMap::new()).expect("allocate");
        let control = handle.control.clone().expect("control present");
        control.signal("TERM");
        let code = handle.exit_code.await.expect("exit");
        assert_eq!(code, -15, "SIGTERM = -15");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancel_cleanup_drops_kill_child() {
        // Use a temp file to capture the child's pid, then drop the handle
        // (which drops exit_code → the ADR-056 guard sends SIGKILL) and
        // assert the process no longer exists via `kill(pid, 0)`.
        let pid_file = std::env::temp_dir().join(format!(
            "alknet_pipe_cancel_pid_{}_{}.txt",
            std::process::id(),
            rand_seed()
        ));
        let cmd = sh_cmd(&[
            "sh",
            "-c",
            &format!("echo $$ > '{}'; exec sleep 60", pid_file.display()),
        ]);
        let handle = allocate_pipe(cmd, None, HashMap::new()).expect("allocate");
        // Wait for the shell to write its pid.
        for _ in 0..100 {
            if pid_file.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let pid_str = std::fs::read_to_string(&pid_file).expect("pid file written");
        let pid: i32 = pid_str.trim().parse().expect("pid parses");
        let _ = std::fs::remove_file(&pid_file);
        // Drop the handle without awaiting exit_code — the ADR-056 guard
        // must kill the child.
        drop(handle);
        // Give the kernel a moment to deliver SIGKILL and reap.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        // kill(pid, 0) returns ESRCH (no such process) when the child is gone.
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        assert!(!alive, "child (pid={pid}) should be killed after drop");
    }

    #[tokio::test]
    async fn resize_is_noop() {
        let handle = allocate_pipe(sh_cmd(&["true"]), None, HashMap::new()).expect("allocate");
        let control = handle.control.as_ref().expect("control present");
        control.resize(120, 40, 800, 600);
        control.resize(0, 0, 0, 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unknown_signal_falls_back_to_sigkill() {
        let handle =
            allocate_pipe(sh_cmd(&["sleep", "60"]), None, HashMap::new()).expect("allocate");
        let control = handle.control.clone().expect("control present");
        // Unknown name → SIGKILL fallback (equivalent to start_kill()).
        control.signal("NOSUCH");
        let code = handle.exit_code.await.expect("exit");
        assert_eq!(code, -9, "SIGKILL = -9");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pipe_control_pid_recorded() {
        let ctrl = PipeControl::new(Some(42));
        assert_eq!(ctrl.pid(), Some(42));
        let ctrl_none = PipeControl::new(None);
        assert_eq!(ctrl_none.pid(), None);
    }

    #[test]
    fn empty_command_returns_alloc_failed() {
        let result = allocate_pipe(vec![], None, HashMap::new());
        assert!(
            matches!(result, Err(TtyError::AllocFailed { .. })),
            "expected AllocFailed"
        );
    }

    fn rand_seed() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    }
}
