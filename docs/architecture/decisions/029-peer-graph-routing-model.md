# ADR-029: Peer-Graph Routing Model for alknet-call Composition

## Status

Accepted (supersedes ADR-028; Assumption 1's `PeerId` source is superseded
by ADR-030 on the source dimension — the one-way door is preserved)

## Context

The call protocol's composition model is **flat per overlay and single-peer**.
`CompositeOperationEnv` holds one `connection: Option<Arc<dyn OperationEnv>>`
overlay; the Layer 2 imported-ops overlay on `CallConnection` is a flat
`HashMap<String, HandlerRegistration>` keyed by operation name. This works for
one remote peer. The head→many-workers / hub→spoke pattern (the ray.io model,
and the primary downstream use case — the container-service rewrite this
completion was supposed to unblock) cannot be expressed:

1. **Overlay collision.** A head importing from worker A and worker B, both
   exposing `/container/exec`, has no way to route
   `invoke("container", "exec")` to the right peer. The composite env holds
   one connection overlay; even with two, `contains("container/exec")` is
   true for both with no disambiguation.

2. **`from_call` namespace prefix is a naming-convention hack.** DC-3 / OQ-28
   made `FromCallConfig::namespace_prefix` the disambiguation mechanism — the
   operator prefixes imported op names so two peers' ops don't collide in a
   flat map. This pushes disambiguation to the caller and into the
   `ScopedOperationEnv { allowed: HashSet<String> }` reachability list. It is
   bolted onto a flat map instead of being structural routing.

3. **ADR-028's `remote_safe: bool` + `trusted_peer: bool` is a second,
   parallel, weaker authorization system.** ADR-028 introduced a
   `RemoteFilter { trusted_peer: bool }` gate in `protocol/dispatch.rs` that
   runs *before* the existing `AccessControl::check`.
   `trusted_peer: true` is a blanket security-bypass flag — the exact
   anti-pattern ADR-015 was written to kill (it replaced `trusted: true` with
   the authority-switch model). ADR-028 reintroduced it at the peer boundary.
   The existing authorization machinery in core (`Identity` with scopes and
   resources, `IdentityProvider`, `AccessControl::check`) is real, grounded,
   and already wired into the dispatch path — ADR-028 should have *used* it for
   peer authorization, not invented a parallel system.

This is a blocking structural fix, not a "v1/later" refinement. The research
at `docs/research/alknet-call-peer-routing/findings.md` validates the design
through a POC that type-checks against the real types (since removed; the
shapes are recorded in the research doc). ADR-028 is superseded by this ADR.

## Decision

### 1. Peer-keyed overlays

The Layer 2 overlay becomes peer-keyed at the composition-env level.
`CompositeOperationEnv`'s singular `connection: Option<Arc<dyn OperationEnv>>`
is replaced by `PeerCompositeEnv` with peer-keyed connections:

```rust
pub struct PeerCompositeEnv {
    pub base: Arc<dyn OperationEnv + Send + Sync>,       // Layer 0 curated
    pub session: Option<Arc<dyn OperationEnv + Send + Sync>>,  // Layer 1
    pub connections: HashMap<PeerId, Arc<dyn OperationEnv + Send + Sync>>,  // Layer 2, peer-keyed
    connection_order: Vec<PeerId>,  // insertion order for PeerRef::Any first-match
}
```

The per-`CallConnection` overlay stays flat (one connection = one peer — a
flat `HashMap<String, HandlerRegistration>` per connection is correct). The
peer-keying is at the *aggregation* layer: the head node's composition env
holds a `HashMap<PeerId, connection_overlay>`, not one overlay. `PeerId` is
the peer's `Identity.id` — the same field `Connection::identity()` already
exposes, already resolved in the dispatch path, and already unique per peer.

### 2. `PeerRef` routing selector

`OperationEnv` gains a peer-routing method with a `PeerRef` selector. The
default-impl preserves back-compat (existing impls that don't override it
delegate to `invoke_with_policy`, preserving current behavior):

```rust
pub enum PeerRef {
    Specific(PeerId),  // route to this peer; NOT_FOUND if it doesn't serve the op
    Any,               // first peer (insertion order) that serves it
}
pub type PeerId = String;  // logical id, NOT Identity.id — see OQ-33

async fn invoke_peer(&self, peer: &PeerRef, namespace: &str, operation: &str,
    input: Value, parent: &OperationContext, policy: AbortPolicy) -> ResponseEnvelope {
    // default: ignore peer selector, dispatch via invoke_with_policy
    self.invoke_with_policy(namespace, operation, input, parent, policy).await
}
fn peer_contains(&self, _peer: &PeerId, name: &str) -> bool { self.contains(name) }
```

