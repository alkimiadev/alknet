---
status: draft
last_updated: 2026-07-08
---

# alknet-docker — Operations

The operation surface alknet-docker registers on the shared
`alknet/call` `OperationRegistry`: lifecycle operations (Query/
Mutation), streaming operations (Subscription via `StreamingHandler`),
access control (ADR-050/060 application), the label namespace, and
teardown coupling. This document specifies what an implementer builds
against. The ALPN decision (shared `alknet/call`, no separate
`alknet/docker`) is in [ADR-058](../../decisions/058-alknet-docker-on-alknet-call.md).

## What

alknet-docker exports a `register_docker_ops()` function (or a
`DockerOps` registration bundle) that the assembly layer calls to add
docker operations to the shared `OperationRegistryBuilder`. Each
operation is a `HandlerRegistration` with:

- An `OperationSpec` (name, namespace, op_type, schemas,
  `access_control`, `resource_id_path` per ADR-050).
- A `HandlerKind` (`Once` for Query/Mutation, `Stream` for
  Subscription — ADR-049).
- `provenance: Local` (assembly-registered, can compose).
- A `CompositionAuthority` (the scopes the docker ops run under when
  composed — e.g., `container:list`, `container:exec`).
- Empty `Capabilities` (local bollard, no outbound credentials).

The operations are dispatched by the existing `CallAdapter` through
`invoke()` / `invoke_streaming()`. There is no docker-specific
dispatch code in alknet-call.

## Why

The POC (`docs/research/alknet-docker/poc-summary.md`) validated the
hard parts (interactive attach, logs subscription, exec with exit
code) and noted the remaining lifecycle operations are "mechanical
bollard wrapping, no feasibility risk... `Query`/`Mutation` operations
with single `call.responded` responses, the boring case" (POC §"What
the POC Does NOT Validate" #4). This document specifies the boring
case + the validated streaming cases as a concrete operation surface.

The operations declare `AccessControl` against the ADR-050
container-as-resource model, applied to bollard's API via ADR-060
(label namespace, `list` filter, hosted-services fallback, teardown
coupling). No new auth model is invented; the existing model is
applied.

## Architecture

### Operation Surface (v1 scope)

| Operation | Type | bollard method | `resource_id_path` | Notes |
|-----------|------|---------------|-------------------|-------|
| `docker/container/list` | Query | `list_containers` | (none — list case) | `owned_only` input flag (ADR-060 §2) |
| `docker/container/inspect` | Query | `inspect_container` | `$.containerId` | |
| `docker/container/create` | Mutation | `create_container` | (none — creates the resource) | Records ownership (ADR-060 §5) |
| `docker/container/start` | Mutation | `start_container` | `$.containerId` | |
| `docker/container/stop` | Mutation | `stop_container` | `$.containerId` | |
| `docker/container/remove` | Mutation | `remove_container` | `$.containerId` | Revokes ownership (ADR-060 §4) |
| `docker/container/restart` | Mutation | `restart_container` | `$.containerId` | |
| `docker/container/logs` | Subscription | `logs` | `$.containerId` | StreamingHandler; `follow` input flag |
| `docker/container/exec` | Subscription | `create_exec` + `start_exec` + `inspect_exec` | `$.containerId` | StreamingHandler; `tty:false` only; exit code on final `call.responded` |
| `docker/image/list` | Query | `list_images` | (none) | |
| `docker/image/pull` | Subscription | `create_image` | (none) | StreamingHandler; progress events |
| `docker/image/inspect` | Query | `inspect_image` | (none) | |

**Out of scope for v1 (deferred):**

- Network operations (`docker/network/*`) — OQ-048.
- Volume operations (`docker/volume/*`) — OQ-048.
- Image build (`docker/image/build` with buildkit) — OQ-049.
- System events (`docker/system/events`) — OQ-050.
- Full `CreateContainerOptions` surface (mounts, port bindings,
  networks) — OQ-051. v1 `create` accepts the image, command, env,
  labels, and name; the full options surface is a v1 implementation
  refinement.
- Interactive exec / attach (`tty: true`) — not a call operation;
  `DockerTtyBackend` on `alknet/tty` (ADR-061).

### Lifecycle Operations (Query / Mutation)

