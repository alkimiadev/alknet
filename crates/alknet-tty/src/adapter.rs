//! `TtyAdapter` (`ProtocolHandler` on `alknet/tty`) and the `drive_session`
//! three-pump bidirectional driver.
//!
//! This is the integration point where the wire format (ADR-052), the
//! backend trait (ADR-053), and the exit-chunk ordering (ADR-055) come
//! together. The adapter is backend-agnostic; backends are
//! wire-format-agnostic. The inversion is the `TtyBackend` trait.
//!
//! # Session lifecycle
//!
//! A `alknet/tty` session on one bidi stream proceeds in three phases:
//!
//! 1. **Negotiation** — read a length-prefixed JSON frame, parse
//!    `NegotiateRequest`, validate (`carriage == "raw"`, `cmd` non-empty),
//!    look up the `TtyBackend`, run access control, construct `TtyParams`.
//!    Errors → JSON error response in negotiation framing, stream close.
//! 2. **Allocation** — `backend.allocate(&params)`. Errors →
//!    `allocate_failed` JSON error response, stream close.
//! 3. **Raw carriage** — three concurrent pumps:
//!    - **A. stdout → client**: `TtyHandle.stdout` → stdout chunks
//!      (stream_type 1); a concurrent stderr pump emits stderr chunks
//!      (stream_type 2) when `TtyHandle.stderr` is `Some`. On backend stdout
//!      EOF, emit a zero-length stdout sentinel.
//!    - **B. client → backend**: stdin chunks (stream_type 0) →
//!      `TtyHandle.stdin`; control chunks (stream_type 3) →
//!      `ControlMessage` dispatch (`Resize`, `Signal`, `Eof`; `Exit` is
//!      server→client only and ignored). Zero-length stdin chunk or
//!      read-half close → EOF to backend stdin.
//!    - **C. exit → exit chunk**: await `TtyHandle.exit_code`; on resolve,
//!      enqueue `{"type":"exit","code":N}` as a control chunk (stream_type
//!      3). On `TtyError` → `{"type":"exit","code":-1}`.
//!
//! The adapter enforces the **exit-chunk-is-last** invariant (ADR-055):
//! it waits for BOTH the stdout/stderr pumps to complete AND `exit_code`
//! to resolve before enqueueing the exit chunk. A drainer task writes
//! chunks to the client in arrival order; the exit chunk is last.
//!
//! # Cancel cleanup (ADR-056)
//!
//! On connection drop or stream reset, the pump tasks are dropped, which
//! drops the `TtyHandle`, which drops the `exit_code` future without
//! driving it to completion — the backend's kill-on-`Drop` guard fires
//! and kills the session target. The adapter has no separate kill
//! method; the cleanup is wired into the `exit_code` future's `Drop` by
//! the backend. A client closing the write half (stdin EOF) does NOT
//! trigger cancel-cleanup — the session runs to completion.

use std::collections::HashMap;
use std::sync::Arc;

use alknet_core::auth::{AuthContext, Identity};
use alknet_core::ownership::OwnershipProvider;
use alknet_core::types::{Connection, HandlerError, ProtocolHandler, StreamError};
use async_trait::async_trait;
use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tracing::{debug, warn};

use crate::backend::{TtyBackend, TtyHandle};
use crate::control::ControlMessage;
use crate::negotiation::{
    error_response_bytes, NegotiateRequest, NegotiationError, NegotiationReader, NegotiationWriter,
};
use crate::wire::{Chunk, ChunkReader, ChunkWriter, RawError, STREAM_CONTROL, STREAM_STDIN};

/// The scope required to open a `alknet/tty` session (ADR-050). A two-way-door
/// choice (reversible: a deployment-configured scope, not a wire-format
/// constant). Callers without this scope get a `forbidden` negotiation error.
pub const TTY_OPEN_SCOPE: &str = "tty:open";

/// The `ProtocolHandler` for `alknet/tty` (ADR-006, ADR-007). Holds a
/// `HashMap<String, Arc<dyn TtyBackend>>` keyed by the negotiation frame's
/// `backend` string and an optional [`OwnershipProvider`] for the ADR-050
/// resource-ownership check. `handle()` accepts the connection and loops
/// `accept_bi`, dispatching each bidi stream to a [`drive_session`] task.
///
/// One `alknet/tty` connection hosts multiple terminal sessions — one session
/// per bidi stream (DP-6). Sessions are independent: one session's exit
/// doesn't affect another.
pub struct TtyAdapter {
    backends: Arc<HashMap<String, Arc<dyn TtyBackend>>>,
    ownership: Option<Arc<dyn OwnershipProvider>>,
}

impl TtyAdapter {
    /// Construct with the given backend map and no ownership provider
    /// (scope-gate only — no resource-level ACL).
    pub fn new(backends: HashMap<String, Arc<dyn TtyBackend>>) -> Self {
        Self {
            backends: Arc::new(backends),
            ownership: None,
        }
    }

    /// Construct with the given backend map and an ownership provider for
    /// the ADR-050 resource-ownership check.
    pub fn with_ownership(
        backends: HashMap<String, Arc<dyn TtyBackend>>,
        ownership: Arc<dyn OwnershipProvider>,
    ) -> Self {
        Self {
            backends: Arc::new(backends),
            ownership: Some(ownership),
        }
    }
}

#[async_trait]
impl ProtocolHandler for TtyAdapter {
    fn alpn(&self) -> &'static [u8] {
        b"alknet/tty"
    }

    async fn handle(&self, connection: Connection, auth: &AuthContext) -> Result<(), HandlerError> {
        if let Some(identity) = auth.identity.clone() {
            let _ = connection.set_identity(identity);
        }
        loop {
            let stream = match connection.accept_bi().await {
                Ok(stream) => stream,
                Err(StreamError::ConnectionClosed) => break,
                Err(StreamError::StreamClosed) => break,
                Err(e) => return Err(HandlerError::from(e)),
            };
            let backends = self.backends.clone();
            let ownership = self.ownership.clone();
            let identity = auth.identity.clone();
            tokio::spawn(async move {
                // `stream` is a `BiStream` (ADR-092) — `AsyncRead + AsyncWrite
                // + Send + Unpin`. Split into halves for `drive_session`
                // (which takes separate `AsyncWrite` + `AsyncRead` args). The
                // split is the stdlib idiom for `TcpStream`-style duplex
                // streams; no per-handler wrapper.
                let (client_read, client_write) = tokio::io::split(stream);
                let _ =
                    drive_session(client_write, client_read, backends, ownership, identity).await;
            });
        }
        Ok(())
    }
}

