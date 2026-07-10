---
status: draft
last_updated: 2026-07-08
---

# alknet-docker — Overview

The docker operations crate: a thin, single-host bollard wrapper that
exposes docker container and image operations as call-protocol
operations on `alknet/call`, plus (behind a `tty` feature) a
`DockerTtyBackend` implementing `alknet-tty`'s `TtyBackend` trait. This
document covers the crate's purpose, the two-role design, its
dependency edges, the ALPN decision, the label namespace, the feature
gates, and the assembly-layer wiring. Component details are in the
sibling documents.

## What

alknet-docker is the docker integration point for the ALPN-as-service
architecture. It does two things:

1. **Registers docker operations on the shared `alknet/call`
   `OperationRegistry`.** The operations — `docker/container/*` and
   `docker/image/*` — are ordinary call-protocol operations with
   `OperationSpec`s, `AccessControl`, and service discovery. The
   existing `CallAdapter` dispatches them through `invoke()` /
   `invoke_streaming()`. There is no `alknet/docker` ALPN and no
   `DockerProtocolHandler`. Decided in
   [ADR-058](../../decisions/058-alknet-docker-on-alknet-call.md).

2. **Provides `DockerTtyBackend`** (behind the `tty` feature), an
   `impl TtyBackend` (ADR-053) for interactive terminal sessions into
   containers over `alknet/tty`. This is the backend the alknet-tty
   spec listed as "future, out of scope here"
   ([tty-backend.md](../tty/tty-backend.md) §"Backend implementations").
   Decided in [ADR-061](../../decisions/061-docker-tty-backend-in-alknet-docker.md).

The two roles share the bollard `Docker` client, the container
identity model, and the label/ownership reasoning (ADR-060), but
diverge at the wire format: call operations use `EventEnvelope` on
`alknet/call`; interactive sessions use the raw chunk codec on
`alknet/tty` (ADR-052).

## Why

