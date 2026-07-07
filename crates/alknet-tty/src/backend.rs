//! Backend trait and handle shapes: `TtyBackend`, `TtyHandle`, `TtyControl`,
//! `TtyControlHandle`, `TtyParams`, `TerminalParams`, and `TtyError`.
//!
//! This is the inversion point (ADR-053) between the wire-format adapter
//! (`crate::adapter`) and the backend crates (`alknet-tty-local`, future
//! `alknet-docker`, `alknet-ssh`). alknet-tty defines the trait; the
//! backends implement it. The trait shape is a one-way door — changing it
//! after backends exist is a rewrite across crates. The adapter holds a
//! `HashMap<String, Arc<dyn TtyBackend>>` keyed by the negotiation frame's
//! `backend` string and pumps the `TtyHandle` fields bidirectionally;
//! backends produce handles, they do not write to the wire.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures_core::Stream;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;

use crate::negotiation::{NegotiateRequest, TerminalParamsWire};

/// A boxed, sendable future used for the [`TtyHandle::exit_code`] field.
///
/// Equivalent to `futures::future::BoxFuture<'static, T>`; defined here as
/// `Pin<Box<dyn Future + Send>>` to avoid pulling in the `futures` crate
/// (the adapter runtime is tokio; `futures_core` provides only the
/// `Stream` trait, which is all this crate needs).
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// The error type for [`TtyBackend::allocate`] and the
/// [`TtyHandle::exit_code`] future.
///
/// `#[non_exhaustive]` so new variants are additive (two-way-door extension
/// within the one-way trait shape — ADR-053).
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum TtyError {
    /// The PTY couldn't be allocated, the docker exec failed to start, the
    /// SSH channel request was rejected. Returned by `allocate()`; the
    /// adapter sends `{"error":"allocate_failed",...}` and closes.
    #[error("allocate failed: {message}")]
    AllocFailed { message: String },
    /// The backend couldn't reap the child / determine the exit code.
    /// Returned by the `exit_code` future; the adapter sends
    /// `{"type":"exit","code":-1}` (ADR-055 §4).
    #[error("wait failed: {message}")]
    WaitFailed { message: String },
    /// An I/O error from a backend's stream/handle.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// A backend-specific error not covered by the above (e.g., a bollard
    /// API error, a russh protocol error).
    #[error("backend-specific: {message}")]
    Backend { message: String },
}

/// Terminal dimensions and mode hints for a PTY allocation
/// (`TtyParams::terminal: Some`).
///
/// `modes` is reserved (OQ-44); backends MUST ignore its content in v1.
#[derive(Debug, Clone)]
pub struct TerminalParams {
    /// `TERM` environment value (e.g., `"xterm-256color"`); `None` =
    /// backend default.
    pub term: Option<String>,
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
    /// Reserved — OQ-44; backends MUST ignore the content in v1.
    pub modes: serde_json::Value,
}

/// The allocation request the adapter passes to [`TtyBackend::allocate`].
///
/// `terminal: None` is pipe/runner mode (no PTY, separate stdout/stderr —
/// ADR-054). `terminal: Some` is PTY mode (stdout/stderr merged into
/// `stdout` by the kernel PTY, real terminal semantics).
///
/// `backend_params` is an opaque JSON object the adapter passes verbatim;
/// each backend deserializes its own strongly-typed params struct from it.
/// alknet-tty has zero knowledge of any backend's params shape. See ADR-053
/// §"Backend params are opaque."
#[derive(Debug, Clone)]
pub struct TtyParams {
    /// Terminal parameters. `None` = pipe mode (no PTY — ADR-054). `Some` =
    /// allocate a PTY with these dimensions.
    pub terminal: Option<TerminalParams>,
    /// Command vector (argv[0] + args). Non-empty.
    pub cmd: Vec<String>,
    /// Working directory (`None` = inherit/default).
    pub cwd: Option<PathBuf>,
    /// Environment variables (empty = inherit).
    pub env: HashMap<String, String>,
    /// Backend-specific selector fields from the negotiation frame,
    /// unparsed. The adapter passes the JSON object through verbatim; the
    /// backend deserializes its own strongly-typed params struct from it.
    /// alknet-tty has zero knowledge of any backend's params shape.
    pub backend_params: serde_json::Map<String, serde_json::Value>,
}