The lifecycle operations are the "boring case": a single bollard
async method call, a single `call.responded` with the JSON result
(or `call.error` on failure). The handler is a `Handler` (not
`StreamingHandler`), registered as `HandlerKind::Once`.

The `start`, `stop`, and `restart` operations share the `inspect`
shape (a `Mutation` with `resource_id_path: "$.containerId"`,
`container:<action>` scope, a single bollard method call, a single
`call.responded`). The representative example below is `inspect`;
the others differ only in the bollard method and the declared
scope.

```rust
// docker/container/inspect — representative lifecycle op.
// The handler closure captures the bollard Docker client by
// Arc::clone at registration time (ADR-062); it does not read the
// client from OperationContext.
let docker_clone = docker.clone();  // Arc<bollard::Docker>
let handler: Handler = Arc::new(move |input, ctx| {
    let docker = docker_clone.clone();
    Box::pin(async move {
        let container_id = input["containerId"].as_str()
            .ok_or_else(|| /* INVALID_INPUT */)?;
        match docker.inspect_container(container_id, None::<()>).await {
            Ok(info) => ResponseEnvelope::ok(to_json_value(info)),
            Err(bollard::Error::DockerResponseServerError { status_code: 404, .. }) =>
                ResponseEnvelope::error(CallError {
                    code: "CONTAINER_NOT_FOUND".into(),
                    message: format!("container {} not found", container_id),
                    retryable: false, details: None,
                }),
            Err(e) => ResponseEnvelope::error(CallError {
                code: "DOCKER_ERROR".into(),
                message: e.to_string(),
                retryable: false, details: None,
            }),
        }
    })
});
```

The handler extracts the container ID from the input, calls the
bollard method, and maps the result to a `ResponseEnvelope`. The
bollard error is mapped to a declared domain error code
(`CONTAINER_NOT_FOUND`, `DOCKER_ERROR`) per ADR-023. The
`OperationSpec.error_schemas` declare these codes so clients get
typed error enums.

### Handler Injection (ADR-062)

The `Docker` client and `OwnershipStore` are **closure-captured** at
registration time, not read from `OperationContext` or smuggled
through `Capabilities`. This matches the established `from_openapi`
pattern (ADR-017): non-secret shared state via closure capture,
secret material via `Capabilities`. A bollard `Docker` handle to a
local unix socket is not secret material — it must not go through
`Capabilities` (ADR-014's contract: `Capabilities` is for outbound
secret material only). The `register_docker_ops` function takes
`Arc<Docker>` + `Arc<dyn OwnershipStore>` + the label config + the
`CompositionAuthority`, and constructs each handler closure with the
handles it needs captured by `Arc::clone`. The `Capabilities` passed
to each registration is empty (`Capabilities::new()`) — local bollard
needs no API key. See [ADR-062](../../decisions/062-docker-client-injection-via-closure-capture.md)
for the full decision and the rationale for why `Capabilities` and
`OperationContext` extension are the wrong channels.

### `docker/container/create` — ownership recording

`create` is the spawn event (ADR-060 §5). The handler:

1. Parses the input (image, command, env, labels, name, and v1-limited
   `CreateContainerOptions` — OQ-051 for the full surface).
2. Injects the alknet labels: `alknet.managed=true`,
   `alknet.owner=<caller_peer_id>`. The caller's peer ID comes from
   `ctx.identity` (`Identity.id`, ADR-030). For a composed call (the
   coordinator composing `create`), this is the coordinator's
   identity, not the end user's — the proxy pattern (ADR-050 §3).
3. Calls `bollard::create_container()`.
4. On success, calls `OwnershipStore::record(identity, "container",
   container_id)` (ADR-050 §1, ADR-060 §5).
5. Returns the `ContainerCreateResponse` (with the container ID) as
   `call.responded`.

The label injection is the handler's job, not bollard's — bollard's
`CreateContainerOptions.labels` accepts a map; the handler merges the
alknet labels with any caller-provided labels (caller labels win on
conflict for non-`alknet.*` keys; `alknet.*` keys are reserved and
overwritten by the handler to prevent a caller spoofing ownership).

### `docker/container/remove` — ownership revocation

`remove` is the teardown event (ADR-060 §4). The handler:

