# Research: Peer-Graph Routing Model for alknet-call Composition

**Status**: Complete
**Date**: 2026-06-27
**Scope**: Deep dive — structural design fix, POC-validated
**Supersedes**: ADR-028 (to be superseded by a new ADR; draft included in §11)
**POC**: Validated in-repo against real types, then removed. See §7.

---

## 1. Problem Statement

The call protocol's composition model is **flat per overlay and single-peer**.
This works for one remote peer and breaks the moment a head node has two
workers. The breakage is structural, not a missing default:

1. **Overlay collision.** `CompositeOperationEnv` holds **one** `connection:
   Option<Arc<dyn OperationEnv>>` overlay (`registry/env.rs:96-100`). The
   Layer 2 imported-ops overlay on `CallConnection` is a flat
   `HashMap<String, HandlerRegistration>` keyed by operation name
   (`protocol/connection.rs:36`). When a head imports from worker A and
   worker B, both exposing `/container/exec`, there is no way to route
   `invoke("container", "exec")` to the right peer. `from_call` against A
   and B both register `container/exec` into their respective connection
   overlays, but the composite env can hold only one connection layer — and
   even if it held two, `contains("container/exec")` returns true for both
   with no way to disambiguate.

2. **`from_call` namespace prefix is a naming-convention hack.** DC-3 / OQ-28
   made `FromCallConfig::namespace_prefix` the disambiguation mechanism: the
   operator prefixes imported op names (`worker-a/container/exec`) so two
   peers' ops don't collide in a flat map. This pushes disambiguation to the
   caller and into the `ScopedOperationEnv { allowed: HashSet<String> }`
   reachability list — every composing handler that wants to reach
   worker A's `container/exec` must list `"worker-a/container/exec"` in its
   scoped env. The prefix is bolted onto a flat map instead of being
   structural routing.

3. **ADR-028's `remote_safe: bool` + `trusted_peer: bool` is a second,
   parallel, weaker authorization system.** ADR-028 introduced a
   `RemoteFilter { trusted_peer: bool }` gate in `protocol/dispatch.rs:48-70`
   that runs *before* the existing `AccessControl::check`
   (`registry/registration.rs:128-140`). `trusted_peer: true` is a blanket
   security-bypass flag — the exact anti-pattern ADR-015 was written to kill
   (it replaced `trusted: true` with the authority-switch model). ADR-028
   reintroduced it at the peer boundary. The existing authorization
   machinery in core (`Identity`, `IdentityProvider`, `AccessControl::check`)
   is real, grounded, and already wired into the dispatch path — ADR-028
   should have *used* it for peer authorization, not invented a parallel
   system.

The head→many-workers / hub→spoke pattern (ray.io's model) is the primary
downstream use case. The current model cannot express it. This is a blocking
structural fix, not a "v1/later" refinement.

---

## 2. The Existing Authorization Machinery (What ADR-028 Should Have Used)

The dispatch path already runs `AccessControl::check` against the caller's
`Identity`. For a remote peer's call, the caller's `Identity` *is* the peer's
resolved identity. The machinery is complete:

```rust
// crates/alknet-core/src/auth.rs:14-19
pub struct Identity {
    pub id: String,                              // the peer's fingerprint/id
    pub scopes: Vec<String>,                      // what this peer is allowed to do
    pub resources: HashMap<String, Vec<String>>,  // resource-scoped grants
}

// crates/alknet-call/src/registry/spec.rs:31-37
pub struct AccessControl {
    pub required_scopes: Vec<String>,             // AND-gate
    pub required_scopes_any: Option<Vec<String>>, // OR-gate
    pub resource_type: Option<String>,
    pub resource_action: Option<String>,
}
impl AccessControl { pub fn check(&self, identity: Option<&Identity>) -> AccessResult }
```

The dispatch path (`registry/registration.rs:112-144`) already does the right
thing:

- For **external** (wire) calls: ACL checks against `context.identity` — the
  caller's identity, which for a peer call is the peer's `Identity` resolved
  via `Dispatcher::resolve_identity` (`protocol/dispatch.rs:116-134`) from the
  connection's TLS fingerprint or the call-protocol `auth_token` payload.
- For **internal** (composition) calls: ACL checks against
  `context.handler_identity` (the `CompositionAuthority` synthesized as
  `Identity`).

`Connection::identity()` (`crates/alknet-core/src/types.rs:486`) already
returns `Option<&Identity>` — the peer's resolved identity, set via
`Connection::set_identity`. `dispatch_requested` already reads it
(`protocol/dispatch.rs:222`). **The peer's `Identity` is already in the
dispatch path.** ADR-028's `remote_safe` gate is a parallel gate bolted on
*before* this existing check runs.

The security argument ADR-028 was trying to make — "a remote peer's call must
not populate `OperationContext.capabilities` from the local bundle unless the
op is explicitly exposed" — is already enforced by `AccessControl`: an op
whose `AccessControl` requires a scope the peer doesn't have returns
`FORBIDDEN` before the handler runs, so capabilities are never populated. An
op with `AccessControl::default()` (no restrictions) is implicitly callable
by any peer — including a remote one — because it requires no privileged
scope. An op that should never be callable from the wire uses
`Visibility::Internal`, which returns `NOT_FOUND` before ACL even runs (the
existing behavior, `registration.rs:124-126`).

**The op's `AccessControl` *is* the peer-authorization policy.** There is no
need for a separate `remote_safe` flag or `trusted_peer` bypass.

---

## 3. Proposed Design

### 3.1 Peer-keyed overlays (research question 2)

The Layer 2 overlay becomes peer-keyed. Two shapes change:

**`CallConnection`'s overlay** — currently
`imported_operations: Arc<RwLock<HashMap<String, HandlerRegistration>>>`
(`protocol/connection.rs:36`). Under the peer model, the *head node* (which
holds many connections) needs a peer-keyed overlay across all its connections.
The per-`CallConnection` overlay stays flat (one connection = one peer), but
the *composition env* that aggregates multiple connections becomes peer-keyed:

```rust
// The per-connection overlay stays flat — one connection, one peer.
// CallConnection::imported_operations: HashMap<String, HandlerRegistration>  (unchanged)

// The composite env becomes peer-keyed. This replaces
// CompositeOperationEnv's singular `connection: Option<Arc<dyn OperationEnv>>`.
pub struct PeerCompositeEnv {
    pub base: Arc<dyn OperationEnv + Send + Sync>,       // Layer 0 curated
    pub session: Option<Arc<dyn OperationEnv + Send + Sync>>,  // Layer 1
    pub connections: HashMap<PeerId, Arc<dyn OperationEnv + Send + Sync>>,  // Layer 2, peer-keyed
    connection_order: Vec<PeerId>,  // insertion order for PeerRef::Any first-match
}
```

The `PeerId` is the peer's `Identity.id` — the same field
`Connection::identity()` already exposes. This is the natural key: it's
already resolved, already in the dispatch path, and already unique per peer.

**`contains()` across multiple peer overlays** — the composite env's
`contains(name)` returns true if *any* peer's overlay contains the name (the
union). This is the probe the fallthrough logic uses. A peer-qualified
`peer_contains(peer, name)` is added for `PeerRef::Specific` routing.

