# ADR-028: Peer-Scoped Registry Filtering for CallClient Inbound Dispatch

## Status

**Superseded** by [ADR-029](029-peer-graph-routing-model.md) (2026-06-27).

ADR-028 introduced `remote_safe: bool` and `trusted_peer: bool` as a parallel
authorization system for peer-scoped dispatch. This was a structural miss: the
flat-namespace single-peer model it built on cannot express the head→N-workers
pattern (the primary use case), and the parallel `remote_safe`/`trusted_peer`
gate duplicates the existing `AccessControl`/`Identity` machinery (which
already authorizes peer calls) while reintroducing the blanket-bypass
anti-pattern ADR-015 was written to kill. ADR-029 replaces the flat overlay
with peer-keyed overlays + `PeerRef` routing, and retires `remote_safe`/
`trusted_peer` in favor of the existing `AccessControl::check(peer_identity)`.
See ADR-029 for the design that replaces this one; see
`docs/research/alknet-call-peer-routing/findings.md` for the research that
identified the gap.

## Context

ADR-017 §1 established that a `CallClient` — which opens an outbound
`alknet/call` connection — "has its own operation registry to dispatch incoming
calls from the remote side." The ADR left the *registry scope* as an explicit
two-way door in its Consequences:

> Sharing the global registry with a `CallClient` exposes local capabilities to
> the remote peer… A peer-scoped subset must filter by capability
> remote-safety, not just operation name. The registry-mechanism choice
> (share global vs subset vs separate) is two-way mechanically but has a
> security dimension post-ADR-022: the "share global" option is a
> capability-exposure decision, not just a dispatch decision.

This is the one decision identified in
`docs/research/alknet-call-completion/gap-analysis.md` (DC-1) that must be
locked before `CallClient` can be implemented correctly. It is a **one-way door
on the security dimension**: the choice of default determines what a remote peer
can reach, and a wrong default silently exposes outbound credentials.

### Why this is a one-way door, not a two-way door

The gap analysis framed the *mechanism* (share-global vs subset vs separate
registry instance) as a two-way door, and that framing holds. But the
**existence of peer-scoped filtering as the v1 default** is one-way, because:

1. Once a downstream consumer (the runner pattern, the container service, the
   NAPI projection) is written against the "remote peer can call any
   `External` op and the local node's capabilities will be populated for it"
   semantics, switching the default to default-deny is a breaking change for
   every consumer. The container-service rewrite at `/workspace/@alkdev/dispatch`
   and the dev/runner patterns are the first consumers; the default is set
   before they're written, so it's still cheap to set correctly — but only now.

2. The security dimension is asymmetric in ADR-009 terms. "Share global" leaks
   silently: there is no error, no log line, no test that fails — the remote
   peer simply receives a populated `OperationContext.capabilities` drawn from
   the local `HandlerRegistration.capabilities`, and the local node's API keys
   get used for the remote peer's call. The reversal cost is "discover which
   consumers quietly depend on the leak and re-audit." Default-deny fails
   loudly (the remote peer's call to an unexposed op returns `NOT_FOUND`),
   which is the cheaper failure mode.

3. ADR-014's invariant — "no handler reads outbound credentials from any
   source other than `OperationContext.capabilities`" — combined with
   ADR-022's dispatch path (which populates `capabilities` from the
   registration bundle) means the registration bundle *is* the exposure
   boundary. Whatever the `CallClient` dispatches determines which
   `Capabilities` objects cross to the remote peer's call context. Filtering
   the registry is filtering capability exposure.

### The runner/dispatch pattern is the primary use case, and it is semi-trusted

The canonical consumer (gap analysis §"Exchange of Operations"): a container
service / dev runner connects *outward* to a hub and exposes `/container/exec`,
`/container/list`, etc. The hub then calls back into the runner. Both sides
are semi-trusted peers, not extensions of self. Exposing every `External`
operation on the runner — including any operation that carries an outbound
API key the runner happens to hold — is wrong by default. The operator who
*does* want full bilateral sharing is making an explicit trust decision.

## Decision

### 1. Default-deny: a CallClient exposes no operations to the remote peer unless explicitly marked remote-safe

The `CallClient` does not share the global `OperationRegistry` by default. It
holds a **peer-scoped subset**: a filtered view containing only
`HandlerRegistration`s that are explicitly marked as remote-safe for this peer.

The *existence* of filtering is the one-way door; this ADR locks it.

### 2. The remote-safe marking lives on the registration bundle, not on capabilities

The marking is added to `HandlerRegistration` (per ADR-022, the registration
bundle) as a peer-exposure field. It is not placed on `Capabilities` entries,
because:

- `Capabilities` is a flat credential bag; marking individual entries
  remote-safe conflates "this credential is safe to send over the wire" with
  "this operation may be dispatched on behalf of a remote peer." Those are
  different questions — an operation may be remote-safe while using a
  credential that must never leave the node, and the dispatch path already
  keeps `Capabilities` off the wire (ADR-014). The exposure question is about
  *which ops dispatch*, not *which credentials are serializable*.
- The registration bundle is already the integration point for provenance,
  composition authority, scoped env, and visibility (ADR-022). Peer-exposure is
  a property of the same shape: a dispatch-path concern set at registration.

The exact shape of the marking (a boolean, a per-peer allowlist, a
capability-class tag) is the two-way-door remainder — tracked as OQ-25, not
decided here. v1 uses the simplest shape that supports default-deny: a boolean
`remote_safe: bool` on `HandlerRegistration`, defaulting to `false`.

### 3. "Share the global registry" remains available as an explicit opt-in

