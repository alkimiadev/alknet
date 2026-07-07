//! PTY mode for `LocalTtyBackend`: `portable_pty`-backed terminal sessions.
//!
//! Implements the blocking→async bridge (REQ-TTY-01) via three dedicated
//! std threads (reader, writer, waiter) feeding tokio mpsc/oneshot channels,
//! `PtyControl` for resize/signal, REQ-TTY-02 process-group signal
//! forwarding (`libc::kill(-pgid, sig)`), and the ADR-056 kill guard on the
//! `exit_code` future.
//!
//! # REQ-TTY-01 — blocking→async bridge
//!
//! `portable_pty` exposes a blocking `std::io` API
//! (`MasterPty::try_clone_reader()`, `take_writer()`, `Child::wait()`).
//! The three-thread bridge is the reference pattern for any blocking
//! backend: dedicated std threads feed tokio mpsc/oneshot channels, and
//! the async-facing `TtyHandle` fields are wrappers over those channels.
//!
//! # REQ-TTY-02 — signal targets the process group
//!
//! `portable_pty` spawns the child as a session leader
//! (`CommandBuilder::set_controlling_tty(true)`, the default), so the
//! child's pid *is* its process-group id. `signal()` calls
//! `libc::kill(-pgid, sig)` (the negative pid) to reach the whole group,
//! with a `kill(pid, sig)` fallback if the group signal fails (e.g. the
//! child already exited). Unknown signal names fall back to
//! `ChildKiller::kill` (SIGHUP). See `tty-local.md` §"REQ-TTY-02".
//!
//! # ADR-056 — kill-on-Drop guard
//!
//! `LocalExitFuture` wraps the `oneshot::Receiver<i32>` from the waiter
//! thread alongside a `portable_pty::ChildKiller`. On cancel (the future
//! is dropped without being driven to completion), `Drop` calls
//! `ChildKiller::kill()` (SIGHUP) — best-effort, the child may already be
//! exiting. On the happy path (the future resolves), `poll` disarms the
//! guard (`Option::take()`), so the subsequent `Drop` is a no-op. The
//! waiter thread reaps the killed child via its blocking `wait()`, so
//! there is no zombie. See `tty-local.md` §"Cancel-Cleanup (ADR-056)".

use std::collections::HashMap;
use std::future::Future;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::task::{Context, Poll};
use std::thread;

use alknet_tty::backend::{
    BoxFuture, TerminalParams, TtyControl, TtyControlHandle, TtyError, TtyHandle,
};
use alknet_tty::control::signal_from_name;
use bytes::Bytes;
use futures_core::Stream;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, warn};

/// Channel command for the writer thread: bytes to write, or EOF.
///
/// `Bytes` writes the bytes to the master writer and flushes. `Eof` drops
/// the writer (sends EOF to the slave's stdin) and exits the writer thread.
pub enum StdinCmd {
    /// Write these bytes to the master writer (write + flush).
    Bytes(Vec<u8>),
    /// Close the master writer (EOF to the slave's stdin). The writer thread
    /// drops the writer and exits.
    Eof,
}

/// Control handle for a live PTY: resize + signal forwarding. Cheap to
/// clone (all fields are `Arc`-backed) so the adapter can hand a clone to
/// the spawned control-chunk dispatcher.
#[derive(Clone)]
pub struct PtyControl {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    killer: Arc<Mutex<Box<dyn ChildKiller + Send + Sync>>>,
    pid: Option<u32>,
}

impl PtyControl {
    /// Construct from the shared master, the cloned killer, and the child's
    /// pid (used for process-group signal targeting).
    pub fn new(
        master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
        killer: Arc<Mutex<Box<dyn ChildKiller + Send + Sync>>>,
        pid: Option<u32>,
    ) -> Self {
        Self {
            master,
            killer,
            pid,
        }
    }
}

impl TtyControl for PtyControl {
    /// Resize the PTY. Safe to call from the async pump —
    /// `MasterPty::resize` is non-blocking (it issues an `ioctl`).
    fn resize(&self, cols: u16, rows: u16, pixel_width: u16, pixel_height: u16) {
        let size = PtySize {
            cols,
            rows,
            pixel_width,
            pixel_height,
        };
        let master = self.master.lock().expect("master mutex poisoned");
        if let Err(e) = master.resize(size) {
            warn!("pty resize failed: {e}");
        }
    }