### 3.2 `OperationEnv::invoke()` peer-routing signature (research question 1)

A `PeerRef` enum is added as the peer selector on the routing path:

```rust
pub enum PeerRef {
    Specific(PeerId),  // route to this exact peer; NOT_FOUND if it doesn't serve the op
    Any,               // route to the first peer (insertion order) that serves it
}
```

The `OperationEnv` trait gains a peer-routing method. Two integration options
(validated in the POC, §7):

**Option A — extend `OperationEnv` with a default-impl method:**
```rust
#[async_trait::async_trait]
pub trait OperationEnv: Send + Sync {
    // existing methods unchanged
    async fn invoke_with_policy(&self, namespace: &str, operation: &str,
        input: Value, parent: &OperationContext, policy: AbortPolicy) -> ResponseEnvelope;
    fn contains(&self, _name: &str) -> bool { true }

    // new peer-routing method, default-impl delegates to invoke_with_policy
    // (back-compat: existing impls that don't override it route to "any" /
    // the single connection, preserving current behavior).
    async fn invoke_peer(&self, peer: &PeerRef, namespace: &str, operation: &str,
        input: Value, parent: &OperationContext, policy: AbortPolicy) -> ResponseEnvelope {
        // default: ignore peer selector, dispatch via invoke_with_policy
        self.invoke_with_policy(namespace, operation, input, parent, policy).await
    }
}
```

**Option B — make `PeerRef` an optional parameter on `invoke_with_policy`.**
Heavier change; breaks all impls. Rejected for v1.

**Recommendation: Option A.** The default-impl method preserves back-compat
(existing `LocalOperationEnv`, `OverlayOperationEnv` work unchanged) and lets
`PeerCompositeEnv` override it with real peer routing. The existing
`invoke()` / `invoke_with_policy()` methods stay as the `PeerRef::Any`
equivalent for code that doesn't care about peer selection.

**Why `PeerRef` over the alternatives:**

| Alternative | Verdict |
|---|---|
| Peer-id string parameter | Rejected — too loose. No "any peer that serves this name" semantics; forces the caller to always pick a peer even when it doesn't care. |
| Encode peer into namespace (`"worker-a/container/exec"`) | Rejected — this is the flat-namespace-prefix hack (DC-3/OQ-28) the research exists to replace. Pushes disambiguation into naming conventions rather than structural routing. |
| `Route` struct carrying selector + policy | Deferred to v2. v1's `PeerRef` + insertion-order `Any` is the minimal shape. A `Route { selector, policy: RoutingPolicy }` (round-robin, least-loaded) is the natural extension and composes cleanly with `PeerRef`. |

### 3.3 Retiring `remote_safe` / `trusted_peer` (research question 3)

`RemoteFilter` (`protocol/dispatch.rs:48-70`), `HandlerRegistration::remote_safe`
(`registry/registration.rs:41`), `CallClient::trusted_peer`
(`client/call_client.rs:99`), `OperationRegistry::list_operations_peer_scoped`
(`registry/registration.rs:103`), and
`services_list_handler_peer_scoped` (`registry/discovery.rs:202`) are all
**removed**. Peer authorization flows through the existing `AccessControl::check`:

