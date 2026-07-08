# ADR-058: alknet-docker Registers on `alknet/call` (No Separate ALPN)

## Status

Accepted

## Context

The alknet-docker POC (`docs/research/alknet-docker/poc-summary.md`)
left two integration questions open (its "Open Unknowns" #1 and #2):

1. **Raw-carriage handoff in the dispatcher.** The POC's `drive_attach_raw`
   reads the `call.requested` frame itself, then switches the bidi stream
   to raw chunks for interactive attach. The POC noted this doesn't fit
   the call dispatcher's `handle_stream` → `dispatch()` →
   `DispatchResult::Stream(ResponseStream)` path, which pumps a stream
   of `EventEnvelope`s (JSON carriage). The POC offered two options: (a)
   branch `handle_stream` on a `carriage` field, handing raw streams to a
   `RawHandler`; or (b) a separate ALPN (`alknet/docker-raw`) that owns
   the whole stream.

2. **ALPN layout.** Should docker ops register on the shared
   `alknet/call` ALPN (operations in the shared `OperationRegistry`) or
   get their own `alknet/docker` ALPN (as a `ProtocolHandler`)? The POC
   leaned shared but didn't decide.

Both questions were open **before** alknet-tty was extracted. The
alknet-tty crate spec (ADR-052, ADR-053) resolves both by moving the one
operation that needed raw carriage — interactive attach/exec with a PTY
— out of the call protocol entirely and into its own ALPN
(`alknet/tty`), backed by the `TtyBackend` trait.

### Why the raw-carriage problem dissolved

The POC's raw-carriage handoff was hard because it tried to do two
different things on one `alknet/call` stream:

- **Structured operations** (lifecycle, logs, inspect) — naturally
  JSON-shaped, fit `call.responded`/`call.completed` exactly. This is
  what the call protocol is for.
- **Interactive attach** — a bidirectional byte pump (stdin/stdout/stderr)
  with a control sideband (resize, signal, exit). This is *not* a
  request/response or subscription; it's a terminal session. Forcing it
  through `EventEnvelope` framing is "wasteful and lossy" (POC §"Why not
  JSON for everything?").

alknet-tty was created precisely to extract the second category into its
own protocol (`alknet/tty`) with its own wire format (ADR-052). The
docker POC's `drive_attach_raw` became the seed of alknet-tty's
`DockerTtyBackend` (ADR-061). What remains in alknet-docker's call
operations is the first category — structured operations that map cleanly
to `call.requested`/`call.responded`/`call.completed`.

The POC's concern ("the dispatcher would need a `carriage` field and a
`RawHandler` branch") is no longer operative: there is no raw-carriage
operation on `alknet/call`. The raw byte pump moved to `alknet/tty`; the
`DockerTtyBackend` implements `TtyBackend` and is reached through the
`TtyAdapter`, not the `CallAdapter`. See ADR-061 for the backend
placement.

### Why shared `alknet/call`, not a separate `alknet/docker` ALPN

The remaining docker operations (lifecycle, logs, exec-with-exit-code,
inspect, list, images) are ordinary call-protocol operations. They have:

- A structured input (JSON), a structured output (JSON), or a stream of
  structured events (logs, image pull progress, exec output).
- Natural `OperationSpec`s with input/output JSON Schemas.
- `AccessControl` declarations against the ADR-050 container-as-resource
  model (`resource_type: "container"`, `resource_id_path: "$.containerId"`).
- Service discovery through `services/list` / `services/schema`.

Putting them on a separate `alknet/docker` ALPN would mean a separate
`ProtocolHandler` that re-implements framing, dispatch, ACL, and service
discovery — or a thin wrapper that delegates to the call protocol's
machinery. Either way, it's a parallel dispatch surface for no benefit.
The shared registry is more composable: docker ops are callable from any
call client, including peer routing (`PeerRef` / `from_call` re-export),
which is the primary use case — a coordinator on the hub composing
`docker/container/exec` on a worker spoke.

A separate ALPN is warranted when the protocol's wire format is
incompatible with `EventEnvelope` framing (alknet-tty, alknet-ssh). The
docker operations remaining on `alknet/call` are all
`EventEnvelope`-shaped. The one that wasn't (raw attach) moved to
alknet-tty. See ADR-052 §"Why not JSON for everything?" for the boundary
criterion.

### Logs and exec streaming — still JSON carriage, still `alknet/call`

Two operations look like they might need raw carriage but don't:

- **`docker/container/logs`** — a `Subscription` operation. bollard's
  `logs()` returns a `Stream<LogOutput>`; each `LogOutput` becomes a
  `call.responded` carrying `{ "stream": "stdout"|"stderr", "text": "..." }`;
  stream end → `call.completed`. This is the `StreamingHandler` shape
  (ADR-049). The POC validated this path (`docker_logs_subscription_pumps_frames_and_completes`).
  No raw carriage needed — each log line is naturally JSON-shaped.
- **`docker/container/exec`** (non-interactive, no TTY) — a `Subscription`
  operation. bollard's `start_exec` returns `StartExecResults::Attached {
  output, input }`; the output stream is pumped as `call.responded`
  frames; after the stream ends, `inspect_exec()` gives the exit code,
  which rides on a final `call.responded` with `{ "exitCode": N,
  "terminal": true }` before `call.completed`. The POC validated this
  (`docker_exec_streams_output_and_exit_code`). This is the exec path
  for `tty: false` — a captured command with separate stdout/stderr and
  an exit code, not an interactive terminal.

The interactive exec path (`tty: true`, bidirectional, resize/signal) is
**not** a call operation — it's a `DockerTtyBackend` session on
`alknet/tty`. ADR-061 covers that. The split is: non-interactive exec is
a call `Subscription`; interactive exec is a tty session. The `tty:
bool` field in the operation input selects the path; the caller picks.

## Decision

### 1. alknet-docker registers its operations on the shared `alknet/call` ALPN

alknet-docker does not register a `ProtocolHandler`. It constructs an
`OperationRegistry` (or a `DockerOps` registration bundle consumed by
the assembly layer's `OperationRegistryBuilder`) and the existing
`CallAdapter` dispatches docker operations through the shared
`OperationRegistry::invoke()` / `invoke_streaming()` paths. There is no
`alknet/docker` ALPN, no `DockerProtocolHandler`, and no parallel
dispatch surface.

This makes docker operations first-class citizens of the call protocol:
they appear in `services/list`, they have `OperationSpec`s with JSON
Schemas, they go through the standard `AccessControl::check`, and they
compose with `from_call` re-export and peer routing (ADR-029) like any
other operation.

### 2. There is no raw-carriage operation on `alknet/call`

The `carriage` field the POC proposed for `call.requested` is not added.
The call protocol's `EventEnvelope` framing is the only carriage on
`alknet/call`. Interactive attach/exec (the one operation that needed
raw carriage) is reached through `alknet/tty` via `DockerTtyBackend`
(ADR-061), not through a call operation. The POC's open question #1
(raw-carriage handoff in the dispatcher) is resolved by removing the
requirement, not by adding a branch.

### 3. The operation taxonomy

| Operation | Call op type | bollard method | Carriage |
|-----------|-------------|---------------|----------|
| `docker/container/list` | Query | `list_containers` | JSON |
| `docker/container/inspect` | Query | `inspect_container` | JSON |
| `docker/container/create` | Mutation | `create_container` | JSON |
| `docker/container/start` | Mutation | `start_container` | JSON |
| `docker/container/stop` | Mutation | `stop_container` | JSON |
| `docker/container/remove` | Mutation | `remove_container` | JSON |
| `docker/container/restart` | Mutation | `restart_container` | JSON |
| `docker/container/logs` | Subscription | `logs` | JSON (StreamingHandler) |
| `docker/container/exec` (tty:false) | Subscription | `create_exec` + `start_exec` + `inspect_exec` | JSON (StreamingHandler) |
| `docker/image/list` | Query | `list_images` | JSON |
| `docker/image/pull` | Subscription | `create_image` | JSON (StreamingHandler) |
| `docker/image/inspect` | Query | `inspect_image` | JSON |

Interactive exec (`tty: true`) and interactive attach are **not** in this
table — they are `alknet/tty` sessions via `DockerTtyBackend`
(ADR-061). A `docker/container/exec` call operation with `tty: true` in
the input returns an `call.error` with code `INVALID_INPUT` directing
the caller to use `alknet/tty`; the operation only accepts `tty: false`
(or absent). This keeps the one-operation-per-carriage invariant clean:
the call operation is captured output; the tty session is interactive.

The full operation surface (including which are in scope for v1 and
which are deferred) is in
[docker-operations.md](../crates/docker/docker-operations.md).

### 4. The assembly layer composes the registry

alknet-docker exports a `DockerOps` construct (a set of
`HandlerRegistration` bundles keyed by operation name) or a
`register_docker_ops(&mut builder, docker_client, ownership, labels)`
function. The assembly layer (the CLI binary or a hub binary) calls this
to add docker operations to the shared `OperationRegistryBuilder`
alongside other operations (services discovery, agent ops, etc.). The
`Docker` bollard client, the `OwnershipStore`, and the label namespace
config are injected by the assembly layer. See
[overview.md](../crates/docker/overview.md) §"Assembly Layer Wiring".

## Consequences

**Positive:**

- Docker operations inherit all call-protocol machinery for free:
  service discovery, JSON Schema validation, `AccessControl`,
  `from_call` re-export, peer routing, `forwarded_for` metadata, abort
  cascade. No parallel dispatch surface.
- The POC's hardest open question (raw-carriage handoff) is resolved by
  removal — the operation that needed it moved to alknet-tty. No
  dispatcher change, no `carriage` field, no `RawHandler` trait.
- The operation taxonomy is clean: `Query`/`Mutation` for lifecycle,
  `Subscription` (ADR-049 `StreamingHandler`) for logs/exec/pull. The
  exit-code-on-final-`call.responded` pattern the POC validated (POC
  target 3) works through the existing `StreamingHandler` → `pump_stream`
  path with no protocol change.
- A hub that wants to manage docker on worker spokes composes
  `docker/container/*` through `from_call` (ADR-017) + peer routing
  (ADR-029) — the proxy pattern from ADR-050. This is the primary use
  case and it works by construction.

**Negative:**

- The `docker/container/exec` operation has a `tty` field whose value
  constrains the dispatch path: `tty: false` → call `Subscription`;
  `tty: true` → reject, point to `alknet/tty`. This is a minor impedance
  — a caller wanting interactive exec must use a different protocol
  (`alknet/tty`) than a caller wanting captured exec. This is the
  correct split (interactive terminal ≠ captured command), but it means
  the "exec" concept spans two protocols. The `DockerTtyBackend`
  (ADR-061) and the `docker/container/exec` call operation share the
  underlying `bollard::exec` API but diverge at the wire format. This is
  the same divergence as "SSH exec" (call op) vs "SSH PTY session"
  (alknet-tty) — the boundary is principled, not accidental.
- A deployment that wants docker operations must wire the
  `OperationRegistry` (the assembly layer's job). This is not a downside
  — it's the same wiring every other operation set requires — but it
  means alknet-docker is not a "drop-in `ProtocolHandler`" the way
  alknet-tty is. The trade is composability (shared registry, peer
  routing) for a slightly more involved assembly step.

## Door type

**One-way.** The decision to register on `alknet/call` rather than a
separate `alknet/docker` ALPN is a structural commitment: the operation
names (`docker/container/*`), the `OperationSpec`s, the
`AccessControl` shapes, and the `from_call` re-export surface all depend
on docker ops being call-protocol operations. Reversing this — moving
docker ops to their own ALPN — would be a rewrite of the operation
surface and a break for any client that calls `docker/container/*`
through the call protocol.

The decision *not* to add a `carriage` field to `call.requested` is
also one-way: the call protocol's wire format (ADR-012) stays
`EventEnvelope`-only on `alknet/call`. If a future operation needs raw
carriage on `alknet/call`, it would require a wire-format change (new
ALPN per ADR-006, or a `call.requested` field addition). The expected
path for raw-carriage operations is a separate ALPN (as alknet-tty did),
not a `call.requested` extension.

## References

- `docs/research/alknet-docker/poc-summary.md` §"Open Unknowns" #1 and #2
  (the two questions this ADR resolves)
- [ADR-012](012-call-protocol-stream-model.md) — the call protocol wire
  format this ADR keeps unchanged (no `carriage` field)
- [ADR-017](017-call-protocol-client-and-adapter-contract.md) — `from_call`
  re-export, the proxy pattern's mechanism
- [ADR-024](024-operation-registry-layering.md) — the registry layering
  docker ops register into
- [ADR-029](029-peer-graph-routing-model.md) — peer routing, the
  head→worker docker management path
- [ADR-049](049-streaming-handler-for-subscriptions.md) —
  `StreamingHandler`, the dispatch path for logs/exec/pull
- [ADR-050](050-dynamic-resource-ownership-for-runtime-spawned-resources.md)
  — containers as resources, the `AccessControl` shape docker ops declare
- [ADR-052](052-alknet-tty-wire-format-and-two-carriage.md) — alknet-tty's
  wire format, where the raw-carriage operation moved
- [ADR-053](053-ttybackend-trait-and-ttyhandle.md) — `TtyBackend`, the
  trait `DockerTtyBackend` implements (ADR-061)
- [ADR-061](061-docker-tty-backend-in-alknet-docker.md) —
  `DockerTtyBackend` placement in alknet-docker
- Spec documents: [overview.md](../crates/docker/overview.md),
  [docker-operations.md](../crates/docker/docker-operations.md)