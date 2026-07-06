# OQ-42: Dynamic Resource Ownership for Runtime-Spawned Resources

- **Origin**: [alknet-docker POC summary](../../research/alknet-docker/poc-summary.md)
  §"Open Unknowns" #3 (container-as-resource identity model); generalized
  during the Phase 1 review pass triggered by that research finding.
- **Status**: resolved — all five sub-questions are decided (storage
  shape, integration point, proxy-vs-grant, and the four clarifications).
  The ADR will write these into decision text; the dependent crate specs
  can declare their `AccessControl` shapes against this model.
- **Door type**: One-way (the `AccessControl::check` signature change and
  the `OperationSpec.resource_id_path` addition in core/call), two-way (the
  ownership provider mechanism, per the established repo/adapter pattern)
- **Priority**: high — blocks the alknet-docker, alknet-tty, opencode-runner
  wrapper, and `alknet-container` (fleet normalization) crate specs. None of
  those specs can declare their `AccessControl` shapes until this is
  resolved, because the model available to them determines what ACL
  declarations are even expressible. Permitting "docker picks a per-crate
  default and the others follow" is the door-type-as-deferral anti-pattern
  (ADR-009 §"What this framework is NOT"): each crate bakes in an ACL shape
  and downstream crates build on whatever default was picked, making the
  "cheap reversal" expensive.

