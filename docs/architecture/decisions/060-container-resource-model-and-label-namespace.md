# ADR-060: Container Resource Model and Label Namespace

## Status

Accepted

## Context

ADR-050 established the dynamic resource ownership model for
runtime-spawned resources: a `OwnershipProvider` (read, sync) +
`OwnershipStore` (write, async) trait pair in alknet-core, with the
spawner owning the resource, proxy-to-share, teardown-revoke. ADR-050
explicitly named alknet-docker as the first consumer: containers as
`AccessControl` resources, `docker/container/exec` requiring
`resource: container/<id>:exec`, `docker/container/create` calling
`OwnershipStore::record` on success, `docker/container/remove` calling
`OwnershipStore::revoke` on teardown.

ADR-050 resolved the *model*. This ADR resolves the *application* of
that model to alknet-docker's concrete operation surface — the three
things ADR-050 left to the crate spec:

1. **The label namespace.** The dispatch POC (`/workspace/@alkdev/dispatch`)
   used `dispatch.managed=true` to mark containers it created. The POC
   summary §"What the POC Does NOT Validate" #6 noted the real crate
   "needs a configurable label prefix and ownership mapping
   (`alknet.owner=<peer-id>`) tied to the call protocol's identity model."
   What is the label scheme, and how does it relate to the ownership store?

2. **The `list` result-filter.** ADR-050 specific #4a says the `list`
   case (resource_type set, resource_id_path absent) is "scope-gate +
   result-filter": the scope check gates the call, and the handler
   filters the result via `OwnershipProvider::owned_resources`. How
   does `docker/container/list` apply this against bollard's
   `list_containers` (which returns all containers the daemon sees)?

3. **Teardown coupling.** ADR-050 specific #4b says teardown is
   handler-driven: the docker handler calls `revoke` on container
   removal. But docker containers can exit on their own (a process
   that finishes, a `--rm` container). How does the ownership store
   learn about autonomous container death, or does it rely solely on
   explicit `docker/container/remove`?

### The two use cases and their ownership profiles

The user's brief named two container use cases:

- **Disposable dev containers** (the common case by volume) — a
  coordinator spawns a container for an implementation agent or an
  isolated env. The container is short-lived; the coordinator owns it
  for its lifetime; it's removed when the agent is done. Ownership is
  per-session, per-coordinator.
- **Long-running hosted services** (less common but important) — the
  production server (`/workspace/system/dev1`) hosts rarely-changing
  services (reverse-proxy, postgres, redis, gitea) in docker. These
  containers are created by an operator (via `docker compose`), not by
  a call-protocol coordinator. They're managed via alknet-docker's
  operations (start/stop/restart/inspect/logs) but their *ownership* is
  not in the alknet ownership store — they predate the connection.

These two cases have different ownership profiles, and the model must
handle both without forcing the hosted-services case through the
disposable-container path.

## Decision

### 1. Label namespace: `alknet.*` prefix, configurable, two labels

alknet-docker applies two labels to containers it creates:

| Label | Value | Purpose |
|-------|-------|---------|
| `alknet.managed` | `"true"` | Marks the container as alknet-managed. The `list` filter and the ownership-existence check key on this label. |
| `alknet.owner` | `<peer_id>` | The `Identity.id` (stable logical peer id, per ADR-030) of the spawner. For composing handlers, the *handler's* identity (the coordinator's `Identity`), not the end user's — the proxy pattern (ADR-050 §3). |

The label prefix (`alknet`) is **configurable** at assembly-layer wiring.
A deployment that runs alongside another alknet-managed fleet (or wants
to avoid collisions with its own `managed` labels) sets a different
prefix. The default is `alknet`. This is a two-way-door config choice;
the label *schema* (two labels, `<prefix>.managed` + `<prefix>.owner`)
is the one-way commitment.

The labels serve two purposes:

- **Ownership-store cross-check.** When `docker/container/exec` checks
  `OwnershipProvider::owns(identity, "container", id, "exec")`, the
  ownership store is the source of truth. The label is a *secondary*
  signal: if the store says "yes" but the label is absent or names a
  different owner, the container's ownership state is stale (the
  container was removed and its ID reused, or the store and docker
  diverged). The label is not authoritative — the store is — but it's a
  debugging aid and a consistency check.
