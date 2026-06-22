# ADR-024: Operation Registry Layering

## Status

Accepted

## Context

The architecture has two registries that the spec documents previously treated
as sharing one immutability argument:

1. **The endpoint's `HandlerRegistry`** (ALPN string → `ProtocolHandler`).
   This is what ADR-010 and OQ-04 are about. Its immutability is load-bearing:
   ALPN strings are baked into the TLS `ServerConfig` at startup, so adding a
   protocol handler at runtime requires rebuilding the TLS config. This is a
   genuine one-way door and the rationale is correct.

2. **The call protocol's `OperationRegistry`** (operation name →
   `HandlerRegistration`). This lives *inside* the `CallAdapter`, which is one
   `ProtocolHandler` behind the single ALPN `alknet/call`. Adding an operation
   to the `OperationRegistry` does **not** touch the TLS `ServerConfig` — the
   ALPN is already `alknet/call`, registered once at startup.

`operation-registry.md` stated the operation registry "is immutable after
construction… consistent with OQ-04 and ADR-010." That inheritance was by
analogy, not by shared rationale. The TLS argument that justifies
`HandlerRegistry` immutability does not apply to the `OperationRegistry`. The
operation registry's mutability profile is a separate question, and it has been
answered incorrectly by inheriting a constraint that belongs to a different
registry.

### Why `from_call` breaks the inherited constraint

The import adapters have different lifecycle requirements:

- **`from_openapi` / `from_mcp`** can run at startup — the assembly layer reads
  a static spec file or queries a known service before the registry is frozen.
  Static import, fits immutability.
- **`from_call`** requires a **live connection** to discover operations
  (`services/list` + `services/schema`). Connections happen at runtime.
  Workers join and leave dynamically in the machine→worker topology. You
  cannot pre-freeze a set you discover over a connection you haven't opened
  yet.