`PeerRef::Specific(PeerId)` routes to the named peer's overlay; if that peer
doesn't serve the op, `NOT_FOUND` (no silent fallthrough — explicit routing
must be honored or fail loudly). `PeerRef::Any` routes to the first peer
(insertion order) whose overlay contains the op — the "any worker that serves
this name" fan-out primitive. A richer `RoutingPolicy` (round-robin,
least-loaded) is the two-way-door remainder tracked as OQ-30; the `PeerRef`
enum is designed to compose with it without breaking the signature.

The existing `invoke()` / `invoke_with_policy()` methods stay as the
`PeerRef::Any` equivalent for code that doesn't care about peer selection.

### 3. `AccessControl`-based peer authorization; retire `remote_safe`/`trusted_peer`

`RemoteFilter`, `HandlerRegistration::remote_safe`,
`CallClient::trusted_peer`, `OperationRegistry::list_operations_peer_scoped`,
and `services_list_handler_peer_scoped` are **removed**. Peer authorization
flows through the existing `AccessControl::check` against the peer's resolved
`Identity`:

- A remote peer's call arrives → `dispatch_requested` resolves the peer's
  `Identity` (already does, from the connection's TLS fingerprint or the
  `auth_token` payload) → `OperationRegistry::invoke` runs
  `AccessControl::check(peer_identity)`.
- If the op's `AccessControl` is satisfied → dispatch (capabilities populated
  from the bundle, same as today).
- If not → `FORBIDDEN` (capabilities never populated — the security property
  ADR-028 wanted, achieved by the existing ACL, not a parallel gate).
- If the op is `Visibility::Internal` → `NOT_FOUND` before ACL (existing
  behavior). This is the "never callable from wire" case.

The three cases `remote_safe` was meant to handle map to existing mechanisms:

| `remote_safe` case | Replacement |
|---|---|
| Op callable by any peer (was `remote_safe: true`) | `AccessControl::default()` — no restrictions; implicitly "remote-safe" because it requires no privileged scope. |
| Op callable only by some peers | `AccessControl { required_scopes: [...] }` — only peers whose `Identity.scopes` satisfy the AND-gate may call. Per-peer differentiation via `IdentityProvider` config. |
| Op never callable from wire | `Visibility::Internal` — `NOT_FOUND` before ACL. Existing mechanism, unchanged. |

**The op's `AccessControl` *is* the peer-authorization policy.** There is no
separate exposure decision. If the peer's `Identity` satisfies the op's
`AccessControl`, the op dispatches and capabilities populate (same as for any
authorized caller). If not, `FORBIDDEN` before the handler — capabilities
never populate. The exposure decision and the authorization decision are the
same decision, made through one mechanism, not two.

### 4. Peer-qualified reachability (`ScopedPeerEnv`)

`ScopedOperationEnv { allowed: HashSet<String> }` is extended with an optional
peer-pinned allowlist. Unqualified reachability (peer-agnostic composition —
"I want to call `container/exec` on whichever worker serves it") stays the
common case; peer-pinning is opt-in for the disambiguation case that replaces
`FromCallConfig::namespace_prefix`:

```rust
pub struct ScopedPeerEnv {
    pub allowed_ops: HashSet<String>,    // peer-agnostic — reachable via PeerRef::Any
    pub peer_pinned: HashSet<String>,    // "peer-id/op-name" — reachable only via PeerRef::Specific(that peer)
}
```

Instead of prefixing the *op name* (the flat-namespace hack), you pin the
*peer* in the reachability set. The existing `ScopedOperationEnv.allowed`
becomes the `allowed_ops` field; peer-pinning is additive.

### 5. `from_call` peer-keyed registration; collision rule change