- **Resolution.**

  **Decision 1 — storage side: reuse the repo/adapter pattern.** The
  ownership store is a fourth instance of the established repo/adapter
  pattern (ADR-033), alongside `IdentityProvider` (ADR-004), `IdentityStore`
  (ADR-035), and `CredentialStore` (ADR-031). Concretely: a trait in
  `alknet-core` (read method: "does identity X own resource R with action
  A?" / "what resources of type T does X own?"; write method: "record X
  spawned R", "revoke R on teardown"), with an in-memory default adapter in
  core. The in-memory default carries the docker/runner cases with no
  backend dependency — ownership is runtime state, meaningless across
  restarts (a container ID from a previous process doesn't exist), so the
  default case has no persistence requirement. A persistence adapter (e.g.
  sqlite/honker-backed, for a hub that wants fleet ownership to survive
  restarts) is separable and built when a concrete use case forces it, same
  as `alknet-store-sqlite` for peer/credential persistence. The read stays
  sync (called from `AccessControl::check` on the dispatch hot path, no
  `.await`), with persistence adapters caching in memory and using honker
  NOTIFY for invalidation — same `ArcSwap`-backed full-reload pattern as
  `ConfigIdentityProvider` (ADR-035). No new shape invented on the storage
  side; no Phase 0 needed for it.

  **Decision 2 — integration point: Option 2, `check` consults the
  ownership provider directly.** `AccessControl::check` grows a parameter
  for the ownership provider (or reads one carried on `OperationContext`),
  and consults it for `resource_type`/`resource_action` checks against
  runtime-spawned resources. The alternative considered and rejected —
  Option 1, augmenting `Identity.resources` with a per-request snapshot
  before calling `check` — preserves `check`'s purity by moving the
  impurity one frame up the stack: the dispatcher would pull owned
  resources into a per-request identity snapshot so `check` *looks*
  unchanged while reading state that was never part of the static
  identity. The purity was always theatrical (the question "can X exec
  into container C" was never purely a function of identity; it just
  looked that way because the resource set was static). Option 2 makes
  `check`'s signature honest about what ACL checking *is* in the presence
  of dynamic resources: a function of (ACL, Identity, current-ownership-
  state). The impurity is real either way; Option 2 puts it in the
  signature where it's visible, rather than hiding it in a per-request
  snapshot pretending to be static identity. Option 3 (handler-level
  ownership check, `AccessControl` gates only scope) was rejected because
  it splits the ACL story — some resources statically checked, some
  handler-checked — which is the kind of inconsistency that creates the
  "figure out how it fits with what is there" cleanup this OQ exists to
  prevent.

  The cost of Option 2 is a `check` signature change — a one-way door,
  every call site and test updates. Per the project's decision principle
  (implementation workload is a non-issue relative to semantic correctness
  and long-term clarity; "path of least resistance compounded over many
  decisions is strictly dominated"), this is implementation cost, not a
  semantic cost, and does not bias the choice.

  **Refinement that makes Option 2 work cleanly: `OperationSpec` declares
  where the resource ID lives in the input.** `OperationSpec` gains a
  `resource_id_path: Option<String>` — a JSON pointer into the operation
  input, e.g. `"$.containerId"` for `docker/container/exec`. The dispatcher
  extracts the resource ID from the input using the spec-declared path,
  passes it to `check`, and `check` asks the provider "does this identity
  own `<resource_type>/<resource_id>` with action `<resource_action>`?" —
  a single targeted lookup, not a whole-resource-set pull. The fit with
  JSON Schema is load-bearing, not incidental: `OperationSpec.input_schema`
  is already a JSON Schema, so `resource_id_path` is a pointer *within* an
  existing schema on the same spec. The `OperationSpec` becomes fully
  self-describing for authorization — what resource type, what action, and
  *which input field* drives the resource lookup. No per-namespace
  conventions, no handler-level knowledge, no "the dispatcher just knows."
  The contract is on the spec, where it belongs.

- **Resolved specifics (the four questions the ADR must write into
  decision text).** The decisions above settle the storage shape and the
  integration point. The four specifics below settle how the model
  behaves at the edges:

  1. **No-specific-resource operations (the `list` case) — scope-gate +
     result-filter, composing.** Operations with `resource_type` set but
     `resource_id_path` absent — e.g. `docker/container/list`, which
     doesn't reference a specific container. When a coordinator lists
     containers it owns, it should see only its own — not every container
     on the host. That's not just scope-gating ("can you call
     `container/list` at all?") and not just result-filtering ("return
     only owned") — it's both: scope-gate the call (does the peer have the
     `container:list` scope), then filter the result to owned resources.
     The default is "allow if scoped, filter to owned." `list` is the case
     that forces this; `exec`/`inspect`/`stop` against a specific
     container are the clean case (single targeted ownership lookup via
     `resource_id_path`). The ADR states the default and how a spec
     declares which it wants.

  2. **Teardown coupling — automatic, handler-driven.** The ownership
     store's write path (revoke on teardown) is coupled to the spawned
     resource's lifecycle. The "burn it and start over" capability depends
     on ownership state tracking the lifecycle correctly. When a container
     dies or is destroyed, the ownership entry is revoked *by the handler
     that managed the lifecycle* (the docker handler calls revoke on
     container exit), not by an operator workflow or a background reaper.
     The burn-and-start-over pattern is: destroy container → ownership
     revoked automatically → spawn new container → new ownership recorded.
     If teardown weren't automatic, stale ownership entries would
     accumulate and the "burn" path would leave dangling ACL state. The
     architectural commitment is: handler-driven revoke on lifecycle end,
     not a reaper. The coupling mechanism (explicit handler call vs.
     lifecycle-hook abstraction) is two-way-door implementation work.

  3. **Fleet representation (spoke resources on the hub) — per-node
     ownership, downstream app tracks "who is this for."** Under the
     proxy pattern (Decision 3 below), the docker node records "coordinator
     owns C" in its local ownership store. The coordinator's "I started C
     for agent Y" mapping lives in the coordinator's own downstream-app
     state, not in the core ownership store. The ownership store is
     per-node (each docker node records its local ownership); the hub's
     agent-to-workspace mapping is app state. There is no cross-node
     ownership propagation in the base model — the spoke sees the hub as
     the owner, and the hub's "who is this for" is its own concern. The
     proxy pattern keeps ownership local, which is why this question is
     less consequential than originally framed.

  4. **Composition interaction — two separate checks, no change to
     `CompositionAuthority`.** In the proxy pattern, the coordinator
     composes `docker/container/exec` on behalf of an agent. Two checks
     must pass: (a) the coordinator's `CompositionAuthority` has the
     `container:exec` scope (static, ADR-015/022 unchanged), and (b) the
     coordinator owns this specific container (dynamic, ownership store).
     The composition authority stays static — it doesn't grow a dynamic
     path. The ownership store handles the dynamic resource-level check.
     Both must pass; they're orthogonal. **ADR-015/022 don't need
     amendment** — the composition authority is unchanged, and the
     ownership store is an additional check, not a modification to the
     existing one.

- **Decision 3 — access pattern: proxy-only as the base model.** The base
  model is "spawner owns, proxy to share, teardown revokes" — with no
  grant/transfer mechanism in the core ownership store. Two patterns for
  how a downstream consumer reaches a runtime-spawned resource were
  identified:

  - **Proxy pattern (the common case, and the only one the core model
    supports).** A coordinator starts a container and manages its
    lifecycle; the end user never talks to docker directly. The
    coordinator re-exports the docker operations it wants to expose (via
    `from_call` — the adapter that imports a peer's operations and
    re-registers them locally, ADR-017 — or by composing them in its own
    handlers), and when the end user invokes one, the coordinator is the
    *direct caller* to the docker endpoint. Docker's ownership store sees the coordinator as the
    owner and as the caller — the check passes. The end user's identity
    rides as `forwarded_for` metadata (ADR-032), and the coordinator does
    whatever end-user-level ACL it wants at its own layer. This is the
    kernel/user-land + forwarded-for model: the hub's authority is used,
    `forwarded_for` is metadata, the hub handles its own ACL.

  - **Grant pattern ("poking holes") — not in the core model.** A
    downstream app wants to give an end user *direct* call-protocol
    access to the docker endpoint for specific containers — the end user
    calls `docker/container/exec` themselves, not through a proxy. Docker's
    ownership store would need a record that the end user has access to
    that container, even though the downstream app spawned it. No
    described use case requires this. The agent-workspace case — the
    concrete one — is entirely the proxy pattern: the coordinator starts
    the workspace container; the agent interacts with what's *inside* the
    container (a TTY, an opencode instance's API surface), not with
    docker operations on the container. Docker-level operations (stop,
    remove, inspect) are the coordinator's job.

  "Poking holes" is a downstream-app concern — the app that owns the
  resources re-exports the operations it wants to share via `from_call`
  with its own ACL layer, rather than the core ownership store growing a
  grant API. The ADR commits to proxy-only and explicitly states that
  "poking holes" is a downstream app's job.

  **A future grant mechanism is additive, not a one-way door closure.**
  If a use case forces the grant pattern, it's a new method on the
  ownership store trait (`grant(identity, resource)` /
  `revoke_grant(...)`). `AccessControl::check` already consults the
  ownership provider; a grant-aware provider would answer "yes" for
  grantees in addition to owners, without a trait-shape change. The
  two-way-door classification (additive) is stated here as reversal-cost
  classification, not as a reason to defer the decision — the decision is
  made (proxy-only), and the cost of reversing it if a future use case
  forces it is low. If the grant pattern is later admitted, specifics 3
  and 4 above are revisited: cross-node ownership propagation returns to
  the table (3), and composition under a grant would need
  `CompositionAuthority` to grow a dynamic path, amending ADR-015/022 (4).

- **Cross-references**: ADR-009 (door-type-as-deferral anti-pattern),
  ADR-015, ADR-022 (the static `CompositionAuthority.resources` model this
  extends — see open question 4), ADR-030, ADR-032 (`forwarded_for`
  metadata — the proxy pattern's end-user-identity carrier), ADR-033
  (repo/adapter pattern — reused for the ownership store), ADR-035
  (`IdentityStore` — administrative peer mutations, a different concern
  from runtime resource ownership, but the sync-read + ArcSwap +
  honker-NOTIFY shape is reused),
  [auth.md](crates/core/auth.md) (`Identity.resources`,
  `AccessControl::check` interaction — both under edit by this decision),
  [operation-registry.md](crates/call/operation-registry.md)
  (`AccessControl`, `OperationSpec` — `resource_id_path` addition),
  [alknet-docker POC summary](../../research/alknet-docker/poc-summary.md)
  §"Open Unknowns" #3
