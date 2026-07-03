---
status: draft
last_updated: 2026-07-03
---

# alknet-tty — Phase 0 Research Findings

This document captures Phase 0 (Exploration) findings for the `alknet-tty`
crate. The objective of Phase 0 per `docs/sdd_process.md` is: *"Capture vision
and guiding principles; research options; validate approaches; converge on a
recommended approach."* It is the input to Phase 1 (Architecture), where the
Architect will produce `docs/architecture/crates/tty/*.md` specs, ADRs, and
open questions.

This document was drafted 2026-07-03, immediately after the `alknet-docker`
POC (`docs/research/alknet-docker/poc-summary.md`) validated that bollard's
container attach maps cleanly onto a framed bidi stream with a 1-byte
stream-type multiplexer. The POC's raw chunk format is the seed of
`alknet-tty`'s wire format.

## Vision Recap

`alknet-tty` is a terminal session protocol handler for the ALPN-as-service
architecture (ADR-001). It registers the `alknet/tty` ALPN on the shared
`AlknetEndpoint` and implements the `ProtocolHandler` trait (ADR-002,
ADR-007).

The guiding insight, surfaced during the alknet-docker POC and recognized in
the conversation that followed:

> **A terminal session is not an SSH concern, or a Docker concern — it is a
> terminal concern. SSH and Docker are just two backends that can allocate
> a PTY.**

The alknet-docker POC proved that the hard part of interactive attach —
bidirectional byte pumping over a framed stream with a multiplexing header —
is the same problem regardless of whether the backend is `bollard::attach_container()`
or russh's `pty_request` + session channel. The POC's raw chunk format
(`[stream_type: u8][length: u32 be][payload bytes]`, with stream_type
0=stdin, 1=stdout, 2=stderr) is a deliberately impoverished version of SSH's
channel multiplexer: fixed set of channel types, no negotiation, no
open/close handshake, no windowing (QUIC provides flow control on the bidi
stream). That impoverishment is the feature — a terminal session needs
exactly those channels and no more.