impl From<TerminalParamsWire> for TerminalParams {
    fn from(w: TerminalParamsWire) -> Self {
        Self {
            term: w.term,
            cols: w.cols,
            rows: w.rows,
            pixel_width: w.pixel_width,
            pixel_height: w.pixel_height,
            modes: w.modes,
        }
    }
}

/// Map a wire negotiation frame to the allocation request the backend
/// consumes. The adapter calls this after parsing the negotiation
/// carriage; `backend_params` is passed through verbatim (alknet-tty has
/// zero knowledge of any backend's params shape — ADR-053). Lives here so
/// the adapter does not hand-roll the mapping.
impl From<NegotiateRequest> for TtyParams {
    fn from(req: NegotiateRequest) -> Self {
        Self {
            terminal: req.tty.map(TerminalParams::from),
            cmd: req.cmd,
            cwd: req.cwd,
            env: req.env,
            backend_params: req.backend_params,
        }
    }
}

/// What a backend's `allocate()` produces. The adapter pumps these fields
/// bidirectionally against the wire format (ADR-052).
pub struct TtyHandle {
    /// Stdin writer — bytes the adapter pumps from client stdin chunks.
    /// `tokio::io::AsyncWrite` (the tokio flavor, not `futures::io` — they
    /// are incompatible traits; the tokio stack is the adapter's runtime).
    pub stdin: Box<dyn tokio::io::AsyncWrite + Send + Unpin>,
    /// Stdout stream — bytes the adapter pumps to client stdout chunks.
    /// Ends when the backend's stdout reaches EOF.
    pub stdout: Pin<Box<dyn Stream<Item = Bytes> + Send>>,
    /// Stderr stream — `None` for PTY backends (stdout/stderr merged into
    /// `stdout` by the kernel PTY). `Some` for pipe backends (separate
    /// streams).
    pub stderr: Option<Pin<Box<dyn Stream<Item = Bytes> + Send>>>,
    /// Exit code — a `Future` the adapter awaits. Resolves when the
    /// process/container/SSH exec exits. The adapter sends the result as
    /// the `{"type":"exit","code":N}` control chunk (ADR-055) and closes
    /// the stream. This is a `BoxFuture`, not a method on `TtyHandle`, so
    /// the adapter can `select` between exit and stream-close without
    /// coupling to the other fields (REQ-TTY-01).
    ///
    /// # ADR-056 — kill-on-Drop contract
    ///
    /// Dropping this future without driving it to completion MUST kill the
    /// session target (the child process, the docker exec, the SSH
    /// channel's process). The kill is best-effort (a no-op if the target
    /// already exited) but MUST be attempted even when the target is
    /// blocked in a state that ignores stdin EOF (a daemon, a process in
    /// uninterruptible sleep, a container whose process ignores channel
    /// close). The adapter triggers this by dropping the `TtyHandle` on
    /// session cancel (connection drop, stream reset, task panic); the
    /// backend wires the kill into this future's `Drop`-on-cancel guard
    /// (e.g. the local backend holds a `portable_pty::ChildKiller`; docker
    /// holds the container id + bollard client; SSH holds the russh
    /// channel). A backend that returns a bare `oneshot::Receiver<i32>`
    /// (or any future without a kill-on-`Drop` guard) as `exit_code`
    /// violates the contract and will orphan processes on cancel. The
    /// `Drop` MUST be a no-op when the future resolved normally (the
    /// adapter awaited it to completion). See ADR-056 and `tty-local.md`
    /// §"Cancel-Cleanup" for the local backend's mechanism.
    pub exit_code: BoxFuture<Result<i32, TtyError>>,
    /// Control handle (resize, signal) — `Clone` so the adapter can hand
    /// it to the spawned control-chunk dispatcher. `None` only when the
    /// backend genuinely has no control path. See OQ-43.
    pub control: Option<TtyControlHandle>,
}