A `CallClient` may be constructed in "trusted-peer" mode that exposes all
`External` operations from the global registry regardless of the remote-safe
marking. This is the explicit-allow path for operators who have made the trust
decision (e.g., two nodes under single administrative control, a test harness).
It is opt-in, never the default.

### 4. Provenance-based defaults

The remote-safe marking has a provenance-aware default at registration time,
before the operator's explicit choice:

| Provenance | Default `remote_safe` |
|-----------|----------------------|
| `Local` | `false` — assembly-written ops are not remote-callable unless the operator says so |
| `Session` | `false` — agent-written ops are sandboxed (ADR-015); exposing them to a remote peer would widen the sandbox |
| `FromOpenAPI`, `FromMCP`, `FromCall`, `FromJsonSchema` | `false` — leaves are composition material, not wire-callable (ADR-015) |

`false` across the board as the default. The operator flips specific
operations to `true` when they want this peer to reach them. This is the same
default-deny posture as ADR-015's visibility (`Internal` by default) and
ADR-022's composition authority (`None` for leaves by default).

### 5. The filtering is a dispatch-time read, not a copy

The `CallClient`'s peer-scoped view is not a second copy of the registry. It
is a dispatch-time read against the global registry, gated by the remote-safe
marking (and the trusted-peer flag). This keeps the curated layer (Layer 0,
ADR-024) single-source — the global registry is still the one Layer-0 store
built by the assembly layer at startup. Only the *visibility* to the remote
peer is filtered.

This avoids a third registry instance (the "separate registry per CallClient"
option from DC-1) and avoids the staleness problem a copied subset would
introduce: if the assembly layer reloads a curated op's spec, the peer-scoped
view reflects it on the next dispatch, not on the next copy.

## Consequences

**Positive:**
- The default is safe-by-construction for the runner/dispatch pattern. A
  container service that connects outward to a hub cannot accidentally expose
  its local vault-derived API keys to the hub's calls.
- The one-way security door is locked before any consumer is written against
  the leaky default. The container-service rewrite and the dev/runner patterns
  implement against default-deny from day one.
- Failure mode is loud: a remote peer calling an unexposed op gets
  `NOT_FOUND`, not silent credential exposure.
- The mechanism is additive. Trusted-peer opt-in preserves the "share global"
  path for operators who want it, without making it the default.
- Single-source Layer 0: no copied registry, no staleness.

**Negative:**
- Adds one field (`remote_safe: bool`) to `HandlerRegistration` (ADR-022).
  The registration bundle grows. This is the smallest shape that supports
  default-deny; OQ-25 may replace it with a richer mechanism (per-peer
  allowlists, capability-class tags).
- Operators must explicitly mark operations remote-safe for bilateral
  exchange. This is friction, deliberately: the bilateral container-service
  pattern requires the operator to declare which of the runner's ops the hub
  may call back into.
- The remote-safe marking is a v1 mechanism and may be superseded. OQ-25
  tracks the shape; a future ADR may amend or supersede this one without
  revisiting the *existence* of filtering.
- The trusted-peer opt-in is a sharp tool. An operator who enables it for the
  wrong peer gets the "share global" exposure this ADR exists to prevent.
  The opt-in is documented as a trust decision, not as a convenience.

## Assumptions

1. **The remote-safe marking is set at registration time, not at connection
   time.** The marking is a property of the operation (per-peer in a richer
   shape, but at least a boolean in v1), set by the assembly layer when it
   builds the registry. Per-connection overrides are not part of v1; if a
   deployment needs different exposure per peer, it uses the richer shape
   (OQ-25) or multiple `CallClient`s with different filtered views.

2. **The peer-scoped view filters dispatch, not `services/list` semantics.**
   The remote peer discovers operations via `services/list` (ADR-017 §3),
   which already filters by `Visibility::External` (ADR-015). The remote-safe
   marking is an *additional* filter for the dispatch path: an op may be
   `External` yet not remote-safe. In v1, `services/list` served to a
   `CallClient` peer **hides** non-remote-safe ops — a peer should not see
   ops it cannot call, so discovery and dispatch filters agree. (The
   pre-filter mental model — "`External` appears in `services/list`, then
   the dispatch path returns `NOT_FOUND` for non-remote-safe" — is *not* the
   v1 behavior; v1 hides them from listing too.) Whether a richer shape
   (OQ-25) should expose-but-deny instead of hide is a two-way-door detail
   tracked in OQ-25.

3. **Filtering is per-`CallClient`, not global.** A node with multiple
   outbound connections may expose different subsets to different peers. The
   v1 boolean marking limits this to "remote-safe for any peer" vs "not"; the
   richer OQ-25 shape is what enables per-peer differentiation. v1's
   limitation is acceptable because the runner/dispatch pattern has one
   remote peer per `CallClient`.

## References

- ADR-009: One-Way Door Decision Framework (the door-type framing this ADR
  relies on)
- ADR-014: Secret Material Flow and Capability Injection (the no-env-vars
  invariant this ADR's security argument rests on)
- ADR-015: Privilege Model and Authority Context (the default-`Internal`,
  default-deny posture this ADR mirrors)
- ADR-017: Call Protocol Client and Adapter Contract (§1 Consequences flagged
  this decision; §1 is amended by this ADR)
- ADR-022: Handler Registration, Provenance, and Composition Authority (the
  registration bundle this ADR adds a field to)
- ADR-024: Operation Registry Layering (Layer 0 single-source; the peer-scoped
  view is a dispatch-time read, not a copy)
- OQ-25: Remote-safe marking shape (the two-way-door remainder)
- `docs/research/alknet-call-completion/gap-analysis.md` — DC-1
- `docs/architecture/crates/call/client-and-adapters.md` — the spec this ADR
  informs