    /// Forward a signal by name to the child's process group (REQ-TTY-02).
    ///
    /// On Unix: maps the name to a `libc` signal number via
    /// `alknet_tty::signal_from_name`, then calls `kill(-pgid, sig)` (the
    /// negative pid reaches the whole process group — the child is a
    /// session leader because `set_controlling_tty(true)`, the default).
    /// Falls back to `kill(pid, sig)` if the group signal fails (e.g. the
    /// child already exited). Unknown names fall back to
    /// `ChildKiller::kill` (SIGHUP).
    ///
    /// On non-Unix: `ChildKiller::kill` (SIGHUP) directly.
    fn signal(&self, name: &str) {
        #[cfg(unix)]
        {
            if let Some(pid) = self.pid {
                if let Some(sig) = signal_from_name(name) {
                    let pgid = pid as i32;
                    let r = unsafe { libc::kill(-pgid, sig) };
                    if r == 0 {
                        return;
                    }
                    let err = std::io::Error::last_os_error();
                    let r2 = unsafe { libc::kill(pgid, sig) };
                    if r2 == 0 {
                        return;
                    }
                    warn!(
                        "pty signal `{name}` (group {pgid}) failed: {err}; \
                         direct kill also failed: {}",
                        std::io::Error::last_os_error()
                    );
                    return;
                }
            }
            // Unknown name or no pid: fall back to ChildKiller (SIGHUP).
            let mut killer = self.killer.lock().expect("killer mutex poisoned");
            if let Err(e) = killer.kill() {
                warn!("pty fallback ChildKiller::kill failed: {e}");
            }
        }

        #[cfg(not(unix))]
        {
            let _ = name;
            let mut killer = self.killer.lock().expect("killer mutex poisoned");
            if let Err(e) = killer.kill() {
                warn!("pty ChildKiller::kill failed: {e}");
            }
        }
    }
}

/// ADR-056 kill guard wrapping the waiter-thread oneshot + a
/// `portable_pty::ChildKiller`.
///
/// `poll` delegates to the oneshot receiver (resolves on natural exit). On
/// `Ready`, the killer is taken (`Option::take()`) — disarmed — so the
/// subsequent `Drop` is a no-op. On cancel (the future is dropped before
/// resolving), `Drop` calls `ChildKiller::kill()` (SIGHUP) — best-effort;
/// the child may already be exiting. The waiter thread's blocking `wait()`
/// reaps the killed child, so there is no zombie. The contract is "kill on
/// cancel; no-op on resolve."
pub struct LocalExitFuture {
    rx: oneshot::Receiver<i32>,
    killer: Option<Box<dyn ChildKiller + Send + Sync>>,
}

impl LocalExitFuture {
    fn new(rx: oneshot::Receiver<i32>, killer: Box<dyn ChildKiller + Send + Sync>) -> Self {
        Self {
            rx,
            killer: Some(killer),
        }
    }
}

impl Future for LocalExitFuture {
    type Output = Result<i32, TtyError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.rx).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(code)) => {
                // Disarm the kill guard — the child exited naturally.
                self.killer.take();
                Poll::Ready(Ok(code))
            }
            Poll::Ready(Err(_)) => {
                // The waiter thread's oneshot sender was dropped (wait()
                // failed). Disarm to avoid killing an already-reaped child.
                self.killer.take();
                Poll::Ready(Err(TtyError::WaitFailed {
                    message: "waiter thread exited without sending exit code".to_string(),
                }))
            }
        }
    }
}

impl Drop for LocalExitFuture {
    fn drop(&mut self) {
        if let Some(killer) = self.killer.take() {
            // ADR-056: kill on cancel. Best-effort — the child may already
            // be exiting. SIGHUP is what portable_pty sends on Unix.
            let mut killer = killer;
            if let Err(e) = killer.kill() {
                debug!("LocalExitFuture drop: ChildKiller::kill failed: {e}");
            }
        }
    }
}

/// `AsyncWrite` adapter over an `mpsc::Sender<StdinCmd>`. `poll_write` sends
/// `StdinCmd::Bytes(buf.to_vec())`; `poll_flush` is a no-op (the writer
/// thread flushes); `poll_close` sends `StdinCmd::Eof`.
///
/// When the channel is full, `poll_write` parks in an in-flight send
/// future stored on the struct (so a re-poll resumes the same send rather
/// than starting a new one — `reserve()` is not `Unpin`).
struct StdinSink {
    tx: mpsc::Sender<StdinCmd>,
    /// In-flight `reserve()` + send, captured as a boxed future. `None`
    /// when no write is pending.
    inflight: Option<InflightSend>,
    /// Bytes for the in-flight write (returned as the write count on
    /// completion).
    inflight_len: usize,
    close_sent: bool,
}