/// Control path for a live terminal session: resize and signal forwarding.
///
/// Object-safe (`Send + Sync`, no `Clone` — `Clone` is not object-safe).
/// The `Clone`-ability lives on the [`TtyControlHandle`] newtype, which
/// holds the trait object behind an `Arc`. A backend produces its own
/// control type via `TtyControlHandle::new(Arc::new(MyControl))` without
/// the adapter knowing the concrete shape (OQ-43).
pub trait TtyControl: Send + Sync {
    /// Resize the terminal. Maps to SSH `window-change`, docker exec
    /// resize, or `ioctl(TIOCSWINSZ)` on a local PTY. No-op for pipe
    /// backends without a PTY (the adapter still calls it; the backend
    /// ignores).
    fn resize(&self, cols: u16, rows: u16, pixel_width: u16, pixel_height: u16);

    /// Forward a signal by name. Best-effort delivery to the foreground
    /// process group (see `tty-local.md` REQ-TTY-02). Unknown names fall
    /// back to the backend's default kill.
    fn signal(&self, name: &str);
}

/// The `Clone`-able handle to a backend's control path. The
/// [`TtyControl`] trait is NOT `Clone` (`Clone` is not object-safe —
/// `fn clone(&self) -> Self` returns `Self`, which forbids `dyn` dispatch);
/// the `Clone`-ability lives on this concrete newtype, which holds the
/// trait object behind an `Arc`. The adapter clones the `Arc` to hand a
/// handle to the spawned control-chunk dispatcher. A backend produces its
/// own control type via `TtyControlHandle::new(Arc::new(MyControl))`
/// without the adapter knowing the concrete shape. See OQ-43.
#[derive(Clone)]
pub struct TtyControlHandle(Arc<dyn TtyControl + Send + Sync>);

impl TtyControlHandle {
    /// Wrap a backend's control implementation. The backend typically
    /// calls `TtyControlHandle::new(Arc::new(MyControl))` inside its
    /// `allocate()`.
    pub fn new(control: Arc<dyn TtyControl + Send + Sync>) -> Self {
        Self(control)
    }

    /// Resize the terminal. Delegates to the inner [`TtyControl`].
    pub fn resize(&self, cols: u16, rows: u16, pixel_width: u16, pixel_height: u16) {
        self.0.resize(cols, rows, pixel_width, pixel_height);
    }

    /// Forward a signal by name. Delegates to the inner [`TtyControl`].
    pub fn signal(&self, name: &str) {
        self.0.signal(name);
    }
}

/// The backend inversion point (ADR-053). alknet-tty defines the trait;
/// the backend crates (`alknet-tty-local`, future `alknet-docker`,
/// `alknet-ssh`) implement it. The adapter holds a
/// `HashMap<String, Arc<dyn TtyBackend>>` keyed by the negotiation frame's
/// `backend` string and dispatches by that key.
///
/// # REQ-TTY-01 — backends need not be natively async
///
/// The adapter-facing types this trait returns (`AsyncWrite`,
/// `Stream<Item = Bytes>`, `BoxFuture`, `TtyControl`) are the **adapter's
/// contract**. A backend may expose blocking handles internally (e.g.
/// `portable_pty`'s blocking `std::io::{Read, Write}` + `Child::wait()`)
/// and bridge them to these async-facing types via dedicated std threads
/// or `tokio::task::spawn_blocking` feeding tokio mpsc/oneshot channels.
/// This bridging pattern is a **documented, supported implementation
/// strategy**, not a workaround. The local backend (`alknet-tty-local`) is
/// the reference implementation: it spawns reader/writer/waiter threads
/// that feed `mpsc::Receiver<Bytes>` (stdout), an `AsyncWrite` adapter
/// over `mpsc::Sender<StdinCmd>` (stdin), and a `oneshot::Receiver<i32>`
/// wrapped in a kill-guard future (exit). The adapter consumes the bridged
/// async-facing types and is unaware of the threading.
///
/// # ADR-056 — kill-on-Drop contract
///
/// The [`TtyHandle::exit_code`] future returned by `allocate()` MUST kill
/// the session target when dropped without being driven to completion. See
/// the doc comment on [`TtyHandle::exit_code`] for the full contract.
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
    /// (e.g., `DockerTtyBackend` returns `Some(("container", id))`). The
    /// adapter calls this at negotiation to gate access; the backend
    /// extracts the id from its own `backend_params`. Default `None`
    /// (most backends create their own resource).
    fn resource_id(&self, _params: &TtyParams) -> Option<(&'static str, String)> {
        None
    }
}