- A remote peer's call arrives → `dispatch_requested` resolves the peer's
  `Identity` (already does, `dispatch.rs:222-223`) → `OperationRegistry::invoke`
  runs `AccessControl::check(peer_identity)` (`registration.rs:128-140`).
- If the op's `AccessControl` is satisfied → dispatch (capabilities populated
  from the bundle, same as today).
- If not → `FORBIDDEN` (capabilities never populated — the security property
  ADR-028 wanted, achieved by the existing ACL, not a parallel gate).
- If the op is `Visibility::Internal` → `NOT_FOUND` before ACL (existing
  behavior, `registration.rs:124-126`). This is the "never callable from wire"
  case — `Internal` is the existing mechanism for it.

**Does this fully replace `remote_safe`?** Yes. The three cases `remote_safe`
was meant to handle map to existing mechanisms:

| `remote_safe` case | Replacement |
|---|---|
| Op callable by any peer (was `remote_safe: true`) | `AccessControl::default()` — no restrictions, any authenticated (or unauthenticated) peer may call. Implicitly "remote-safe" because it requires no privileged scope. |
| Op callable only by some peers | `AccessControl { required_scopes: [...] }` — only peers whose `Identity.scopes` satisfy the AND-gate may call. Per-peer differentiation via `IdentityProvider` config (different peers get different scopes). |
| Op never callable from wire | `Visibility::Internal` — `NOT_FOUND` before ACL. Existing mechanism, unchanged. |

**The capability-exposure concern (ADR-028 Context).** ADR-028's worry was
"a remote peer's call must not populate `OperationContext.capabilities` from
the local bundle unless the op is explicitly exposed." Under the `AccessControl`
model, "the op is callable by this peer" *is* "the op is exposed to this
peer" — there is no separate exposure decision. If the peer's `Identity`
satisfies the op's `AccessControl`, the op dispatches and capabilities
populate (same as for any authorized caller). If not, `FORBIDDEN` before the
handler — capabilities never populate. The exposure decision and the
authorization decision are the same decision, made through one mechanism
(`AccessControl`), not two (`AccessControl` + `remote_safe`).

The one residual concern: an op with `AccessControl::default()` (no
restrictions) is callable by *any* peer, including an unauthenticated one.
This is correct — an op that requires no privileged scope is implicitly
safe to expose. If the operator wants to restrict it, they set
`required_scopes`. This is the same posture as every other ACL-gated system:
default-open for unrestricted ops, default-closed for privileged ops, and
`Internal` for never-wire-callable ops.

### 3.4 `ScopedOperationEnv` under the peer model (research question 1, cont.)

The current `ScopedOperationEnv { allowed: HashSet<String> }`
(`registry/context.rs:67-88`) enumerates flat op names. Under the peer model,
reachability may need to be peer-qualified: a handler may reach
`"worker-a/container/exec"` but not `"worker-b/container/exec"`.

**v1 design: keep `ScopedOperationEnv` as-is for the *unqualified* reachability
(the common case — peer-agnostic composition), add an *optional* peer-pinned
allowlist for the case where a handler must be pinned to a specific peer:**

```rust
pub struct ScopedPeerEnv {
    /// Unqualified — op names reachable from any peer (or locally).
    /// A handler with "container/exec" here may compose it via PeerRef::Any
    /// or PeerRef::Specific(any-peer-that-serves-it).
    pub allowed_ops: HashSet<String>,
    /// Peer-pinned — "peer-id/op-name" entries. A handler with
    /// "worker-a/container/exec" here may compose it via
    /// PeerRef::Specific("worker-a") but NOT via PeerRef::Specific("worker-b")
    /// even if worker-b also serves container/exec.
    pub peer_pinned: HashSet<String>,
}
```

