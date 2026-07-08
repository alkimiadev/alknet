---
status: draft
last_updated: 2026-07-08
---

# alknet-docker

The docker operations crate for the ALPN-as-service architecture: a
thin, single-host bollard wrapper that exposes docker container and
image operations as call-protocol operations on the shared
`alknet/call` ALPN, plus (behind a `tty` feature) a `DockerTtyBackend`
implementing `alknet-tty`'s `TtyBackend` trait for interactive terminal
sessions into containers.

## What

alknet-docker does two things:

1. **Call operations.** A set of `OperationSpec`-registered operations
   (`docker/container/*`, `docker/image/*`) on the shared
   `alknet/call` ALPN, mapping bollard's docker API to the call
   protocol's `Query` / `Mutation` / `Subscription` dispatch paths.
   Lifecycle (create/start/stop/remove/list/inspect) is `Query`/
   `Mutation`; logs, non-interactive exec, and image pull are
   `Subscription` (streaming via `StreamingHandler`, ADR-049). The
   operations declare `AccessControl` against the ADR-050 container-
   as-resource model. Decided in
   [ADR-058](../../decisions/058-alknet-docker-on-alknet-call.md).

2. **`DockerTtyBackend`** (behind the `tty` feature). An
   `impl TtyBackend` (ADR-053) wrapping `bollard::attach_container()`
   and `bollard::exec::start_exec` with `tty: true`, for interactive
   terminal sessions into containers over `alknet/tty`. This is the
   `TtyBackend` row the alknet-tty spec left open ("future, out of
   scope here"). Decided in
   [ADR-061](../../decisions/061-docker-tty-backend-in-alknet-docker.md).

The two use cases the crate serves (per the user's brief and the system
docs):

- **Disposable dev containers** (the common case by volume) — a
  coordinator spawns a container for an implementation agent or an
  isolated env. alknet-docker's `create` records ownership
  (`OwnershipStore::record`); the coordinator owns the container for
  its lifetime; `remove` revokes. Interactive sessions into the
  container go through `DockerTtyBackend` on `alknet/tty`.
- **Long-running hosted services** (the production server case) —
  rarely-changing services (reverse-proxy, postgres, redis, gitea on
  dev1, per `/workspace/system/dev1/docker.md`) created by an operator
  via `docker compose`, not via alknet. alknet-docker's operations
  (start/stop/inspect/logs) manage them; the operator role reaches them
  via the static-resource fallback (ADR-060 §3).

## Documents

| Document | Status | Description |
|----------|--------|-------------|
| [overview.md](overview.md) | draft | Crate purpose, two-role design (call ops + DockerTtyBackend), dependencies, ALPN, label namespace, feature gates, assembly-layer wiring |
| [docker-operations.md](docker-operations.md) | draft | The operation surface: lifecycle (Query/Mutation), logs/exec/pull (Subscription via StreamingHandler), access control, label namespace, teardown coupling |
| [docker-tty-backend.md](docker-tty-backend.md) | draft | `DockerTtyBackend` (impl `TtyBackend`): attach vs exec mode, `TtyHandle` field mapping, `TtyControl` → bollard resize/signal, `exit_code` Drop-kill (ADR-056) |

## Applicable ADRs

| ADR | Title | Relevance |
|-----|-------|-----------|
| [058](../../decisions/058-alknet-docker-on-alknet-call.md) | alknet-docker Registers on `alknet/call` | Docker ops are call-protocol operations, not a separate ALPN; raw-carriage dissolved by alknet-tty extraction |
| [059](../../decisions/059-bollard-021-dependency-and-features.md) | bollard 0.21 Dependency and Feature Selection | Version pin (0.21, verified current); features `http`+`pipe`+`time`, no `ssl`/`ssh`/`websocket`/`buildkit` |
| [060](../../decisions/060-container-resource-model-and-label-namespace.md) | Container Resource Model and Label Namespace | ADR-050 application: `alknet.managed`/`alknet.owner` labels; `list` owned_only flag; hosted-services static-resource fallback; handler-driven revoke + autonomous-death tolerance |
| [061](../../decisions/061-docker-tty-backend-in-alknet-docker.md) | DockerTtyBackend in alknet-docker | Backend in alknet-docker behind `tty` feature; attach/exec mode split; POC `drive_attach_raw` as reference |
| [003](../../decisions/003-crate-decomposition.md) | Crate Decomposition | alknet-docker depends on alknet-core + alknet-call (ops) and alknet-tty (tty feature); no handler-depends-on-handler violation |
| [012](../../decisions/012-call-protocol-stream-model.md) | Call Protocol Stream Model | The wire format docker ops use (`EventEnvelope`, no `carriage` field) |
| [017](../../decisions/017-call-protocol-client-and-adapter-contract.md) | Call Protocol Client and Adapter Contract | `from_call` re-export — the proxy pattern's mechanism for hub→worker docker management |
| [023](../../decisions/023-operation-error-schemas.md) | Operation Error Schemas | Docker ops declare domain error codes (CONTAINER_NOT_FOUND, IMAGE_NOT_FOUND, etc.) |
| [024](../../decisions/024-operation-registry-layering.md) | Operation Registry Layering | Docker ops register in the curated Layer 0 at startup |
| [029](../../decisions/029-peer-graph-routing-model.md) | Peer-Graph Routing Model | Head→worker docker management via `PeerRef` / `invoke_peer` |
| [032](../../decisions/032-forwarded-for-identity.md) | Forwarded-For Identity | End-user identity as metadata when a coordinator proxies docker ops |
| [049](../../decisions/049-streaming-handler-for-subscriptions.md) | Streaming Handler for Subscriptions | `StreamingHandler` for logs/exec/pull; exit code on final `call.responded` |
| [050](../../decisions/050-dynamic-resource-ownership-for-runtime-spawned-resources.md) | Dynamic Resource Ownership | Containers as `AccessControl` resources; the model ADR-060 applies |
| [052](../../decisions/052-alknet-tty-wire-format-and-two-carriage.md) | alknet-tty Wire Format | The `alknet/tty` wire format `DockerTtyBackend` sessions use |
| [053](../../decisions/053-ttybackend-trait-and-ttyhandle.md) | TtyBackend Trait and TtyHandle | The trait `DockerTtyBackend` implements |
| [055](../../decisions/055-exit-code-on-control-chunk.md) | Exit Code on a Control Chunk | The exit-chunk ordering `DockerTtyBackend`'s `exit_code` feeds into |
| [056](../../decisions/056-backend-cleanup-on-session-cancel.md) | Backend Cleanup on Session Cancel | `DockerTtyBackend`'s `exit_code` future `Drop` kills the container/exec |
| [014](../../decisions/014-secret-material-flow-and-capability-injection.md) | Secret Material Flow and Capability Injection | `Capabilities` is for secret material only — the `Docker` handle is not secret (ADR-062) |
| [062](../../decisions/062-docker-client-injection-via-closure-capture.md) | Docker Client and OwnershipStore Injection via Closure Capture | Closure capture at registration; `Capabilities` for secrets only; matches `from_openapi` pattern |
| [063](../../decisions/063-exit-code-on-terminal-call-responded.md) | Exit Code on a Terminal `call.responded` for Non-Interactive Exec | `{ "exitCode": N, "terminal": true }` on final `call.responded`; `call.completed` stays empty |

## Relevant Open Questions

| OQ | Title | Status | Relevance |
|----|-------|--------|-----------|
| OQ-048 | Network and volume operation surface | deferred(scope) | Network/volume CRUD deferred; v1 is containers + images |
| OQ-049 | Image build (buildkit) scope | deferred(scope) | `buildkit` feature deferred; v1 has `image/pull` + `image/list` + `image/inspect` |
| OQ-050 | Docker system events subscription | deferred(scope) | `docker/system/events` subscription for stale-ownership cleanup deferred |
| OQ-051 | Container create options surface | deferred(scope) | Full `CreateContainerOptions` (mounts, port bindings, networks) surface deferred to v1 implementation |

## Key Design Principles

1. **Single-host, bollard-specific.** alknet-docker talks to one local
   docker daemon over `/var/run/docker.sock`. The fleet case (multiple
   daemons on different machines) is a call-protocol concern — a
   `CallClient` per remote daemon, each running alknet-docker locally
   — not a bollard-feature concern. No `ssl`/`ssh` features, no remote
   daemon over TLS. See [overview.md](overview.md) and
   [ADR-058](../../decisions/058-alknet-docker-on-alknet-call.md).

2. **Call operations on `alknet/call`, not a separate ALPN.** Docker
   ops are ordinary call-protocol operations with `OperationSpec`s,
   `AccessControl`, and service discovery. They inherit `from_call`
   re-export and peer routing. The one operation that didn't fit
   (interactive attach) moved to `alknet/tty` via `DockerTtyBackend`.
   No `carriage` field on `call.requested`, no parallel dispatch
   surface. See [ADR-058](../../decisions/058-alknet-docker-on-alknet-call.md).

3. **Containers are runtime-spawned resources (ADR-050).** `create`
   records ownership; `remove` revokes; `exec`/`stop`/`inspect` check
   ownership via `OperationSpec.resource_id_path`. The
   hosted-services case (operator-managed, pre-existing containers)
   works through the static-resource fallback. Two labels
   (`alknet.managed`, `alknet.owner`) mark alknet-spawned containers
   for the `list` filter and the cross-check. See
   [docker-operations.md](docker-operations.md) and
   [ADR-060](../../decisions/060-container-resource-model-and-label-namespace.md).

4. **Interactive terminal is a tty concern, not a docker concern.**
   `DockerTtyBackend` implements `alknet-tty`'s `TtyBackend` trait,
   behind a `tty` feature. The wire format, the session lifecycle, and
   the exit-chunk ordering live in alknet-tty; the backend produces
   bollard-backed handles. This is the same inversion as
   `LocalTtyBackend` (local process) and `SshTtyBackend` (SSH). See
   [docker-tty-backend.md](docker-tty-backend.md) and
   [ADR-061](../../decisions/061-docker-tty-backend-in-alknet-docker.md).

5. **bollard 0.21, verified current.** The POC used a local 0.21
   checkout; the crate depends on published 0.21 from crates.io
   (verified as latest). Features: `http` + `pipe` (default, local
   daemon) + `time` (log timestamps). No `websocket` (reliable attach
   only), no `ssl`/`ssh` (fleet is call-protocol), no `buildkit`
   (deferred). See [ADR-059](../../decisions/059-bollard-021-dependency-and-features.md).

6. **Mechanical mapping, no feasibility risk.** The POC validated the
   hard parts (raw carriage attach, logs subscription, exec with exit
   code). The remaining lifecycle operations (create/start/stop/remove/
   list/inspect) are mechanical bollard wrapping — `Query`/`Mutation`
   with single `call.responded` responses, "the boring case" (POC §"What
   the POC Does NOT Validate" #4). See
   [docker-operations.md](docker-operations.md).

## References

- `docs/research/alknet-docker/poc-summary.md` — the POC that validated
  the hard parts (interactive attach, logs subscription, exec with exit
  code) and surfaced the open unknowns this spec set resolves
- `/workspace/alknet-docker-poc/` — the POC source (`src/ops.rs`
  `DockerOps`, `src/raw.rs` chunk codec, `src/frame.rs` EventEnvelope
  mirror, `tests/integration.rs` 6 tests against a live daemon)
- `/workspace/bollard/` — bollard 0.21.0 source (the local checkout the
  POC used; identical API surface to the published 0.21)
- `/workspace/@alkdev/dispatch/` — the dispatch POC (bollard 0.18,
  `dispatch.managed=true` labels, SSH-tunnel fleet model — the prior
  art this crate generalizes and the friction it removes)
- `/workspace/system/dev1/docker.md` — the production hosted-services
  use case (reverse-proxy, postgres, redis, gitea on dev1)
- `/workspace/@alkdev/reverse-proxy/deploy/docker-compose.yml` — the
  reverse-proxy's docker setup (operator-created, not alknet-spawned)
- `docs/architecture/crates/tty/` — the alknet-tty spec
  (`DockerTtyBackend` is the row `tty-backend.md` left open)