/// Mock `TtyControl` for tests. Records the last resize/signal call so
/// tests can assert delegation through [`TtyControlHandle`].
#[derive(Default)]
pub struct MockControl {
    pub last_resize: std::sync::Mutex<Option<(u16, u16, u16, u16)>>,
    pub last_signal: std::sync::Mutex<Option<String>>,
}

impl TtyControl for MockControl {
    fn resize(&self, cols: u16, rows: u16, pixel_width: u16, pixel_height: u16) {
        *self.last_resize.lock().expect("resize mutex poisoned") =
            Some((cols, rows, pixel_width, pixel_height));
    }

    fn signal(&self, name: &str) {
        *self.last_signal.lock().expect("signal mutex poisoned") = Some(name.to_string());
    }
}

/// In-memory `TtyBackend` for tests. `allocate()` wires tokio mpsc
/// channels for stdin/stdout/stderr, a oneshot for `exit_code`, and a
/// mock [`TtyControl`]. The caller can drive the channels directly or via
/// the adapter pump. Use [`MockBackend::with_exit_code`] to fix the exit
/// code the handle resolves to.
#[derive(Default)]
pub struct MockBackend {
    pub exit_code: Option<i32>,
}

impl MockBackend {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_exit_code(exit_code: i32) -> Self {
        Self {
            exit_code: Some(exit_code),
        }
    }
}

#[async_trait]
impl TtyBackend for MockBackend {
    async fn allocate(&self, _params: &TtyParams) -> Result<TtyHandle, TtyError> {
        let (_stdout_tx, stdout_rx) = mpsc::channel::<Bytes>(8);
        let (_stderr_tx, stderr_rx) = mpsc::channel::<Bytes>(8);
        let (stdin_tx, _stdin_rx) = mpsc::channel::<Bytes>(8);
        let (exit_tx, exit_rx) = oneshot::channel::<Result<i32, TtyError>>();

        let code = self.exit_code.unwrap_or(0);
        tokio::spawn(async move {
            let _ = exit_tx.send(Ok(code));
        });

        let stdout: Pin<Box<dyn Stream<Item = Bytes> + Send>> =
            Box::pin(ReceiverStream::new(stdout_rx));
        let stderr: Option<Pin<Box<dyn Stream<Item = Bytes> + Send>>> =
            Some(Box::pin(ReceiverStream::new(stderr_rx)));
        let stdin: Box<dyn tokio::io::AsyncWrite + Send + Unpin> =
            Box::new(MockStdinSink { tx: stdin_tx });
        let control = Some(TtyControlHandle::new(Arc::new(MockControl::default())));

        let exit_code: BoxFuture<Result<i32, TtyError>> = Box::pin(async move {
            exit_rx
                .await
                .map_err(|_| TtyError::WaitFailed {
                    message: "exit_code sender dropped".to_string(),
                })
                .and_then(|r| r)
        });

        Ok(TtyHandle {
            stdin,
            stdout,
            stderr,
            exit_code,
            control,
        })
    }
}

/// `AsyncWrite` adapter over an `mpsc::Sender<Bytes>` — the mock
/// backend's stdin sink. Copies the buffer into a `Bytes` and best-effort
/// sends; on a full channel returns `Pending`, on a closed channel returns
/// a broken-pipe error.
struct MockStdinSink {
    tx: mpsc::Sender<Bytes>,
}