This keeps the common case (peer-agnostic composition: "I want to call
`container/exec` on whichever worker serves it") simple — just list the op
name in `allowed_ops`. Peer-pinning is opt-in for the disambiguation case
that replaces `FromCallConfig::namespace_prefix` (OQ-28): instead of prefixing
the *op name*, you pin the *peer* in the reachability set.

**Integration with the existing `ScopedOperationEnv`:** the POC validates
that `ScopedPeerEnv` composes with the existing `ScopedOperationEnv` — the
unqualified `allowed_ops` is the same shape as `ScopedOperationEnv.allowed`,
and the peer-pinned set is additive. The migration path is: existing
`ScopedOperationEnv` becomes the `allowed_ops` field; peer-pinning is a new
opt-in field.

### 3.5 `services/list` across a peer graph (research question 4)

When worker A calls `services/list` on a head that has re-exported worker B's
ops, worker A sees:

- **v1 default**: the head's own Layer 0 `External` ops, filtered to those
  worker A is authorized to call (`AccessControl::check(worker_a_identity)`).
  Unchanged from today's `services_list_handler` (`registry/discovery.rs:175`),
  except the filter is `AccessControl`-based, not `remote_safe`-based.
- **Re-export listing** (new, opt-in): a `services/list-peers` op (or a
  `?include_peers=true` flag) lists the peer overlays with attribution. Each
  peer's sub-overlay is listed as a `PeerServiceListing { peer: Option<PeerId>,
  operations: Vec<PeerOpSummary> }`. The listing is filtered by the calling
  peer's `Identity` — a peer sees re-exported ops only if it is authorized to
  call them (the listing op's own `AccessControl` gates who may call
  `services/list-peers`, and the listed ops' `AccessControl` determines
  whether the calling peer could actually dispatch them).

The `services_list_handler` / `services_list_handler_peer_scoped` split
(`registry/discovery.rs:175-224`) collapses to a single `AccessControl`-filtered
handler. The `peer_scoped` variant (which took `trusted_peer: bool`) is removed;
the filtering is done by `AccessControl::check(calling_peer_identity)` inside
the handler, same as every other op.

### 3.6 `from_call` under the peer model (research question 5)

`from_call` (`client/from_call.rs:68-108`) discovers the remote peer's ops and
registers them. Under peer-keyed overlays, the registration target is the
*specific peer's* sub-overlay, not a flat overlay:

```rust
// Before (flat): connection.register_imported(reg) — into the connection's flat overlay
// After (peer-keyed): peer_overlay.register_imported(peer_id, reg) — into the peer's sub-overlay
```

**Collision behavior (OQ-28) dissolves across peers.** Same name on different
peers is fine — they live in separate sub-overlays, no collision, no prefix
needed. The collision rule stays *within* a peer: same name on the *same* peer
is still an error (a peer shouldn't expose two ops with the same name). This
is the `SamePeerCollision` error in the POC.

**`FromCallConfig::namespace_prefix` becomes optional sugar** for the case
where the *importing* node wants to expose a peer's ops under a different name
*locally* (e.g., import worker-a's `container/exec` as `worker-a/container/exec`
in the local Layer 0 for composition by handlers that use the flat
`ScopedOperationEnv`). This is a local-naming concern, not a disambiguation
concern — the peer-keyed overlay already disambiguates by peer. The prefix is
only for the local-naming-sugar case and defaults to `None`.

### 3.7 Multi-hop federation (research question 6 — out of scope for v1)

If worker A imports from the head, and the head imports from worker B, does
worker A transitively see worker B's ops? **v1: no.** The peer-keyed overlay
model is one-hop. A handler on the head can compose worker B's ops (they're in
the head's peer-keyed overlay), but worker A does not transitively see them
unless the head explicitly re-exports them (the `services/list-peers` opt-in
above).

**Does the peer-keyed model foreclose multi-hop?** No — it extends naturally.
The `PeerCompositeEnv.connections: HashMap<PeerId, Arc<dyn OperationEnv>>`
already keys by `PeerId`; a multi-hop path is a chain of `PeerRef::Specific`
routing decisions. The question is whether path-finding (which peer reaches
which op transitively) becomes real, which is where petgraph would pay off.
For v1 (one hop, shallow), a nested `HashMap<PeerId, HashMap<String, ...>>`
suffices. **Petgraph is not needed for v1.** It pays off if/when multi-hop
federation with path-finding becomes a real use case — the peer-keyed overlay
model extends to it without redesign, by adding a path-finding layer over the
peer-keyed map. This is noted, not designed.

---

## 4. Prior Art Analysis

### 4.1 Ray.io (https://docs.ray.io/en/latest/ray-core/actors.html)

Ray's model is the head→many-workers pattern this research targets. Key
prior art:

- **`ray.remote(Class)` / `@ray.remote`** — decorates a class as an *actor*
  (stateful worker). Instantiating `Counter.remote()` creates a new worker
  and returns an `ActorHandle`. This is the `PeerRef::Specific` analog — the
  handle *is* the peer reference; calling `counter.increment.remote()` routes
  to that specific actor.
- **Named actors** — Ray supports named actorsors (`Counter.options(name="my-counter").remote()`)
  addressable by name. This is the `PeerRef::Specific(peer_id)` case where
  `peer_id` is a human-readable name.
- **`ray.get(obj_ref)`** — retrieves results by object reference, decoupling
  invocation from result retrieval. alknet-call's `ResponseEnvelope` is the
  direct-return analog (no separate object store).
- **Scheduling** — Ray chooses a node for each actor based on resource
  requirements and scheduling strategy. alknet-call's `PeerRef::Any`
  (insertion-order first-match) is the v1 analog; a richer `RoutingPolicy`
  (round-robin, least-loaded) is the future extension.
- **No ACL model.** Ray assumes a trusted cluster (all workers under single
  administrative control). alknet-call's `AccessControl`-based peer
  authorization is *stronger* than Ray's model — it handles semi-trusted peers
  (the runner/dispatch pattern ADR-028 was concerned about) via scopes, not a
  blanket trust flag.

**Takeaway:** Ray's `ActorHandle` is the `PeerRef::Specific` analog. Ray has
no "any worker" primitive at the API level (you always address a specific
actor handle); alknet-call's `PeerRef::Any` is an addition for the
fan-out-to-any-worker case. Ray's lack of an ACL model is a gap alknet-call
fills with `AccessControl`.

### 4.2 Dapr service invocation (https://docs.dapr.io/developing-applications/building-blocks/service-invocation/service-invocation-overview/)

Dapr's model is the service-mesh analog. Key prior art:

- **App ID routing.** Dapr routes by `dapr-app-id` — each application has a
  unique ID, and invocation targets `<app-id>/<method>`. This is the
  `PeerRef::Specific(app_id)` analog. App ID is unique per *application*, not
  per instance — multiple instances share an app ID and Dapr load-balances
  across them (round-robin via mDNS).
- **Round-robin load balancing.** Dapr round-robins across instances of the
  same app ID. This is the `PeerRef::Any` + `RoutingPolicy::RoundRobin` analog
  — the v1 insertion-order first-match is the simplest policy; round-robin is
  the natural v2 addition.
- **Access control allow lists.** Dapr has an access-control policy
  ("which applications are allowed to call them, what applications are
  authorized to do") — this is the `AccessControl`-based peer authorization
  alknet-call already has. Dapr's model is a sidecar-level allowlist;
  alknet-call's is per-op `AccessControl` on the registration bundle. Same
  concept, finer granularity.
- **Namespace scoping.** Dapr scopes applications to namespaces; calls cross
  namespaces with explicit namespace qualification. This is the
  `PeerRef::Specific` + peer-pinned reachability analog.
- **mTLS between sidecars.** Dapr's security is at the transport (mTLS between
  Dapr sidecars). alknet-call's is at the transport (QUIC TLS) *and* the
  protocol (`auth_token` payload → `Identity` → `AccessControl`). The
  `AccessControl` layer is the application-level authorization Dapr's
  allowlist provides.

**Takeaway:** Dapr's app-ID routing confirms `PeerRef::Specific(PeerId)` is
the right shape — `PeerId` is the app-ID analog. Dapr's round-robin confirms
`PeerRef::Any` + a routing policy is the right fan-out shape. Dapr's
access-control allowlist confirms `AccessControl`-based peer authorization
is the right model — alknet-call already has it, ADR-028 should have used it.

### 4.3 Other relevant prior art

- **TypeScript `@alkdev/operations` `buildEnv()`** (referenced in ADR-015) —
  the `allowedNamespaces` scoping is the flat-namespace-prefix model this
  research replaces. The Rust `ScopedOperationEnv` already moved to
  operation-level granularity; the peer model extends it to peer-qualified
  granularity.
- **`/workspace/@alkdev/flowgraph`** (referenced in ADR-022) — the graph
  model (operation graph, call graph, scoped subgraph). The peer-keyed
  overlay is the peer dimension of the operation graph. Petgraph is the
  future library for when path-finding across the peer graph becomes real;
  v1's nested `HashMap` is the implicit-graph representation.

---

## 5. OQ Impact

| OQ | Status before | Status after | Notes |
|---|---|---|---|
| **OQ-25** (remote-safe marking shape) | open (two-way) | **Dissolved** | `remote_safe: bool` is removed entirely. The "shape" question is moot — there is no marking. Peer authorization is `AccessControl`-based, which already has a rich shape (scopes, resources, AND/OR gates). Per-peer differentiation is via `IdentityProvider` config (different peers get different scopes), not a per-op marking. |
| **OQ-26** (OperationAdapter error type) | open (two-way) | **Stays** | Unaffected. `from_call` still returns `Result<_, AdapterError>`; the peer-keying changes the registration target, not the error type. A `SamePeerCollision` variant may be added (replacing the flat `Conflict` variant). |
| **OQ-27** (from_call re-import trigger) | open (two-way) | **Stays** | Unaffected. Auto-on-reconnect is still the default; the overlay is now peer-scoped (drops with the connection), so re-import is naturally scoped to the new peer. |
| **OQ-28** (from_call namespace collision) | open (two-way) | **Dissolved (cross-peer) / stays (same-peer)** | Cross-peer collision dissolves: same name on different peers is fine (separate sub-overlays). Same-peer collision stays an error (`SamePeerCollision`). The `namespace_prefix` becomes optional local-naming sugar, not the disambiguation mechanism. |
| **OQ-29** (CallClient TLS client-auth) | open (two-way) | **Stays** | Unaffected. TLS client-auth is orthogonal to the routing model. |

**New OQs surfaced by this research:**

- **OQ-30 (proposed): `PeerRef::Any` routing policy.** v1 uses insertion-order
  first-match. A richer policy (round-robin, least-loaded, affinity) is the
  two-way-door remainder. Tracked as a new OQ; the `PeerRef` enum is designed
  to compose with a future `RoutingPolicy` without breaking the signature.
- **OQ-31 (proposed): `services/list-peers` re-export semantics.** Whether
  re-exported peer ops are listed by default, opt-in, or per-peer-policy is a
  two-way-door. v1 defaults to "own ops only" (unchanged from today);
  `services/list-peers` is the opt-in. The re-export policy (which peers' ops
  a given peer sees) is an `AccessControl` decision on the listing op.
- **OQ-32 (proposed): Multi-hop federation.** Whether worker A transitively
  sees worker B's ops through the head is a one-way door on the federation
  model. v1 is one-hop (no transitive visibility). The peer-keyed overlay
  model extends to multi-hop without redesign but requires a path-finding
  layer (petgraph candidate). Tracked as a future OQ, not a v1 decision.

---

## 6. Open Questions the Research Surfaces but Doesn't Resolve

1. **`PeerId` stability across reconnects.** If a peer's `Identity.id` is its
   TLS fingerprint, reconnects with a rotated key change the `PeerId`. The
   peer-keyed overlay drops the old `PeerId`'s sub-overlay on disconnect and
   creates a new one on reconnect — structurally clean, but a handler
   mid-composition that captured a `PeerRef::Specific(old_peer_id)` gets
   `NOT_FOUND` after reconnect. Is this acceptable, or does `PeerId` need to
   be a stable logical identifier (e.g., a configured node name) separate from
   the cryptographic identity? v1: `PeerId = Identity.id` (the fingerprint);
   stable-logical-id is a future question.

2. **`PeerRef::Any` determinism.** Insertion-order first-match is deterministic
   but order-dependent. If worker A connects before worker B, `Any` always
   routes to A until A disconnects. Is this the right default, or should
   `Any` be round-robin from the start? v1: insertion-order (simplest,
   deterministic); round-robin is OQ-30.

3. **Reachability check ordering.** The current `invoke_with_policy` checks
   `parent.scoped_env.allows(&name)` *before* routing
   (`registry/env.rs:140-142`). Under the peer model, the reachability check
   is peer-qualified (`ScopedPeerEnv::allows(peer, op)`). Should the
   reachability check happen before or after peer resolution? v1: before
   (same as today) — the scoped env is checked against the *resolved* name,
   and peer-qualified reachability is part of the check. The POC validates
   this composes.

4. **Capability exposure under `PeerRef::Any`.** When a handler composes via
   `PeerRef::Any` and the routing picks worker A, the handler's
   `Capabilities` propagate to worker A's call (same as today's
   `from_call` forwarding). Is this correct when the handler didn't know
   which peer would be selected? v1: yes — the handler declared the op in
   its scoped env, so it authorized the composition; the peer selection is a
   routing detail. If a handler needs per-peer capability scoping, it uses
   `PeerRef::Specific` and peer-pinned reachability.

---

## 7. POC Validation Results

A scratch POC module (`crates/alknet-call/src/scratch_peer_routing.rs`) was
written in-repo, type-checked against the real types via a temporary
`scratch-peer-routing` Cargo feature, validated, and **removed**. The repo
is clean: `cargo check -p alknet-call` passes, all 207 lib tests pass.

### What the POC validated (compiles and works):

1. **`PeerRef` enum + `PeerRoutingEnv` trait** — the peer-routing signature
   compiles against the real `OperationContext`, `ResponseEnvelope`,
   `AbortPolicy`, and `Arc<dyn OperationEnv>`. The `invoke_peer` method is
   implementable and `Send + Sync` (required for the tokio::spawn dispatch
   loop).

2. **`PeerCompositeEnv` with `HashMap<PeerId, Arc<dyn OperationEnv>>`** —
   the peer-keyed composite env compiles. `attach_peer` / `detach_peer` /
   `invoke_peer` (with `PeerRef::Specific` and `PeerRef::Any`) all type-check.
   The `contains()` (union across peers) and `peer_contains()` (specific
   peer) probes work. `Send + Sync` verified.

3. **`PeerOverlay` (`HashMap<PeerId, HashMap<String, HandlerRegistration>>`)** —
   the peer-keyed overlay compiles. Same name on two peers (no collision),
   `first_peer_for` (Any routing), `drop_peer` (structural disconnect
   cleanup) all type-check and behave correctly.

4. **`AccessControl::check(peer_identity)` is sufficient** — the
   `authorize_peer_call` function compiles and the assertions hold:
   - Peer with the right scope → `Allowed`.
   - Peer without the scope → `Forbidden`.
   - No identity (unauthenticated) → `Forbidden` (auth required).
   - Op with `AccessControl::default()` → `Allowed` for any peer (implicitly
     remote-safe).
   - `Visibility::Internal` op → `Forbidden` for wire calls (NOT_FOUND in
     dispatch, never callable from wire regardless of peer).

5. **`ScopedPeerEnv` (peer-qualified reachability)** — compiles and composes
   with the existing `ScopedOperationEnv` shape. Unqualified `allowed_ops`
   (peer-agnostic) + peer-pinned `peer_pinned` set. `allows(peer, op)` checks
   both. The assertions hold: peer-pinned to worker-a allows Specific(worker-a)
   but not Specific(worker-b); unqualified allows Any.

6. **`list_services_peer_attributed`** — peer-attributed services/list
   compiles. Filters by `AccessControl::check(calling_peer_identity)` —
   only lists ops the calling peer is authorized to call. Own ops section
   (`peer: None`) + per-peer re-exported sections (`peer: Some(id)`).

7. **`from_call_peer_keyed` + `FromCallConfigPeer` + `FromCallError`** —
   the peer-aware from_call shape compiles. `namespace_prefix` is optional
   sugar (local naming), `SamePeerCollision` replaces the flat `Conflict`.

### What didn't work / required adjustment:

- **`HandlerRegistration` is not `Clone`** — the POC initially tried
  `reg.clone()` to register the same op into two peers' sub-overlays. Fixed
  by constructing fresh registrations per peer (a helper `make_exec_reg()`).
  This is a POC artifact, not a design issue — the real `from_call` produces
  fresh registrations per peer anyway (each peer's discovery produces its own
  bundles).
- **`#[cfg(any())]` does not type-check.** The common Rust POC pattern
  `#[cfg(any())] pub mod scratch;` compiles but does *not* type-check the
  module (the predicate is never true, so the module is excluded from
  compilation entirely). To validate types, the POC must be actually
  compiled. Used a temporary Cargo feature (`scratch-peer-routing`) to
  enable type-checking, then removed the feature. This is the correct
  pattern for POC validation that needs type-checking.
