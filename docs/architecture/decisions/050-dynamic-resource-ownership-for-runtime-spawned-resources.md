# ADR-050: Dynamic Resource Ownership for Runtime-Spawned Resources

## Status

Accepted (resolves OQ-42; amends the `AccessControl::check` signature and
adds `OperationSpec.resource_id_path`. Does not amend ADR-015 or ADR-022 —
specific #4 confirms the composition authority stays static. Blocks lifted:
the alknet-docker, alknet-tty, opencode-runner wrapper, and
`alknet-container` (fleet normalization) crate specs can declare their
`AccessControl` shapes against this model.)

## Context

The alknet-docker POC (`docs/research/alknet-docker/poc-summary.md`)
surfaced a class of resource that the existing auth model doesn't handle:
**runtime-spawned resources with derived ownership**. A coordinator starts
a container and exposes docker operations over the call protocol; the
question "is this peer allowed to `docker/container/exec` against container
C?" is not answerable by the static `Identity.resources` model —
`Identity.resources` is config-sourced (from `PeerEntry` on the fingerprint
path, from `CompositionAuthority` on the composition path) and set at
registration or connection time. The container didn't exist then. Its
ownership was derived at spawn time: whoever started it owns it.

This generalizes beyond docker. Every "spawn a thing at runtime and expose
it over the call protocol" crate has the same shape:

- **alknet-docker** — containers as `AccessControl` resources.
- **alknet-tty** — terminal sessions as resources (who owns this TTY?).
- **opencode-runner wrapper** — workspace containers / processes as
  resources.
- **`alknet-container`** (fleet normalization) — the fleet layer that
  normalizes container operations across hosts.

None of those crate specs can declare their `AccessControl` shapes until
the core model answers: **how does `AccessControl::check` learn whether
identity X owns runtime-spawned resource R?**

OQ-42 tracked this. The OQ resolved five sub-questions:

1. Storage shape — reuse the repo/adapter pattern (ADR-033).
2. Integration point — `AccessControl::check` consults an ownership
   provider directly (Option 2), with `OperationSpec` gaining a
   `resource_id_path` JSON pointer.
3. Access pattern — proxy-only (spawner owns, proxy to share, teardown
   revokes; no grant mechanism in core).
4. Four edge specifics — the `list` case, teardown coupling, fleet
   representation, composition interaction.

This ADR writes those decisions into ADR format.

### Why the static model breaks

The current `AccessControl::check` is a pure function of `(ACL,
Identity)`:

```rust
// operation-registry.md (current)
pub struct AccessControl {
    pub required_scopes: Vec<String>,
    pub required_scopes_any: Option<Vec<String>>,
    pub resource_type: Option<String>,          // e.g., "service"
    pub resource_action: Option<String>,        // e.g., "read"
}

impl AccessControl {
    pub fn check(&self, identity: Option<&Identity>) -> bool {
        // scope check: identity.scopes ⊇ required_scopes
        // resource check: identity.resources[resource_type] ∋ resource_action
    }
}
```

`Identity.resources` is a `HashMap<String, Vec<String>>` — named resource
lists, populated from `PeerEntry.resources` (fingerprint path, ADR-030) or
`CompositionAuthority.resources` (composition path, ADR-022). Both are
**static**: `PeerEntry` is config-sourced; `CompositionAuthority` is set
at registration. Neither grows at runtime.