1. Calls `bollard::remove_container()`.
2. On success, calls `OwnershipStore::revoke("container", container_id)`.
3. Returns `call.responded` with an empty/ok result.

Autonomous container death (a `--rm` exit, external `docker rm`,
daemon restart) is tolerated — stale ownership entries are not
promptly cleaned up (no reaper); they're inert (a reused container ID
gets a fresh `record` on its next `create`). See ADR-060 §4.

### `docker/container/list` — scope-gate + optional result-filter

`list` is the ADR-050 #4a "list case": `resource_type: "container"`,
no `resource_id_path`. The input accepts an `owned_only: bool` flag
(default `false`):

- `owned_only: false` — `bollard::list_containers()` with no label
  filter; returns all containers the daemon sees. The scope check
  gates the call (caller needs `container:list`). This is the
  hosted-services case (operator lists all).
- `owned_only: true` — `bollard::list_containers()` with a label
  filter (`label: alknet.owner=<caller_peer_id>`); returns only the
  caller's containers. This is the disposable-dev-container case
  (coordinator lists its own).

The result-filter is bollard-side (the label filter is in the
`ListContainersOptions`), not handler-side — bollard filters at the
daemon. The handler doesn't call `OwnershipProvider::owned_resources`
and filter in Rust; it pushes the filter to bollard via the label.
This is more efficient (the daemon filters before sending) and
correct (the label and the ownership store agree for alknet-spawned
containers; the cross-check is ADR-060 §1's secondary signal).

### Streaming Operations (Subscription)

The streaming operations (logs, exec, image pull) are `Subscription`
operations using `StreamingHandler` (ADR-049). The handler returns a
`Stream<ResponseEnvelope>`; the dispatcher's `pump_stream` writes
each `Ok(value)` as `call.responded`, an `Err` as `call.error`
(terminal), and natural stream end as `call.completed`.

#### `docker/container/logs`

Maps `bollard::container::logs()` (returns
`Stream<Item = Result<LogOutput, Error>>`) to a stream of
`call.responded` frames. The POC validated this path (POC target 2:
`docker_logs_subscription_pumps_frames_and_completes`).

```rust
// The StreamingHandler closure captures the bollard Docker client
// by Arc::clone at registration time (ADR-062). ResponseStream is
// the type alias Pin<Box<dyn Stream<Item = ResponseEnvelope> + Send>>
// (ADR-049, operation-registry.md §"Handler").
let docker_clone = docker.clone();
let handler: StreamingHandler = Arc::new(move |input, ctx| {
    let docker = docker_clone.clone();
    Box::pin(async_stream::stream! {
        let container_id = input["containerId"].as_str().unwrap();
        let follow = input["follow"].as_bool().unwrap_or(false);
        let options = LogsOptionsBuilder::default()
            .follow(follow)
            .stdout(true).stderr(true)
            .timestamps(true)  // ADR-059 §4: time feature
            .build();
        let mut stream = docker.logs(container_id, Some(options));
        while let Some(result) = stream.next().await {
            match result {
                Ok(LogOutput::StdOut { message }) |
                Ok(LogOutput::Console { message }) => yield ResponseEnvelope::ok(json!({
                    "stream": "stdout",
                    "timestamp": /* from LogOutput */,
                    "text": /* message bytes as UTF-8 */
                })),
                Ok(LogOutput::StdErr { message }) => yield ResponseEnvelope::ok(json!({
                    "stream": "stderr",
                    "timestamp": /* ... */,
                    "text": /* ... */
                })),
                Ok(_) => yield ResponseEnvelope::ok(json!({})),  // other variants
                Err(e) => yield ResponseEnvelope::error(CallError {
                    code: "DOCKER_ERROR".into(),
                    message: e.to_string(),
                    retryable: false, details: None,
                }),
            }
        }
        // stream end → call.completed (the dispatcher's pump_stream
        // writes call.completed on natural stream end — ADR-049)
    }) as ResponseStream
});
```

Each `LogOutput` becomes a `call.responded` with `stream`
(stdout/stderr), `timestamp` (from the `time` feature, ADR-059 §4),
and `text` (the log line, as UTF-8 — the POC's single-`text`-field
refinement separates timestamp and text). Stream end (the container
exits for `follow: true`, or historical logs drain for `follow: false`)
→ `call.completed` (the dispatcher's `pump_stream` handles this on
natural stream end).

#### `docker/container/exec` (non-interactive, `tty: false`)

Maps `bollard::create_exec` + `start_exec` + `inspect_exec` to a
stream of `call.responded` frames, with the exit code on a final
`call.responded` before `call.completed`. The POC validated this
path (POC target 3: `docker_exec_streams_output_and_exit_code`).

The `tty` field in the input MUST be `false` (or absent). If `tty:
true`, the handler returns a single `ResponseEnvelope::error` with
code `INVALID_INPUT` and a message directing the caller to
`alknet/tty` for interactive exec (ADR-058 §3). This is the
one-operation-per-carriage invariant: the call operation is captured
output; the tty session is interactive.

The handler:
1. `create_exec(container_id, CreateExecOptions { cmd, env, tty: false, ... })`
   → exec ID.
2. `start_exec(exec_id, None)` → `StartExecResults::Attached { output, input }`.
3. Pump `output` (a `Stream<LogOutput>`) as `call.responded` frames
   (stdout/stderr separated, since `tty: false`).
4. After the output stream ends, `inspect_exec(exec_id)` →
   `ExecInspectResponse { exit_code, ... }`.
5. Emit a final `call.responded` with `{ "exitCode": N, "terminal": true }`.
6. Stream end → `call.completed` (the exit-code `call.responded` is
   the last one before `completed`).

The exit code rides on a `call.responded`, not on `call.completed`.
The `terminal: true` flag marks this `call.responded` as the final
value before `call.completed`. The full shape decision (why
`terminal: true` on `call.responded` and not an exit code on
`call.completed` or a `call.exit` event) is in
[ADR-063](../../decisions/063-exit-code-on-terminal-call-responded.md).

#### `docker/image/pull`

Maps `bollard::image::create_image()` (returns
`Stream<Item = Result<CreateImageInfo, Error>>`) to a stream of
`call.responded` frames with progress events. Same shape as logs:
each progress event → `call.responded`, stream end → `call.completed`.
No exit code (image pull has no exit code; success is stream end
without error).

### Access Control

The operations declare `AccessControl` against the ADR-050 model,
applied via ADR-060. Three patterns:

#### Specific-container operations (`exec`, `inspect`, `start`, `stop`, `remove`, `restart`, `logs`)

```rust
OperationSpec {
    name: "docker/container/exec",
    access_control: AccessControl {
        required_scopes: vec!["container:exec".into()],
        resource_type: Some("container".into()),
        resource_action: Some("exec".into()),
        ..
    },
    resource_id_path: Some("$.containerId".into()),
    ..
}
```

The dispatcher extracts `containerId` from the input via
`resource_id_path` and passes it to `AccessControl::check`, which
consults the `OwnershipProvider` (ADR-050 §2). The check passes if:
- the caller owns the container (the ownership store says so — the
  `create` path recorded it), OR
- the caller's static `Identity.resources["container"]` includes an
  action that subsumes the required action (the operator-role
  fallback — ADR-060 §3, e.g., `container:manage` ⊇ `container:exec`).

#### The `list` case (`container/list`)

```rust
OperationSpec {
    name: "docker/container/list",
    access_control: AccessControl {
        required_scopes: vec!["container:list".into()],
        resource_type: Some("container".into()),
        // no resource_action — the list case
        ..
    },
    // no resource_id_path — the list case (ADR-050 #4a)
    ..
}
```

The scope check gates the call. The `owned_only` input flag selects
the result-filter (label-based, bollard-side). See ADR-060 §2.

#### The `create` case (`container/create`)

```rust
OperationSpec {
    name: "docker/container/create",
    access_control: AccessControl {
        required_scopes: vec!["container:create".into()],
        // no resource_type — create spawns the resource; the
        // ownership is recorded after the bollard call succeeds.
        // The scope check gates; the ownership is a side effect.
        ..
    },
    // no resource_id_path — the container doesn't exist yet
    ..
}
```

`create` has no `resource_type` (the resource doesn't exist yet);
the scope check gates. The ownership recording is a handler side
effect (ADR-060 §5), not an ACL check.

#### Image operations (`image/list`, `image/pull`, `image/inspect`)

```rust
OperationSpec {
    name: "docker/image/pull",
    access_control: AccessControl {
        required_scopes: vec!["image:pull".into()],
        // no resource_type — images are not runtime-spawned resources
        // in the ADR-050 sense (they're pulled, not spawned with
        // per-caller ownership). Scope-gate only.
        ..
    },
    ..
}
```

Images are not runtime-spawned resources with per-caller ownership
— they're shared daemon state. The scope check gates; no ownership
provider consultation.

### Error Schemas (ADR-023)

Each operation declares its domain error codes in
`OperationSpec.error_schemas`:

| Code | Operations | HTTP status (for to_openapi projection) | Description |
|------|-----------|----------------------------------------|-------------|
| `CONTAINER_NOT_FOUND` | inspect, start, stop, remove, restart, logs, exec | 404 | The container ID doesn't exist on the daemon |
| `IMAGE_NOT_FOUND` | image/inspect | 404 | The image doesn't exist locally |
| `IMAGE_PULL_FAILED` | image/pull | 502 | The pull failed (registry error, network) |
| `DOCKER_ERROR` | all | 500 | A bollard error not covered by a specific code |
| `INVALID_INPUT` | exec (tty:true) | 400 | `tty: true` on `docker/container/exec` — use `alknet/tty` |

The `INVALID_INPUT` for `tty: true` on exec carries a message
directing the caller to `alknet/tty` for interactive exec. This is
the one-operation-per-carriage invariant's user-visible surface
(ADR-058 §3).

### Composition and Peer Routing

Docker operations compose through the standard call-protocol
mechanisms:

- **`from_call` re-export (ADR-017).** A hub that wants to manage
  docker on a worker imports the worker's `docker/container/*`
  operations via `from_call` and re-registers them locally. The hub's
  clients call the re-exported operations; the hub is the direct
  caller to the worker; the worker's ownership store sees the hub as
  the owner (the proxy pattern, ADR-050 §3). The end user's identity
  rides as `forwarded_for` (ADR-032).
- **Peer routing (ADR-029).** A hub with multiple worker connections
  composes `docker/container/exec` via `invoke_peer(PeerRef::Specific(
  worker_peer_id), ...)`. The `PeerCompositeEnv` routes to the named
  worker's sub-overlay. This is the head→worker fan-out primitive.
- **Local composition.** A coordinator handler that spawns a
  container and then execs into it composes `docker/container/create`
  then `docker/container/exec` through `OperationEnv::invoke()`. The
  coordinator's `CompositionAuthority` has `container:create` +
  `container:exec` scopes (static, ADR-022); the ownership check
  passes because `create` recorded the coordinator as the owner.

The `docker/container/exec` with `tty: true` rejection is a
composition boundary: a coordinator composing exec must use `tty:
false` (captured output); interactive exec requires an `alknet/tty`
session, which is a different protocol (the coordinator opens a tty
connection, not a call operation). This is the same boundary as
"SSH exec" (call op) vs "SSH PTY session" (alknet-tty).

## Constraints

- **No `carriage` field on `call.requested`.** The call protocol's
  wire format (ADR-012) is `EventEnvelope`-only on `alknet/call`.
  The one operation that needed raw carriage (interactive attach)
  moved to `alknet/tty` via `DockerTtyBackend` (ADR-058 §2, ADR-061).
  A `docker/container/exec` call with `tty: true` is rejected with
  `INVALID_INPUT`.
- **`resource_id_path` is required for specific-container ops.** The
  dispatcher extracts the container ID from the input via the JSON
  pointer; `AccessControl::check` receives it. Operations without
  `resource_id_path` (`list`, `create`, image ops) don't reference a
  specific container and don't consult the ownership provider for a
  specific resource.
- **`create` records ownership; `remove` revokes; lifecycle ops do
  neither.** Only the spawn (`create`) and the teardown (`remove`)
  touch the ownership store. `start`/`stop`/`restart` act on an
  existing container whose ownership was recorded at create time
  (or which pre-exists and is reached via the static fallback).
- **Stale ownership entries are tolerated.** Autonomous container
  death leaves a stale entry; no reaper cleans it promptly. The
  entry is inert (a reused container ID gets a fresh `record`). A
  `docker/system/events` subscription for prompt cleanup is deferred
  (OQ-050).
- **`owned_only` on `list` is the caller's choice.** Default
  `false` (returns all); `true` filters to the caller's containers.
  The default and the two cases are decided in ADR-060 §2; the scope
  check gates regardless of the flag.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| Docker ops on `alknet/call` | [ADR-058](../../decisions/058-alknet-docker-on-alknet-call.md) | No separate ALPN; raw-carriage dissolved; `tty:true` exec rejected |
| Container resource model + labels | [ADR-060](../../decisions/060-container-resource-model-and-label-namespace.md) | `alknet.managed`/`alknet.owner` labels; `list` `owned_only`; hosted-services fallback; teardown coupling |
| Streaming handler for subscriptions | [ADR-049](../../decisions/049-streaming-handler-for-subscriptions.md) | `StreamingHandler` for logs/exec/pull; exit code on final `call.responded` |
| Dynamic resource ownership | [ADR-050](../../decisions/050-dynamic-resource-ownership-for-runtime-spawned-resources.md) | Containers as `AccessControl` resources; `resource_id_path` |
| Operation error schemas | [ADR-023](../../decisions/023-operation-error-schemas.md) | `CONTAINER_NOT_FOUND`, `IMAGE_NOT_FOUND`, `DOCKER_ERROR`, `INVALID_INPUT` |
| Peer-graph routing | [ADR-029](../../decisions/029-peer-graph-routing-model.md) | Head→worker docker management |
| Forwarded-for identity | [ADR-032](../../decisions/032-forwarded-for-identity.md) | End-user identity as metadata in the proxy pattern |
| bollard 0.21 + features | [ADR-059](../../decisions/059-bollard-021-dependency-and-features.md) | `time` feature for log timestamps |
| Docker client + OwnershipStore injection | [ADR-062](../../decisions/062-docker-client-injection-via-closure-capture.md) | Closure capture at registration (not `Capabilities`, not `OperationContext`); matches `from_openapi` pattern |
| Exit code on terminal `call.responded` | [ADR-063](../../decisions/063-exit-code-on-terminal-call-responded.md) | `{ "exitCode": N, "terminal": true }` on final `call.responded`; `call.completed` stays empty |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-048** (deferred(scope)): Network and volume operation surface.
- **OQ-049** (deferred(scope)): Image build (buildkit) scope.
- **OQ-050** (deferred(scope)): Docker system events subscription.
- **OQ-051** (deferred(scope)): Container create options surface.

## References

- [ADR-058](../../decisions/058-alknet-docker-on-alknet-call.md) — the
  ALPN decision (shared `alknet/call`, no `carriage` field)
- [ADR-060](../../decisions/060-container-resource-model-and-label-namespace.md)
  — the resource model and label namespace this surface applies
- [ADR-049](../../decisions/049-streaming-handler-for-subscriptions.md)
  — `StreamingHandler` for logs/exec/pull
- [ADR-050](../../decisions/050-dynamic-resource-ownership-for-runtime-spawned-resources.md)
  — the ownership model
- [ADR-023](../../decisions/023-operation-error-schemas.md) — declared
  domain error codes
- [ADR-062](../../decisions/062-docker-client-injection-via-closure-capture.md)
  — the Docker client + OwnershipStore injection model (closure
  capture, not `Capabilities`)
- [ADR-063](../../decisions/063-exit-code-on-terminal-call-responded.md)
  — the exec exit-code frame shape (`terminal: true`)
- `docs/research/alknet-docker/poc-summary.md` — the POC (validated
  logs subscription, exec with exit code; lifecycle ops are "the
  boring case")
- `/workspace/alknet-docker-poc/src/ops.rs` — `DockerOps`:
  `drive_logs`, `drive_exec` (the streaming-handler reference)
- `/workspace/bollard/src/container.rs` (`list_containers` :245,
  `create_container` :296, `inspect_container` :777, `logs` :928),
  `/workspace/bollard/src/exec.rs` (`create_exec` :172, `start_exec`
  :225, `inspect_exec` :315), `/workspace/bollard/src/image.rs`
  (`create_image` :120, `list_images` :66, `inspect_image` :177)