impl tokio::io::AsyncWrite for MockStdinSink {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        use std::task::Poll;
        match self.tx.try_reserve() {
            Ok(permit) => {
                permit.send(Bytes::copy_from_slice(buf));
                Poll::Ready(Ok(buf.len()))
            }
            Err(mpsc::error::TrySendError::Full(_)) => Poll::Pending,
            Err(mpsc::error::TrySendError::Closed(_)) => Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "stdin channel closed",
            ))),
        }
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        std::task::Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tty_control_handle_clone_delegates_resize_and_signal() {
        let control = Arc::new(MockControl::default());
        let handle = TtyControlHandle::new(control.clone());
        let handle_clone = handle.clone();

        handle.resize(80, 24, 0, 0);
        handle_clone.signal("HUP");

        let resize = control.last_resize.lock().expect("resize mutex poisoned");
        assert_eq!(*resize, Some((80, 24, 0, 0)));
        let signal = control.last_signal.lock().expect("signal mutex poisoned");
        assert_eq!(*signal, Some("HUP".to_string()));
    }

    #[tokio::test]
    async fn mock_backend_allocates_and_exits() {
        let backend = MockBackend::with_exit_code(42);
        let params = TtyParams {
            terminal: None,
            cmd: vec!["echo".to_string(), "hi".to_string()],
            cwd: None,
            env: HashMap::new(),
            backend_params: serde_json::Map::new(),
        };
        let handle = backend.allocate(&params).await.expect("allocate");
        assert!(handle.stderr.is_some());
        assert!(handle.control.is_some());
        let code = handle.exit_code.await.expect("exit_code");
        assert_eq!(code, 42);
    }

    #[tokio::test]
    async fn mock_backend_resource_id_default_none() {
        let backend = MockBackend::new();
        let params = TtyParams {
            terminal: None,
            cmd: vec!["true".to_string()],
            cwd: None,
            env: HashMap::new(),
            backend_params: serde_json::Map::new(),
        };
        assert!(backend.resource_id(&params).is_none());
    }

    #[test]
    fn tty_params_from_negotiate_request_maps_fields() {
        let req = NegotiateRequest {
            carriage: "raw".to_string(),
            backend: "local".to_string(),
            tty: Some(TerminalParamsWire {
                term: Some("xterm-256color".to_string()),
                cols: 80,
                rows: 24,
                pixel_width: 0,
                pixel_height: 0,
                modes: serde_json::Value::Null,
            }),
            cmd: vec!["bash".to_string()],
            cwd: Some(PathBuf::from("/tmp")),
            env: HashMap::from([("FOO".to_string(), "bar".to_string())]),
            backend_params: {
                let mut m = serde_json::Map::new();
                m.insert(
                    "container".to_string(),
                    serde_json::Value::String("abc".to_string()),
                );
                m
            },
        };
        let params = TtyParams::from(req);
        let term = params.terminal.expect("terminal");
        assert_eq!(term.term.as_deref(), Some("xterm-256color"));
        assert_eq!(term.cols, 80);
        assert_eq!(term.rows, 24);
        assert_eq!(params.cmd, vec!["bash".to_string()]);
        assert_eq!(params.cwd.as_deref(), Some(std::path::Path::new("/tmp")));
        assert_eq!(params.env.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(
            params
                .backend_params
                .get("container")
                .and_then(|v| v.as_str()),
            Some("abc"),
        );
    }

    #[test]
    fn tty_params_from_negotiate_request_pipe_mode() {
        let req = NegotiateRequest {
            carriage: "raw".to_string(),
            backend: "local".to_string(),
            tty: None,
            cmd: vec!["true".to_string()],
            cwd: None,
            env: HashMap::new(),
            backend_params: serde_json::Map::new(),
        };
        let params = TtyParams::from(req);
        assert!(params.terminal.is_none());
    }
}
