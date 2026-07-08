# ADR-061: DockerTtyBackend in alknet-docker

## Status

Accepted

## Context

The alknet-tty spec ([tty-backend.md](../crates/tty/tty-backend.md)
§"Backend implementations") lists three `TtyBackend` implementations:
`LocalTtyBackend` (in `alknet-tty-local`), `DockerTtyBackend` (location
"future, out of scope here"), and `SshTtyBackend` (in `alknet-ssh`).
The alknet-tty spec deliberately left the `DockerTtyBackend` location
open — it committed the trait shape but not the crate placement.

alknet-docker is now being specified. The `DockerTtyBackend` — the
`impl TtyBackend` that wraps `bollard::attach_container()` for
interactive attach and `bollard::exec::start_exec` with `tty: true` for
interactive exec — is the natural extension of the alknet-docker POC's
`drive_attach_raw` (`/workspace/alknet-docker-poc/src/ops.rs`). The
question is which crate it lives in.

### The options

Three placements were considered:

1. **In `alknet-docker`** — the `DockerTtyBackend` is a type in
   alknet-docker, behind a `tty` feature gate that pulls in
   `alknet-tty` (for the `TtyBackend` trait) and `bollard` (for the
   attach/exec API). alknet-docker already depends on bollard for its
   call operations; the `tty` feature adds only the `alknet-tty` edge.

2. **In `alknet-tty-docker`** (a sibling adapter crate) — the
   `DockerTtyBackend` in its own crate, depending on `alknet-tty` (the
   trait) and `bollard` (the API). alknet-docker stays leaner (no
   `alknet-tty` edge); the adapter is its own publishable unit.

3. **In `alknet-tty-local`** (extending the local sibling) — rejected
   immediately. `alknet-tty-local` is for *local* backends (`portable_pty`,
   `std::process`); docker is not local-process, and pulling bollard into
   the local sibling would violate the feature-gate rationale (a
   docker-only deployment pulling in PTY code, and a local-only
   deployment pulling in bollard).

### Why option 1 (in alknet-docker)

The `DockerTtyBackend` is tightly coupled to alknet-docker's domain:
it uses the same bollard client, the same container identity model
(`resource_id` extraction from `backend_params.container`), and the
same label/ownership reasoning (ADR-060) as the call operations. A
separate `alknet-tty-docker` crate would duplicate the bollard client
construction, the container-id validation, and the ownership wiring —
or import them from alknet-docker, creating a cycle
(`alknet-tty-docker` → `alknet-docker` for the client, `alknet-docker`
→ `alknet-tty-docker` for the backend registration).

The dependency edge from alknet-docker to alknet-tty is clean:
alknet-docker depends on alknet-tty for the `TtyBackend` trait (and
`TtyHandle`, `TtyControl`, `TtyError`, `TtyParams`). This is the same
edge any backend crate has — `alknet-tty-local` depends on
`alknet-tty` for the trait. The trait is the inversion point
(ADR-053); the backend impls live where their transport deps live
(bollard, for `DockerTtyBackend`). bollard already lives in
alknet-docker; the `DockerTtyBackend` belongs with it.

The `tty` feature gate means a deployment that wants only the call
operations (no interactive terminal) doesn't pull in `alknet-tty`. The
feature is opt-in, matching `alknet-tty-local`'s `local` feature
pattern (ADR-054).

### The POC's `drive_attach_raw` as the reference

The POC's `drive_attach_raw` (`/workspace/alknet-docker-poc/src/ops.rs`)
is the seed of `DockerTtyBackend::allocate()`. The mapping:

| POC `drive_attach_raw` | `DockerTtyBackend::allocate()` |
|---|---|
| reads `call.requested` frame, extracts container id | `TtyParams.backend_params` → `DockerBackendParams { container }` |
| `bollard::attach_container()` → `AttachContainerResults { output, input }` | same; `output` → `TtyHandle.stdout`, `input` → `TtyHandle.stdin` |
| `ChunkReader`/`ChunkWriter` on the bidi stream | the `TtyAdapter` owns the chunk codec (ADR-052); the backend produces handles, not wire bytes |
| zero-length stdin chunk → `container_input.shutdown()` | `TtyControl` / stdin EOF — the adapter signals; the backend's `AsyncWrite` close maps to `shutdown()` |
| completion: output stream ends → zero-length stdout sentinel → close | `TtyHandle.exit_code` future — for attach, the exit comes from `wait_container` or the output stream end; for exec with `tty: true`, `inspect_exec` after the output stream ends |
| `LogOutput` → `Chunk` stream_type mapping (StdOut→1, StdErr→2) | `TtyHandle.stdout`/`stderr` — PTY mode merges (stderr is `None`); the adapter pumps stdout only |

