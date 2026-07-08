---
status: draft
last_updated: 2026-07-08
---

# alknet-docker — DockerTtyBackend

The `DockerTtyBackend` is alknet-docker's `impl TtyBackend` (ADR-053)
for interactive terminal sessions into docker containers over
`alknet/tty`. It wraps `bollard::attach_container()` (attach mode) and
`bollard::exec::start_exec` with `tty: true` (exec mode), producing a
`TtyHandle` the `TtyAdapter` pumps. This document specifies the
backend's allocation, control, and cancel-cleanup. The crate
placement is decided in
[ADR-061](../../decisions/061-docker-tty-backend-in-alknet-docker.md).

## What

`DockerTtyBackend` is the `TtyBackend` implementation the
`TtyAdapter` dispatches to when a client opens an `alknet/tty` session
with `backend: "docker"` in the negotiation frame. The adapter selects
the backend by the `backend` field, calls `allocate()`, and pumps the
resulting `TtyHandle`'s fields bidirectionally using the alknet-tty
wire format (ADR-052). The backend produces handles; the adapter owns
the wire format.

```rust
// behind the `tty` feature in alknet-docker
pub struct DockerTtyBackend {
    docker: bollard::Docker,
    label_prefix: String,  // "alknet" by default; for the ownership cross-check
}

#[async_trait]
impl TtyBackend for DockerTtyBackend {
    async fn allocate(&self, params: &TtyParams) -> Result<TtyHandle, TtyError>;
    fn resource_id(&self, params: &TtyParams) -> Option<(&'static str, String)>;
}
```