/// Check whether `identity` has `scope` in its scopes list.
fn has_scope(identity: &Option<Identity>, scope: &str) -> bool {
    identity
        .as_ref()
        .map(|id| id.scopes.iter().any(|s| s == scope))
        .unwrap_or(false)
}

/// Send a negotiation error frame and close the write half. Consumes the
/// writer so the underlying transport's shutdown runs after the frame is
/// flushed.
async fn send_negotiation_error<W: AsyncWrite + Unpin>(
    mut writer: NegotiationWriter<W>,
    error: &str,
    fields: &[(&str, &str)],
) {
    match error_response_bytes(error, fields) {
        Ok(body) => {
            if let Err(e) = writer.write_frame(&body).await {
                debug!("tty: failed to write error frame: {e}");
            }
        }
        Err(e) => warn!("tty: failed to serialize error response: {e}"),
    }
    let _ = writer.into_inner().shutdown().await;
}

/// Drive a `alknet/tty` session end-to-end over a bidi stream.
///
/// `client_send` / `client_recv` are the two halves of the bidi stream
/// (split from the `BiStream` yielded by `accept_bi` via `tokio::io::split`).
/// Returns when the session is complete (exit chunk sent, stream closed) or
/// when the stream is reset (cancel-cleanup path — no exit chunk sent).
///
/// This is the per-stream session driver — the counterpart to the POC's
/// `session::drive_session` (`/workspace/alknet-tty-poc/src/session.rs`),
/// generalized from the local PTY backend to the [`TtyBackend`] trait.
pub async fn drive_session(
    client_send: impl AsyncWrite + Send + Unpin + 'static,
    client_recv: impl AsyncRead + Send + Unpin + 'static,
    backends: Arc<HashMap<String, Arc<dyn TtyBackend>>>,
    ownership: Option<Arc<dyn OwnershipProvider>>,
    identity: Option<Identity>,
) {
    if let Err(e) =
        drive_session_inner(client_send, client_recv, &backends, &ownership, &identity).await
    {
        debug!("tty: session ended with error: {e}");
    }
}

async fn drive_session_inner<W, R>(
    client_send: W,
    client_recv: R,
    backends: &HashMap<String, Arc<dyn TtyBackend>>,
    ownership: &Option<Arc<dyn OwnershipProvider>>,
    identity: &Option<Identity>,
) -> Result<(), std::io::Error>
where
    W: AsyncWrite + Send + Unpin + 'static,
    R: AsyncRead + Send + Unpin + 'static,
{
    let mut neg_reader = NegotiationReader::new(client_recv);
    let neg_writer = NegotiationWriter::new(client_send);

    let frame = match neg_reader.read_frame().await {
        Ok(f) => f,
        Err(NegotiationError::ConnectionClosed) => return Ok(()),
        Err(NegotiationError::Io(_)) => return Ok(()),
        Err(NegotiationError::FrameTooLarge(_)) => {
            send_negotiation_error(
                neg_writer,
                "malformed_negotiation",
                &[("message", "frame too large")],
            )
            .await;
            return Ok(());
        }
        Err(e) => {
            debug!("tty: negotiation read error: {e}");
            return Ok(());
        }
    };

    let req: NegotiateRequest = match serde_json::from_slice(&frame) {
        Ok(r) => r,
        Err(e) => {
            send_negotiation_error(
                neg_writer,
                "malformed_negotiation",
                &[("message", &e.to_string())],
            )
            .await;
            return Ok(());
        }
    };

    if req.carriage != "raw" {
        send_negotiation_error(
            neg_writer,
            "malformed_negotiation",
            &[("message", "carriage must be 'raw'")],
        )
        .await;
        return Ok(());
    }
    if req.cmd.is_empty() {
        send_negotiation_error(
            neg_writer,
            "malformed_negotiation",
            &[("message", "cmd must be non-empty")],
        )
        .await;
        return Ok(());
    }

    let backend = match backends.get(&req.backend) {
        Some(b) => b.clone(),
        None => {
            send_negotiation_error(neg_writer, "unknown_backend", &[("backend", &req.backend)])
                .await;
            return Ok(());
        }
    };

    if !has_scope(identity, TTY_OPEN_SCOPE) {
        send_negotiation_error(neg_writer, "forbidden", &[]).await;
        return Ok(());
    }

    let params = crate::backend::TtyParams::from(req);

    if let Some(provider) = ownership {
        if let Some((kind, id)) = backend.resource_id(&params) {
            let owns = identity
                .as_ref()
                .map(|id_ref| provider.owns(id_ref, kind, &id, "tty"))
                .unwrap_or(false);
            if !owns {
                send_negotiation_error(neg_writer, "forbidden", &[]).await;
                return Ok(());
            }
        }
    }

    let handle = match backend.allocate(&params).await {
        Ok(h) => h,
        Err(e) => {
            send_negotiation_error(
                neg_writer,
                "allocate_failed",
                &[("message", &e.to_string())],
            )
            .await;
            return Ok(());
        }
    };

    let client_read = neg_reader.into_inner();
    let client_write = neg_writer.into_inner();
    pump_session(client_write, client_read, handle).await
}