`from_call` registers into the specific peer's sub-overlay, not a flat
overlay. Cross-peer collision dissolves: same name on different peers is fine
(separate sub-overlays, no collision, no prefix needed). Same-peer collision
stays an error (a peer shouldn't expose two ops with the same name).

`FromCallConfig::namespace_prefix` becomes optional local-naming sugar for
the case where the importing node wants to expose a peer's ops under a
different name *locally* — a local-naming concern, not a disambiguation
concern. It defaults to `None`.

### 6. `services/list` `AccessControl`-filtered; `services/list-peers` opt-in

`services/list` filters by `AccessControl::check(calling_peer_identity)` — the
calling peer sees only ops it is authorized to call. The
`services_list_handler` / `services_list_handler_peer_scoped` split collapses
to a single `AccessControl`-filtered handler. `services/list-peers` is the
opt-in for peer-attributed re-export listing (each peer's sub-overlay listed
with attribution, filtered by the calling peer's authorization).

## Consequences

**Positive:**
- The head→N-workers pattern works. A head with multiple worker connections
  routes `invoke()` to the right peer via `PeerRef`. This is the primary use
  case the previous model couldn't express.
- One authorization system, not two. Peer authorization flows through the
  existing `AccessControl`/`Identity` machinery — the same mechanism that
  gates every other call. No parallel `remote_safe` gate, no blanket-bypass
  `trusted_peer` flag. Per-peer differentiation is via `IdentityProvider`
  config (different peers get different scopes), which is a real
  authorization decision, not a boolean.
- Structural disconnect cleanup. When a peer disconnects, its sub-overlay
  drops (the `PeerId` key is removed from `connections`). No stale overlay,
  no explicit deregistration. An in-flight `PeerRef::Specific(that_peer)` gets
  `NOT_FOUND` — the correct failure mode.
- `from_call` collision dissolves across peers. Two workers exposing
  `/container/exec` coexist; the prefix is no longer the disambiguation
  mechanism.
- The `OperationEnv` trait gains a method with a default-impl, preserving
  back-compat. Existing impls (`LocalOperationEnv`, `OverlayOperationEnv`)
  work unchanged; `PeerCompositeEnv` overrides with real peer routing.
- The peer-keyed overlay model extends naturally to multi-hop federation (a
  chain of `PeerRef::Specific` routing decisions) without redesign. Petgraph
  is not needed for v1 (one-hop, shallow); it pays off if multi-hop
  path-finding becomes real (OQ-32).

**Negative:**
- `CompositeOperationEnv` → `PeerCompositeEnv` is a migration. Existing call
  sites that construct `CompositeOperationEnv::new(base, Some(conn), session)`
  migrate to `PeerCompositeEnv::new(base).with_session(session).attach_peer(peer_id, conn)`.
  The singular-connection case (one peer) is the degenerate case
  (`connections` with one entry).
- `OperationEnv` trait gains a method. The default-impl preserves back-compat,
  but it's a trait surface change; downstream impls (`alknet-http`,
  `alknet-agent`) gain the method with the default delegation.
- `services/list` semantics change: the filter is `AccessControl`-based, not
  `remote_safe`-based. An op with `AccessControl::default()` (no restrictions)
  is now listed to any peer — this is correct (it's implicitly callable by
  any authenticated peer), but operators who relied on `remote_safe: false` to
  hide ops from peers must instead set `required_scopes` or `Visibility::Internal`.
- ADR-028 is superseded. The `remote_safe` field, `trusted_peer` flag,
  `RemoteFilter`, `list_operations_peer_scoped`, and
  `services_list_handler_peer_scoped` are removed. Code that references them
  (the `CallClient`, `Dispatcher`, `HandlerRegistration`, `discovery.rs`)
  changes. This is the cost of fixing a one-way-door miss — the previous model
  shipped and was reviewed before the structural gap was caught.
- `PeerId` is a logical identifier, **not** `Identity.id` (the fingerprint or
  API-key prefix). Coupling `PeerId` to the crypto material would break every
  in-flight `PeerRef::Specific` and every ACL entry referencing that peer on
  key rotation. v1 uses a connection-assigned UUID; a configured node name is
  the future shape. See OQ-33 for the full decision and the key-rotation/ACL
  rationale.

## Assumptions

1. **`PeerId` is a logical identifier, not `Identity.id`.** v1 source is a
   connection-assigned UUID (v4) — stable for the connection's lifetime,
   changes on reconnect. This is a no-storage workaround: the core crates are
   deliberately DB-free (smaller, fewer deps), which works for local-only
   state but not for cross-node peer identity that wants to persist across
   restarts and key rotations. An in-flight `PeerRef::Specific(stale_uuid)`
   gets `NOT_FOUND` on reconnect — the correct failure mode (the peer is
   gone); re-`from_call` produces a fresh `PeerRef`. The real solution (a
   persistent peer registry that maps a stable logical name to current crypto
   material, surviving key rotation) is tracked as OQ-34, not a v1 blocker.
   The one-way door: `PeerId` is logical, not crypto — this determines the
   `PeerCompositeEnv` key type and `PeerRef::Specific` payload. See OQ-33.

   > **Superseded by ADR-030 on the `PeerId` source dimension.** The
   > one-way door (`PeerId` is logical, not crypto) is preserved. The v1
   > UUID source is replaced by `Identity.id` from `PeerEntry.peer_id`
   > (stable across key rotation). The "no-storage workaround" framing is
   > no longer accurate — the storage boundary is now `config + in-memory
   > adapter` (ADR-030 + ADR-033), with persistence adapters additive. See
   > ADR-030 and OQ-33 (resolved).

2. **`PeerRef::Any` = insertion-order first-match.** Deterministic but
   order-dependent (worker A connects before worker B → `Any` routes to A
   until A disconnects). This is the simplest routing policy and is correct for
   the immediate use case (the head picks the first worker that serves the
   op). A richer `RoutingPolicy` (round-robin, least-loaded, affinity) is OQ-30;
   the `PeerRef` enum composes with it without breaking the signature.

3. **`services/list` defaults to "own ops only" (unchanged from today).**
   Re-exported peer ops are not listed unless the calling peer invokes
   `services/list-peers` (the opt-in). The re-export policy (which peers' ops a
   given peer sees) is an `AccessControl` decision on the listing op.

4. **Capability exposure under `PeerRef::Any`.** When a handler composes via
   `Any` and routing picks worker A, the handler's `Capabilities` propagate to
   worker A's call (same as today's `from_call` forwarding). This is correct:
   the handler declared the op in its scoped env, so it authorized the
   composition; the peer selection is a routing detail. If a handler needs
   per-peer capability scoping, it uses `PeerRef::Specific` and peer-pinned
   reachability.