The backend is constructed by the assembly layer with the bollard
client (shared with the call operations) and the label prefix (shared
with ADR-060's label namespace), then registered in the
`TtyAdapter`'s backend map under the `"docker"` key.

## Why

The alknet-tty spec ([tty-backend.md](../tty/tty-backend.md) §"Backend
implementations") listed the `DockerTtyBackend` location as "future,
out of scope here." This spec fills that row: the backend lives in
alknet-docker (ADR-061), behind the `tty` feature, and wraps the
bollard attach/exec API the POC validated.

The POC's `drive_attach_raw` (`/workspace/alknet-docker-poc/src/ops.rs`)
is the seed of `allocate()`. The POC proved the mechanism (raw chunks
on a bidi stream after a JSON request); the `TtyBackend` trait
extracts the bollard interaction into a backend that produces handles,
while the `TtyAdapter` (in alknet-tty) owns the wire format and
session lifecycle. The backend is the inversion point
([ADR-053](../../decisions/053-ttybackend-trait-and-ttyhandle.md)).

## Architecture

### Backend Params

The `DockerTtyBackend` deserializes its params from
`TtyParams.backend_params` (the opaque `serde_json::Map` the adapter
passes through verbatim, per ADR-053 §"Backend params are opaque"):

```rust
#[derive(Deserialize)]
struct DockerBackendParams {
    /// The container to attach to or exec in.
    container: String,
    /// "attach" (attach to the running primary process) or "exec"
    /// (start a new process with a PTY). Default: "exec".
    #[serde(default = "default_mode")]
    mode: String,
}

fn default_mode() -> String { "exec".into() }
```

- `container` — the container ID or name. Required. The backend
  validates it exists (the bollard call will fail if not; the error
  maps to `TtyError::Backend`).
- `mode` — `"attach"` or `"exec"`. `"attach"` attaches to the
  container's primary process (the running process, pid 1 or the
  `ENTRYPOINT`); `"exec"` starts a new process in the container with a
  PTY. Default `"exec"` (the common case — a new shell). For `exec`
  mode, `TtyParams.cmd` is the command vector; for `attach` mode,
  `cmd` is ignored (the primary process is already running).

The adapter does not interpret these fields — it passes the
negotiation frame's backend-specific JSON through as
`backend_params`, and the backend deserializes its own
strongly-typed struct (ADR-053).

### `allocate()` — attach mode

Wraps `bollard::container::attach_container()` (the reliable
HTTP-upgrade-to-TCP path, per the POC and
[ADR-059](../../decisions/059-bollard-021-dependency-and-features.md)
§3 — not the websocket path).

```rust
async fn allocate_attach(&self, params: &TtyParams, container: &str) -> Result<TtyHandle, TtyError> {
    let options = AttachContainerOptionsBuilder::default()
        .stdout(true).stderr(true).stdin(true)
        .stream(true).logs(false)
        .build();
    let AttachContainerResults { output, input } = self.docker
        .attach_container(container, Some(options))
        .await
        .map_err(|e| TtyError::Backend { message: e.to_string() })?;

    // output: Stream<LogOutput> → TtyHandle.stdout (Stream<Bytes>)
    //   PTY mode (tty: true on the container) merges stdout/stderr;
    //   bollard's LogOutput on a TTY returns StdOut only, so stderr
    //   is None. (tty-backend.md §DockerTtyBackend notes this.)
    let stdout = Box::pin(output.map(|r| match r {
        Ok(LogOutput::StdOut { message }) | Ok(LogOutput::Console { message }) =>
            message,  // Bytes
        Ok(_) => bytes::Bytes::new(),
        Err(_) => bytes::Bytes::new(),
    }));

    // input: AsyncWrite → TtyHandle.stdin (Box<dyn AsyncWrite + Send + Unpin>)
    let stdin: Box<dyn tokio::io::AsyncWrite + Send + Unpin> = Box::new(input);

    // exit_code: wait for the container's process to exit, then read
    // the exit status from inspect_container. This future resolves
    // independently of the output stream — the adapter (ADR-055)
    // awaits BOTH the stdout pump's EOF AND this future's resolve
    // before sending the exit chunk, so the backend does not need to
    // drain stdout itself. For a container that exits with a non-zero
    // status, the State.exit_code carries it. The Drop of this future
    // (cancel) kills the container (ADR-056) — see "Cancel-Cleanup".
    let docker = self.docker.clone();
    let container_for_exit = container.to_string();
    let exit_code: BoxFuture<'static, Result<i32, TtyError>> = Box::pin(async move {
        // Poll the container until it exits. wait_container returns a
        // stream of WaitContainerResponse; the first (and usually
        // only) one carries the exit code. Alternatively, poll
        // inspect_container until State.running is false. The
        // adapter's stdout pump drains the output stream in
        // parallel; this future resolves when the process exits.
        let mut wait_stream = docker.wait_container(&container_for_exit,
            None::<WaitContainerOptions<String>>);
        if let Some(result) = wait_stream.next().await {
            match result {
                Ok(resp) => Ok(resp.status_code.unwrap_or(-1) as i32),
                Err(e) => Err(TtyError::WaitFailed { message: e.to_string() }),
            }
        } else {
            Ok(-1)
        }
    });

    Ok(TtyHandle {
        stdin,
        stdout,
        stderr: None,  // PTY mode merges
        exit_code,
        control: Some(TtyControlHandle::new(Arc::new(DockerControl {
            docker: self.docker.clone(),
            container: container.to_string(),
            mode: "attach".into(),
        }))),
    })
}
```

The output stream maps `LogOutput` → `Bytes` (the adapter pumps these
as stdout chunks, stream_type 1). PTY mode (`tty: true` on the
container) merges stdout/stderr into `StdOut`, so `TtyHandle.stderr` is
`None` — the adapter pumps only stdout chunks. This matches the
`tty-backend.md` sketch: "stdout/stderr are merged when `tty: true`
(bollard's `LogOutput` on a TTY exec returns `StdOut` only), so
`TtyHandle.stderr` is `None` for the PTY case."

### `allocate()` — exec mode

Wraps `bollard::exec::create_exec` + `start_exec` with `tty: true`:

```rust
async fn allocate_exec(&self, params: &TtyParams, container: &str) -> Result<TtyHandle, TtyError> {
    let cmd = &params.cmd;  // argv[0] + args
    let config = CreateExecOptions {
        cmd: Some(cmd.clone()),
        env: Some(params.env.clone()),
        attach_stdout: Some(true),
        attach_stderr: Some(true),
        attach_stdin: Some(true),
        tty: Some(true),  // PTY mode
        ..Default::default()
    };
    let exec = self.docker.create_exec(container, config).await
        .map_err(|e| TtyError::Backend { message: e.to_string() })?;
    let exec_id = exec.id;

    let StartExecResults::Attached { output, input } = self.docker
        .start_exec(&exec_id, None).await
        .map_err(|e| TtyError::Backend { message: e.to_string() })?;

    // Same mapping as attach mode: output → stdout, input → stdin,
    // stderr None (PTY mode merges).
    let stdout = /* output → Stream<Bytes> */;
    let stdin: Box<dyn AsyncWrite + Send + Unpin> = Box::new(input);

    // exit_code: after output stream ends, inspect_exec for the exit code
    let docker = self.docker.clone();
    let exec_id = exec_id.clone();
    let exit_code: BoxFuture<'static, Result<i32, TtyError>> = Box::pin(async move {
        // Wait for output stream end (the future resolves after the
        // pump drains output — the adapter's exit-chunk ordering
        // awaits both stdout EOF and exit_code resolve, ADR-055).
        // Then inspect_exec:
        let inspect = docker.inspect_exec(&exec_id).await
            .map_err(|e| TtyError::WaitFailed { message: e.to_string() })?;
        Ok(inspect.exit_code.unwrap_or(-1))
    });

    Ok(TtyHandle { stdin, stdout, stderr: None, exit_code,
        control: Some(TtyControlHandle::new(Arc::new(DockerControl {
            docker: self.docker.clone(),
            container: container.to_string(),
            mode: "exec".into(),
            exec_id: Some(exec_id),
        }))) })
}
```

The exit-code future for exec mode follows the POC's pattern (POC
target 3): pump the output stream, then `inspect_exec` for the exit
code. The adapter's exit-chunk ordering (ADR-055) awaits both the
stdout pump's EOF and the `exit_code` future's resolve before sending
the exit chunk — so the `exit_code` future must not resolve until the
output stream has ended. For exec mode, the output stream end is the
signal that the exec process exited; `inspect_exec` after that returns
the exit code.

### `resource_id()` — ownership check delegation

```rust
fn resource_id(&self, params: &TtyParams) -> Option<(&'static str, String)> {
    let p: DockerBackendParams = serde_json::from_value(
        serde_json::Value::Object(params.backend_params.clone())
    ).ok()?;
    Some(("container", p.container))
}
```

The adapter calls this at negotiation and checks
`OwnershipProvider::owns(identity, "container", container_id, "tty")`
if an ownership provider is wired (ADR-050, ADR-060). The backend owns
the extraction; the adapter doesn't parse docker-specific JSON. The
`"tty"` action is a distinct resource action from `"exec"` (the call
operation's action) — a caller authorized to `docker/container/exec`
(captured, non-interactive) is not automatically authorized for an
interactive tty session; the `tty` action is a separate scope. This
matches the "interactive terminal ≠ captured command" boundary
(ADR-058 §3).

### `DockerControl` — `TtyControl` impl

```rust
struct DockerControl {
    docker: bollard::Docker,
    container: String,
    mode: String,           // "attach" or "exec"
    exec_id: Option<String>, // Some for exec mode (for resize_exec)
}

impl TtyControl for DockerControl {
    fn resize(&self, cols: u16, rows: u16, pixel_width: u16, pixel_height: u16) {
        let docker = self.docker.clone();
        let container = self.container.clone();
        let exec_id = self.exec_id.clone();
        let mode = self.mode.clone();
        tokio::spawn(async move {
            let _ = match mode.as_str() {
                "attach" => docker.resize_container_tty(&container,
                    ResizeContainerTtyOptions { width: cols, height: rows, .. }).await,
                "exec" if exec_id.is_some() => docker.resize_exec(
                    exec_id.as_ref().unwrap(),
                    ResizeExecOptions { height: rows, width: cols, .. }).await,
                _ => Ok(()),
            };
        });
    }

    fn signal(&self, name: &str) {
        // docker has no per-exec signal API in bollard's stable surface.
        // Best-effort: kill_container with the mapped signal.
        // Unknown signals fall back to SIGKILL (TtyControl::signal contract).
        let docker = self.docker.clone();
        let container = self.container.clone();
        let sig = signal_name_to_bollard(name);  // SIGINT, SIGTERM, etc.
        tokio::spawn(async move {
            let _ = docker.kill_container(&container,
                Some(KillContainerOptions { signal: sig })).await;
        });
    }
}
```

- `resize()` — `bollard::container::resize_container_tty()` (attach
  mode) or `bollard::exec::resize_exec()` (exec mode with an exec ID).
  Fire-and-forget (the `TtyControl` trait methods are sync, not async —
  ADR-053; the bollard call is spawned).
- `signal()` — docker has no per-exec signal API in bollard's stable
  surface. The best-effort path is `bollard::kill_container()` with
  the mapped signal (SIGINT, SIGTERM, etc.); unknown signals fall back
  to SIGKILL. For exec mode, the signal goes to the container's main
  process, not just the exec's process — docker's exec isolation
  limits per-exec signal targeting. This is within the
  `TtyControl::signal` contract ("best-effort delivery to the
  foreground process group") but is a docker-semantics limitation: a
  user expecting `Ctrl-C` to kill only the exec process may see the
  container killed. This is documented in the spec, not hidden.

The signal-name → bollard-signal mapping (`signal_name_to_bollard`)
covers the standard set (`HUP`, `INT`, `QUIT`, `TERM`, `KILL`, `USR1`,
`USR2`, `TSTP`, `CONT`) — the same set alknet-tty's control channel
supports (ADR-052 §Control Channel). Unknown names fall back to the
backend's default kill (`SIGKILL`).

### Cancel-Cleanup (ADR-056)

The `TtyHandle.exit_code` future's `Drop`-on-cancel (the adapter drops
the `TtyHandle` on connection drop, stream reset, or pump-task panic —
ADR-056) MUST kill the session target. The docker backend's
`exit_code` future wraps the kill in its `Drop`:

```rust
// The exit_code future holds a guard that kills on Drop.
struct DockerExitGuard {
    docker: bollard::Docker,
    container: String,
    mode: String,
    killed: bool,
}

impl Drop for DockerExitGuard {
    fn drop(&mut self) {
        if !self.killed {
            let docker = self.docker.clone();
            let container = self.container.clone();
            tokio::spawn(async move {
                let _ = docker.kill_container(&container,
                    Some(KillContainerOptions { signal: "SIGKILL".into() })).await;
            });
        }
    }
}
```

- **Attach mode** — `kill_container` with `SIGKILL`. The container is
  not removed (attach doesn't own the container's lifecycle — the
  container may be a hosted service the operator wants to keep); the
  kill terminates the process the terminal session was attached to.
- **Exec mode** — `kill_container` with `SIGKILL`, targeting the
  container's main process (best-effort, per docker's exec isolation).
  The exec instance's process is not separately killable in docker's
  model; the container-level kill is the available mechanism.

The guard's `killed` flag prevents a double-kill if the exit_code
future resolved normally (the process exited) before the Drop. The
spawn-on-Drop pattern (tokio::spawn in the Drop impl) is the standard
way to run async cleanup from a sync `Drop`; the spawned task outlives
the dropping context.

This satisfies ADR-056's contract: dropping `exit_code` (cancel) kills
the session target. The "session target" for docker is the container
(attach) or the exec's process (exec, best-effort via the container).
A backend that returned an `exit_code` future without a kill-on-Drop
guard would violate the contract and could leave orphaned containers.

### Backend Registration

```rust
// assembly layer (behind `tty` feature)
let docker_backend = Arc::new(DockerTtyBackend::new(
    docker.clone(),
    "alknet".into(),  // label prefix (ADR-060)
)) as Arc<dyn TtyBackend>;

let mut backends = HashMap::new();
backends.insert("docker".into(), docker_backend);
// (other backends: "local" from alknet-tty-local, "ssh" from alknet-ssh, etc.)
let tty_adapter = TtyAdapter::new(Arc::new(backends), ownership_provider);
```

The backend is registered under the `"docker"` key — the
`backend: "docker"` field in the negotiation frame selects it. A
deployment that doesn't want docker terminals doesn't register it; a
deployment that wants both docker and local terminals registers both.

## Constraints

- **The backend produces handles; the adapter owns the wire format.**
  The backend does not write chunks to the bidi stream — the
  `TtyAdapter` does (ADR-052). The backend's `allocate()` returns a
  `TtyHandle`; the adapter pumps `stdin`, `stdout`, `exit_code`, and
  dispatches `control` chunks. A backend that wrote to the wire
  directly would break the wire-format invariants (the exit-chunk
  ordering, ADR-055).
- **PTY mode merges stdout/stderr.** `TtyHandle.stderr` is `None` for
  both attach and exec modes (both set `tty: true`). The adapter pumps
  only stdout chunks (stream_type 1). This matches the PTY property
  (one output stream from the slave) and the
  [tty-backend.md](../tty/tty-backend.md) sketch.
- **`exit_code` future's `Drop` kills (ADR-056).** The `DockerExitGuard`
  in the `exit_code` future issues `kill_container` on `Drop`-without-
  resolve. The guard's `killed` flag prevents double-kill. The
  spawned-on-Drop pattern runs the async kill from the sync `Drop`.
- **Signal delivery is best-effort.** docker has no per-exec signal
  API in bollard's stable surface. `TtyControl::signal()` maps to
  `kill_container` with the container's main process as the target.
  Unknown signals fall back to `SIGKILL`. This is within the
  `TtyControl::signal` contract (ADR-053) but is a docker-semantics
  limitation, documented not hidden.
- **Attach doesn't own the container lifecycle.** The cancel-cleanup
  kills the container (terminates the attached process) but does not
  remove it. A container that was a hosted service (operator-created)
  stays after the terminal session ends — the operator may want it
  running. The kill (not remove) is the correct teardown for attach.
- **`resource_id()` delegates to the backend.** The adapter doesn't
  parse docker JSON to extract the container ID; the backend's
  `resource_id()` does. The adapter checks
  `OwnershipProvider::owns(identity, "container", id, "tty")`. The
  `"tty"` action is distinct from the call operation's `"exec"`
  action — interactive terminal authorization is a separate scope.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| DockerTtyBackend in alknet-docker | [ADR-061](../../decisions/061-docker-tty-backend-in-alknet-docker.md) | Behind `tty` feature; attach/exec mode; POC `drive_attach_raw` as reference |
| TtyBackend trait and TtyHandle | [ADR-053](../../decisions/053-ttybackend-trait-and-ttyhandle.md) | The trait this backend implements; backend params opaque; REQ-TTY-01 (backends need not be natively async — bollard is async, so no bridge needed) |
| Wire format | [ADR-052](../../decisions/052-alknet-tty-wire-format-and-two-carriage.md) | The chunk codec + control channel the adapter pumps to/from these handles |
| Exit code on a control chunk | [ADR-055](../../decisions/055-exit-code-on-control-chunk.md) | The adapter awaits `exit_code`, sends the exit chunk; the backend's `exit_code` resolves after output EOF + `inspect_exec` |
| Backend cleanup on session cancel | [ADR-056](../../decisions/056-backend-cleanup-on-session-cancel.md) | `exit_code` future's `Drop` kills the container (attach) or container's main process (exec, best-effort) |
| Container resource model | [ADR-060](../../decisions/060-container-resource-model-and-label-namespace.md) | `resource_id()` delegation; `"tty"` action distinct from call op's `"exec"` |
| bollard 0.21 + features | [ADR-059](../../decisions/059-bollard-021-dependency-and-features.md) | No `websocket` feature — reliable `attach_container()` only |

## Open Questions

None. The backend's design is decided in ADR-061. The
signal-delivery path's precision in exec mode is a docker-semantics
limitation (docker has no per-exec signal API in bollard's stable
surface; `kill_container` targets the container's main process,
best-effort per the `TtyControl::signal` contract), not an open
question — the limitation is documented in §"DockerControl" and
acknowledged in ADR-061 §3.

## References

- [ADR-061](../../decisions/061-docker-tty-backend-in-alknet-docker.md)
  — the crate placement and attach/exec mode decision
- [ADR-053](../../decisions/053-ttybackend-trait-and-ttyhandle.md) — the
  trait this backend implements
- [ADR-052](../../decisions/052-alknet-tty-wire-format-and-two-carriage.md)
  — the wire format the adapter pumps
- [ADR-055](../../decisions/055-exit-code-on-control-chunk.md) — the
  exit-chunk ordering the `exit_code` future feeds into
- [ADR-056](../../decisions/056-backend-cleanup-on-session-cancel.md)
  — the cancel-cleanup contract (`exit_code` future's `Drop` kills)
- [ADR-060](../../decisions/060-container-resource-model-and-label-namespace.md)
  — the `resource_id()` delegation and the `"tty"` action
- `docs/research/alknet-docker/poc-summary.md` §"POC Target 1"
  (interactive attach — the seed of this backend)
- `/workspace/alknet-docker-poc/src/ops.rs` — `drive_attach_raw` (the
  reference for `allocate()`)
- `/workspace/bollard/src/container.rs` (`attach_container` :540,
  `AttachContainerResults` :80, `LogOutput` :96,
  `resize_container_tty` :687, `kill_container` :1059)
- `/workspace/bollard/src/exec.rs` (`create_exec` :172, `start_exec`
  :225, `inspect_exec` :315, `resize_exec` :362, `StartExecResults` :99)
- [tty-backend.md](../tty/tty-backend.md) §"Backend implementations"
  (the row this spec fills)