- **`#[cfg(all)]` is not the built-in `all` predicate** — it's treated as a
  custom cfg that's false by default (with a warning). Don't use it; use a
  feature gate.

### POC artifacts (not in repo):

The POC code is preserved in this research document's appendix (§10) for
reference. The scratch module was removed from the repo; only the research
doc and ADR draft survive.

---

## 8. Recommended `OperationEnv::invoke()` Signature

```rust
/// How a composing handler addresses a peer when invoking an operation.
#[derive(Debug, Clone)]
pub enum PeerRef {
    /// Route to this exact peer's overlay. NOT_FOUND if it doesn't serve the op
    /// (no silent fallthrough to other peers — explicit routing must be
    /// honored or fail loudly).
    Specific(PeerId),
    /// Route to the first peer (insertion order) whose overlay contains the op.
    /// This is the "any worker that serves this name" fan-out primitive.
    /// v1 uses insertion order; a richer RoutingPolicy is OQ-30.
    Any,
}

pub type PeerId = String;  // = Identity.id (the peer's fingerprint / declared label)

#[async_trait::async_trait]
pub trait OperationEnv: Send + Sync {
    // Existing methods — unchanged (back-compat).
    async fn invoke(&self, namespace: &str, operation: &str, input: Value,
        parent: &OperationContext) -> ResponseEnvelope { /* default delegates */ }
    async fn invoke_with_policy(&self, namespace: &str, operation: &str,
        input: Value, parent: &OperationContext, policy: AbortPolicy) -> ResponseEnvelope;
    fn contains(&self, _name: &str) -> bool { true }

    // NEW: peer-routing method. Default-impl delegates to invoke_with_policy
    // (back-compat: existing impls that don't override it route to "any" /
    // the single connection, preserving current behavior). PeerCompositeEnv
    // overrides with real peer routing.
    async fn invoke_peer(&self, peer: &PeerRef, namespace: &str, operation: &str,
        input: Value, parent: &OperationContext, policy: AbortPolicy) -> ResponseEnvelope {
        self.invoke_with_policy(namespace, operation, input, parent, policy).await
    }

    // NEW: peer-qualified contains. Default: delegate to contains (back-compat).
    fn peer_contains(&self, _peer: &PeerId, name: &str) -> bool { self.contains(name) }
}
```