For a static resource set ("alice can access services `gitea` and
`registry`"), this works: the config lists the resources, the identity
carries them, `check` matches them. For a runtime-spawned resource
("alice can exec into container `C` which didn't exist when the config was
written"), there's nowhere to record that alice owns C. The identity was
resolved at connection time, before C was spawned. The ownership exists —
the coordinator that started C knows alice asked for it — but the auth
model has no path from that knowledge to `check`.

### The options considered

Three integration points were considered (see OQ-42 for the full
reasoning):

- **Option 1 — augment `Identity.resources` with a per-request snapshot.**
  The dispatcher would pull owned resources into a per-request identity
  snapshot before calling `check`, so `check` *looks* unchanged while
  reading state that was never part of the static identity. **Rejected:**
  the purity was always theatrical (the question "can X exec into C" was
  never purely a function of identity; it just looked that way because the
  resource set was static). Option 1 hides the impurity in a snapshot
  pretending to be static identity.

- **Option 2 — `check` consults the ownership provider directly.**
  `AccessControl::check` grows a parameter (or reads one from
  `OperationContext`) for the ownership provider, and consults it for
  `resource_type`/`resource_action` checks against runtime-spawned
  resources. **Accepted.** This makes `check`'s signature honest about
  what ACL checking *is* in the presence of dynamic resources: a function
  of (ACL, Identity, current-ownership-state). The impurity is real either
  way; Option 2 puts it in the signature where it's visible.

- **Option 3 — handler-level ownership check, `AccessControl` gates only
  scope.** Some resources statically checked, some handler-checked.
  **Rejected:** it splits the ACL story — the kind of inconsistency that
  creates the "figure out how it fits with what is there" cleanup this ADR
  exists to prevent.

### The two access patterns

Walking through the concrete use cases (the agent-workspace case in
particular) surfaced two patterns for how a downstream consumer reaches a
runtime-spawned resource:

- **Proxy pattern (the common case).** A coordinator starts a container
  and manages its lifecycle; the end user never talks to docker directly.
  The coordinator re-exports the docker operations it wants to expose (via
  `from_call` — the adapter that imports a peer's operations and
  re-registers them locally, ADR-017 — or by composing them in its own
  handlers), and when the end user invokes one, the coordinator is the
  *direct caller* to the docker endpoint. Docker's ownership store sees
  the coordinator as the owner and as the caller — the check passes. The
  end user's identity rides as `forwarded_for` metadata (ADR-032), and the
  coordinator does whatever end-user-level ACL it wants at its own layer.
  This is the kernel/user-land + forwarded-for model: the hub's authority
  is used, `forwarded_for` is metadata, the hub handles its own ACL.

- **Grant pattern ("poking holes").** A downstream app wants to give an
  end user *direct* call-protocol access to the docker endpoint for
  specific containers — the end user calls `docker/container/exec`
  themselves, not through a proxy. Docker's ownership store would need a
  record that the end user has access to that container, even though the
  downstream app spawned it.

The agent-workspace case — the concrete one — is entirely the proxy
pattern. The coordinator starts the workspace container; the agent
interacts with what's *inside* the container (a TTY, an opencode
instance's API surface), not with docker operations on the container.
Docker-level operations (stop, remove, inspect) are the coordinator's
job. No described use case requires the grant pattern. This ADR commits to
proxy-only (Decision 3).

## Decision

### 1. Storage: reuse the repo/adapter pattern (fourth instance)

The ownership store is a fourth instance of the established repo/adapter
pattern (ADR-033), alongside `IdentityProvider` (ADR-004), `IdentityStore`
(ADR-035), and `CredentialStore` (ADR-031). A trait in `alknet-core` with
an in-memory default adapter:

```rust
// alknet-core

/// Read side: consulted by AccessControl::check on the dispatch hot path.
/// Sync — called in the accept/dispatch loop, no .await.
pub trait OwnershipProvider: Send + Sync + 'static {
    /// Does `identity` own `resource_type/resource_id` with `action`?
    /// Called when AccessControl has resource_type + resource_action set
    /// and the dispatcher has extracted resource_id from the input via
    /// OperationSpec.resource_id_path.
    fn owns(
        &self,
        identity: &Identity,
        resource_type: &str,
        resource_id: &str,
        action: &str,
    ) -> bool;

    /// What resources of `resource_type` does `identity` own?
    /// Called for the `list` case (resource_type set, resource_id_path
    /// absent) — the result-filter path. Returns the set of resource IDs
    /// the caller owns, for the handler to filter against.
    fn owned_resources(
        &self,
        identity: &Identity,
        resource_type: &str,
    ) -> Vec<String>;

    /// Does `identity` own *any* resource of `resource_type`?
    /// Called for the `list` case — the scope-gate path. Cheap boolean
    /// for the "allow if scoped" default.
    fn owns_any(
        &self,
        identity: &Identity,
        resource_type: &str,
    ) -> bool;
}

/// Write side: called by the handler that manages the resource lifecycle.
/// Async — not on the dispatch hot path.
#[async_trait]
pub trait OwnershipStore: Send + Sync + 'static {
    /// Record that `identity` spawned `resource_type/resource_id`.
    /// Called by the docker handler after `docker/container/create`
    /// succeeds.
    async fn record(
        &mut self,
        identity: &Identity,
        resource_type: &str,
        resource_id: &str,
    ) -> Result<(), OwnershipError>;

    /// Revoke ownership of `resource_type/resource_id`.
    /// Called by the docker handler on container exit / removal
    /// (specific #2 — handler-driven teardown).
    async fn revoke(
        &mut self,
        resource_type: &str,
        resource_id: &str,
    ) -> Result<(), OwnershipError>;
}

/// In-memory default adapter. Carries the docker/runner cases with no
/// backend dependency — ownership is runtime state, meaningless across
/// restarts (a container ID from a previous process doesn't exist).
pub struct InMemoryOwnershipStore { /* HashMap<...> */ }
```

The read trait (`OwnershipProvider`) is sync — called from
`AccessControl::check` on the dispatch hot path, no `.await`. The write
trait (`OwnershipStore`) is async — called by handlers off the hot path.

A persistence adapter (e.g., sqlite/honker-backed, for a hub that wants
fleet ownership to survive restarts) is separable and built when a
concrete use case forces it — same as `alknet-store-sqlite` for
peer/credential persistence (ADR-035). The in-memory default carries no
persistence; ownership is runtime state. A persistence adapter would cache
in memory and use honker `NOTIFY` for invalidation — same
`ArcSwap`-backed full-reload pattern as `ConfigIdentityProvider`
(ADR-035).

**The read/write split mirrors ADR-035.** `OwnershipProvider` (read,
sync) is the trait the dispatch path depends on. `OwnershipStore` (write,
async) is the trait the handler lifecycle calls. The in-memory default
implements both. A persistence adapter would implement both with an
in-memory read cache backed by SQLite, same as `SqliteIdentityProvider`
implements `IdentityProvider` (sync, cached) + `IdentityStore` (async
write).

### 2. Integration: `AccessControl::check` consults the ownership provider

`AccessControl::check` grows a parameter for the ownership provider. The
provider is carried on `OperationContext` (populated by the dispatch path
from the registry's wiring), not threaded through every call site
manually:

```rust
pub struct AccessControl {
    pub required_scopes: Vec<String>,
    pub required_scopes_any: Option<Vec<String>>,
    pub resource_type: Option<String>,
    pub resource_action: Option<String>,
}

impl AccessControl {
    /// `ownership` is None when the operation has no resource_type
    /// (pure scope check) or when no ownership provider is wired
    /// (the static `Identity.resources` path — backward compatible).
    /// `resource_id` is None for the `list` case (resource_type set,
    /// resource_id_path absent — specific #4a).
    pub fn check(
        &self,
        identity: Option<&Identity>,
        resource_id: Option<&str>,
        ownership: Option<&dyn OwnershipProvider>,
    ) -> bool {
        // 1. Scope check (unchanged): identity.scopes ⊇ required_scopes.
        //    If identity is None and scopes are required, deny here.
        // 2. Resource check (only if self.resource_type is Some):
        //    a. If resource_id is Some(id) and ownership is Some(p):
        //         → identity must be Some (owns takes &Identity, not
        //           Option); if identity is None, deny. Otherwise
        //         → p.owns(identity.unwrap(), resource_type, id, resource_action)
        //    b. If resource_id is None (the `list` case) and ownership is Some(p):
        //         → if identity is None, deny; otherwise
        //         → p.owns_any(identity.unwrap(), resource_type)  [scope-gate; see #4a]
        //    c. If ownership is None → fall back to static
        //       identity.resources[resource_type] ∋ resource_action
        //       (backward compat for non-runtime resources; identity
        //       may be None here — empty resources → deny if action required)
    }
}
```

The `resource_id` parameter is extracted by the dispatcher from the
operation input using `OperationSpec.resource_id_path` (Decision 2a
below). When the spec has no `resource_id_path` (the `list` case), the
dispatcher passes `resource_id: None`, and `check` takes the scope-gate
path (specific #1).

**Backward compatibility.** When `ownership` is `None`, `check` falls
back to the static `Identity.resources` path. This means existing
operations with static resource sets (no runtime spawning) work unchanged
— the ownership provider is an additional check, not a replacement. The
signature change is a one-way door (every call site and test updates), but
the semantic change is additive: operations that don't wire an ownership
provider behave exactly as before.

### 2a. `OperationSpec` gains `resource_id_path`

```rust
pub struct OperationSpec {
    pub name: String,
    pub namespace: String,
    pub op_type: OperationType,
    pub visibility: Visibility,
    pub input_schema: Value,
    pub output_schema: Value,
    pub error_schemas: Vec<ErrorDefinition>,
    pub access_control: AccessControl,
    /// JSON pointer into the input for the resource ID, when
    /// `access_control.resource_type` is set and the operation targets a
    /// specific runtime-spawned resource. e.g., `"$.containerId"` for
    /// `docker/container/exec`. Absent for no-specific-resource operations
    /// (the `list` case — specific #1). The dispatcher extracts the
    /// resource ID from the input using this path and passes it to
    /// `AccessControl::check`.
    pub resource_id_path: Option<String>,
}
```

The fit with JSON Schema is load-bearing, not incidental: `input_schema`
is already a JSON Schema, so `resource_id_path` is a pointer *within* an
existing schema on the same spec. The `OperationSpec` becomes fully
self-describing for authorization — what resource type, what action, and
*which input field* drives the resource lookup. No per-namespace
conventions, no handler-level knowledge, no "the dispatcher just knows."
The contract is on the spec, where it belongs.

### 3. Access pattern: proxy-only

The base model is **"spawner owns, proxy to share, teardown revokes"** —
with no grant/transfer mechanism in the core ownership store.

When a coordinator spawns a container and wants to expose docker
operations on it to an end user, the coordinator re-exports those
operations via `from_call` (or composes them in its own handlers). The end
user invokes the re-exported operation; the coordinator is the direct
caller to the docker endpoint; docker's ownership store sees the
coordinator as owner and caller; the check passes. The end user's identity
rides as `forwarded_for` metadata (ADR-032), and the coordinator handles
its own end-user-level ACL at its own layer.

**"Poking holes" (the grant pattern) is a downstream-app concern, not a
core-model concern.** The app that owns the resources re-exports the
operations it wants to share via `from_call` with its own ACL layer,
rather than the core ownership store growing a grant API. The ADR commits
to proxy-only and explicitly states that "poking holes" is a downstream
app's job.

**A future grant mechanism is additive, not a one-way door closure.** If
a use case forces the grant pattern, it's a new method on the ownership
store trait (`grant(identity, resource)` / `revoke_grant(...)`).
`AccessControl::check` already consults the ownership provider; a
grant-aware provider would answer "yes" for grantees in addition to
owners, without a trait-shape change. The two-way-door classification
(additive) is stated here as reversal-cost classification, not as a reason
to defer the decision — the decision is made (proxy-only), and the cost of
reversing it if a future use case forces it is low. If the grant pattern
is later admitted, specifics #3 and #4 are revisited: cross-node ownership
propagation returns to the table (#3), and composition under a grant would
need `CompositionAuthority` to grow a dynamic path, amending ADR-015/022
(#4).

### 4. The four edge specifics

#### 4a. The `list` case: scope-gate + result-filter, composing

Operations with `resource_type` set but `resource_id_path` absent — e.g.
`docker/container/list` — don't reference a specific container. When a
coordinator lists containers it owns, it should see only its own — not
every container on the host. That's not just scope-gating ("can you call
`container/list` at all?") and not just result-filtering ("return only
owned") — it's **both**:

1. **Scope-gate** (the call): does the peer have the `container:list`
   scope? This is the static `required_scopes` check — unchanged. If the
   peer doesn't have the scope, the call is denied before the ownership
   provider is consulted.
2. **Result-filter** (the response): the handler calls
   `OwnershipProvider::owned_resources(identity, "container")` and filters
   the result to only the containers the caller owns. The default is
   "allow if scoped, filter to owned."

The scope-gate is `AccessControl::check`'s scope path (static, unchanged).
The result-filter is a handler-level concern — the handler calls the
ownership provider's `owned_resources` method and filters. The ADR states
the default ("allow if scoped, filter to owned") and the composition
(scope-gate the call, then filter the result). A spec declares it wants
this by setting `resource_type` without `resource_id_path`.

`exec`/`inspect`/`stop` against a specific container are the clean case:
`resource_id_path: Some("$.containerId")`, the dispatcher extracts the ID,
`check` calls `owns(identity, "container", id, "exec")` — a single
targeted lookup.

#### 4b. Teardown coupling: automatic, handler-driven

The ownership store's write path (revoke on teardown) is coupled to the
spawned resource's lifecycle. The "burn it and start over" capability
depends on ownership state tracking the lifecycle correctly. When a
container dies or is destroyed, the ownership entry is revoked **by the
handler that managed the lifecycle** (the docker handler calls
`OwnershipStore::revoke` on container exit), not by an operator workflow
or a background reaper.

The burn-and-start-over pattern is:
1. Destroy container → handler calls `revoke("container", id)` → ownership
   revoked automatically.
2. Spawn new container → handler calls `record(identity, "container", new_id)`
   → new ownership recorded.

If teardown weren't automatic, stale ownership entries would accumulate
and the "burn" path would leave dangling ACL state — an ACL check could
reference a resource that no longer exists, and a reused container ID
could grant access to the wrong caller.

The architectural commitment is: **handler-driven revoke on lifecycle
end, not a reaper.** The coupling mechanism (explicit handler call vs. a
lifecycle-hook abstraction the handler framework provides) is two-way-door
implementation work — the docker handler calling `revoke` directly is the
initial mechanism (explicit handler call); a lifecycle-hook abstraction is
a refinement if multiple resource-spawning crates share the pattern.

#### 4c. Fleet representation: per-node ownership, downstream app tracks "who is this for"

Under the proxy pattern (Decision 3), the docker node records "coordinator
owns C" in its local ownership store. The coordinator's "I started C for
agent Y" mapping lives in the coordinator's own downstream-app state, not
in the core ownership store.

The ownership store is **per-node** — each docker node records its local
ownership. The hub's agent-to-workspace mapping is app state. There is
**no cross-node ownership propagation in the base model** — the spoke
sees the hub as the owner (the hub's `Identity` is what the spoke's
ownership store records), and the hub's "who is this for" is its own
concern, tracked in the hub's app state, carried as `forwarded_for`
metadata on the wire (ADR-032).

This simplifies fleet representation: the proxy pattern keeps ownership
local. The spoke doesn't need to know about the end user; the hub doesn't
need to push ownership records to the spoke. The hub authenticates as
itself (its own `auth_token`), the spoke records the hub as the owner, and
the hub's end-user ACL is its own layer.

#### 4d. Composition interaction: two separate checks, no change to `CompositionAuthority`

In the proxy pattern, the coordinator composes `docker/container/exec` on
behalf of an agent. Two checks must pass:

1. **Static scope check** (ADR-015/022, unchanged): the coordinator's
   `CompositionAuthority` has the `container:exec` scope. This is the
   existing `CompositionAuthority.scopes` check — static, set at
   registration, no dynamic path.
2. **Dynamic ownership check** (this ADR): the coordinator owns this
   specific container. This is the new `OwnershipProvider::owns` check —
   dynamic, consults the ownership store.

The composition authority stays static — it doesn't grow a dynamic path.
The ownership store handles the dynamic resource-level check. Both must
pass; they're orthogonal. **ADR-015 and ADR-022 do not need amendment.**

The `CompositionAuthority.resources` field (ADR-022, line 180:
`resources: HashMap<String, Vec<String>>`) continues to serve its existing
purpose: static resource lists for composition authority (e.g.,
`{"service": ["vastai", "github"]}` bounds which services the handler can
reach in composition). It is not involved in the dynamic ownership check —
that's the ownership provider's job. The two are separate:

- `CompositionAuthority.resources` — static, "what services can this
  handler compose," checked against the composition authority's declared
  resource lists.
- `OwnershipProvider::owns` — dynamic, "does this identity own this
  specific runtime-spawned resource," checked against the ownership store.

A handler composing `docker/container/exec` passes both: its composition
authority has `container:exec` in its scopes (static), and the ownership
provider confirms it owns the container (dynamic).

## Consequences

**Positive:**

- The alknet-docker, alknet-tty, opencode-runner wrapper, and
  `alknet-container` crate specs can declare their `AccessControl` shapes
  against a single coherent model. The block on those specs is lifted.
- The ownership store reuses the established repo/adapter pattern
  (ADR-033) — no new shape invented on the storage side. The in-memory
  default carries the docker/runner cases with no backend dependency; a
  persistence adapter is additive when a use case forces it.
- `OperationSpec` is fully self-describing for authorization: resource
  type, action, and which input field drives the resource lookup are all
  on the spec. No per-namespace conventions, no handler-level knowledge.
- The proxy-only model keeps the base model simple: spawner owns, proxy
  to share, teardown revokes. The `forwarded_for` metadata (ADR-032) is
  the end-user-identity carrier; the coordinator handles its own ACL. No
  grant API in the core ownership store.
- ADR-015/022 are unchanged. The composition authority stays static; the
  ownership store is an additional check, not a modification to the
  existing one. The privilege model stays coherent with the ownership
  model.
- The `list` case has a clean default ("allow if scoped, filter to owned")
  that composes scope-gating and result-filtering without conflating them.
- Teardown is automatic and handler-driven, so the "burn it and start
  over" pattern leaves no dangling ACL state.

**Negative:**

- `AccessControl::check` gains a parameter. This is a one-way door —
  every call site and test updates. Per the project's decision principle
  (implementation workload is a non-issue relative to semantic
  correctness and long-term clarity), this is implementation cost, not
  semantic cost.
- `OperationSpec` gains a field (`resource_id_path`). Spec-constructing
  code (tests, adapter registrations) must add the field. The field is
  `Option<String>` — `None` for operations without runtime-spawned
  resources, so existing specs are unchanged in shape (the field defaults
  to `None`).
- `alknet-core` gains two new traits (`OwnershipProvider`,
  `OwnershipStore`) and an in-memory default adapter. Each trait is a
  contract downstream crates depend on. The trait shapes are the one-way
  doors; the adapter shapes are two-way.
- The handler that manages a resource's lifecycle has an additional
  responsibility: calling `record` on spawn and `revoke` on teardown. If
  a handler forgets to call `revoke`, stale ownership entries accumulate.
  This is the coupling requirement (specific #2) — it's the handler's
  job, not the framework's, and the handler framework can provide a
  lifecycle-hook abstraction to reduce boilerplate (two-way-door
  mechanism work).
- The ownership provider is consulted on every resource-typed
  `AccessControl::check`. The in-memory default is a `HashMap` lookup —
  negligible. A persistence adapter caches in memory (sync read from
  cache, same `ArcSwap` pattern as `ConfigIdentityProvider`), so the hot
  path stays sync and fast.
- The proxy-only decision means a downstream app that wants to give an
  end user direct access to a runtime-spawned resource must build its own
  re-export + ACL layer, rather than using a core grant mechanism. This
  is the intended trade — "poking holes" is the app's job, not the core
  model's. If a future use case forces the grant pattern, it's additive
  (a new trait method), not a redesign.

## Assumptions

1. **The ownership store is per-node.** Each node records its local
   ownership. There is no cross-node ownership propagation in the base
   model. The hub's "who is this for" mapping is app state, not core
   ownership state. (Specific #4c.)

2. **The proxy pattern is sufficient for all current use cases.** The
   agent-workspace case, the docker coordinator case, and the runner
   wrapper case are all proxy-pattern. No described use case requires the
   grant pattern. If one emerges, the grant mechanism is additive (a new
   method on the ownership store trait), not a redesign.

3. **The ownership store's read trait is sync.** It is called from
   `AccessControl::check` on the dispatch hot path, no `.await`. A
   persistence adapter caches in memory and uses honker `NOTIFY` for
   invalidation — same `ArcSwap`-backed full-reload pattern as
   `ConfigIdentityProvider` (ADR-035).

4. **Ownership is runtime state, meaningless across restarts.** A
   container ID from a previous process doesn't exist. The in-memory
   default carries no persistence; a persistence adapter is built when a
   concrete use case forces it (e.g., a hub that wants fleet ownership to
   survive restarts).

5. **The handler that manages a resource's lifecycle is responsible for
   calling `record` and `revoke`.** This is the coupling requirement
   (specific #4b). The framework can provide a lifecycle-hook abstraction
   to reduce boilerplate, but the responsibility is the handler's, not
   the framework's.

6. **`CompositionAuthority.resources` (ADR-022) is not involved in the
   dynamic ownership check.** It serves its existing purpose (static
   resource lists for composition). The dynamic ownership check is the
   ownership provider's job. The two are separate and orthogonal.

7. **`AccessControl::check`'s new `ownership` parameter is `None` for
   operations without runtime-spawned resources.** This preserves
   backward compatibility — operations with static resource sets work
   unchanged via the `Identity.resources` fallback path.

## References

- OQ-42: Dynamic Resource Ownership for Runtime-Spawned Resources
  (resolved by this ADR — the five sub-questions this ADR writes into
  decision text)
- ADR-004: Auth as Shared Core (`IdentityProvider` — the first instance
  of the repo/adapter pattern the ownership store reuses; ADR-033 makes
  the pattern explicit, ADR-004 is the concrete first instance)
- ADR-009: One-Way Door Decision Framework (the door-type-as-deferral
  anti-pattern this ADR's proxy-only decision avoids; the reversal-cost
  classification of the grant pattern's additive nature)
- ADR-015: Privilege Model and Authority Context (the static
  composition-authority model; this ADR adds an **orthogonal** dynamic
  ownership check alongside it — ADR-015's text is **unchanged** per
  specific #4d; the system gains a second check, not a modification to
  the first)
- ADR-022: Handler Registration, Provenance, and Composition Authority
  (`CompositionAuthority.resources` — the static resource list field this
  ADR confirms is not involved in the dynamic ownership check —
  **unchanged** per specific #4d)
- ADR-030: PeerEntry and Identity.id Decoupling (`Identity.resources` —
  the static resource path this ADR's ownership provider extends for
  runtime-spawned resources)
- ADR-032: Forwarded-For Identity (Metadata, Not Authority) (`forwarded_for`
  — the proxy pattern's end-user-identity carrier; the proxy-only model
  relies on this)
- ADR-033: Storage Boundary and Repo/Adapter Pattern (the pattern this ADR
  reuses for the ownership store — fourth instance alongside
  `IdentityProvider`/`IdentityStore`/`CredentialStore`)
- ADR-035: Concrete Persistence Adapter Shapes (the sync-read + ArcSwap +
  honker-NOTIFY shape this ADR's persistence adapter would follow, if
  built; `IdentityStore` is the write-trait analogue)
- ADR-017: Call Protocol Client and Adapter Contract (`from_call` — the
  adapter that imports a peer's operations and re-registers them locally;
  the proxy pattern's re-export mechanism)
- [auth.md](../crates/core/auth.md) (`Identity.resources`,
  `AccessControl::check` interaction — both under edit by this decision)
- [operation-registry.md](../crates/call/operation-registry.md)
  (`AccessControl`, `OperationSpec` — `resource_id_path` addition)
- [alknet-docker POC summary](../../research/alknet-docker/poc-summary.md)
  §"Open Unknowns" #3 (the research finding that surfaced this question)