`alknet-tty` extracts that pattern into its own crate and ALPN. The
backends (Docker, SSH, local process) implement a `TtyBackend` trait; the
`alknet/tty` handler is backend-agnostic. This dissolves the PTY hedge in
the alknet-ssh research (`docs/research/alknet-ssh/phase-0-findings.md`
DP-5: "shell_request and pty_request default-reject; interactive shell is
an explicit opt-in") — PTY is not an SSH feature, it's a tty feature that
SSH happens to be able to provide.

Beyond terminals, the same wire format and backend trait support a general
"runner" pattern: a process (local `std::process::Command`, docker
container, SSH exec) whose stdin/stdout/stderr/exit-code are streamed over
a framed bidi connection. The dispatch project
(`/workspace/@alkdev/dispatch/`) is a reverse runner that currently requires
an SSH server on the remote end; with `alknet-tty` and a local-process
backend, the same runner pattern works without SSH at all — the endpoint
runs the process directly and streams its I/O back. This is the same shape
as GitHub/Gitea Actions runners, just over alknet's transport instead of
HTTP polling.

## Sources Investigated

| Source | Path | Note |
|--------|------|------|
| alknet-docker POC | `/workspace/alknet-docker-poc/` | Validated raw chunk format, two-carriage model, bidirectional pumping against live docker. The POC's `src/raw.rs` is the seed of alknet-tty's wire format. |
| alknet-docker POC summary | `docs/research/alknet-docker/poc-summary.md` | Documents the two-carriage model (JSON negotiation → raw bytes), the three validated targets, the open unknowns. |
| alknet-ssh phase-0 findings | `docs/research/alknet-ssh/phase-0-findings.md` | DP-5 hedges PTY as an SSH concern; the channel decomposition (Layers 1-7) treats PTY as part of Layer 4 (Session/exec). This document dissolves that hedge. |
| alknet-core types | `crates/alknet-core/src/types.rs` | `ProtocolHandler`, `Connection`, `SendStream`, `RecvStream` — the handler interface alknet-tty implements. |
| alknet-call wire format | `crates/alknet-call/src/protocol/wire.rs` | `EventEnvelope`, `FrameFramedReader/Writer` — the JSON carriage layer alknet-tty uses for the initial `call.requested` negotiation frame. |
| alknet-call dispatch | `crates/alknet-call/src/protocol/dispatch.rs` | `handle_stream` (:295), `pump_stream` (:340) — the streaming pump pattern. alknet-tty's raw-carriage path is a sibling to this, not a consumer of it. |
| bollard source | `/workspace/bollard/src/` | `container.rs` (`attach_container` :540, `LogOutput` :96, `AttachContainerResults` :80), `read.rs` (`NewlineLogOutputDecoder` :32 — the 8-byte header format our chunk format mirrors), `exec.rs` (`StartExecResults` enum :99) |
| bollard examples | `/workspace/bollard/examples/attach_container.rs` | Reliable attach + TTY passthrough. |
| dispatch project | `/workspace/@alkdev/dispatch/` | The "reverse runner" — axum + russh SSH client for exec/forwarding/sync over Docker/vast.ai. `src/handlers.rs` (`start_job`, `job_status`, `job_logs`) is the runner pattern alknet-tty generalizes. Currently requires SSH on the remote; alknet-tty with a local-process backend removes that requirement. |
| russh source | `/workspace/russh/` | `server::Handler` — `pty_request` (allocates PTY), `window_change` (resize), `signal` (signal forwarding), `shell_request`/`exec_request`. These are the SSH-side operations a `SshTtyBackend` wraps. |
| alknet-runtime research | `docs/research/alknet-runtime/summary.md` | The "operation host" pattern — a node that exposes ops on a registry. alknet-tty is the same pattern for process execution: a node that can run a process and stream its I/O. |
| Rust std::process | stdlib | `Command`, `Stdio` (piped stdin/stdout/stderr), `Child::wait` (exit code). The local-process backend. The threading/deadlock caveat (must read stdout/stderr concurrently with writing stdin to avoid pipe-buffer deadlock) is handled by the bidirectional pump, same as docker attach. |

## The Wire Format: From POC to Spec

### What the alknet-docker POC validated

The POC's `src/raw.rs` defines a chunk format for raw carriage on a bidi
stream:

```text
[stream_type: u8][length: u32 be][payload bytes]
```

- `stream_type` mirrors bollard's `NewlineLogOutputDecoder` header byte
  (`/workspace/bollard/src/read.rs:46`): 0=stdin, 1=stdout, 2=stderr.
- `length` is the payload length in bytes (u32 big-endian, max 16 MiB).
- A zero-length chunk is a sentinel (used for completion notification).

The POC proved this format works for:
- **server→client stdout/stderr**: each `LogOutput` from bollard's attach
  stream becomes a chunk with the matching stream_type.
- **client→server stdin**: `ChunkWriter::write_stdin(bytes)` writes a
  type-0 chunk; the server reads it and writes the bytes to bollard's
  `container_input` (`AsyncWrite`).
- **completion**: when bollard's output stream ends (container exited),
  the server sends a zero-length type-1 chunk as a "drained" sentinel.

### What alknet-tty adds

A terminal session needs two things the docker attach POC didn't:

1. **Control messages during the raw phase.** Window resize (SIGWINCH) and
   signal forwarding (Ctrl-C → SIGINT) must ride *during* the byte stream,
   not as a new request. The chunk format handles this by reserving a 4th
   stream_type:

   | stream_type | channel | direction | payload |
   |---|---|---|---|
   | 0 | data-in (stdin) | client→server | raw bytes |
   | 1 | data-out (stdout) | server→client | raw bytes |
   | 2 | data-err (stderr) | server→client | raw bytes |
   | 3 | control | bidirectional | JSON control message |

   Control chunks carry a small JSON payload:
   - `{"type":"resize","cols":80,"rows":24,"pixel_width":0,"pixel_height":0}` —
     window resize (maps to SSH `window-change`, docker exec resize, or
     `ioctl(TIOCSWINSZ)` on a local PTY).
   - `{"type":"signal","name":"INT"}` — signal forwarding (maps to SSH
     `signal`, docker exec signal, or `kill(pid, sig)` on a local process).
   - `{"type":"eof"}` — client signals no more stdin (maps to SSH channel
     EOF, docker stdin close, or `ChildStdin::drop`).
   - `{"type":"exit","code":0}` — server signals process exit (terminal,
     no more data chunks follow; the stream then closes).

2. **Terminal parameters at negotiation time.** The initial `call.requested`
   frame (JSON carriage, same as the POC) carries the terminal attributes
   that the backend needs to allocate the PTY:

   ```json
   {
     "operationId": "/tty/open",
     "carriage": "raw",
     "backend": "docker",
     "container": "abc123",
     "tty": {
       "term": "xterm-256color",
       "cols": 80,
       "rows": 24,
       "pixel_width": 0,
       "pixel_height": 0,
       "modes": {}
     },
     "cmd": ["/bin/bash"]
   }
   ```

   The `tty` block maps directly to SSH's `pty_request` parameters
   (term, cols, rows, pixel_width, pixel_height, modes) and to docker's
   `CreateExecOptions { tty: true }`. A local-process backend passes them
   to `portable_pty::PtySystem::openpty` (or equivalent).

### Why fixed channel set, not extensible

SSH's channels are `ChannelId(u32)` with string-named types negotiated per
channel. alknet-tty's channels are a fixed `u8` set with no negotiation.
This is a one-way door (adding a 5th channel type is a wire-format change),
and it's the right one-way door:

- **The use cases are bounded.** A terminal session has stdin, stdout,
  stderr, and control. If something genuinely new appears (say, a
  sideband file-transfer channel alongside the terminal), that's a
  different ALPN, not a 5th tty channel type. The ALPN model handles
  extensibility at the protocol level — a new ALPN is cheap, a wire-format
  change is not.
- **1 byte vs length-prefixed string + negotiation round-trip.** The fixed
  set is faster, simpler, and the demuxing is a `match` instead of a hash
  lookup. For a terminal session where every chunk is hot, this matters.
- **The comparison to SSH channels is the justification, not the
  constraint.** SSH needs dynamic channels because it multiplexes
  *arbitrary* services (forwarding, SFTP, agent, X11) over one connection.
  alknet-tty multiplexes *one* service (a terminal session) with a fixed
  channel structure. The impoverishment is the feature.

## The Backend Trait

The `TtyBackend` trait is the inversion point that keeps alknet-tty
decoupled from its backends:

```rust
#[async_trait]
pub trait TtyBackend: Send + Sync {
    async fn allocate(&self, params: &TtyParams) -> Result<TtyHandle, TtyError>;
}

pub struct TtyParams {
    pub backend_params: BackendParams,  // backend-specific (container id, ssh host, command)
    pub terminal: TerminalParams,        // term, cols, rows, modes
    pub cmd: Vec<String>,
}

pub enum BackendParams {
    Docker { container: String },
    Ssh { channel: SshChannelRef },
    Local { cwd: Option<PathBuf>, env: HashMap<String, String> },
}

pub struct TtyHandle {
    pub stdin: Box<dyn AsyncWrite + Send + Unpin>,
    pub stdout: Pin<Box<dyn Stream<Item = Bytes> + Send>>,
    pub stderr: Option<Pin<Box<dyn Stream<Item = Bytes> + Send>>>,  // None if PTY (merged into stdout)
    pub exit_code: BoxFuture<'static, Result<i32, TtyError>>,
    pub control: Box<dyn TtyControl + Send + Unpin>,  // resize, signal
}
```

The `TtyAdapter` (the `ProtocolHandler` for `alknet/tty`) receives the
`Connection`, reads the `call.requested` frame, selects the backend by the
`backend` field, calls `allocate()`, and pumps bytes bidirectionally using
the chunk format. Control chunks are dispatched to `TtyHandle::control`.
When `exit_code` resolves, the server sends a `{"type":"exit","code":N}`
control chunk and closes the stream.

Three implementations, each in its own crate (the no-handler-depends-on-
another-handler rule from ADR-003 is preserved — backends depend on
alknet-tty for the trait, alknet-tty doesn't depend on them):

- **`DockerTtyBackend`** (in alknet-docker, or a thin adapter): wraps
  `bollard::attach_container()` → `AttachContainerResults { output, input }`
  for interactive attach, or `bollard::exec::start_exec` with `tty: true`
  for exec-with-PTY. The POC's `drive_attach_raw` *is* this backend,
  inlined; with the trait, it becomes `impl TtyBackend for DockerTtyBackend`.
  `control.resize()` calls `bollard::exec::resize_exec` or
  `bollard::container::resize_container`.

- **`SshTtyBackend`** (in alknet-ssh): wraps russh's `pty_request` +
  `shell_request` (or `exec_request` with a PTY) on a session channel.
  `channel.into_stream()` gives `(AsyncRead, AsyncWrite)` — the stream
  *is* the PTY; russh handles kernel PTY allocation on the server side.
  `control.resize()` sends a `window_change` channel request;
  `control.signal()` sends a `signal` channel request. stdout and stderr
  are merged (PTY property), so `TtyHandle.stderr` is `None`.

- **`LocalTtyBackend`** (in alknet-tty or a sibling crate): wraps
  `std::process::Command` with `Stdio::piped()` for stdin/stdout/stderr,
  OR `portable_pty` for a real PTY (needed for terminal escape sequences,
  signal delivery, window resize). Without a PTY, it's a "runner" (piped
  process); with a PTY, it's a terminal. `control.resize()` calls
  `ioctl(TIOCSWINSZ)` on the PTY master; `control.signal()` calls
  `kill(child.pid, sig)`. The threading/deadlock caveat (must read
  stdout/stderr concurrently with writing stdin to avoid pipe-buffer
  deadlock) is handled by the bidirectional pump — the same pattern as
  docker attach, where `tokio::spawn` runs the two directions concurrently.

### The runner generalization

The `LocalTtyBackend` without a PTY is the "runner" pattern: a process
whose stdin/stdout/stderr/exit-code are streamed over a framed bidi
connection. This is functionally identical to GitHub/Gitea Actions runners,
just over alknet's transport instead of HTTP polling:

- A coordinator sends `{"backend":"local","cmd":["cargo","test"],"tty":null}`
  — no terminal, just a command.
- The endpoint runs `cargo test` with piped stdio, streams stdout/stderr
  chunks back, sends `{"type":"exit","code":N}` when it finishes.
- The coordinator gets reliable completion notification (the exit control
  chunk + stream close) — the same stopgap property as the docker logs
  subscription.

The dispatch project (`/workspace/@alkdev/dispatch/`) is a reverse runner
that currently requires an SSH server on the remote end (it uses russh to
exec commands and stream output). With `LocalTtyBackend`, the same pattern
works without SSH — the endpoint runs the process directly. SSH becomes
one transport option (for reaching hosts that don't run alknet), not a
requirement. This is "discuss afterwards" territory per the conversation,
but the trait shape preserves the option.

## What This Dissolves in alknet-ssh

### DP-5's PTY hedge

The alknet-ssh research (`phase-0-findings.md` DP-5) says:

> `shell_request` and `pty_request` default-reject; `exec_request`
> permitted (gated by ACL). This keeps alknet-ssh a focused forwarding/exec
> appliance rather than a general-purpose interactive login server.
> Interactive shell is an explicit opt-in (two-way door).

With alknet-tty, PTY is not an SSH feature — it's a tty feature. alknet-ssh
implements `TtyBackend` for SSH session channels; alknet-tty owns the
terminal session lifecycle. alknet-ssh's session channel (Layer 4) still
does `exec` (structured, JSON carriage, exit code on completion) but
*delegates* PTY to alknet-tty. The "default-reject" stance stays for the
SSH channel policy (alknet-ssh still rejects `pty_request` on its own
session channels — it doesn't serve terminals directly), but the PTY
capability is provided by a separate crate via a separate ALPN, not hedged
inside alknet-ssh.

### Layer 4 simplifies

The alknet-ssh build order was "1-4 first (SSH+exec), then 5 (forwarding),
then 6/7 (SOCKS5/SFTP)." PTY was a deferred wart on Layer 4. With
alknet-tty, Layer 4 is just `exec` (one-shot command, JSON carriage, exit
code on completion) — clean and complete. PTY is a *different ALPN*
(`alknet/tty`) that happens to use SSH as its backend.

### The browser case gets a terminal for free

The alknet-ssh research notes the browser runs a WASM SSH client over
WebTransport (ADR-040). But a browser terminal (xterm.js) doesn't want SSH
— it wants a terminal. With `alknet/tty` as an ALPN, xterm.js connects via
WebTransport to `/alknet/tty`, negotiates a session (docker container, SSH
PTY, or local process), and gets raw bytes. The browser doesn't need to
implement SSH at all for the terminal use case — it only needs SSH if it
wants SSH-specific features (port forwarding, SFTP). This is a cleaner
browser story than "run a WASM SSH client."

## Straightforward Parts

These are settled by the POC, existing ADRs, and the wire format above.
Phase 1 should document them as spec rather than re-litigate.

### 1. alknet-tty is a `ProtocolHandler` on `alknet/tty`

Same pattern as every other handler: `TtyAdapter` implements
`ProtocolHandler::handle(&self, connection: Connection, auth: &AuthContext)`
with `alpn() = b"alknet/tty"`. The handler owns the entire `Connection`
lifecycle (ADR-006) and accepts one bidi stream per terminal session.

### 2. The two-carriage model is inherited from the POC

The initial `call.requested` frame is JSON (length-prefixed `EventEnvelope`,
identical to alknet-call's `FrameFramedReader/Writer`). After the request,
the stream switches to raw chunks. The `carriage` field in the request
payload is `"raw"` for terminal sessions. This is the same mechanism the
POC validated; no new wire-format invention.

### 3. Raw chunk format is POC-validated

The `[stream_type: u8][length: u32 be][payload]` format, the `ChunkReader`/
`ChunkWriter` types, and the bidirectional pump pattern are all directly
from the POC's `src/raw.rs`. The only addition is `stream_type: 3` for
control messages, which is a 1-byte extension to a validated format.

### 4. Backend trait is the inversion point

alknet-tty defines `TtyBackend`; the backend crates (alknet-docker,
alknet-ssh, local) implement it. The `TtyAdapter` is backend-agnostic.
This preserves ADR-003's no-handler-depends-on-another-handler rule:
alknet-tty depends on alknet-core; the backend crates depend on alknet-tty
(for the trait); alknet-tty doesn't depend on any backend.

### 5. Completion notification is free

The exit control chunk (`{"type":"exit","code":N}`) + stream close gives
the coordinator deterministic completion notification — the same stopgap
property the docker POC validated for logs subscriptions. No plugin state,
no polling. The container/process exiting is the signal.

## Less Straightforward Parts (Decision Points)

### DP-1: Local-process backend in alknet-tty or a sibling crate?

*(Recommended: two-way door — start in alknet-tty, extract if warranted)*

The `LocalTtyBackend` (std::process::Command / portable_pty) is the
simplest backend and the one that enables the runner pattern. It has no
heavy dependencies (no bollard, no russh — just std + optionally
`portable_pty`). Two options:

- **(a) In alknet-tty**: the crate ships with the local backend built-in.
  Pro: zero-config runner, one crate gets you a terminal/process-streaming
  endpoint. Con: alknet-tty pulls in `portable_pty` even for deployments
  that only use docker/ssh backends.
- **(b) In a sibling crate (`alknet-tty-local`)**: alknet-tty defines the
  trait; the local backend is a separate crate. Pro: alknet-tty stays
  dependency-light; consumers opt into the local backend explicitly. Con:
  one extra crate for the common case.

**Recommendation**: **(b) sibling crate**, behind a feature flag on
alknet-tty for the common case (`features = ["local"]` → re-export from
`alknet-tty-local`). This keeps alknet-tty's default dependency surface
minimal while making the local backend a one-feature opt-in. The local
backend is where the `portable_pty` dependency lives; alknet-tty itself
depends only on alknet-core and the frame/raw codec. Extraction is cheap
because the trait is the seam.

### DP-2: PTY vs pipe for the local backend

*(Recommended: two-way door — support both, PTY is opt-in)*

`std::process::Command` with `Stdio::piped()` gives pipes (no terminal
semantics — no signal delivery, no window resize, no escape-sequence
handling). `portable_pty` gives a real PTY (terminal semantics, resize,
signals, escape sequences). The `TtyParams.terminal` field distinguishes:
if `terminal` is `Some(TerminalParams { ... })`, the backend allocates a
PTY; if `None`, it uses pipes (the runner case).

**Recommendation**: support both. The `TtyHandle.stderr` field is `None`
for PTY (stdout/stderr merged) and `Some` for pipes (separate streams).
The `control` field is a no-op impl for pipes (resize/signal don't apply
without a PTY — though `kill(pid, sig)` still works for signal forwarding).
The decision is per-session, not per-deployment.

### DP-3: Control message format — JSON vs binary

*(Recommended: two-way door — JSON first, binary if hot)*

Control chunks (stream_type 3) carry a JSON payload (`{"type":"resize",
"cols":80,"rows":24}`). This is consistent with the call protocol's
JSON-everything stance and easy to extend. A binary format
(`[control_type: u8][params...]`) would be faster but harder to extend and
inconsistent with the negotiation layer.

**Recommendation**: JSON first. Control messages are rare (resize happens
on window drag, signal on Ctrl-C) — the serialization cost is negligible
compared to the data chunks. If a hot control path appears (unlikely for
terminals), a binary format can be added as a `control_type` extension
without breaking the chunk format.

### DP-4: The threading/deadlock caveat for piped processes

*(Recommended: acknowledged constraint — the bidirectional pump handles it)*

`std::process::Command` with piped stdio can deadlock if stdin writes
block while stdout/stderr buffers fill — the classic pipe-buffer deadlock.
The fix is concurrent reads on stdout/stderr alongside stdin writes, which
is exactly what the bidirectional pump does (the POC's `drive_attach_raw`
runs the two directions as concurrent `tokio::spawn` tasks). The same
pattern works for `LocalTtyBackend`: spawn one task pumping stdin→process,
one task pumping process→stdout-chunks, one for stderr if piped.

**Recommendation**: Phase 1 records this as a known constraint with a
known solution (concurrent pumping). No design decision needed — the POC
already proved the pattern. The spec notes that `LocalTtyBackend` must use
the concurrent-pump pattern, not sequential read-then-write.

### DP-5: Exit code propagation — control chunk vs final data chunk

*(Recommended: one-way door — control chunk)*

The alknet-docker POC validated exit-code-on-final-`call.responded` for
the JSON carriage path (exec with exit code). The raw carriage path needs
a different mechanism because there's no `call.responded` after the raw
phase begins. Two options:

- **(a) Control chunk**: `{"type":"exit","code":N}` as the last chunk
  before stream close. Clean, explicit, carries the code as structured
  data.
- **(b) Final data chunk with exit code**: a special stdout chunk with an
  exit-code payload. Hacky — overloads the data channel for metadata.

**Recommendation**: **(a) control chunk**. The exit code is control
metadata, not data. The control channel (stream_type 3) exists for exactly
this. The chunk is the last thing before stream close; the client reads it
and knows the process exited with code N. This is a one-way door because
clients will depend on the "exit chunk is last" invariant.

### DP-6: Multiple sessions per connection

*(Recommended: two-way door — one session per stream, multiple streams per connection)*

A `Connection` (ADR-007) can open/accept multiple bidi streams. Should one
`alknet/tty` connection host multiple terminal sessions (one per stream),
or one session per connection?

**Recommendation**: **one session per bidi stream, multiple streams per
connection**. This matches the call protocol's model (one operation per
stream, multiple operations per connection) and is the natural fit for
QUIC's stream multiplexing. A coordinator opens one connection to an
endpoint and launches multiple sessions (one stream each) for parallel
tasks. The `TtyAdapter::handle` accepts the connection and loops
`accept_bi`, dispatching each stream to a session — same pattern as
alknet-call's `Dispatcher::run_loop` (`protocol/dispatch.rs:369`).

## Recommended Approach

### Crate

`alknet-tty`, depends on `alknet-core` (for `ProtocolHandler`, `Connection`).
Defines the `TtyBackend` trait, the wire format (chunk codec + control
messages), and the `TtyAdapter` (`ProtocolHandler` for `alknet/tty`). Does
not depend on bollard, russh, or portable_pty — those are in the backend
crates.

### Build order

**Step 1: Wire format + TtyAdapter + mock backend.**
- Extract `raw.rs` from the POC into alknet-tty's wire format module.
- Add `stream_type: 3` (control) and the control message types
  (resize, signal, eof, exit).
- Implement `TtyAdapter` with a mock backend (in-memory pipes) to validate
  the full protocol: negotiate → pump → control → exit → close.
- **Result**: a working `alknet/tty` handler with no real backends, but
  the wire format and session lifecycle are proven.

**Step 2: LocalTtyBackend (runner).**
- `alknet-tty-local` crate (or feature): `impl TtyBackend for LocalTtyBackend`
  using `std::process::Command` with piped stdio.
- Validate the runner pattern: `cargo test` as the command, stream
  stdout/stderr/exit over `alknet/tty`.
- Add `portable_pty` for the PTY case (terminal semantics, resize, signals).
- **Result**: a working runner/terminal endpoint with no docker or SSH
  dependency.

**Step 3: DockerTtyBackend.**
- In alknet-docker: `impl TtyBackend for DockerTtyBackend` wrapping
  `bollard::attach_container` / `exec with tty:true`.
- The POC's `drive_attach_raw` becomes this backend; the `TtyAdapter` calls
  it via the trait.
- **Result**: docker containers as terminal sessions via `alknet/tty`.

**Step 4: SshTtyBackend.**
- In alknet-ssh: `impl TtyBackend for SshTtyBackend` wrapping russh's
  `pty_request` + `shell_request`/`exec_request` on a session channel.
- `control.resize()` → `window_change` channel request;
  `control.signal()` → `signal` channel request.
- **Result**: SSH PTYs as terminal sessions via `alknet/tty`. alknet-ssh's
  DP-5 hedge dissolves — PTY is delegated to alknet-tty.

### De-risk POC (extending the alknet-docker POC)

The alknet-docker POC already validated targets 1 (attach round-trip), 2
(logs completion), and 3 (exec exit code). Two extensions validate the
alknet-tty additions:

1. **Control message during raw phase** — add `stream_type: 3` to the POC's
   chunk format, send a `resize` control chunk mid-session, prove the
   backend receives it. For docker this requires `tty: true` on the exec
   and `bollard::exec::resize_exec`. Small POC, validates the control
   channel mechanism.

2. **PTY allocation via docker exec with TTY** — `CreateExecOptions { tty:
   true }` allocates a real PTY. Validate that stdout/stderr merge
   (stream_type always 1) and that resize works. Proves the docker-as-PTY-
   backend path.

Both are extensions to the existing POC, not new POCs. The wire format and
bidirectional pump are already proven; these just confirm the control
channel and PTY-specific paths.

## Open Questions to Carry into Phase 1

- **OQ-TTY-01 (backend trait shape)**: the exact `TtyHandle` field set —
  is `control` a separate trait object or are resize/signal methods on
  `TtyHandle` directly? Does `exit_code` belong on the handle or is it a
  separate `Future` the adapter awaits? Resolved by Phase 1 spec; the POC
  extension informs the decision.
- **OQ-TTY-02 (terminal modes)**: SSH's `pty_request` carries TTY modes
  (echo, raw, canonical, etc.) as a packed bitmask. Does alknet-tty
  support these, or defer to the backend's defaults? Likely defer for v1
  (the common case is "default terminal modes"); the `modes` field in
  `TerminalParams` is reserved for future use.
- **OQ-TTY-03 (flow control)**: the chunk format has no windowing (QUIC
  provides flow control on the bidi stream). Is this sufficient for
  high-throughput stdout (e.g., `cargo build` output)? QUIC's per-stream
  flow control should handle it, but a POC with real high-volume output
  would confirm. Low risk — the docker POC's logs subscription handled
  multi-line output without issue.
- **OQ-TTY-04 (local backend crate placement)**: confirm `alknet-tty-local`
  as a sibling crate vs a feature flag on alknet-tty. DP-1 recommends
  sibling + feature re-export; Phase 1 confirms.
- **OQ-TTY-05 (runner API surface)**: the "runner" generalization
  (local-process backend without PTY) is noted as "discuss afterwards" in
  the conversation. Phase 1 should at minimum preserve the option
  (`TtyParams.terminal = None` → pipe mode) even if the runner-specific
  API surface (job management, log persistence, task graph integration) is
  deferred to a later crate.

## Next Steps (Phase 0 → Phase 1)

1. **POC extension**: extend `/workspace/alknet-docker-poc` with
   `stream_type: 3` (control) and `tty: true` exec to validate the control
   channel and PTY allocation. Timeboxed; the wire format is already
   proven, these are extensions.
2. **You decide** on the DP recommendations (or amend them). DP-1 (local
   backend placement) and DP-5 (exit code on control chunk) are the
   load-bearing choices. DP-2, DP-3, DP-4, DP-6 are defaults recommended
   as-is.
3. **Phase 1 (Architect)**: produce `docs/architecture/crates/tty/README.md`
   + component specs (`tty-wire.md` for the chunk format + control
   messages, `tty-backend.md` for the `TtyBackend` trait + `TtyHandle`,
   `tty-adapter.md` for the `ProtocolHandler` + session lifecycle,
   `tty-local.md` for the local backend / runner), ADRs for the accepted
   DPs (wire format + fixed channel set, backend trait as inversion point,
   local backend placement, exit code on control chunk), and the OQs above
   in `open-questions.md`. Update `docs/architecture/README.md` index and
   ADR table.

## References

- `docs/research/alknet-docker/poc-summary.md` — the POC that seeded this
  crate. Raw chunk format, two-carriage model, three validated targets.
- `/workspace/alknet-docker-poc/src/raw.rs` — the chunk codec
  (`ChunkReader`, `ChunkWriter`, stream_type 0/1/2) that alknet-tty
  extends with stream_type 3.
- `/workspace/alknet-docker-poc/src/ops.rs` — `drive_attach_raw` (the
  bidirectional pump pattern, the session lifecycle) that the
  `TtyAdapter` generalizes.
- `docs/research/alknet-ssh/phase-0-findings.md` — DP-5 (PTY hedge, dissolved
  by this crate), the channel decomposition (Layers 1-7, PTY moves out of
  Layer 4), the browser case (xterm.js over WebTransport to `/alknet/tty`).
- `docs/architecture/decisions/001-alpn-protocol-dispatch.md` — ALPN dispatch
- `docs/architecture/decisions/002-protocol-handler-trait.md` — ProtocolHandler
- `docs/architecture/decisions/007-bistream-type-definition.md` — Connection,
  SendStream, RecvStream
- `docs/architecture/decisions/003-crate-decomposition.md` — no-handler-depends-
  on-another-handler (alknet-tty depends on alknet-core; backends depend on
  alknet-tty for the trait)
- `docs/architecture/decisions/040-webtransport-alpn-stream-proxy.md` —
  WebTransport stream → `Connection` (the browser terminal path)
- `/workspace/bollard/src/read.rs` — `NewlineLogOutputDecoder` (the 8-byte
  header format our chunk format mirrors)
- `/workspace/russh/` — `server::Handler` (`pty_request`, `window_change`,
  `signal`) — the SSH operations a `SshTtyBackend` wraps
- `/workspace/@alkdev/dispatch/` — the reverse runner that currently requires
  SSH; `LocalTtyBackend` removes that requirement
- `docs/research/alknet-runtime/summary.md` — the "operation host" pattern
  (alknet-tty is the same pattern for process execution)