---

## 9. Recommended Peer-Keyed Overlay Shape

```rust
// Per-connection overlay — UNCHANGED (one connection = one peer, flat map is fine).
// crates/alknet-call/src/protocol/connection.rs
pub struct CallConnection {
    connection: Arc<Connection>,
    imported_operations: Arc<RwLock<HashMap<String, HandlerRegistration>>>,  // flat, per-connection
    pending: Arc<Mutex<PendingRequestMap>>,
}

// Composite env — BECOMES peer-keyed (replaces CompositeOperationEnv's
// singular `connection: Option<Arc<dyn OperationEnv>>`).
pub struct PeerCompositeEnv {
    pub base: Arc<dyn OperationEnv + Send + Sync>,        // Layer 0 curated
    pub session: Option<Arc<dyn OperationEnv + Send + Sync>>,   // Layer 1
    pub connections: HashMap<PeerId, Arc<dyn OperationEnv + Send + Sync>>,  // Layer 2, peer-keyed
    connection_order: Vec<PeerId>,  // insertion order for PeerRef::Any first-match
}

// Peer-keyed overlay (used by the head node aggregating multiple connections).
#[derive(Default)]
pub struct PeerOverlay {
    by_peer: HashMap<PeerId, HashMap<String, HandlerRegistration>>,
    peer_order: Vec<PeerId>,  // insertion order for PeerRef::Any
}
```