The overwhelming common use case (per the user's brief) is a hub that
runs as a call-protocol client, connects to worker nodes, and exposes
docker operations on those workers. alknet-docker is what runs on the
worker: it wraps the local docker daemon and registers the operations.
The hub composes them via `from_call` (ADR-017) + peer routing
(ADR-029) — the proxy pattern (ADR-050 §3). For interactive terminals
into the containers (an agent needing a shell), the hub opens an
`alknet/tty` session that the worker's `DockerTtyBackend` serves.

The two container use cases:

- **Disposable dev containers** (common by volume) — a coordinator
  spawns a container for an implementation agent or an isolated env.
  `docker/container/create` records ownership; the coordinator owns
  the container; `docker/container/remove` revokes. Interactive
  sessions go through `DockerTtyBackend`. The container is
  short-lived; "burn it and start over" is the recovery model
  (ADR-050 §4b).

- **Long-running hosted services** (less common, important) — the
  production server (`/workspace/system/dev1`) hosts rarely-changing
  services (reverse-proxy, postgres, redis, gitea) in docker. These
  are created by an operator via `docker compose`, not via alknet.
  alknet-docker's operations manage them (start/stop/inspect/logs);
  the operator role reaches them via the static-resource fallback
  (ADR-060 §3) — no ownership-store entry needed for pre-existing
  containers an operator is statically authorized to manage.

Both use cases work through one `AccessControl` model (ADR-050) and
one operation surface. No special-casing, no "is this a managed
container?" branch.

## The Two Roles in Brief

### Role 1: Call operations on `alknet/call`

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
| `docker/system/events` | Subscription | `events` | JSON (StreamingHandler) |

Interactive exec (`tty: true`) and interactive attach are **not** call
operations — they are `alknet/tty` sessions via `DockerTtyBackend`.
A `docker/container/exec` call with `tty: true` in the input returns
`INVALID_INPUT` directing the caller to `alknet/tty`. The full surface,
access control, and the streaming shapes are in
[docker-operations.md](docker-operations.md).

### Role 2: `DockerTtyBackend` on `alknet/tty`

```rust
// behind the `tty` feature
pub struct DockerTtyBackend {
    docker: Docker,
    label_prefix: String,  // "alknet" by default; configurable
}

#[async_trait]
impl TtyBackend for DockerTtyBackend {
    async fn allocate(&self, params: &TtyParams) -> Result<TtyHandle, TtyError>;
    fn resource_id(&self, params: &TtyParams) -> Option<(&'static str, String)>;
}
```

The backend wraps `bollard::attach_container()` (attach mode) or
`bollard::exec::start_exec` with `tty: true` (exec mode), producing a
`TtyHandle` the `TtyAdapter` pumps. The wire format, the session
lifecycle, and the exit-chunk ordering live in alknet-tty; the backend
produces bollard-backed handles. See
[docker-tty-backend.md](docker-tty-backend.md).

## Dependencies

```
alknet-docker
├── alknet-core    (Identity, AccessControl, OwnershipProvider/Store — ADR-050)
├── alknet-call     (OperationSpec, Handler, StreamingHandler, OperationRegistryBuilder)
├── bollard 0.21    (http + pipe + time features; the docker API — ADR-059)
├── alknet-tty      (TtyBackend trait — only behind `tty` feature; ADR-061)
├── tokio           (async runtime — bollard is tokio-based)
├── serde / serde_json (operation input/output, label values)
└── thiserror       (DockerError enum)
```

The `alknet-call` edge is the protocol-foundation exception
(ADR-003 Am. 1): alknet-docker consumes `OperationSpec`, `Handler`,
`StreamingHandler`, and `OperationRegistryBuilder` from alknet-call
to register its operations. This is the same edge `alknet-http` has
(HTTP uses the call protocol types). alknet-docker does **not** depend
on `alknet-http`, `alknet-tty-local`, or any other handler crate — the
no-handler-depends-on-another-handler rule (ADR-003) is preserved.

The `alknet-tty` edge is **only** behind the `tty` feature. A
deployment using alknet-docker for call operations only (the common
case) does not pull in alknet-tty. See Feature Gates below.

### bollard version and features

bollard 0.21, verified as the latest published version on crates.io
(as of 2026-07-08). The POC's local checkout (0.21.0) matches the
published version — identical API surface. Features: `http` + `pipe`
(default, local daemon connect) + `time` (log timestamp typing). No
`ssl` (no remote daemon over TLS — fleet is call-protocol), no `ssh`
(the SSH-tunnel fleet model is what alknet replaces), no `websocket`
(reliable attach only, per the POC), no `buildkit` (deferred, OQ-049).
See [ADR-059](../../decisions/059-bollard-021-dependency-and-features.md).

## ALPN

| ALPN | Role | Handler | Transport |
|------|------|---------|-----------|
| `alknet/call` | Call operations | `CallAdapter` (shared, not docker-specific) | QUIC bidi stream, `EventEnvelope` framing |
| `alknet/tty` | Interactive terminal sessions | `TtyAdapter` (shared, not docker-specific) | QUIC bidi stream, raw chunk codec (ADR-052) |

alknet-docker registers **no ALPN of its own**. The call operations
are dispatched by the shared `CallAdapter` on `alknet/call`; the
interactive sessions are dispatched by the shared `TtyAdapter` on
`alknet/tty`, using `DockerTtyBackend` as one of potentially several
registered backends. This is the ADR-058 decision: docker ops are
call-protocol operations, and the one operation that needed raw
carriage moved to alknet-tty.

## Label Namespace

alknet-docker applies two labels to containers it creates (ADR-060):

| Label | Value | Purpose |
|-------|-------|---------|
| `alknet.managed` | `"true"` | Marks the container as alknet-managed; the `list` owned_only filter and the ownership cross-check key on this. |
| `alknet.owner` | `<peer_id>` | The `Identity.id` of the spawner (the coordinator's identity, not the end user's — proxy pattern). |

The prefix (`alknet`) is configurable at assembly-layer wiring
(two-way-door). The label schema (two labels,
`<prefix>.managed` + `<prefix>.owner`) is the one-way commitment.
Containers created by `docker compose` or `docker run` (the
hosted-services case) have no alknet labels and no ownership-store
entry; they're reached via the static-resource operator fallback.

Full details: [docker-operations.md](docker-operations.md) §"Label
Namespace" and
[ADR-060](../../decisions/060-container-resource-model-and-label-namespace.md).

## Feature Gates

```toml
# alknet-docker Cargo.toml
[features]
default = ["ops"]
ops = []                  # docker/container/* and docker/image/* call operations
tty = ["dep:alknet-tty"]  # DockerTtyBackend (impl TtyBackend)
```

- `default = ["ops"]` — the call operations. A hub or coordinator
  wiring docker management over the call protocol uses the default
  features. Pulls in bollard + alknet-call + alknet-core; no alknet-tty.
- `tty` — adds `DockerTtyBackend`, pulling in `alknet-tty` for the
  `TtyBackend` trait. A deployment that wants interactive terminal
  sessions into containers enables this and registers
  `DockerTtyBackend` with the `TtyAdapter`. A call-operations-only
  deployment leaves this off and doesn't pull in alknet-tty.

The `tty` feature mirrors `alknet-tty`'s `local` feature pattern
(ADR-054): the heavy backend edge is opt-in, so a deployment that
doesn't want it doesn't pay for it. See
[ADR-061](../../decisions/061-docker-tty-backend-in-alknet-docker.md).

## Assembly Layer Wiring

The assembly layer (the CLI binary or a hub/worker binary) constructs
the bollard client, the ownership store, and the label config, then
registers the docker operations and (optionally) the tty backend:

```rust
// 1. Construct the bollard client (local daemon, ADR-059 §2)
let docker = bollard::Docker::connect_with_local_defaults()?;

// 2. Construct the ownership store (in-memory default, ADR-050 §1)
let ownership_store = Arc::new(InMemoryOwnershipStore::new());

// 3. Register docker call operations on the shared registry
let mut builder = OperationRegistryBuilder::new();
// ... other operations (services/list, agent ops, etc.) ...
register_docker_ops(
    &mut builder,
    docker.clone(),
    ownership_store.clone(),
    &DockerLabels { prefix: "alknet".into() },
    CompositionAuthority::new("docker-ops", ["container:list", "container:exec", /* ... */]),
);

// 4. (optional, behind `tty` feature) Register DockerTtyBackend
#[cfg(feature = "tty")]
{
    let docker_backend = Arc::new(DockerTtyBackend::new(
        docker.clone(),
        "alknet".into(),
    )) as Arc<dyn TtyBackend>;
    // Insert into the TtyAdapter's backend map under "docker"
    backends.insert("docker".into(), docker_backend);
}

// 5. Build the registry and start the endpoint
let registry = builder.build();
let call_adapter = CallAdapter::new(Arc::new(registry), identity_provider);
// ... register handlers on the endpoint, start ...
```

The `register_docker_ops` function (or a `DockerOps` struct the
builder consumes) adds each `docker/container/*` and `docker/image/*`
operation as a `HandlerRegistration` with the right `HandlerKind`
(`Once` for Query/Mutation, `Stream` for Subscription — ADR-049) and
`AccessControl` (per ADR-060). The assembly layer provides the
`CompositionAuthority` for the docker-ops handler (the scopes the
docker ops run under when composed) and the `Capabilities` (empty —
`Capabilities::new()` for local bollard; no outbound credentials
needed). The `Docker` client and `OwnershipStore` are
**closure-captured** into each handler at registration time (ADR-062)
— not read from `OperationContext` and not put in `Capabilities`
(`Capabilities` is for secret material only, per ADR-014).

The `OwnershipStore` is shared between the docker ops (which `record`
on create and `revoke` on remove) and the `AccessControl::check` path
(which consults the `OwnershipProvider` read side). The in-memory
default is sufficient for the single-host case; a persistence adapter
(ADR-035 shape) is built when a hub wants fleet ownership to survive
restarts.

## Architecture (component pointers)

- **[docker-operations.md](docker-operations.md)** — the operation
  surface: lifecycle (Query/Mutation), logs/exec/pull (Subscription
  via `StreamingHandler`), access control (ADR-050/050 application),
  label namespace, teardown coupling, the exit-code-on-final-
  `call.responded` pattern for exec.
- **[docker-tty-backend.md](docker-tty-backend.md)** — the
  `DockerTtyBackend`: attach vs exec mode, `TtyHandle` field mapping
  (PTY mode merges stderr), `TtyControl` → bollard resize/signal,
  `exit_code` future `Drop`-kill (ADR-056), `resource_id()` delegation.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| Docker ops on `alknet/call` (no separate ALPN) | [ADR-058](../../decisions/058-alknet-docker-on-alknet-call.md) | Call-protocol operations; raw-carriage dissolved by alknet-tty; no `carriage` field |
| bollard 0.21 + feature selection | [ADR-059](../../decisions/059-bollard-021-dependency-and-features.md) | Version verified current; `http`+`pipe`+`time`; no `ssl`/`ssh`/`websocket`/`buildkit` |
| Container resource model + label namespace | [ADR-060](../../decisions/060-container-resource-model-and-label-namespace.md) | ADR-050 application; `alknet.managed`/`alknet.owner` labels; `list` owned_only; hosted-services static fallback |
| DockerTtyBackend in alknet-docker | [ADR-061](../../decisions/061-docker-tty-backend-in-alknet-docker.md) | Behind `tty` feature; attach/exec mode; POC `drive_attach_raw` as reference |
| Crate decomposition | [ADR-003](../../decisions/003-crate-decomposition.md) Am. 1 | alknet-docker depends on alknet-call (protocol-foundation exception) + alknet-tty (tty feature) |
| Call protocol stream model | [ADR-012](../../decisions/012-call-protocol-stream-model.md) | `EventEnvelope` framing, no `carriage` extension |
| Streaming handler for subscriptions | [ADR-049](../../decisions/049-streaming-handler-for-subscriptions.md) | `StreamingHandler` for logs/exec/pull; exit code on final `call.responded` |
| Dynamic resource ownership | [ADR-050](../../decisions/050-dynamic-resource-ownership-for-runtime-spawned-resources.md) | Containers as `AccessControl` resources; the model ADR-060 applies |
| Peer-graph routing | [ADR-029](../../decisions/029-peer-graph-routing-model.md) | Head→worker docker management via `PeerRef` / `invoke_peer` |
| Forwarded-for identity | [ADR-032](../../decisions/032-forwarded-for-identity.md) | End-user identity as metadata when proxying |
| TtyBackend trait | [ADR-053](../../decisions/053-ttybackend-trait-and-ttyhandle.md) | The trait `DockerTtyBackend` implements |
| Exit code on control chunk | [ADR-055](../../decisions/055-exit-code-on-control-chunk.md) | The exit-chunk ordering `DockerTtyBackend`'s `exit_code` feeds into |
| Backend cleanup on cancel | [ADR-056](../../decisions/056-backend-cleanup-on-session-cancel.md) | `DockerTtyBackend`'s `exit_code` `Drop` kills the container/exec |
| Docker client + OwnershipStore injection | [ADR-062](../../decisions/062-docker-client-injection-via-closure-capture.md) | Closure capture at registration; `Capabilities` for secrets only |
| Exit code on terminal `call.responded` | [ADR-063](../../decisions/063-exit-code-on-terminal-call-responded.md) | `{ "exitCode": N, "terminal": true }` on final `call.responded` for non-interactive exec |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-048** (deferred(scope)): Network and volume operation surface.
- **OQ-049** (deferred(scope)): Image build (buildkit) scope.
- **OQ-051** (deferred(scope)): Container create options surface.

## References

- `docs/research/alknet-docker/poc-summary.md` — the POC summary this
  spec set builds on (validated targets, open unknowns)
- `/workspace/alknet-docker-poc/` — the POC source
- `/workspace/bollard/` — bollard 0.21.0 source (verified current)
- `/workspace/@alkdev/dispatch/` — the dispatch POC (prior art:
  `dispatch.managed` labels, SSH-tunnel fleet model)
- `/workspace/system/dev1/docker.md` — the hosted-services use case
- `/workspace/@alkdev/reverse-proxy/deploy/docker-compose.yml` —
  operator-created container example