So `from_call` is structurally incompatible with "frozen at startup, never
touched again." The pre-ADR-024 spec held two contradictory positions: the
registry is immutable (operation-registry.md), and `from_call` imports remote
operations at connection time (ADR-017). An implementer would have to resolve
the contradiction by guessing — likely by either forcing all `from_call`
imports to happen at startup (awkward, doesn't fit worker topologies) or
quietly making the registry mutable (undermining the stated constraint without
acknowledging it).

### Why immutability is not the load-bearing security control for imported ops

Imported operations (`FromOpenAPI`, `FromMCP`, `FromCall`) are leaves — they
cannot compose (ADR-022 Assumption 5). They have no composition authority, no
scoped env, `Internal` visibility by default, and their trust model is "the
remote endpoint is trusted as much as my own handlers" (ADR-017). Their
reachability from a composing handler is bounded by the *parent handler's*
scoped env, not by their registration timing.

The security controls on imported ops are **provenance** and **composition
authority** — both set at registration, both checked at dispatch. Immutability
is redundant here. An imported op registered at runtime is no more or less
privileged than one registered at startup; it's a forwarding stub either way,
and its capacity to do harm is bounded by what the *composing parent*'s
authority and scoped env permit.

Immutability *is* load-bearing for **curated** operations — the `Local` ops
the assembly layer writes at startup, which *can* compose and therefore *can*
escalate privilege under their own authority. For those, the trust boundary is
"the assembly layer declared them at startup," and immutability is what locks
that declaration. But that's a constraint on `Local` provenance specifically,
not on the registry as a whole.

### The trust-boundary principle

The right axis is not visibility (`Internal` vs `External`) or wire-vs-local —
it is **provenance combined with import timing**, which maps to where each
operation's trust decision is made:

| Provenance | Import timing | Trust boundary | Layer | Lifetime |
|-----------|---------------|----------------|-------|----------|
| `Local` | Startup | Assembly layer at startup | 0 (curated) | Process — immutable |
| `Session` | Sandbox creation | Composing handler at sandbox creation | 1 (session) | Session — dynamic |
| `FromCall` | Connection (runtime) | Remote node at connection time | 2 (connection) | Connection — dynamic |
| `FromOpenAPI` / `FromMCP` | Startup | External endpoint, discovered at startup | 0 (curated) | Process — immutable |
| `FromOpenAPI` / `FromMCP` | Runtime (rare) | External endpoint, discovered at runtime | 2 (discovery) | Discovery-scoped — dynamic |

`FromOpenAPI` / `FromMCP` provenance is **layer-polymorphic**: the same
provenance lands in Layer 0 (immutable) or Layer 2 (dynamic) depending on
when the import happens. The common case is startup import into Layer 0
(Decision 6); runtime import into Layer 2 is permitted but rare.

**Immutability follows the trust boundary.** Operations are mutable at the
scope where their trust decision is made. `Local` ops (and startup-imported
`FromOpenAPI`/`FromMCP`) are trusted at startup → immutable. Session ops
are trusted at sandbox creation → session-scoped dynamic. `FromCall` ops
(and runtime-imported `FromOpenAPI`/`FromMCP`) are trusted at
connection/discovery time → connection/runtime dynamic.

Session ops are the edge case that proves the rule: they are `Internal`
visibility and can compose, but their trust boundary is per-session (the
parent handler grants them restricted authority at sandbox creation, per
ADR-022 Assumption 6), not per-startup. Visibility alone would misclassify
them; provenance correctly identifies them as dynamic.

### The precedent: `IdentityProvider`

The structural problem — *N consumers need to resolve something from M
sources, don't globalize the sources into one pot, don't make each consumer
know about all sources* — is the same problem `IdentityProvider` solves for
auth (ADR-004). An `IdentityProvider` is a trait (`Arc<dyn IdentityProvider>`)
that centralizes resolution policy behind a stable interface; source
composition is an impl detail. Handlers consume the result; the trait owns the
routing.

`OperationEnv` is the same problem one layer over: *N handlers need to
dispatch to operations, operations come from M sources (curated local, this
session, this peer connection, that peer connection), don't globalize all
sources into one mutable pot, don't make each handler know about all sources
and pick the right registry.* The solution is the same shape: a trait —
`Arc<dyn OperationEnv>` — that centralizes dispatch routing behind a stable
interface, with overlay composition as an impl detail.

The alternative — a single global `ArcSwap<OperationRegistry>` into which all
imported ops merge with namespace prefixes — is the registry equivalent of
"every handler reads identity from a global env var." It works at one
connection. At many connections it produces: an unbounded pot, namespace
collisions scaling with connection count, disconnect cleanup requiring a
reverse index (op → owning connection), zero source isolation, and
routing-by-naming-convention instead of routing-by-structure. That is the
failure mode the `IdentityProvider` pattern exists to prevent.

## Decision

### 1. The operation registry is layered by trust boundary

The `OperationRegistry` is not a single flat map. It is a layered structure
where each layer corresponds to a trust boundary:

```
Layer 0 — Curated (static, immutable, startup trust boundary)
  Local provenance operations from the assembly layer.
  Registered once at startup, never mutated for the process lifetime.
  This is where immutability is load-bearing: these ops can compose,
  therefore can escalate privilege under their own authority. The
  startup trust boundary + immutability is the security control.

Layer 1 — Session (dynamic, per-session, sandbox-creation trust boundary)
  Session provenance operations, agent-written, sandboxed.
  Created and destroyed with each session.
  Already specified by OQ-19 as an overlay on Layer 0.

Layer 2 — Imported (dynamic, per-connection, peer trust boundary)
  FromCall operations discovered when a peer connects.
  FromOpenAPI / FromMCP operations when imported at runtime (rare;
  usually at startup into Layer 0, but runtime import is permitted).
  Created and destroyed with the connection / discovery event.
```

Layers 1 and 2 are the same shape: **per-scope dynamic overlays on the static
curated base.** The scope is "session" for Layer 1 and "connection" (or
"discovery event") for Layer 2. OQ-19 already specified the overlay mechanism
for Layer 1 (session env wraps global env via `OperationEnv` trait layering).
This ADR generalizes the same mechanism to Layer 2.

### 2. The `OperationEnv` trait is the integration point

`OperationContext.env` is `Arc<dyn OperationEnv + Send + Sync>` — a trait
object, not a concrete struct. This is required by the overlay model: a
composite env (curated base + connection overlay + session overlay) is built
by composing `OperationEnv` impls, not by merging registries.

This resolves review #002 finding C6 (`OperationContext.env` type identity
crisis). The pre-ADR-024 spec had `env: OperationEnv` (a trait, which can't
be a field without `dyn`) and used the same field as both a reachability set
(`parent.env.allows()`) and a dispatch trait (`context.env.invoke()`). One
field cannot be both. The split:

- `scoped_env: ScopedOperationEnv` — reachability data. Populated from the
  registration bundle's `scoped_env` (ADR-022). The reachability check in
  `invoke()` consults `parent.scoped_env.allows(&name)`.
- `env: Arc<dyn OperationEnv + Send + Sync>` — dispatch trait. The handler
  calls `context.env.invoke(...)`; the trait impl routes to the right
  overlay.

This is the `IdentityProvider`-shaped integration point: handlers consume
the trait; source composition is an impl detail.

### 3. The `CallAdapter` composes the root env per incoming call

When a `call.requested` arrives over connection C, the `CallAdapter` does
not look up the operation in a single global registry. It composes the root
`OperationContext.env` from the layers active for this call:

```
root env = CompositeOperationEnv {
    base:       curated_registry_env,       // Layer 0 — static
    connection: C.imported_operations,       // Layer 2 — this connection's overlay
    session:    active_session_overlay,      // Layer 1 — if a session is active
}
```

The composite impl checks overlays in order (session first, then connection,
then curated base) and dispatches to the first match. This is structural
source binding: a handler composing `worker/exec` reaches it via the
connection overlay that contains it, not via a naming convention in a
global pot.

**Env inheritance through composition**: the child's `env` is
`parent.env.clone()` — an `Arc::clone`, not a re-composition. Overlay
composition happens once at the root (in `build_root_context`) and
propagates by `Arc` through the composition tree. A child handler sees the
same active overlays its parent saw. This is deliberate: re-composing per
`invoke()` would re-resolve overlays on every dispatch and would break the
session-overlay case (a session that was active when the parent ran must
still be active for the child, even if the session ended mid-composition —
the child is part of the same call tree the parent started). The root env
is composed per incoming call; nested calls inherit it by `Arc::clone`.

When connection C disconnects, its overlay is dropped. Operations imported
from C vanish from the reachable set with no global mutation and no reverse
index. Handlers that try to compose a now-gone op receive `NOT_FOUND` (if
the overlay was already dropped when `invoke()` runs the reachability
check) or a connection error with code `INTERNAL` (if the call was
dispatched to the forwarding handler and the connection drops mid-flight).
Both cases are clean failures — no stale-handler-binds-to-dead-connection
hazard.

### 4. Curated operations remain immutable; imported and session ops are dynamic

The blanket immutability claim in `operation-registry.md` is replaced by:

- **Layer 0 (curated, `Local`)**: immutable after startup. The
  `OperationRegistry` holding curated ops is constructed once by the
  assembly layer and never mutated. This is where the security argument for
  immutability applies: composing ops are privileged, the startup trust
  boundary is where that privilege is granted, immutability locks it.
- **Layer 1 (session, `Session`)**: dynamic, per-session. Created at sandbox
  creation, destroyed at session end. Already specified by OQ-19.
- **Layer 2 (imported, `FromCall` etc.)**: dynamic, per-connection. Created
  when a peer connection completes `from_call` discovery, destroyed when the
  connection closes.

Adding a `Local` op at runtime is not supported — it would require re-entering
the startup trust boundary, which is a deployment (restart), not a runtime
operation. This preserves the security property ADR-010/OQ-04 were concerned
with, scoped to where it actually applies.