5. **Multi-hop federation is out of scope for v1.** Worker A does not
   transitively see worker B's ops through the head unless the head explicitly
   re-exports them. The peer-keyed overlay model extends to multi-hop without
   redesign (a chain of `PeerRef::Specific` decisions), but path-finding
   (which peer reaches which op transitively) is where petgraph would pay off
   (OQ-32, not designed).

## References

- ADR-015: Privilege Model and Authority Context (the authority-switch pattern
  ADR-028 violated by reintroducing a blanket-bypass flag)
- ADR-017: Call Protocol Client and Adapter Contract (amended: `CallClient`
  no longer has `trusted_peer`; the client/adapter spec updates)
- ADR-022: Handler Registration, Provenance, and Composition Authority
  (`remote_safe` field removed from the registration bundle)
- ADR-024: Operation Registry Layering (Layer 2 becomes peer-keyed at the
  composition-env aggregation level)
- ADR-028: Peer-Scoped Registry Filtering for CallClient Inbound Dispatch
  (superseded)
- OQ-25: dissolved (no `remote_safe` marking — `AccessControl` is the policy)
- OQ-26: resolved (`AdapterError` variants — `SamePeerCollision` replaces
  the flat `Conflict` variant; `#[non_exhaustive]`)
- OQ-27: stays (re-import trigger — unchanged; the overlay is now peer-scoped)
- OQ-28: dissolved cross-peer (same name on different peers is fine); stays
  same-peer
- OQ-29: stays (TLS client-auth — orthogonal to the routing model)
- OQ-30: `PeerRef::Any` routing policy (new — round-robin/least-loaded)
- OQ-31: `services/list-peers` re-export semantics (new)
- OQ-32: Multi-hop federation (new — petgraph candidate)
- OQ-33: resolved — `PeerId` is a logical id (UUID v1), not `Identity.id`;
  decoupling from crypto material keeps the door open for key-rotation-safe ACLs
- OQ-34: persistent peer registry (new — the storage dimension OQ-33 surfaced;
  not a v1 blocker, tracked so the no-DB posture's limit is deliberate)
- Research: `docs/research/alknet-call-peer-routing/findings.md`
- Prior art: Ray.io actors (`ActorHandle` = `PeerRef::Specific`), Dapr service
  invocation (app-ID routing = `PeerRef::Specific`, access-control allowlist =
  `AccessControl`-based peer authorization)