/// Phase 3: the bidirectional pump. Three concurrent tasks plus a drainer.
///
/// Enforces the exit-chunk-is-last invariant (ADR-055): the adapter waits for
/// BOTH the stdout/stderr pumps to complete AND `exit_code` to resolve
/// before enqueueing the exit chunk.
async fn pump_session<W, R>(
    client_write: W,
    client_read: R,
    handle: TtyHandle,
) -> Result<(), std::io::Error>
where
    W: AsyncWrite + Send + Unpin + 'static,
    R: AsyncRead + Send + Unpin + 'static,
{
    let (writer_tx, mut writer_rx) = mpsc::channel::<Chunk>(64);

    let TtyHandle {
        stdin,
        stdout,
        stderr,
        exit_code,
        control,
    } = handle;

    let writer_tx_out = writer_tx.clone();
    let stdout_pump = tokio::spawn(pump_stdout(stdout, writer_tx_out));

    let stderr_pump = if let Some(stderr) = stderr {
        let writer_tx_err = writer_tx.clone();
        Some(tokio::spawn(pump_stderr(stderr, writer_tx_err)))
    } else {
        None
    };

    let control_clone = control.clone();
    let input_pump = tokio::spawn(pump_client_to_backend(client_read, stdin, control_clone));

    let exit_future = async {
        let code = exit_code.await.unwrap_or(-1);
        code
    };

    match stderr_pump {
        Some(stderr_pump) => {
            let (_stdout_join, _stderr_join, exit_code_value) =
                tokio::join!(stdout_pump, stderr_pump, exit_future);
            send_exit_chunk(&writer_tx, exit_code_value).await;
        }
        None => {
            let (_stdout_join, exit_code_value) = tokio::join!(stdout_pump, exit_future);
            send_exit_chunk(&writer_tx, exit_code_value).await;
        }
    }

    drop(writer_tx);
    drop(input_pump);

    let mut chunk_writer = ChunkWriter::new(client_write);
    while let Some(chunk) = writer_rx.recv().await {
        if let Err(e) = chunk_writer.write_chunk(&chunk).await {
            debug!("tty: write_chunk to client failed: {e}");
            break;
        }
    }
    let _ = chunk_writer.into_inner().shutdown().await;
    debug!("tty: session complete");
    Ok(())
}

async fn send_exit_chunk(writer_tx: &mpsc::Sender<Chunk>, code: i32) {
    let exit_msg = ControlMessage::Exit { code };
    match exit_msg.to_json() {
        Ok(json) => {
            let chunk = Chunk::control(json);
            if writer_tx.send(chunk).await.is_err() {
                debug!("tty: writer channel closed before exit chunk");
            }
        }
        Err(e) => warn!("tty: failed to serialize exit control chunk: {e}"),
    }
}

/// Pump backend stdout → stdout chunks (stream_type 1). On backend stdout
/// EOF, emit a zero-length stdout sentinel.
async fn pump_stdout(
    mut stdout: std::pin::Pin<Box<dyn futures_core::Stream<Item = Bytes> + Send>>,
    writer_tx: mpsc::Sender<Chunk>,
) {
    while let Some(bytes) = stdout.next().await {
        if bytes.is_empty() {
            continue;
        }
        let chunk = Chunk::stdout(bytes);
        if writer_tx.send(chunk).await.is_err() {
            break;
        }
    }
    let _ = writer_tx.send(Chunk::stdout(Bytes::new())).await;
    debug!("tty: stdout pump done");
}

/// Pump backend stderr → stderr chunks (stream_type 2).
async fn pump_stderr(
    mut stderr: std::pin::Pin<Box<dyn futures_core::Stream<Item = Bytes> + Send>>,
    writer_tx: mpsc::Sender<Chunk>,
) {
    while let Some(bytes) = stderr.next().await {
        if bytes.is_empty() {
            continue;
        }
        let chunk = Chunk::stderr(bytes);
        if writer_tx.send(chunk).await.is_err() {
            break;
        }
    }
    debug!("tty: stderr pump done");
}