### 5. `from_call` imports into the connection's overlay, not the global registry

The `from_call` adapter (ADR-017) discovers operations on a remote peer and
produces `HandlerRegistration` bundles. Under ADR-024, those bundles are
registered into the **connection's overlay**, not a global mutable registry.

```rust
// On CallConnection establishment:
let imported = from_call(&connection, config).await;
connection.imported_operations.extend(imported);
// The connection's env now includes these ops.
```

The handler closures produced by `from_call` capture the `CallConnection` —
when the connection drops, the handlers become unreachable (their env is
dropped), and any in-flight calls to them return connection errors. This is
the natural lifecycle; no explicit deregistration is needed.

### 6. `from_openapi` and `from_mcp` default to startup import into Layer 0

For the common case — the assembly layer imports a static OpenAPI spec or
connects to a known MCP server at startup — `from_openapi` / `from_mcp`
register into the curated (Layer 0) registry, which is then frozen. This
preserves the pre-ADR-024 behavior for the case where it was correct.

Runtime `from_openapi` / `from_mcp` import (e.g., discovering an MCP server
at connection time) is permitted and follows the Layer 2 model — the imported
ops live in a connection/discovery-scoped overlay. This is additive and
does not affect the startup-import path.

### 7. OQ-04 scope clarification and OQ-19 generalization

This ADR amends OQ-04 to scope its immutability claim to the
**`HandlerRegistry`** (ALPN-level, ADR-010). The `OperationRegistry`'s
mutability profile is now governed by this ADR: curated (Layer 0) is
immutable; session and imported layers are dynamic at their trust-boundary
scopes. See the OQ-04 amendment in `open-questions.md`.

This ADR generalizes OQ-19's session-overlay mechanism to also cover
connection-scoped remote imports. Both are per-scope dynamic overlays on the
static curated base, composed into the per-call `OperationContext.env` by
the `CallAdapter`. `OperationEnv` being a trait object is what enables
both. See the OQ-19 resolution update in `open-questions.md`.

## Consequences

**Positive:**

- `from_call` has a coherent home. Imported ops live with the connection
  that produced them, appear when the connection is established, and
  disappear when it closes. No contradiction with immutability, no awkward
  "import everything at startup" workaround.
- The immutability argument is now correctly scoped. Layer 0 (curated,
  composing ops) is immutable because that's where the security control
  applies. Layers 1 and 2 are dynamic because their trust boundaries are
  per-scope. An implementer reading the spec sees the right constraint in
  the right place, instead of a blanket claim that doesn't fit all cases.
- The `OperationEnv`-as-trait constraint (OQ-19) is now required by the
  overlay model, not just by the session-overlay pattern. The same
  mechanism (trait layering) supports both session overlays and connection
  overlays — one pattern, two scopes. This makes C6's resolution
  (`env: Arc<dyn OperationEnv>`) structurally motivated, not just a
  type-system cleanup.
- Disconnect handling is structural. A connection drops → its overlay drops
  → its ops vanish from the reachable set. No `ArcSwap` coordination, no
  reverse index from op to owning connection, no stale handlers bound to a
  dead connection. This is the same lifecycle property session overlays
  already have (session ends → session overlay drops).
- Source isolation is structural. Imported ops from peer X are only
  reachable from handlers whose `OperationEnv` is wired to X's overlay.
  They are not globally callable. A handler that shouldn't be able to
  reach peer X's ops simply doesn't have X's overlay in its env. This is
  better hygiene than a global registry with namespace prefixes, where
  every handler sees every imported op and isolation is a naming
  convention.
- The `IdentityProvider` precedent makes the design legible. A future
  reader sees "trait-object integration point, source composition as impl
  detail" and recognizes the pattern; they don't have to re-derive why
  trait-composed overlays were chosen over a global mutable registry.

**Negative:**