The POC's `drive_exec` (non-interactive, exit code via `inspect_exec`)
is the basis for the **call operation** `docker/container/exec`
(`tty: false`), not the `DockerTtyBackend` (which is the `tty: true`
interactive path). The split: non-interactive exec is a call
`Subscription` (ADR-058); interactive exec is a `DockerTtyBackend`
session. Both use `bollard::exec`; they diverge at the wire format.

## Decision

### 1. `DockerTtyBackend` is a type in alknet-docker, behind a `tty` feature

```toml
# alknet-docker Cargo.toml
[features]
default = ["ops"]     # the call operations
ops = []               # docker/container/* and docker/image/* operations
tty = ["dep:alknet-tty"]  # DockerTtyBackend (impl TtyBackend)
```

- `default = ["ops"]` — the call operations (lifecycle, logs, exec
  non-interactive, images). This is what a hub or coordinator wires to
  manage containers over the call protocol.
- `tty` — adds `DockerTtyBackend` (impl `TtyBackend`), pulling in
  `alknet-tty` for the trait. A deployment that wants interactive
  terminal sessions into docker containers enables this and registers
  `DockerTtyBackend` with the `TtyAdapter` (alongside or instead of
  `LocalTtyBackend`).

The feature gate means a deployment that uses alknet-docker only for
call operations (the common case — a hub managing dev containers)
doesn't pull in `alknet-tty`. A deployment that wants interactive
terminals into those containers enables `tty` and registers the backend.

### 2. `DockerTtyBackend::allocate()` wraps `attach_container` or exec-with-PTY

`allocate()` branches on `TtyParams.terminal` and a
`DockerBackendParams` field for the attach mode:

- **`attach` mode** — wraps `bollard::attach_container()` (the reliable
  HTTP-upgrade path, per the POC and ADR-059 §3). Returns
  `AttachContainerResults { output, input }`. `output` → `TtyHandle.stdout`
  (as a `Stream<Bytes>`, mapping `LogOutput` → `Bytes` with the
  stream_type flattened — PTY mode has no separate stderr, so
  `TtyHandle.stderr` is `None`). `input` → `TtyHandle.stdin` (the
  `AsyncWrite` bollard provides). Exit code: the output stream end +
  `wait_container()` or `inspect_container()` for the exit status.