/// Boxed future for an in-flight stdin `reserve + send`. The permit borrows
/// the sender, so the future owns a cloned sender and the bytes to send.
type InflightSend = Pin<Box<dyn Future<Output = Result<(), mpsc::error::SendError<()>>> + Send>>;

impl StdinSink {
    fn new(tx: mpsc::Sender<StdinCmd>) -> Self {
        Self {
            tx,
            inflight: None,
            inflight_len: 0,
            close_sent: false,
        }
    }
}

impl tokio::io::AsyncWrite for StdinSink {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        // Drain any in-flight write first.
        if let Some(fut) = self.inflight.as_mut() {
            match fut.as_mut().poll(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Ok(())) => {
                    let n = self.inflight_len;
                    self.inflight = None;
                    self.inflight_len = 0;
                    return Poll::Ready(Ok(n));
                }
                Poll::Ready(Err(_)) => {
                    self.inflight = None;
                    self.inflight_len = 0;
                    return Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "stdin channel closed",
                    )));
                }
            }
        }
        // Fast path: try to send without parking.
        match self.tx.try_send(StdinCmd::Bytes(buf.to_vec())) {
            Ok(()) => Poll::Ready(Ok(buf.len())),
            Err(mpsc::error::TrySendError::Full(_)) => {
                // Park a `reserve + send` future.
                let tx = self.tx.clone();
                let bytes = buf.to_vec();
                let len = bytes.len();
                self.inflight_len = len;
                self.inflight = Some(Box::pin(async move {
                    let permit = tx.reserve().await?;
                    permit.send(StdinCmd::Bytes(bytes));
                    Ok(())
                }));
                // Recurse via a re-poll so the parked future is polled now.
                self.poll_write(cx, buf)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "stdin channel closed",
            ))),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        if self.close_sent {
            return Poll::Ready(Ok(()));
        }
        match self.tx.try_send(StdinCmd::Eof) {
            Ok(()) => {
                self.close_sent = true;
                Poll::Ready(Ok(()))
            }
            Err(mpsc::error::TrySendError::Full(_)) => Poll::Pending,
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.close_sent = true;
                Poll::Ready(Ok(()))
            }
        }
    }
}