- The dispatch path is a composite lookup (session → connection → curated)
  rather than a single `HashMap` lookup. This is a small constant cost —
  three hash lookups in the worst case instead of one — and the overlays are
  small (a session's ops, a connection's imported ops). The common case
  (composing a curated op) hits Layer 0 after two empty-overlay misses, which
  is a predictable and cache-friendly path. The cost is justified by the
  source isolation and lifecycle properties it buys.
- `OperationContext.env` is now `Arc<dyn OperationEnv + Send + Sync>`, which
  is a trait object with dynamic dispatch. This is the same cost as
  `Arc<dyn IdentityProvider>` — a vtable call per `invoke()`. Negligible
  relative to the work an operation does, and the same pattern the codebase
  already uses for auth.
- The `CallAdapter` has more responsibility: it composes the root env per
  call from the active layers, rather than handing every call the same
  global registry. This is expected — the CallAdapter is the integration
  point for the call protocol, and per-call env composition is the same
  shape as per-call identity resolution (which the CallAdapter already does
  via `IdentityProvider`).
- Naming across overlays: if two connections import ops with the same name
  (e.g., both peers expose `worker/exec`), the composite env dispatches to
  the first overlay that contains the name. This is the same ambiguity
  `FromCallConfig`'s namespace prefix (ADR-017) was designed to address —
  the caller disambiguates with a prefix at import time. ADR-024 does not
  change this; it makes the disambiguation structural (which overlay is in
  the env) rather than nominal (which prefix is in the name).