- **`list` filter.** `docker/container/list` with an `owned_only: true`
  input flag filters to containers whose `alknet.owner` label matches
  the caller's `Identity.id`. This is the result-filter path (ADR-050
  #4a). Without the flag, the list returns all containers the daemon
  sees (the hosted-services case — see §3 below).

**`alknet.*` label reservation.** The `<prefix>.*` label keys are
reserved: the `docker/container/create` handler overwrites
`<prefix>.managed` and `<prefix>.owner` on the container's labels
regardless of what the caller provided, preventing a caller from
spoofing ownership (setting `alknet.owner` to another peer's id). A
caller's own labels (non-`<prefix>.*` keys) are preserved. This is a
security property: ownership is set by the handler (from the
caller's `Identity.id`), not by the caller's input.

**Resource action vocabulary.** The `resource_action` field on
`AccessControl` for containers uses these actions:

| Action | Used by | Scope name |
|--------|---------|-----------|
| `exec` | `docker/container/exec` (call op, non-interactive) | `container:exec` |
| `tty` | `DockerTtyBackend` sessions (`alknet/tty`) | `container:tty` |
| `start` | `docker/container/start` | `container:start` |
| `stop` | `docker/container/stop` | `container:stop` |
| `remove` | `docker/container/remove` | `container:remove` |
| `restart` | `docker/container/restart` | `container:restart` |
| `manage` | (static, operator role) `Identity.resources["container"]` | `container:manage` |
| `list` | `docker/container/list` (scope-gate only, no resource_id) | `container:list` |
| `create` | `docker/container/create` (scope-gate only, no resource_id) | `container:create` |

The `tty` action is distinct from `exec` (ADR-061 §4): a caller
authorized for non-interactive exec (`container:exec`) is not
automatically authorized for an interactive terminal session
(`container:tty`). The `manage` action is the static operator-role
resource (§3) that subsumes the per-container actions for pre-existing
hosted-service containers. The `list` and `create` actions are
scope-gate-only (no `resource_id_path`); the rest target a specific
container via `resource_id_path: "$.containerId"`.

### 2. `docker/container/list`: scope-gate + optional result-filter

`docker/container/list` is a `Query` operation with
`resource_type: "container"`, no `resource_id_path` (the `list` case per
ADR-050 #4a). The input accepts an optional `owned_only: bool` flag
(default `false`):

- **`owned_only: false`** (default) — the handler calls
  `bollard::list_containers()` and returns all containers the daemon
  sees. The scope check gates the call (the caller needs
  `container:list`); there is no result-filter. This is the
  hosted-services case: an operator listing all containers on dev1,
  including ones they didn't spawn through alknet.
- **`owned_only: true`** — the handler calls
  `bollard::list_containers()` with a label filter
  (`label: alknet.owner=<caller_peer_id>`), returning only the
  containers the caller owns. This is the disposable-dev-container
  case: a coordinator listing its own workspaces. The scope check still
  gates the call; the label filter is the result-filter.

The two paths use the same bollard method (`list_containers` with
different `ListContainersOptions`), differing only in the label filter.
The `owned_only` flag is the caller's choice; the scope check is the
server's. This matches ADR-050 #4a's "allow if scoped, filter to owned"
default, with the filter opt-in (the hosted-services case needs the
unfiltered list).

### 3. Hosted services: ownership is not required for pre-existing containers

The hosted-services case (dev1's reverse-proxy, postgres, redis, gitea)
involves containers created by an operator via `docker compose`, not via
`docker/container/create`. These containers have no `alknet.owner`
label and no ownership-store entry. Operations on them (`start`,
`stop`, `inspect`, `logs`, `restart`) must still work — but
`AccessControl::check` with `resource_type: "container"` and a
`resource_id` would consult the ownership store, find no entry, and
deny.

The resolution: **the `AccessControl` for operations on a specific
container (`exec`, `stop`, `remove`, `inspect` with `resource_id_path`)
declares `resource_type: "container"` and `resource_action`, but the
ownership check is against the store, and the store's "no entry" result
falls through to the static `Identity.resources` path (ADR-050 §2
backward-compat).** A peer whose `Identity.resources["container"]`
includes `"manage"` (a deployment-configured scope for the operator
role) passes the check for any container, owned or not. A peer without
that resource but with `container:exec` scope passes only for containers
the ownership store says they own.

Concretely, the two roles:

| Role | Scope | Owns a specific container? | Can exec into container C? |
|------|-------|---------------------------|-----------------------------|
| Operator (manages hosted services) | `container:manage` in `Identity.resources["container"]` | No (no ownership entry) | Yes — the static-resource fallback passes (`manage` ⊇ `exec`) |
| Coordinator (spawns dev containers) | `container:exec` scope | Yes (ownership store: coordinator owns C) | Yes — the ownership check passes |
| Random peer | `container:exec` scope | No | No — ownership check fails, static fallback fails |

The operator role is configured statically (the dev1 operator's
`PeerEntry.resources` includes `container:manage`); the coordinator
role is dynamic (the ownership store records the spawn). Both paths
reach the same `AccessControl::check`; the difference is which branch
satisfies.

This means `docker/container/exec` and friends work on hosted-service
containers *for the operator role* without the operator having to
"claim" ownership of containers they didn't spawn. The
`docker/container/create` operation (the spawner path) always records
ownership; `docker/container/start` on a pre-existing container does
not (it's not a spawn). The model is: **spawn → own; manage (operator) →
static-resource-pass; neither → deny.**

### 4. Teardown coupling: handler-driven revoke + autonomous-death tolerance

ADR-050 #4b says teardown is handler-driven: `docker/container/remove`
calls `OwnershipStore::revoke("container", id)`. But containers can die
autonomously:

- A `--rm` container exits and is auto-removed by the daemon.
- A container crashes and an operator removes it via `docker rm` (not
  through alknet-docker).
- The daemon restarts (all containers stop; container IDs are stale).

alknet-docker's teardown coupling is **handler-driven revoke on the
alknet-managed remove path, with autonomous-death tolerance**:

- **`docker/container/remove` (the alknet operation)** calls
  `bollard::remove_container()`, and on success calls
  `OwnershipStore::revoke("container", id)`. This is the ADR-050 #4b
  contract: the handler that manages the lifecycle revokes.
- **Autonomous container death** (a `--rm` exit, an external `docker rm`,
  a daemon restart) leaves a stale ownership-store entry. The entry is
  not proactively cleaned up — there is no reaper, no event subscription
  to the docker daemon's death events. Instead, the entry is cleaned up
  lazily: the next operation against that container ID fails at the
  bollard layer (the container doesn't exist), the error propagates as
  a `call.error`, and a subsequent `docker/container/list` with
  `owned_only: true` for the stale owner filters the container out
  (it's not in the daemon's list). The ownership store's
  `owned_resources` can be cross-checked against the live daemon list
  on read, or left to be overwritten on the next spawn — the container ID
  is unique per daemon run; a reused ID would be a new container.

The tolerance for stale entries is intentional: ownership is runtime
state (ADR-050 assumption 4), meaningless across restarts. The
in-memory default store loses all entries on restart; a persistence
adapter would cache and invalidate. A reaper that subscribes to docker
daemon death events would be more prompt but adds a subscription
surface for a marginal gain (the stale entry is inert — it can't grant
access to a container that doesn't exist, and a reused container ID
gets a fresh `record` on its next `create`).

A future "prompt stale-entry cleanup" feature is additive: a
`docker/system/events` subscription operation (deferred, OQ-050) that
the ownership store could subscribe to. The base model tolerates stale
entries; the prompt-cleanup path is a refinement.

### 5. `docker/container/create` records ownership; `start`/`stop`/`restart` do not

Only `docker/container/create` calls `OwnershipStore::record`. The
lifecycle operations (`start`, `stop`, `restart`, `remove`) do not
record — they act on an existing container whose ownership was recorded
at create time (or which pre-exists and is reached via the
static-resource operator path). `remove` calls `revoke` (the teardown
half); the others neither record nor revoke.

This matches ADR-050's "spawner owns" model: the *create* is the spawn
event; the lifecycle operations are management of an existing resource.

## Consequences

**Positive:**

- The two use cases (disposable dev containers, hosted services) both
  work through one `AccessControl` model. The operator role reaches
  hosted-service containers via the static-resource fallback; the
  coordinator role reaches spawned containers via the ownership store.
  No special-casing, no "is this a managed container?" branch in the
  handlers.
- The label namespace is configurable and minimal (two labels). The
  `managed` flag marks alknet-spawned containers; the `owner` label
  carries the peer id for the `list` filter and the cross-check.
- Teardown is handler-driven on the alknet path and tolerant of
  autonomous death on the non-alknet path. No reaper subscription
  surface in the base model.
- ADR-050's model is applied without amendment. The `list` case
  (specific #4a), the teardown coupling (#4b), and the
  backward-compat static-resource fallback (#2) all map cleanly to
  bollard's API (`list_containers` with a label filter, `remove_container`
  + `revoke`, the static `Identity.resources` path).

**Negative:**

- Stale ownership entries from autonomous container death are not
  promptly cleaned up. An `owned_only: true` list could theoretically
  return a container ID that no longer exists — but bollard's
  `list_containers` returns *live* containers, so the list is always
  accurate against the daemon; the stale entry only affects the
  ownership store's internal map, not the list result. The cross-check
  (label vs store) could flag divergence; this is a debugging aid, not
  a correctness issue.
- The operator role requires static configuration
  (`Identity.resources["container"] ⊇ "manage"`). A deployment that
  wants the operator to manage hosted services must configure the
  operator's `PeerEntry.resources`. This is not a downside — it's the
  same static configuration every role requires — but it means the
  hosted-services case isn't "zero-config"; the operator peer must be
  declared.
- The `owned_only` flag on `list` is an opt-in. A coordinator that
  forgets to set it gets all containers, not just its own. The default
  (`false`) is correct for the hosted-services case (operator lists
  all), but a coordinator expecting isolation must set the flag. This
  is a caller-side convention, enforced by the scope check (a
  non-operator peer without `container:manage` can't call `list` at all
  — the scope gates it), but within the `container:list` scope, the
  filter is the caller's choice.

## Door type

**One-way (label schema, ownership model application) + two-way (label
prefix, stale-entry policy).** The label schema (two labels,
`<prefix>.managed` + `<prefix>.owner`) and the
create-records/remove-revokes ownership coupling are one-way: clients
and the ownership store depend on the labels and the record/revoke
timing. The label prefix (default `alknet`) is two-way-door config. The
stale-entry policy (tolerate, no reaper) is two-way — a reaper
subscription is an additive refinement.

## References

- [ADR-050](050-dynamic-resource-ownership-for-runtime-spawned-resources.md)
  — the model this ADR applies (specifics #4a, #4b, #2)
- `docs/research/alknet-docker/poc-summary.md` §"What the POC Does NOT
  Validate" #6 (label namespace / ownership mapping)
- `/workspace/@alkdev/dispatch/src/docker.rs` — the dispatch POC's
  `dispatch.managed=true` label (prior art this ADR generalizes)
- [ADR-030](030-peerentry-and-identity-id-decoupling.md) — `Identity.id`
  as the stable peer id used in the `alknet.owner` label
- [ADR-032](032-forwarded-for-identity.md) — `forwarded_for` for the
  proxy pattern's end-user identity
- [ADR-015](015-privilege-model-and-authority-context.md) — the static
  `Identity.resources` path (the operator-role fallback)
- `/workspace/system/dev1/docker.md` — the hosted-services use case
  (reverse-proxy, postgres, redis, gitea on dev1)
- `/workspace/@alkdev/reverse-proxy/deploy/docker-compose.yml` — the
  reverse-proxy's docker setup (operator-created, not alknet-spawned)
- Spec: [docker-operations.md](../crates/docker/docker-operations.md)
  §"Access Control" and §"Label Namespace"