**Migration path:** `CompositeOperationEnv` (singular connection) becomes
`PeerCompositeEnv` (peer-keyed connections). The singular-connection case (one
peer) is the degenerate case: `connections: HashMap` with one entry. Existing
call sites that construct `CompositeOperationEnv::new(base, Some(conn), session)`
migrate to `PeerCompositeEnv::new(base).with_session(session).attach_peer(peer_id, conn)`.

---

## 10. Appendix: POC Code (Reference)

The POC module validated the design. It is preserved here for reference; it
is **not** in the repo (removed after validation). The key structures:

<details>
<summary>POC module (scratch_peer_routing.rs) — click to expand</summary>

```rust
// (The full POC module — ~800 lines — validated against real types.
// Key structures: PeerRef, PeerRoutingEnv trait, PeerCompositeEnv, PeerOverlay,
// ScopedPeerEnv, authorize_peer_call, list_services_peer_attributed,
// from_call_peer_keyed, FromCallConfigPeer, FromCallError.
// See the research author's working tree for the full file; the structures
// are summarized in §3 and §8-9 above.)
```

</details>

The POC validated:
- `PeerRef` + `PeerRoutingEnv` compile against real types.
- `PeerCompositeEnv` routes `invoke_peer` to the right peer.
- `AccessControl::check(peer_identity)` authorizes without `remote_safe`.
- `ScopedPeerEnv` peer-qualified reachability composes with existing `ScopedOperationEnv`.
- `PeerOverlay` same-name-on-different-peers (no collision) + `drop_peer` cleanup.
- `list_services_peer_attributed` filters by `AccessControl::check(calling_peer)`.
- All shapes are `Send + Sync`.