/// Pump client chunks → backend: stdin chunks → `TtyHandle.stdin`, control
/// chunks → `ControlMessage` dispatch. On client read-half close or a
/// zero-length stdin chunk, signal EOF to the backend's stdin.
async fn pump_client_to_backend<R>(
    client_read: R,
    mut stdin: Box<dyn tokio::io::AsyncWrite + Send + Unpin>,
    control: Option<crate::backend::TtyControlHandle>,
) where
    R: AsyncRead + Send + Unpin + 'static,
{
    let mut chunk_reader = ChunkReader::new(client_read);
    loop {
        match chunk_reader.read_chunk().await {
            Ok(chunk) => match chunk.stream_type {
                STREAM_STDIN => {
                    if chunk.bytes.is_empty() {
                        let _ = stdin.shutdown().await;
                        debug!("tty: client stdin EOF (zero-length chunk)");
                    } else if let Err(e) = stdin.write_all(&chunk.bytes).await {
                        warn!("tty: backend stdin write failed: {e}");
                        break;
                    }
                }
                STREAM_CONTROL => match ControlMessage::from_slice(&chunk.bytes) {
                    Ok(ControlMessage::Resize {
                        cols,
                        rows,
                        pixel_width,
                        pixel_height,
                    }) => {
                        if let Some(c) = &control {
                            c.resize(cols, rows, pixel_width, pixel_height);
                        }
                    }
                    Ok(ControlMessage::Signal { name }) => {
                        if let Some(c) = &control {
                            c.signal(&name);
                        }
                    }
                    Ok(ControlMessage::Eof) => {
                        let _ = stdin.shutdown().await;
                        debug!("tty: client stdin EOF (eof control)");
                    }
                    Ok(ControlMessage::Exit { .. }) => {
                        debug!("tty: ignoring Exit control from client (server→client only)");
                    }
                    Err(e) => {
                        debug!("tty: ignoring unknown control type: {e}");
                    }
                },
                other => {
                    debug!("tty: ignoring stream_type {other} from client");
                }
            },
            Err(RawError::ConnectionClosed) => {
                debug!("tty: client closed read half");
                let _ = stdin.shutdown().await;
                break;
            }
            Err(e) => {
                debug!("tty: read_chunk error: {e}");
                let _ = stdin.shutdown().await;
                break;
            }
        }
    }
    debug!("tty: client→server pump done");
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap as StdHashMap;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex as StdMutex;
    use std::task::{Context, Poll};

    use alknet_core::auth::Identity;
    use alknet_core::ownership::InMemoryOwnershipStore;
    use alknet_core::OwnershipStore;
    use tokio::io::duplex;
    use tokio::sync::{mpsc, oneshot, Mutex};
    use tokio_stream::wrappers::ReceiverStream;

    use crate::backend::{BoxFuture, MockControl, TtyControlHandle, TtyError, TtyParams};
    use crate::wire::STREAM_STDOUT;

    const TEST_NEG: &str = r#"{"carriage":"raw","backend":"mock","cmd":["bash"]}"#;

    fn identity_with_scope(scope: &str) -> Option<Identity> {
        Some(Identity {
            id: "test-user".to_string(),
            scopes: vec![scope.to_string()],
            resources: StdHashMap::new(),
        })
    }

    fn identity_no_scope() -> Option<Identity> {
        Some(Identity {
            id: "test-user".to_string(),
            scopes: vec![],
            resources: StdHashMap::new(),
        })
    }

    fn make_backends(backend: Arc<dyn TtyBackend>) -> Arc<StdHashMap<String, Arc<dyn TtyBackend>>> {
        let mut map: StdHashMap<String, Arc<dyn TtyBackend>> = StdHashMap::new();
        map.insert("mock".to_string(), backend);
        Arc::new(map)
    }

    /// A test backend that wires the adapter to channels the test drives.
    ///
    /// `allocate()` swaps in fresh channels and stashes the test-facing
    /// senders/receivers behind shared mutexes so the test can drive
    /// stdout (send), read stdin (recv), and resolve exit (send). A
    /// `ready` oneshot lets the test wait for allocation before taking the
    /// channels (the adapter spawns `drive_session` async; allocation
    /// happens after the negotiation frame is read).
    struct TestBackend {
        stdout_tx: StdMutex<Option<mpsc::Sender<Bytes>>>,
        stderr_tx: StdMutex<Option<mpsc::Sender<Bytes>>>,
        stdin_rx: StdMutex<Option<mpsc::Receiver<Bytes>>>,
        exit_tx: StdMutex<Option<oneshot::Sender<Result<i32, TtyError>>>>,
        ready_tx: StdMutex<Option<oneshot::Sender<()>>>,
        ready_rx: Mutex<Option<oneshot::Receiver<()>>>,
        control: Arc<MockControl>,
        resource: Option<(&'static str, String)>,
        allocate_fail: bool,
        cancel_dropped: Arc<Mutex<bool>>,
    }

    impl TestBackend {
        fn builder() -> TestBackendBuilder {
            TestBackendBuilder {
                resource: None,
                allocate_fail: false,
            }
        }

        /// Wait until `allocate()` has run and the channels are available.
        async fn wait_allocated(&self) {
            if let Some(rx) = self.ready_rx.lock().await.take() {
                let _ = rx.await;
            }
        }

        async fn take_stdout_tx(&self) -> Option<mpsc::Sender<Bytes>> {
            self.wait_allocated().await;
            self.stdout_tx.lock().unwrap().take()
        }

        async fn take_stderr_tx(&self) -> Option<mpsc::Sender<Bytes>> {
            self.wait_allocated().await;
            self.stderr_tx.lock().unwrap().take()
        }

        async fn take_stdin_rx(&self) -> Option<mpsc::Receiver<Bytes>> {
            self.wait_allocated().await;
            self.stdin_rx.lock().unwrap().take()
        }

        async fn take_exit_tx(&self) -> Option<oneshot::Sender<Result<i32, TtyError>>> {
            self.wait_allocated().await;
            self.exit_tx.lock().unwrap().take()
        }
    }

    struct TestBackendBuilder {
        resource: Option<(&'static str, String)>,
        allocate_fail: bool,
    }

    impl TestBackendBuilder {
        fn with_resource(mut self, kind: &'static str, id: &str) -> Self {
            self.resource = Some((kind, id.to_string()));
            self
        }

        fn with_allocate_fail(mut self) -> Self {
            self.allocate_fail = true;
            self
        }

        fn build(self) -> (Arc<TestBackend>, Arc<MockControl>, Arc<Mutex<bool>>) {
            let cancel_dropped = Arc::new(Mutex::new(false));
            let (ready_tx, ready_rx) = oneshot::channel();
            let backend = Arc::new(TestBackend {
                stdout_tx: StdMutex::new(None),
                stderr_tx: StdMutex::new(None),
                stdin_rx: StdMutex::new(None),
                exit_tx: StdMutex::new(None),
                ready_tx: StdMutex::new(Some(ready_tx)),
                ready_rx: Mutex::new(Some(ready_rx)),
                control: Arc::new(MockControl::default()),
                resource: self.resource,
                allocate_fail: self.allocate_fail,
                cancel_dropped: cancel_dropped.clone(),
            });
            (backend.clone(), backend.control.clone(), cancel_dropped)
        }
    }

    /// `AsyncWrite` adapter over `mpsc::Sender<Bytes>` — the test backend's
    /// stdin sink. On shutdown, drops the sender so the test's
    /// `stdin_rx` observes EOF (channel close).
    struct TestStdinSink {
        tx: Option<mpsc::Sender<Bytes>>,
    }

    impl tokio::io::AsyncWrite for TestStdinSink {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<Result<usize, std::io::Error>> {
            match self.get_mut().tx.as_ref() {
                Some(tx) => match tx.try_reserve() {
                    Ok(permit) => {
                        permit.send(Bytes::copy_from_slice(buf));
                        Poll::Ready(Ok(buf.len()))
                    }
                    Err(mpsc::error::TrySendError::Full(_)) => Poll::Pending,
                    Err(mpsc::error::TrySendError::Closed(_)) => Poll::Ready(Err(
                        std::io::Error::new(std::io::ErrorKind::BrokenPipe, "stdin channel closed"),
                    )),
                },
                None => Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "stdin shut down",
                ))),
            }
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), std::io::Error>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), std::io::Error>> {
            self.tx.take();
            Poll::Ready(Ok(()))
        }
    }

    /// A kill-guard future wrapping `oneshot::Receiver<Result<i32, TtyError>>`.
    /// On `Drop`-without-resolve, sets `cancel_dropped` to true (the
    /// cancel-cleanup signal for tests — ADR-056).
    struct ExitFuture {
        rx: Option<oneshot::Receiver<Result<i32, TtyError>>>,
        cancel_dropped: Arc<Mutex<bool>>,
    }

    impl Future for ExitFuture {
        type Output = Result<i32, TtyError>;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            if let Some(rx) = self.rx.as_mut() {
                if let Poll::Ready(v) = Pin::new(rx).poll(cx) {
                    self.rx.take();
                    let resolved: Result<Result<i32, TtyError>, _> = v;
                    let mapped: Result<i32, TtyError> = resolved
                        .map_err(|_| TtyError::WaitFailed {
                            message: "exit_code sender dropped".to_string(),
                        })
                        .and_then(|inner| inner);
                    return Poll::Ready(mapped);
                }
            }
            Poll::Pending
        }
    }

    impl Drop for ExitFuture {
        fn drop(&mut self) {
            if self.rx.is_some() {
                let cancel_dropped = self.cancel_dropped.clone();
                tokio::spawn(async move {
                    *cancel_dropped.lock().await = true;
                });
            }
        }
    }

    #[async_trait]
    impl TtyBackend for TestBackend {
        async fn allocate(&self, _params: &TtyParams) -> Result<TtyHandle, TtyError> {
            if self.allocate_fail {
                return Err(TtyError::AllocFailed {
                    message: "test allocate fail".to_string(),
                });
            }

            let (stdout_tx, stdout_rx) = mpsc::channel::<Bytes>(8);
            let (stderr_tx, stderr_rx) = mpsc::channel::<Bytes>(8);
            let (stdin_tx, stdin_rx) = mpsc::channel::<Bytes>(8);
            let (_exit_tx, exit_rx) = oneshot::channel::<Result<i32, TtyError>>();

            *self.stdout_tx.lock().unwrap() = Some(stdout_tx);
            *self.stderr_tx.lock().unwrap() = Some(stderr_tx);
            *self.stdin_rx.lock().unwrap() = Some(stdin_rx);

            let stdout: Pin<Box<dyn futures_core::Stream<Item = Bytes> + Send>> =
                Box::pin(ReceiverStream::new(stdout_rx));
            let stderr: Option<Pin<Box<dyn futures_core::Stream<Item = Bytes> + Send>>> =
                Some(Box::pin(ReceiverStream::new(stderr_rx)));
            let stdin: Box<dyn tokio::io::AsyncWrite + Send + Unpin> =
                Box::new(TestStdinSink { tx: Some(stdin_tx) });
            let control = Some(TtyControlHandle::new(self.control.clone()));

            let cancel_dropped = self.cancel_dropped.clone();
            let exit_future = ExitFuture {
                rx: Some(exit_rx),
                cancel_dropped,
            };
            let exit_code: BoxFuture<Result<i32, TtyError>> = Box::pin(exit_future);

            *self.exit_tx.lock().unwrap() = Some(_exit_tx);

            if let Some(tx) = self.ready_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }

            Ok(TtyHandle {
                stdin,
                stdout,
                stderr,
                exit_code,
                control,
            })
        }

        fn resource_id(&self, _params: &TtyParams) -> Option<(&'static str, String)> {
            self.resource.clone()
        }
    }

    use crate::wire::ChunkReader as WireChunkReader;

    /// Test harness: drives a session over a duplex pair and provides
    /// helpers for the client side to write negotiation/chunks and read
    /// chunks/error frames.
    struct ClientSide {
        write: tokio::io::WriteHalf<tokio::io::DuplexStream>,
        read: tokio::io::ReadHalf<tokio::io::DuplexStream>,
    }

    impl ClientSide {
        async fn write_negotiation(&mut self, body: &str) {
            let len = body.len() as u32;
            self.write.write_all(&len.to_be_bytes()).await.unwrap();
            self.write.write_all(body.as_bytes()).await.unwrap();
            self.write.flush().await.unwrap();
        }

        async fn write_chunk(&mut self, stream_type: u8, payload: &[u8]) {
            let mut header = [0u8; 5];
            header[0] = stream_type;
            let len = payload.len() as u32;
            header[1..].copy_from_slice(&len.to_be_bytes());
            self.write.write_all(&header).await.unwrap();
            if !payload.is_empty() {
                self.write.write_all(payload).await.unwrap();
            }
            self.write.flush().await.unwrap();
        }

        async fn read_chunk(&mut self) -> (u8, Bytes) {
            let mut reader = WireChunkReader::new(&mut self.read);
            let chunk = reader.read_chunk().await.unwrap();
            (chunk.stream_type, chunk.bytes)
        }

        async fn read_error_frame(&mut self) -> serde_json::Value {
            use tokio::io::AsyncReadExt;
            let mut len_buf = [0u8; 4];
            self.read.read_exact(&mut len_buf).await.unwrap();
            let len = u32::from_be_bytes(len_buf) as usize;
            let mut body = vec![0u8; len];
            self.read.read_exact(&mut body).await.unwrap();
            serde_json::from_slice(&body).unwrap()
        }
    }

    fn make_client_and_server() -> (ClientSide, tokio::io::DuplexStream) {
        let (a, b) = duplex(8 * 1024);
        let (a_read, a_write) = tokio::io::split(a);
        (
            ClientSide {
                write: a_write,
                read: a_read,
            },
            b,
        )
    }

    /// Split a bidirectional duplex stream into read/write halves and call
    /// `drive_session` with them.
    async fn drive_session_server(
        server: tokio::io::DuplexStream,
        backends: Arc<StdHashMap<String, Arc<dyn TtyBackend>>>,
        ownership: Option<Arc<dyn OwnershipProvider>>,
        identity: Option<Identity>,
    ) {
        let (server_read, server_write) = tokio::io::split(server);
        drive_session(server_write, server_read, backends, ownership, identity).await;
    }

    #[tokio::test]
    async fn happy_path_negotiate_stdin_stdout_exit() {
        let (backend, _control, _cancel) = TestBackend::builder().build();
        let backends = make_backends(backend.clone());
        let (mut client, server) = make_client_and_server();

        let identity = identity_with_scope(TTY_OPEN_SCOPE);
        let session = tokio::spawn(async move {
            drive_session_server(server, backends, None, identity).await;
        });

        client.write_negotiation(TEST_NEG).await;

        let stdout_tx = backend.take_stdout_tx().await.expect("stdout tx");
        let _ = backend.take_stderr_tx().await;
        stdout_tx.send(Bytes::from_static(b"hello")).await.unwrap();
        drop(stdout_tx);

        let exit_tx = backend.take_exit_tx().await.expect("exit tx");
        exit_tx.send(Ok(0)).unwrap();

        let (st, bytes) = client.read_chunk().await;
        assert_eq!(st, STREAM_STDOUT);
        assert_eq!(bytes.as_ref(), b"hello");

        let (st, bytes) = client.read_chunk().await;
        assert_eq!(st, STREAM_STDOUT);
        assert!(bytes.is_empty());

        let (st, bytes) = client.read_chunk().await;
        assert_eq!(st, crate::wire::STREAM_CONTROL);
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["type"], "exit");
        assert_eq!(v["code"], 0);

        let _ = session.await;
    }

    #[tokio::test]
    async fn exit_chunk_is_last_no_stdout_after_exit() {
        let (backend, _control, _cancel) = TestBackend::builder().build();
        let backends = make_backends(backend.clone());
        let (mut client, server) = make_client_and_server();

        let identity = identity_with_scope(TTY_OPEN_SCOPE);
        let session = tokio::spawn(async move {
            drive_session_server(server, backends, None, identity).await;
        });

        client.write_negotiation(TEST_NEG).await;

        let stdout_tx = backend.take_stdout_tx().await.expect("stdout tx");
        let _ = backend.take_stderr_tx().await;
        stdout_tx.send(Bytes::from_static(b"out1")).await.unwrap();
        drop(stdout_tx);

        let exit_tx = backend.take_exit_tx().await.expect("exit tx");
        exit_tx.send(Ok(7)).unwrap();

        loop {
            let (st, bytes) = client.read_chunk().await;
            if st == crate::wire::STREAM_CONTROL {
                let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                if v["type"] == "exit" {
                    assert_eq!(v["code"], 7);
                    break;
                }
            }
        }
        let _ = session.await;
    }

    #[tokio::test]
    async fn stdin_eof_zero_length_chunk_closes_backend_stdin() {
        let (backend, _control, _cancel) = TestBackend::builder().build();
        let backends = make_backends(backend.clone());
        let (mut client, server) = make_client_and_server();

        let identity = identity_with_scope(TTY_OPEN_SCOPE);
        let session = tokio::spawn(async move {
            drive_session_server(server, backends, None, identity).await;
        });

        client.write_negotiation(TEST_NEG).await;

        let stdin_rx = backend.take_stdin_rx().await.expect("stdin rx");

        client.write_chunk(STREAM_STDIN, b"data").await;
        client.write_chunk(STREAM_STDIN, b"").await;

        let mut received = Vec::new();
        let mut stdin_rx = stdin_rx;
        while let Some(b) = stdin_rx.recv().await {
            received.extend_from_slice(&b);
        }
        assert_eq!(received, b"data");

        let stdout_tx = backend.take_stdout_tx().await.expect("stdout tx");
        let _ = backend.take_stderr_tx().await;
        drop(stdout_tx);
        let exit_tx = backend.take_exit_tx().await.expect("exit tx");
        exit_tx.send(Ok(0)).unwrap();

        let _ = session.await;
    }

    #[tokio::test]
    async fn resize_and_signal_control_dispatched() {
        let (backend, control, _cancel) = TestBackend::builder().build();
        let backends = make_backends(backend.clone());
        let (mut client, server) = make_client_and_server();

        let identity = identity_with_scope(TTY_OPEN_SCOPE);
        let session = tokio::spawn(async move {
            drive_session_server(server, backends, None, identity).await;
        });

        client.write_negotiation(TEST_NEG).await;

        client
            .write_chunk(
                crate::wire::STREAM_CONTROL,
                br#"{"type":"resize","cols":100,"rows":50}"#,
            )
            .await;
        client
            .write_chunk(
                crate::wire::STREAM_CONTROL,
                br#"{"type":"signal","name":"INT"}"#,
            )
            .await;

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        {
            let resize = control.last_resize.lock().unwrap();
            assert_eq!(*resize, Some((100, 50, 0, 0)));
        }
        {
            let signal = control.last_signal.lock().unwrap();
            assert_eq!(*signal, Some("INT".to_string()));
        }

        let stdout_tx = backend.take_stdout_tx().await.expect("stdout tx");
        let _ = backend.take_stderr_tx().await;
        drop(stdout_tx);
        let exit_tx = backend.take_exit_tx().await.expect("exit tx");
        exit_tx.send(Ok(0)).unwrap();
        let _ = session.await;
    }

    #[tokio::test]
    async fn unknown_control_type_ignored() {
        let (backend, _control, _cancel) = TestBackend::builder().build();
        let backends = make_backends(backend.clone());
        let (mut client, server) = make_client_and_server();

        let identity = identity_with_scope(TTY_OPEN_SCOPE);
        let session = tokio::spawn(async move {
            drive_session_server(server, backends, None, identity).await;
        });

        client.write_negotiation(TEST_NEG).await;

        client
            .write_chunk(crate::wire::STREAM_CONTROL, br#"{"type":"unknown"}"#)
            .await;

        let stdout_tx = backend.take_stdout_tx().await.expect("stdout tx");
        let _ = backend.take_stderr_tx().await;
        stdout_tx
            .send(Bytes::from_static(b"after-unknown"))
            .await
            .unwrap();
        drop(stdout_tx);
        let exit_tx = backend.take_exit_tx().await.expect("exit tx");
        exit_tx.send(Ok(0)).unwrap();

        let (st, bytes) = client.read_chunk().await;
        assert_eq!(st, STREAM_STDOUT);
        assert_eq!(bytes.as_ref(), b"after-unknown");

        let _ = session.await;
    }

    #[tokio::test]
    async fn exit_control_from_client_ignored() {
        let (backend, _control, _cancel) = TestBackend::builder().build();
        let backends = make_backends(backend.clone());
        let (mut client, server) = make_client_and_server();

        let identity = identity_with_scope(TTY_OPEN_SCOPE);
        let session = tokio::spawn(async move {
            drive_session_server(server, backends, None, identity).await;
        });

        client.write_negotiation(TEST_NEG).await;

        client
            .write_chunk(crate::wire::STREAM_CONTROL, br#"{"type":"exit","code":99}"#)
            .await;

        let stdout_tx = backend.take_stdout_tx().await.expect("stdout tx");
        let _ = backend.take_stderr_tx().await;
        stdout_tx
            .send(Bytes::from_static(b"still-pumping"))
            .await
            .unwrap();
        drop(stdout_tx);
        let exit_tx = backend.take_exit_tx().await.expect("exit tx");
        exit_tx.send(Ok(0)).unwrap();

        let (st, bytes) = client.read_chunk().await;
        assert_eq!(st, STREAM_STDOUT);
        assert_eq!(bytes.as_ref(), b"still-pumping");

        let _ = session.await;
    }

    #[tokio::test]
    async fn unknown_backend_error() {
        let (backend, _control, _cancel) = TestBackend::builder().build();
        let backends = make_backends(backend.clone());
        let (mut client, server) = make_client_and_server();

        let identity = identity_with_scope(TTY_OPEN_SCOPE);
        let session = tokio::spawn(async move {
            drive_session_server(server, backends, None, identity).await;
        });

        client
            .write_negotiation(r#"{"carriage":"raw","backend":"nope","cmd":["bash"]}"#)
            .await;

        let err = client.read_error_frame().await;
        assert_eq!(err["error"], "unknown_backend");
        assert_eq!(err["backend"], "nope");

        let _ = session.await;
    }

    #[tokio::test]
    async fn malformed_negotiation_bad_json() {
        let (backend, _control, _cancel) = TestBackend::builder().build();
        let backends = make_backends(backend.clone());
        let (mut client, server) = make_client_and_server();

        let identity = identity_with_scope(TTY_OPEN_SCOPE);
        let session = tokio::spawn(async move {
            drive_session_server(server, backends, None, identity).await;
        });

        client.write_negotiation("not json").await;

        let err = client.read_error_frame().await;
        assert_eq!(err["error"], "malformed_negotiation");

        let _ = session.await;
    }

    #[tokio::test]
    async fn malformed_negotiation_carriage_not_raw() {
        let (backend, _control, _cancel) = TestBackend::builder().build();
        let backends = make_backends(backend.clone());
        let (mut client, server) = make_client_and_server();

        let identity = identity_with_scope(TTY_OPEN_SCOPE);
        let session = tokio::spawn(async move {
            drive_session_server(server, backends, None, identity).await;
        });

        client
            .write_negotiation(r#"{"carriage":"json","backend":"mock","cmd":["bash"]}"#)
            .await;

        let err = client.read_error_frame().await;
        assert_eq!(err["error"], "malformed_negotiation");

        let _ = session.await;
    }

    #[tokio::test]
    async fn malformed_negotiation_empty_cmd() {
        let (backend, _control, _cancel) = TestBackend::builder().build();
        let backends = make_backends(backend.clone());
        let (mut client, server) = make_client_and_server();

        let identity = identity_with_scope(TTY_OPEN_SCOPE);
        let session = tokio::spawn(async move {
            drive_session_server(server, backends, None, identity).await;
        });

        client
            .write_negotiation(r#"{"carriage":"raw","backend":"mock","cmd":[]}"#)
            .await;

        let err = client.read_error_frame().await;
        assert_eq!(err["error"], "malformed_negotiation");

        let _ = session.await;
    }

    #[tokio::test]
    async fn allocate_failed_error() {
        let (backend, _control, _cancel) = TestBackend::builder().with_allocate_fail().build();
        let backends = make_backends(backend.clone());
        let (mut client, server) = make_client_and_server();

        let identity = identity_with_scope(TTY_OPEN_SCOPE);
        let session = tokio::spawn(async move {
            drive_session_server(server, backends, None, identity).await;
        });

        client.write_negotiation(TEST_NEG).await;

        let err = client.read_error_frame().await;
        assert_eq!(err["error"], "allocate_failed");

        let _ = session.await;
    }

    #[tokio::test]
    async fn exit_error_sends_minus_one() {
        let (backend, _control, _cancel) = TestBackend::builder().build();
        let backends = make_backends(backend.clone());
        let (mut client, server) = make_client_and_server();

        let identity = identity_with_scope(TTY_OPEN_SCOPE);
        let session = tokio::spawn(async move {
            drive_session_server(server, backends, None, identity).await;
        });

        client.write_negotiation(TEST_NEG).await;

        let stdout_tx = backend.take_stdout_tx().await.expect("stdout tx");
        let _ = backend.take_stderr_tx().await;
        drop(stdout_tx);
        let exit_tx = backend.take_exit_tx().await.expect("exit tx");
        exit_tx
            .send(Err(TtyError::WaitFailed {
                message: "boom".to_string(),
            }))
            .unwrap();

        loop {
            let (st, bytes) = client.read_chunk().await;
            if st == crate::wire::STREAM_CONTROL {
                let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(v["type"], "exit");
                assert_eq!(v["code"], -1);
                break;
            }
        }

        let _ = session.await;
    }

    #[tokio::test]
    async fn cancel_cleanup_drops_exit_future() {
        let (backend, _control, cancel) = TestBackend::builder().build();
        let backends = make_backends(backend.clone());
        let (mut client, server) = make_client_and_server();

        let identity = identity_with_scope(TTY_OPEN_SCOPE);
        let session = tokio::spawn(async move {
            drive_session_server(server, backends, None, identity).await;
        });

        client.write_negotiation(TEST_NEG).await;

        let _stdout_tx = backend.take_stdout_tx().await.expect("stdout tx");
        let _exit_tx = backend.take_exit_tx().await.expect("exit tx");

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        session.abort();
        let _ = session.await;

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(
            *cancel.lock().await,
            "exit_code future dropped without resolve"
        );
    }

    #[tokio::test]
    async fn scope_gate_forbidden_without_tty_open() {
        let (backend, _control, _cancel) = TestBackend::builder().build();
        let backends = make_backends(backend.clone());
        let (mut client, server) = make_client_and_server();

        let identity = identity_no_scope();
        let session = tokio::spawn(async move {
            drive_session_server(server, backends, None, identity).await;
        });

        client.write_negotiation(TEST_NEG).await;

        let err = client.read_error_frame().await;
        assert_eq!(err["error"], "forbidden");

        let _ = session.await;
    }

    #[tokio::test]
    async fn ownership_check_denies_non_owner() {
        let (backend, _control, _cancel) = TestBackend::builder()
            .with_resource("container", "c1")
            .build();
        let backends = make_backends(backend.clone());
        let (mut client, server) = make_client_and_server();

        let store = Arc::new(InMemoryOwnershipStore::new());
        let identity = identity_with_scope(TTY_OPEN_SCOPE);
        let session = tokio::spawn(async move {
            drive_session_server(server, backends, Some(store), identity).await;
        });

        client.write_negotiation(TEST_NEG).await;

        let err = client.read_error_frame().await;
        assert_eq!(err["error"], "forbidden");

        let _ = session.await;
    }

    #[tokio::test]
    async fn ownership_check_allows_owner() {
        let (backend, _control, _cancel) = TestBackend::builder()
            .with_resource("container", "c1")
            .build();
        let backends = make_backends(backend.clone());
        let (mut client, server) = make_client_and_server();

        let store = Arc::new(InMemoryOwnershipStore::new());
        let owner = Identity {
            id: "test-user".to_string(),
            scopes: vec![TTY_OPEN_SCOPE.to_string()],
            resources: StdHashMap::new(),
        };
        store.record(&owner, "container", "c1").await.unwrap();
        let identity = identity_with_scope(TTY_OPEN_SCOPE);
        let session = tokio::spawn(async move {
            drive_session_server(server, backends, Some(store), identity).await;
        });

        client.write_negotiation(TEST_NEG).await;

        let stdout_tx = backend.take_stdout_tx().await.expect("stdout tx");
        let _ = backend.take_stderr_tx().await;
        drop(stdout_tx);
        let exit_tx = backend.take_exit_tx().await.expect("exit tx");
        exit_tx.send(Ok(0)).unwrap();

        loop {
            let (st, bytes) = client.read_chunk().await;
            if st == crate::wire::STREAM_CONTROL {
                let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(v["type"], "exit");
                assert_eq!(v["code"], 0);
                break;
            }
        }

        let _ = session.await;
    }

    #[tokio::test]
    async fn stderr_pump_concurrent_with_stdout() {
        let (backend, _control, _cancel) = TestBackend::builder().build();
        let backends = make_backends(backend.clone());
        let (mut client, server) = make_client_and_server();

        let identity = identity_with_scope(TTY_OPEN_SCOPE);
        let session = tokio::spawn(async move {
            drive_session_server(server, backends, None, identity).await;
        });

        client.write_negotiation(TEST_NEG).await;

        let stdout_tx = backend.take_stdout_tx().await.expect("stdout tx");
        let stderr_tx = backend.take_stderr_tx().await.expect("stderr tx");
        stdout_tx.send(Bytes::from_static(b"out")).await.unwrap();
        stderr_tx.send(Bytes::from_static(b"err")).await.unwrap();
        drop(stdout_tx);
        drop(stderr_tx);
        let exit_tx = backend.take_exit_tx().await.expect("exit tx");
        exit_tx.send(Ok(0)).unwrap();

        let mut saw_stdout = false;
        let mut saw_stderr = false;
        let mut saw_exit = false;
        loop {
            let (st, bytes) = client.read_chunk().await;
            match st {
                STREAM_STDOUT => {
                    assert!(!saw_exit, "stdout after exit");
                    if bytes.is_empty() {
                        saw_stdout = true;
                    } else {
                        assert_eq!(bytes.as_ref(), b"out");
                    }
                }
                crate::wire::STREAM_STDERR => {
                    assert!(!saw_exit, "stderr after exit");
                    assert_eq!(bytes.as_ref(), b"err");
                    saw_stderr = true;
                }
                crate::wire::STREAM_CONTROL => {
                    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                    assert_eq!(v["type"], "exit");
                    saw_exit = true;
                    break;
                }
                _ => panic!("unexpected stream_type {st}"),
            }
        }
        assert!(saw_stdout);
        assert!(saw_stderr);
        assert!(saw_exit);

        let _ = session.await;
    }

    #[tokio::test]
    async fn eof_control_closes_stdin() {
        let (backend, _control, _cancel) = TestBackend::builder().build();
        let backends = make_backends(backend.clone());
        let (mut client, server) = make_client_and_server();

        let identity = identity_with_scope(TTY_OPEN_SCOPE);
        let session = tokio::spawn(async move {
            drive_session_server(server, backends, None, identity).await;
        });

        client.write_negotiation(TEST_NEG).await;

        let stdin_rx = backend.take_stdin_rx().await.expect("stdin rx");

        client.write_chunk(STREAM_STDIN, b"first").await;
        client
            .write_chunk(crate::wire::STREAM_CONTROL, br#"{"type":"eof"}"#)
            .await;

        let mut received = Vec::new();
        let mut stdin_rx = stdin_rx;
        while let Some(b) = stdin_rx.recv().await {
            received.extend_from_slice(&b);
        }
        assert_eq!(received, b"first");

        let stdout_tx = backend.take_stdout_tx().await.expect("stdout tx");
        let _ = backend.take_stderr_tx().await;
        drop(stdout_tx);
        let exit_tx = backend.take_exit_tx().await.expect("exit tx");
        exit_tx.send(Ok(0)).unwrap();
        let _ = session.await;
    }
}