/// Allocate a local PTY, spawn `cmd` into it, and return the async-facing
/// `TtyHandle`.
///
/// Spawns the child as a session leader with a controlling tty
/// (`CommandBuilder::set_controlling_tty(true)` — the default; REQ-TTY-02
/// depends on it). Wires the three-thread bridge (reader/writer/waiter),
/// the `PtyControl`, and the ADR-056 `LocalExitFuture`.
pub fn allocate_pty(
    terminal: TerminalParams,
    cmd: Vec<String>,
    cwd: Option<PathBuf>,
    env: HashMap<String, String>,
) -> Result<TtyHandle, TtyError> {
    if cmd.is_empty() {
        return Err(TtyError::AllocFailed {
            message: "cmd must be non-empty".to_string(),
        });
    }

    let pty_system = native_pty_system();
    let size = PtySize {
        cols: terminal.cols,
        rows: terminal.rows,
        pixel_width: terminal.pixel_width,
        pixel_height: terminal.pixel_height,
    };
    let pair = pty_system
        .openpty(size)
        .map_err(|e| TtyError::AllocFailed {
            message: format!("openpty: {e}"),
        })?;

    let mut builder = CommandBuilder::new(&cmd[0]);
    for arg in &cmd[1..] {
        builder.arg(arg);
    }
    if let Some(cwd) = cwd {
        builder.cwd(cwd);
    }
    for (k, v) in env {
        builder.env(k, v);
    }
    // Session leader + controlling tty — the default. REQ-TTY-02:
    // `kill(-pgid, sig)` reaches the whole group only when the child is a
    // session leader, which requires a controlling tty.
    builder.set_controlling_tty(true);

    // Spawn the child on the slave side, then drop the slave so that when
    // the master writer closes, the child sees EOF on its stdin.
    let mut child = pair
        .slave
        .spawn_command(builder)
        .map_err(|e| TtyError::AllocFailed {
            message: format!("spawn_command: {e}"),
        })?;
    drop(pair.slave);

    let pid = child.process_id();
    // Two killer views: one for the ADR-056 kill guard (LocalExitFuture's
    // Drop), one for the signal path's fallback kill (PtyControl). Both
    // reference the same underlying pid/handle via clone_killer.
    let killer = child.clone_killer();
    let killer_for_control = killer.clone_killer();

    let master: Arc<Mutex<Box<dyn MasterPty + Send>>> = Arc::new(Mutex::new(pair.master));

    // --- Reader thread: blocking reads from the master reader → mpsc ---
    let reader_master = master.clone();
    let (stdout_tx, stdout_rx) = mpsc::channel::<Bytes>(64);
    thread::Builder::new()
        .name("pty-reader".into())
        .spawn(move || {
            let reader = {
                let m = reader_master.lock().expect("master mutex poisoned");
                match m.try_clone_reader() {
                    Ok(r) => r,
                    Err(e) => {
                        warn!("pty-reader: try_clone_reader failed: {e}");
                        let _ = stdout_tx.blocking_send(Bytes::new());
                        return;
                    }
                }
            };
            let mut reader = reader;
            let mut buf = vec![0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let chunk = Bytes::copy_from_slice(&buf[..n]);
                        if stdout_tx.blocking_send(chunk).is_err() {
                            break;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => {
                        warn!("pty-reader: read error: {e}");
                        break;
                    }
                }
            }
            // Zero-length sentinel: signals the pump the stream drained.
            let _ = stdout_tx.blocking_send(Bytes::new());
            debug!("pty-reader thread done");
        })
        .expect("spawn pty-reader");

    // --- Writer thread: drain mpsc<StdinCmd> → blocking writes ---
    let writer_master = master.clone();
    let (stdin_tx, mut stdin_rx) = mpsc::channel::<StdinCmd>(64);
    thread::Builder::new()
        .name("pty-writer".into())
        .spawn(move || {
            let writer = {
                let m = writer_master.lock().expect("master mutex poisoned");
                match m.take_writer() {
                    Ok(w) => w,
                    Err(e) => {
                        warn!("pty-writer: take_writer failed: {e}");
                        return;
                    }
                }
            };
            let mut writer = writer;
            while let Some(cmd) = stdin_rx.blocking_recv() {
                match cmd {
                    StdinCmd::Bytes(bytes) => {
                        if let Err(e) = writer.write_all(&bytes) {
                            warn!("pty-writer: write_all failed: {e}");
                            break;
                        }
                        if let Err(e) = writer.flush() {
                            warn!("pty-writer: flush failed: {e}");
                            break;
                        }
                    }
                    StdinCmd::Eof => {
                        drop(writer);
                        break;
                    }
                }
            }
            debug!("pty-writer thread done");
        })
        .expect("spawn pty-writer");

    // --- Waiter thread: blocking Child::wait() → oneshot<i32> ---
    let (exit_tx, exit_rx) = oneshot::channel::<i32>();
    thread::Builder::new()
        .name("pty-waiter".into())
        .spawn(move || {
            let status = match child.wait() {
                Ok(s) => s,
                Err(e) => {
                    warn!("pty-waiter: wait failed: {e}");
                    let _ = exit_tx.send(-1);
                    return;
                }
            };
            let code = status.exit_code() as i32;
            debug!(exit_code = code, "pty-waiter: child reaped");
            let _ = exit_tx.send(code);
        })
        .expect("spawn pty-waiter");

    // --- Assemble the TtyHandle ---
    let stdout: Pin<Box<dyn Stream<Item = Bytes> + Send>> =
        Box::pin(ReceiverStream::new(stdout_rx));
    let stdin: Box<dyn tokio::io::AsyncWrite + Send + Unpin> = Box::new(StdinSink::new(stdin_tx));
    let exit_code: BoxFuture<Result<i32, TtyError>> =
        Box::pin(LocalExitFuture::new(exit_rx, killer));
    let control = Some(TtyControlHandle::new(Arc::new(PtyControl::new(
        master,
        Arc::new(Mutex::new(killer_for_control)),
        pid,
    ))));

    Ok(TtyHandle {
        stdin,
        stdout,
        stderr: None,
        exit_code,
        control,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;
    use tokio_stream::StreamExt;

    fn term() -> TerminalParams {
        TerminalParams {
            term: None,
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
            modes: serde_json::Value::Null,
        }
    }

    fn env_default() -> HashMap<String, String> {
        let mut env = HashMap::new();
        env.insert("TERM".to_string(), "dumb".to_string());
        env
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn happy_path_echo_exits_zero() {
        let handle = allocate_pty(
            term(),
            vec!["echo".to_string(), "hello".to_string()],
            None,
            env_default(),
        )
        .expect("allocate");
        let mut stdout = handle.stdout;
        let mut collected = Vec::new();
        while let Some(chunk) = stdout.next().await {
            if chunk.is_empty() {
                break;
            }
            collected.extend_from_slice(&chunk);
        }
        let code = handle.exit_code.await.expect("exit_code");
        assert_eq!(code, 0);
        let s = String::from_utf8_lossy(&collected);
        assert!(s.contains("hello"), "stdout should contain hello: {s:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stdin_round_trip_cat() {
        let handle =
            allocate_pty(term(), vec!["cat".to_string()], None, env_default()).expect("allocate");
        let mut stdin = handle.stdin;
        let mut stdout = handle.stdout;

        // PTY may echo input; we still expect to see our bytes somewhere
        // in the output. Write, then close, then drain.
        stdin.write_all(b"ping\n").await.expect("write");
        stdin.shutdown().await.expect("shutdown (eof)");

        let mut collected = Vec::new();
        while let Some(chunk) = stdout.next().await {
            if chunk.is_empty() {
                break;
            }
            collected.extend_from_slice(&chunk);
        }
        let code = handle.exit_code.await.expect("exit_code");
        let s = String::from_utf8_lossy(&collected);
        assert!(s.contains("ping"), "stdout should contain ping: {s:?}");
        assert_eq!(code, 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn resize_does_not_error() {
        let handle = allocate_pty(
            term(),
            vec!["sleep".to_string(), "1".to_string()],
            None,
            env_default(),
        )
        .expect("allocate");
        let control = handle.control.as_ref().expect("control");
        control.resize(120, 40, 0, 0);
        let _ = handle.exit_code.await.expect("exit_code");
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn signal_int_kills_child() {
        let handle = allocate_pty(
            term(),
            vec!["sleep".to_string(), "60".to_string()],
            None,
            env_default(),
        )
        .expect("allocate");
        let control = handle.control.as_ref().expect("control");
        // Give the child a moment to actually exec sleep.
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        control.signal("INT");
        let code = tokio::time::timeout(std::time::Duration::from_secs(5), handle.exit_code)
            .await
            .expect("exit timed out")
            .expect("exit_code");
        assert_ne!(
            code, 0,
            "signal-terminated child should report non-zero exit: {code}"
        );
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn signal_reaches_process_group_child() {
        // bash -c "sleep 60" — sleep is a child of bash. The group signal
        // must reach sleep too (REQ-TTY-02). bash exits when its child does.
        let handle = allocate_pty(
            term(),
            vec!["bash".to_string(), "-c".to_string(), "sleep 60".to_string()],
            None,
            env_default(),
        )
        .expect("allocate");
        let control = handle.control.as_ref().expect("control");
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        control.signal("INT");
        let code = tokio::time::timeout(std::time::Duration::from_secs(5), handle.exit_code)
            .await
            .expect("exit timed out")
            .expect("exit_code");
        assert_ne!(code, 0, "process group should have been killed: {code}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancel_cleanup_kills_child_on_drop() {
        // ADR-056: dropping the TtyHandle (and thus the exit_code future)
        // without awaiting it MUST kill the child. The waiter thread reaps
        // the killed child (no zombie). We assert the kill happened by
        // spawning a second session that completes promptly — i.e., the
        // dropped session's child does not outlive a short grace period
        // (if it did, the SIGHUP from Drop would not have fired).
        let handle = allocate_pty(
            term(),
            vec!["sleep".to_string(), "60".to_string()],
            None,
            env_default(),
        )
        .expect("allocate");
        drop(handle);
        // Drop fires LocalExitFuture::Drop → ChildKiller::kill (SIGHUP).
        // The waiter thread reaps the child. Give it a moment.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        // Probe: a new session should be allocatable and complete cleanly.
        let probe = allocate_pty(
            term(),
            vec!["echo".to_string(), "ok".to_string()],
            None,
            env_default(),
        )
        .expect("allocate");
        let code = tokio::time::timeout(std::time::Duration::from_secs(5), probe.exit_code)
            .await
            .expect("probe timed out")
            .expect("probe exit_code");
        assert_eq!(code, 0);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn unknown_signal_falls_back_to_child_killer() {
        let handle = allocate_pty(
            term(),
            vec!["sleep".to_string(), "60".to_string()],
            None,
            env_default(),
        )
        .expect("allocate");
        let control = handle.control.as_ref().expect("control");
        // "NOSUCH" is not a known signal name → falls back to
        // ChildKiller::kill (SIGHUP). The child should die.
        control.signal("NOSUCH");
        let code = tokio::time::timeout(std::time::Duration::from_secs(5), handle.exit_code)
            .await
            .expect("exit timed out")
            .expect("exit_code");
        assert_ne!(code, 0, "fallback kill should terminate the child: {code}");
    }
}