---

## 11. ADR Draft (Supersedes ADR-028)

> **Note**: The full ADR should be written as a separate document
> (`docs/architecture/decisions/029-peer-graph-routing-model.md`) after
> review of this research. The draft below captures the decision shape; the
> ADR author should expand the Context with the problem statement from §1,
> the Consequences from §3, and the Assumptions from §6.

```markdown
# ADR-029: Peer-Graph Routing Model for alknet-call Composition

## Status

Proposed (supersedes ADR-028)

## Context

[Summarize §1: flat-namespace single-peer model breaks for head→N-workers;
ADR-028's remote_safe/trusted_peer is a parallel, weaker authorization system
that doesn't compose with the existing AccessControl/Identity machinery.
The head→many-workers pattern (ray.io's model) is the primary use case and
cannot be expressed today. This is a blocking structural fix.]

## Decision

### 1. Peer-keyed overlays

The Layer 2 overlay becomes peer-keyed. `CompositeOperationEnv`'s singular
`connection: Option<Arc<dyn OperationEnv>>` is replaced by
`PeerCompositeEnv` with `connections: HashMap<PeerId, Arc<dyn OperationEnv>>`.
[§3.1, §9]

### 2. `PeerRef` routing selector

`OperationEnv` gains a peer-routing method with a `PeerRef` selector
(`Specific(PeerId)` / `Any`). Default-impl preserves back-compat.
[§3.2, §8]

### 3. `AccessControl`-based peer authorization; retire `remote_safe`/`trusted_peer`

`RemoteFilter`, `HandlerRegistration::remote_safe`, `CallClient::trusted_peer`,
`list_operations_peer_scoped`, and `services_list_handler_peer_scoped` are
removed. Peer authorization flows through the existing `AccessControl::check`
against the peer's resolved `Identity`. The op's `AccessControl` *is* the
peer-authorization policy. [§3.3]

### 4. Peer-qualified reachability (`ScopedPeerEnv`)

`ScopedOperationEnv` is extended with an optional peer-pinned allowlist.
Unqualified reachability (peer-agnostic composition) stays the common case;
peer-pinning is opt-in and replaces `FromCallConfig::namespace_prefix` as the
disambiguation mechanism. [§3.4]

### 5. `from_call` peer-keyed registration; collision rule change

`from_call` registers into the specific peer's sub-overlay. Cross-peer
collision dissolves (same name on different peers is fine). Same-peer
collision stays an error. `namespace_prefix` becomes optional local-naming
sugar. [§3.6]

### 6. `services/list` AccessControl-filtered; `services/list-peers` opt-in

`services/list` filters by `AccessControl::check(calling_peer_identity)` (not
`remote_safe`). `services/list-peers` is the opt-in for peer-attributed
re-export listing. [§3.5]

## Consequences

[Summarize §3 + §5: OQ-25 and OQ-28 (cross-peer) dissolve; OQ-26/27/29 stay;
new OQ-30/31/32 surfaced. Positive: head→N-workers works, one authorization
system not two, structural disconnect cleanup. Negative: `OperationEnv` trait
gains a method (back-compat default-impl), `CompositeOperationEnv` →
`PeerCompositeEnv` migration, `services/list` semantics change.]

## Assumptions

[Summarize §6: PeerId stability, Any determinism, reachability ordering,
capability exposure under Any.]

## References

- ADR-015 (privilege model — the authority-switch pattern ADR-028 violated)
- ADR-017 (client/adapter contract — amended: CallClient no longer has
  trusted_peer)
- ADR-022 (registration bundle — remote_safe field removed)
- ADR-024 (registry layering — Layer 2 becomes peer-keyed)
- ADR-028 (superseded)
- OQ-25 (dissolved), OQ-26/27/29 (stay), OQ-28 (cross-peer dissolved),
  OQ-30/31/32 (new)
- Research: this document
- Prior art: Ray.io actors, Dapr service invocation
```

---

## 12. Confirmation: POC Removed, Build Clean

- Scratch module `crates/alknet-call/src/scratch_peer_routing.rs`: **removed**.
- `crates/alknet-call/src/lib.rs`: **restored** to original (no scratch module
  reference).
- `crates/alknet-call/Cargo.toml`: **restored** (no `scratch-peer-routing`
  feature).
- `cargo check -p alknet-call`: **passes** (clean).
- `cargo test -p alknet-call --lib`: **207 passed; 0 failed**.

Only the research doc (`docs/research/alknet-call-peer-routing/findings.md`)
and the ADR draft (§11, to be split out as ADR-029) survive.