- **`exec` mode** — wraps `bollard::create_exec` + `start_exec` with
  `tty: true`. `start_exec` returns `StartExecResults::Attached {
  output, input }`; same mapping as attach. Exit code: after the output
  stream ends, `inspect_exec()` for the exit code (the POC's pattern).

The mode is selected by a `DockerBackendParams.mode` field
(`"attach" | "exec"`, default `"exec"` for a new command, `"attach"`
for attaching to a running container's primary process). For `exec`
mode, `TtyParams.cmd` is the command vector; for `attach` mode, `cmd`
is ignored (the primary process is already running).

### 3. `TtyControl` maps to bollard's resize and signal

- `resize()` → `bollard::container::resize_container_tty()` (attach
  mode) or `bollard::exec::resize_exec()` (exec mode).
- `signal()` → docker has no direct container signal API in bollard's
  stable surface. The POC used `kill_container` for the attach case.
  For exec mode, there is no per-exec signal; the signal is delivered
  to the container's PID namespace via `kill_container` with the
  container's main PID, or the exec's PID if exposed. This is a
  best-effort mapping per the `TtyControl::signal` contract
  ("best-effort delivery to the foreground process group"); unknown
  signals fall back to `kill_container` with `SIGKILL`. The exact
  signal-delivery path (bollard's `kill_container` vs a future exec-PID
  signal API) is a two-way-door implementation detail within the
  one-way backend placement.

### 4. `resource_id()` returns `Some(("container", container_id))`

`DockerTtyBackend::resource_id(&params)` extracts the container ID
from `backend_params` and returns `Some(("container", id))`, per the
`tty-backend.md` sketch. The `TtyAdapter` calls
`OwnershipProvider::owns(identity, "container", id, "tty")` at
negotiation (ADR-050, ADR-060). The backend owns the extraction; the
adapter doesn't parse docker-specific JSON. This is the same shape
the `tty-backend.md` sketch anticipates.

### 5. `exit_code` future's Drop kills the container/exec (ADR-056)

The `TtyHandle.exit_code` future's `Drop`-on-cancel (ADR-056) issues:

- **Attach mode** — `bollard::kill_container()` (or the container's
  natural exit if the stream end already triggered). The container is
  not removed (attach doesn't own the container's lifecycle); it's
  killed so the process doesn't outlive the session.
- **Exec mode** — the exec instance is not a separate killable
  process in docker's model; the signal goes to the container. The
  `Drop` issues `kill_container` with the container's main PID as a
  best-effort. A container running an exec that the terminal session
  cancels should have the exec's process terminated; docker's exec
  isolation means this is best-effort, not guaranteed.

This satisfies ADR-056's contract: dropping `exit_code` (cancel) kills
the session target. The "session target" for docker is the container
(attach) or the exec's process (exec, best-effort via the container).

## Consequences

**Positive:**

- The `DockerTtyBackend` lives where its deps live (bollard in
  alknet-docker), matching the ADR-053 inversion principle. No
  `alknet-tty-docker` sibling crate, no cycle risk.
- The `tty` feature gate keeps the `alknet-tty` edge opt-in. A
  call-operations-only deployment (the common case) doesn't pull in
  alknet-tty.
- The POC's `drive_attach_raw` maps cleanly to
  `DockerTtyBackend::allocate()` — the backend produces handles, the
  `TtyAdapter` pumps the wire format. The POC's three-pump driver
  (`session.rs`) is the reference; the backend extracts the bollard
  interaction from it.
- The `resource_id()` delegation means the adapter's ownership check
  (ADR-060) works without the adapter parsing docker JSON — the
  backend extracts the container ID, the adapter checks the store.

**Negative:**

- alknet-docker gains an `alknet-tty` dependency edge (behind the
  `tty` feature). This is the intended trade: the backend lives with
  its transport dep, and the trait edge is feature-gated. A deployment
  that wants interactive docker terminals accepts this; one that
  doesn't is unaffected.
- Signal delivery in docker exec mode is best-effort (docker's exec
  isolation limits per-exec signal targeting). The `TtyControl::signal`
  contract is already best-effort (ADR-053), so this is within the
  contract, but a user expecting `Ctrl-C` to kill only the exec
  process (not the container) may see the container killed instead.
  This is a docker-semantics limitation, not an alknet limitation;
  the local backend (portable_pty) has precise process-group
  targeting (REQ-TTY-02). The docker backend's signal semantics are
  documented in the spec, not hidden.
- The `attach` vs `exec` mode split in `DockerBackendParams` is a
  backend-specific detail the caller must know. The `TtyParams` is
  opaque to the adapter (ADR-053), but the caller (the client opening
  the tty session) must set `mode: "attach"` or `mode: "exec"` in the
  negotiation frame's backend params. This is the same opacity as the
  local backend's `cwd`/`env` — the caller knows their backend. No
  adapter change is needed for a new mode; the backend parses its own
  params.

## Door type

**One-way (crate placement) + two-way (feature gate, attach/exec mode
split, signal path).** The decision to put `DockerTtyBackend` in
alknet-docker (not a sibling crate) is one-way: the backend type, its
registration with the `TtyAdapter`, and its `bollard`/`alknet-tty`
edges are structural. Reversing to a sibling crate would move the type
and re-edge the dependencies.

The feature gate (`tty` opt-in), the attach/exec mode split in
`DockerBackendParams`, and the exact signal-delivery path
(`kill_container` vs a future exec-PID API) are two-way-door
implementation details within the one-way placement. The feature can
be renamed, the mode field can gain a third value, and the signal path
can be refined — all without a structural change.

## References

- [ADR-053](053-ttybackend-trait-and-ttyhandle.md) — the
  `TtyBackend` trait this backend implements; the inversion principle
  (backends live where their transport deps live)
- [ADR-052](052-alknet-tty-wire-format-and-two-carriage.md) — the wire
  format the `TtyAdapter` pumps (the backend produces handles, not wire)
- [ADR-054](054-local-tty-backend-sibling-crate.md) — the sibling-crate
  pattern for `LocalTtyBackend` (the parallel this ADR follows, with
  the difference that docker's deps are already in alknet-docker)
- [ADR-056](056-backend-cleanup-on-session-cancel.md) — the
  cancel-cleanup contract (`exit_code` future's `Drop` kills)
- [ADR-050](050-dynamic-resource-ownership-for-runtime-spawned-resources.md)
  — the ownership model `resource_id()` delegates to
- [ADR-058](058-alknet-docker-on-alknet-call.md) — why the
  non-interactive exec is a call op, not this backend
- [ADR-059](059-bollard-021-dependency-and-features.md) — the bollard
  dependency (no `websocket` feature — the reliable attach path)
- `docs/research/alknet-docker/poc-summary.md` §"POC Target 1"
  (interactive attach — the seed of this backend)
- `/workspace/alknet-docker-poc/src/ops.rs` — `drive_attach_raw` (the
  reference for `allocate()`)
- [tty-backend.md](../crates/tty/tty-backend.md) §"Backend
  implementations" (the row this ADR fills: "DockerTtyBackend | alknet-docker
  or sibling adapter | future, out of scope here")
- Spec: [docker-tty-backend.md](../crates/docker/docker-tty-backend.md)