- The blanket immutability claim in `operation-registry.md` and the
  cross-references that inherit it (the "Two-way door —
  `ArcSwap<OperationRegistry>` can be added later" note, OQ-04's framing)
  must be updated. This is a spec edit, not a migration — no implementation
  exists yet.

**On review #002 findings resolved by this ADR:**

- **C6** (`OperationContext.env` type identity crisis): resolved by Decision 2.
  The field is split into `scoped_env` (reachability data) and `env` (dispatch
  trait object). The split is structurally motivated by the overlay model,
  not just a type-system cleanup.
- **W4** (hot-swap ↔ registry mutability coupling): localized to the
  connection scope. There is no global mutable registry to hot-swap.
  Overlays are per-scope and replace naturally with connect/disconnect and
  session start/end. The schema-drift hazard (a peer re-runs
  `services/list` on reconnect and re-imports with a changed schema) moves
  from global to per-connection — it does not vanish. A handler
  mid-composition whose peer reconnects with a changed schema sees the old
  schema until the overlay is rebuilt. This is a per-connection concern,
  not a global one; the guard clause the review asked for becomes a note on
  overlay rebuild semantics rather than a global hot-swap protocol.
- **W3** (CallClient registry security dimension): partially addressed. The
  *registry-shape* sub-question is resolved by the overlay model — a
  `CallClient`'s incoming-call dispatch uses the same overlay composition,
  and sharing the curated base with a remote peer is fine (curated ops are
  trusted). The *capability-exposure* sub-question (a remote peer calling
  `/llm/generate` uses the local node's API key) is **not resolved by this
  ADR** — it is a separate concern about what capabilities a remote peer
  can trigger, and it is unaffected by the registry shape. That sub-question
  remains open for ADR-017 (a guard-clause note: a peer-scoped subset must
  filter by capability remote-safety, not just operation name). ADR-024
  resolves the dispatch shape; ADR-017 retains the capability-exposure
  decision.

## Assumptions

1. **Provenance is knowable at registration time and stable for the
   registration's lifetime.** A `Local` op does not become `FromCall` later;
   a `FromCall` op does not become `Local`. If a remote-imported op is later
   "promoted" to curated, that's a re-registration at the next startup
   (deployment), not a runtime mutation. Inherited from ADR-022 Assumption 2.

2. **Layer 0 immutability is the security control for composing ops.** The
   pre-ADR-024 blanket immutability claim was overbroad but not wrong about
   `Local` ops. Curated composing ops must be immutable because the startup
   trust boundary is where their authority is granted. This ADR narrows the
   claim, it does not remove it.

3. **Imported and session ops do not need immutability as a security
   control for privilege escalation.** Their security against privilege
   escalation is bounded by provenance (no composition authority → no
   privilege escalation) and by the parent handler's scoped env
   (reachability control). This is the central argument; if it's wrong —
   if a `from_call` op can escalate in some way provenance + scoped env
   don't bound — the model needs revisiting. **Immutability is not the
   control for non-escalation threats** (availability, schema drift):
   availability is bounded by per-handler timeouts (ADR-016) and the
   connection's overlay being drop-on-disconnect; schema drift on
   reconnect is a per-connection overlay-rebuild concern (see W4 in
   Consequences), not a global-registry-mutation concern. The point of
   scoping immutability to Layer 0 is that immutability is the right
   control *for composing ops* and the wrong control *for non-composing
   ops*; it is not a claim that non-composing ops face no threats.

4. **A connection's overlay is the right scope for `from_call` imports.**
   Operations discovered from peer X are reachable from handlers whose env
   includes X's overlay. If a use case requires imported ops to be globally
   reachable (every handler sees every peer's ops), the composite env can be
   built to include all active connection overlays — but the default is
   per-connection scoping for isolation.

5. **Disconnect → overlay drop → op vanishes is acceptable behavior.** A
   handler composing an op whose peer has disconnected receives `NOT_FOUND`
   (or a connection error if the in-flight call was mid-dispatch). This is
   the same behavior as a peer that never exposed the op. If a use case
   requires disconnected-peer ops to remain reachable (e.g., cached results),
   that's a handler-level caching concern, not a registry concern.

6. **The root env is composed per incoming call, not cached per
   connection.** The active session overlay can change during a connection's
   lifetime (a session starts or ends mid-connection), so the env cannot be
   composed once at connection establishment and reused. `build_root_context`
   runs per `call.requested` and composes the env from the layers active at
   that moment. The cost (constructing an `Arc<CompositeOperationEnv>` per
   call) is negligible — it's three `Arc::clone`s, not three registry
   traversals.

7. **Session-overlay attachment is an agent-crate concern.** ADR-024
   generalizes OQ-19's session overlay to also cover connection overlays,
   but the mechanism by which a session overlay attaches to a given wire
   call (session ID in metadata, payload field, connection-bound session
   state, etc.) is not specified here. The `CallAdapter` is wired with an
   optional session-overlay source by the assembly layer; the lookup
   mechanism belongs to the agent crate spec (OQ-19: "the agent-specific
   mechanism belongs to the agent crate spec"). If a wire call has no
   active session, the root env is `curated base + connection overlay`
   (no session layer).

## References

- ADR-010: ALPN router and endpoint (the `HandlerRegistry` immutability
  argument — this ADR clarifies that it applies to the ALPN registry, not
  the operation registry)
- ADR-014: Secret material flow and capability injection (capabilities are
  per-`HandlerRegistration` bundle, not per-registry — the overlay model
  doesn't change how capabilities flow; an imported op's capabilities come
  from its bundle, which for `from_call` is whatever the assembly layer
  granted the import)
- ADR-017: Call protocol client and adapter contract (`from_call` adapter;
  the `FromCallConfig` namespace prefix is the disambiguation mechanism this
  ADR's overlay model uses structurally)
- ADR-022: Handler registration, provenance, and composition authority
  (provenance is the axis this ADR's layering is based on; the
  `HandlerRegistration` bundle shape is unchanged)
- ADR-004: Auth as shared core (`IdentityProvider` — the precedent for the
  trait-object integration point pattern this ADR applies to `OperationEnv`)
- OQ-04: Dynamic handler registration (this ADR amends OQ-04 to scope it to
  the `HandlerRegistry`; the operation registry's mutability is now governed
  by ADR-024)
- OQ-19: Session-scoped operation registries (this ADR generalizes the
  session-overlay mechanism to connection overlays — same pattern, two
  scopes)
- docs/reviews/002-pre-implementation-architecture-sanity-check.md
  (findings C6, W3, W4 — resolved by this ADR)