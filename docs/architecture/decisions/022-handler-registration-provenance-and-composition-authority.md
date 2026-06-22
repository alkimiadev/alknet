# ADR-022: Handler Registration, Provenance, and Composition Authority

## Status

Accepted

## Context

ADR-015 established the privilege model: the `internal` flag marks
composition-originated calls and switches the ACL from the caller's identity
to the handler's identity. This replaces the old `trusted: bool` flag, which
skipped ACL entirely — a privilege escalation vector. The core decision in
ADR-015 is sound: internal calls switch authority, they don't skip ACL.

However, ADR-015 left three things unspecified, which the pre-implementation
review (docs/reviews/001-pre-implementation-architecture-sanity-check.md,
findings C1–C4) identified as critical gaps:

1. **`handler_identity` has no registration path.** ADR-015 says the handler's
   identity is "set at registration by the assembly layer" (Assumption 2) and
   that "ACL check runs against the handler's identity (set at registration)"
   (Decision 1). But the registration API shown in operation-registry.md —
   `register(spec, handler)` and `OperationRegistryBuilder::with(spec,
   handler)` — accepts no identity. Tracing the dispatch path reveals that
   `build_root_context` sets `handler_identity: None` for wire calls (correct
   for the root), and `OperationEnv::invoke()` propagates
   `parent.handler_identity.clone()` to children. Since the root's
   `handler_identity` is `None`, every internal call gets `handler_identity:
   None` — meaning ADR-015's "ACL runs against `handler_identity` for internal
   calls" checks against `None`, which is the privilege-escalation gap ADR-015
   was written to close.

2. **The scoped composition env has no registration/construction path.**
   ADR-015 says the `OperationEnv` given to a handler is "scoped — it can
   only invoke a declared set of operations, set at registration by the
   assembly layer" (Decision 4, Assumption 3). But `register(spec, handler)`
   takes no scoped-env declaration, `OperationSpec` has no field for it, and
   the only `OperationEnv` implementation shown is `LocalOperationEnv` wrapping
   the *full* registry — no scoping layer exists.

3. **`Capabilities` lives in two unconnected models.** ADR-014 and
   operation-registry.md show two models for how a handler gets outbound
   credentials: construction-time capture in the handler closure (Model A) and
   per-request on `OperationContext.capabilities` propagated through
   composition (Model B). The two don't connect: if the handler closure
   captured capabilities at construction, `OperationContext.capabilities` is
   either redundant or must be populated from the closure — but the closure
   receives the context, it isn't passed it. An implementer would have to
   invent the bridge, and the consuming crates (call, agent, napi) could
   diverge.

Beyond these wiring gaps, there is a deeper issue with ADR-015's Assumption 6:
"the handler identity is a full `Identity` (with scopes), not a special
principal type." `Identity` was designed for **inbound peer identity** — who
is calling me from the network. A handler is not a peer. Its `id` field would
be something like `"agent-chat-handler"` — a label, not something resolvable
through `IdentityProvider`. Calling it an `Identity` implies it's a peer,
which it isn't. It's an authority bundle.

### The kernel/user analogy

This is structurally the same problem an operating system solves with
kernel/user mode:

- User calls `getaddrinfo()` — the syscall gate (an **External** op). The
  kernel checks the user's capabilities at entry.
- `getaddrinfo` internally makes DNS queries, allocates sockets, reads
  `/etc/hosts` — **Internal** kernel functions. They don't check the user's
  `CAP_NET_RAW`. They run under **kernel authority**.
- The user does NOT need `CAP_NET_RAW` to resolve DNS. The kernel does network
  access on the user's behalf, under the kernel's own authority.

The key principle: **the user's authority is checked once at the gate. Inside,
the handler runs under its own authority. The user's authority does not
propagate into internal calls.**

This is exactly what ADR-015 specifies. The `internal` flag is the boundary
crossing. When `internal: true`, ACL switches from the caller's identity to
the handler's composition authority. The user's `[chat]` scope got them through
`/agent/chat`'s External ACL. Once inside, it's `/agent/chat`'s composition
authority that authorizes composing `/vastai/listMachines` — not the user's.

### The graph framing

Call trees and operation registries are graph-shaped. The TypeScript
`@alkdev/flowgraph` package models this explicitly with three graphs:

1. **Operation Graph** (static) — nodes are registered operations, edges are
   type-compatibility relationships. Built from `OperationSpec`s at startup.
2. **Call Graph** (dynamic) — nodes are call invocations (request IDs), edges
   are parent-child relationships (`parent_request_id`). Built from call
   protocol events at runtime.
3. **Scoped Operation Subgraph** (per-handler, static) — the declared subset
   of the operation graph that a handler may reach. This is what ADR-015 calls
   the "scoped env," framed as a subgraph rather than a list of names.

This ADR uses the graph *model* as structural framing but does not mandate a
graph *library*. For v1, the operation graph can be implicit (a
`HashMap<String, OperationNode>`), the call graph can be implicit (the
`PendingRequestMap` indexed by `parent_request_id` *is* a call graph), and the
scoped env can be a `HashSet<String>` of reachable operation names. A
dedicated `alknet-flowgraph` crate (or folding graph structures into
`alknet-call`) is a future enhancement for workflow templates, type
compatibility validation, and call-graph observability — not a prerequisite
for the security model.

## Decision

### 1. Provenance is the primary registration axis

Every registered operation carries a provenance tag that classifies where it
came from. Provenance determines whether the operation can compose, whether it
has composition authority, its default visibility, and its trust model.

```rust
pub enum OperationProvenance {
    /// Assembly-written, trusted code, can compose.
    Local,
    /// HTTP forwarding stub (from_openapi), leaf — cannot compose.
    FromOpenAPI,
    /// MCP forwarding stub (from_mcp), leaf — cannot compose.
    FromMCP,
    /// QUIC forwarding stub (from_call). Leaf in the local registry —
    /// forwards calls to a remote node; cannot compose locally.
    FromCall,
    /// JSON Schema definition (from_jsonschema), no handler — schema only.
    FromJsonSchema,
    /// Agent-written, sandboxed, can compose within sandbox bounds.
    Session,
}
```

| Provenance | Can compose? | Has composition authority? | Default visibility | Trust model |
|-----------|-------------|---------------------------|-------------------|-------------|
| `Local` | Yes | Yes — scopes set by assembly layer | External or Internal (assembly declares) | Trusted code |
| `FromOpenAPI` | No (leaf) | No | Internal | HTTP endpoint trusted; handler is a forwarding stub |
| `FromMCP` | No (leaf) | No | Internal | MCP server trusted; handler is a forwarding stub |
| `FromCall` | No (leaf in local registry) | No | Internal | Remote node trusted; handler is a forwarding stub |
| `FromJsonSchema` | N/A (no handler) | No | N/A | N/A |
| `Session` | Yes (within sandbox) | Yes — scopes set by assembly layer at sandbox creation | Internal always | Untrusted code in sandbox |

Only `Local` and `Session` ops get composition authority. Leaves
(`FromOpenAPI`, `FromMCP`, `FromCall`) don't compose, so they don't get one.
The assembly layer does not invent identities for leaves.

### 2. Composition authority replaces `handler_identity: Identity`

ADR-015's Assumption 6 said "the handler identity is a full `Identity` (with
scopes), not a special principal type." This ADR refines that: composition
authority is a declared authority bundle, not a peer `Identity`. It's only set
for ops that can compose (`Local`, `Session`). Leaves don't have one.

```rust
/// Authority under which a handler composes child operations.
///
/// This is NOT a peer `Identity` — it's not resolvable through
/// `IdentityProvider` and doesn't represent an inbound caller. It's the
/// declared authority (scopes + resources + label) that the assembly layer
/// grants a handler for composition. When the handler composes children via
/// `OperationEnv::invoke()`, the child's ACL runs against this authority,
/// not the caller's identity and not as a blanket skip.
///
/// Only ops that can compose (`Local`, `Session`) have one. Leaves
/// (`FromOpenAPI`, `FromMCP`, `FromCall`) have `None`.
pub struct CompositionAuthority {
    /// Human-readable label for attribution and logging
    /// (e.g., "agent-chat", "fs-handler"). Not a peer id — not resolvable
    /// through IdentityProvider.
    pub label: String,

    /// Scopes the handler operates under for composition. When the handler
    /// composes a child via `env.invoke()`, the child's ACL checks against
    /// these scopes. Least privilege: the assembly layer grants only the
    /// scopes the handler needs for its declared composition.
    pub scopes: Vec<String>,

    /// Named resource lists, same shape as `Identity.resources`. Optional.
    /// e.g., {"service": ["vastai", "github"]} bounds which services the
    /// handler can reach in composition.
    pub resources: HashMap<String, Vec<String>>,
}

impl CompositionAuthority {
    /// `None` — for leaves that don't compose (convenience for
    /// `composition_authority: CompositionAuthority::none()`).
    pub fn none() -> Option<Self> { None }

    /// Construct a composition authority with the given label and scopes.
    pub fn new(
        label: &str,
        scopes: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            label: label.to_string(),
            scopes: scopes.into_iter().collect(),
            resources: HashMap::new(),
        }
    }
}
```

This supersedes ADR-015's Assumption 6. ADR-015's core decision (authority
switch, not ACL skip) holds unchanged — the only change is *what* the
authority is and which ops have it.

### 3. The scoped env is a declared subgraph (reachability control)

The scoped composition env from ADR-015 is the **reachability control**: it
bounds which operations a handler can reach via `env.invoke()`. ADR-015
specifies it as "a declared set of operations, set at registration by the
assembly layer." This ADR makes the registration path explicit and frames it
as a subgraph of the operation graph.

```rust
/// The set of operations a handler may reach via `env.invoke()`.
///
/// This is the reachability control from ADR-015: a handler (or an LLM
/// picking tools, or a quickjs sandbox) can only compose declared operations,
/// not the entire registry. Set at registration by the assembly layer for
/// composing ops (`Local`, `Session`). `None` for leaves — they don't
/// compose, so they get an empty/no-op env.
///
/// Conceptually a subgraph of the operation graph. For v1, implemented as a
/// set of operation names — the *model* is a subgraph (which nodes this
/// handler can reach), but type-compatibility edges between those nodes are
/// a future enhancement for static validation, not a v1 requirement.
pub struct ScopedOperationEnv {
    /// Operation names this handler may compose (e.g., {"fs/readFile",
    /// "vastai/listMachines"}). `env.invoke()` for any name not in this set
    /// returns NOT_FOUND. This is the reachability boundary — it bounds the
    /// parameterized-dispatch attack surface.
    pub allowed_operations: HashSet<String>,
}

impl ScopedOperationEnv {
    /// Empty set — for leaves that don't compose (no reachable operations).
    pub fn empty() -> Self {
        Self { allowed_operations: HashSet::new() }
    }

    /// Construct from an iterable of operation names.
    pub fn new(ops: impl IntoIterator<Item = String>) -> Self {
        Self { allowed_operations: ops.into_iter().collect() }
    }

    /// Returns true if the given operation name is reachable.
    pub fn allows(&self, name: &str) -> bool {
        self.allowed_operations.contains(name)
    }
}
```

### 4. The registration bundle carries all three

The three controls from ADR-015 (visibility, composition authority, scoped
env) plus the capability injection from ADR-014 all enter the system at the
same boundary: the assembly layer hands the registry a `(spec, handler)` pair
*plus* the handler's runtime context material. This ADR makes that explicit
as a registration bundle.

```rust
pub struct HandlerRegistration {
    pub spec: OperationSpec,
    pub handler: Handler,
    pub provenance: OperationProvenance,
    /// Composition authority for this handler. `None` for leaves
    /// (`FromOpenAPI`, `FromMCP`, `FromCall`) — they don't compose.
    /// `Some(...)` for `Local` and `Session` ops that can compose children.
    pub composition_authority: Option<CompositionAuthority>,
    /// Scoped composition env. `None` for leaves — they get an empty
    /// no-op env. `Some(...)` for composing ops.
    pub scoped_env: Option<ScopedOperationEnv>,
    /// Outbound credentials the handler may use (decrypted API keys, signing
    /// keys, HTTP tokens). Populated by the assembly layer from the vault
    /// at handler construction. See ADR-014.
    pub capabilities: Capabilities,
}
```

The registry's `register` and builder's `with` accept a `HandlerRegistration`,
not a bare `(OperationSpec, Handler)` pair:

```rust
impl OperationRegistry {
    pub fn register(&mut self, registration: HandlerRegistration);
}

impl OperationRegistryBuilder {
    pub fn with(mut self, registration: HandlerRegistration) -> Self;
}
```

Adapter convenience methods (`from_openapi`, `from_mcp`, `from_call`)
construct `HandlerRegistration` with `composition_authority: None` and
`scoped_env: None` for the leaf ops they produce — the adapter doesn't grant
composition authority, and the assembly layer doesn't have to invent values
for leaves.

### 5. The dispatch path reads from the registration bundle

The CallAdapter's `build_root_context` and `OperationEnv::invoke()` read
composition authority, scoped env, and capabilities from the registration
bundle, looked up by operation name.

**`build_root_context` (wire-originated call, `internal: false`):**

```rust
fn build_root_context(
    &self,
    request_id: String,
    operation_name: &str,        // looked up in registry
    identity: Option<Identity>,  // resolved per-request from AuthContext/auth_token
) -> OperationContext {
    let registration = self.registry.registration(operation_name);
    OperationContext {
        request_id,
        parent_request_id: None,
        identity,                           // caller's identity (inbound — gate credential)
        handler_identity: registration.composition_authority,  // C1: from bundle, None for leaves
        capabilities: registration.capabilities.clone(),       // C3: from bundle
        metadata: HashMap::new(),
        env: registration.scoped_env.clone()
            .unwrap_or_else(ScopedOperationEnv::empty),       // C2: from bundle, empty for leaves
        internal: false,                    // wire call — ACL against caller identity
    }
}
```

ACL for the root checks against `identity` (the caller's identity, resolved
per-request). `handler_identity` is on the context for *propagation* to
children, not for the root's own ACL.

**`OperationEnv::invoke()` (composition-originated call, `internal: true`):**

```rust
async fn invoke(&self, namespace: &str, operation: &str, input: Value,
                parent: &OperationContext) -> ResponseEnvelope {
    let name = format!("{namespace}/{operation}");

    // Reachability check (C2): is this op in the parent's scoped env?
    // If not, return NOT_FOUND. This is the reachability control.
    if !parent.env.allows(&name) {
        return ResponseEnvelope::not_found(name);
    }

    let registration = self.registry.registration(&name);
    let context = OperationContext {
        request_id: generate_request_id(),
        parent_request_id: Some(parent.request_id.clone()),
        identity: parent.handler_identity_as_identity(),  // parent's authority becomes the caller
        handler_identity: registration.composition_authority.clone(),  // C1: child's own authority
        capabilities: parent.capabilities.clone(),       // C3: propagate through composition
        metadata: HashMap::new(),                        // fresh — does NOT propagate (ADR-014)
        env: registration.scoped_env.clone()
            .unwrap_or_else(ScopedOperationEnv::empty),   // C2: child's own scoped env
        internal: true,                                    // composition — ACL against handler_identity
    };
    self.registry.invoke(&name, input, context).await
}
```

Two things happen here:

1. **Reachability check**: before constructing the child context, `invoke()`
   checks whether the requested op is in the parent's scoped env. If not,
   `NOT_FOUND`. This bounds the parameterized-dispatch attack surface — a
   handler (or an LLM picking tools) can only reach declared ops.

2. **Authority propagation**: the child's `identity` is the parent's
   `handler_identity` (the parent's composition authority becomes the caller
   for the child). The child's `handler_identity` is the *child's own*
   registration's `composition_authority` — so if the child itself composes
   further, its children inherit the child's authority. This is the
   principal/agent chain from ADR-015, now wired.

ACL for the child checks against `handler_identity` (the child's composition
authority). For leaves, `handler_identity` is `None` — but leaves don't
compose, so their `handler_identity` is never used for ACL on a grandchild.
Leaves only have ACL checked against *themselves* (as the target of
composition), where the check is: does the parent's composition authority
satisfy the leaf's `AccessControl`?

### 6. Capabilities are per-request, populated from the bundle (Model A reconciled)

This ADR resolves the C3 ambiguity by adopting option (a) from the review:
capabilities are only per-request on `OperationContext`, populated by the
dispatch path from the per-handler capabilities in the registration bundle.
The construction-time "baking" described in ADR-014 L82 populates the
registration bundle's `capabilities` field — the handler closure does not
capture capabilities.

```rust
// Assembly layer: construct registration with capabilities from vault
let google_api_key = vault.decrypt(&google_key_blob)?;
let agent_registration = HandlerRegistration {
    spec: agent_chat_spec(),
    handler: Arc::new(agent_chat_handler),  // closure captures nothing
    provenance: OperationProvenance::Local,
    composition_authority: Some(CompositionAuthority {
        label: "agent-chat".into(),
        scopes: vec!["llm:call".into(), "fs:read".into(), "vastai:query".into()],
        resources: HashMap::new(),
    }),
    scoped_env: Some(ScopedOperationEnv {
        allowed_operations: HashSet::from(["fs/readFile".into(), "vastai/listMachines".into(),
                                           "llm/generate".into()]),
    }),
    capabilities: Capabilities::new()
        .with_api_key("google", google_api_key),  // C3: in the bundle, not the closure
};
```

The handler reads `context.capabilities` at call time. The dispatch path
populates it from `registration.capabilities`. Composition propagates it via
`parent.capabilities.clone()` in `invoke()`. No circular dependency, no
redundant models.

### 7. The three controls together (ADR-015's model, now wired)

| Control | What it gates | Where it's set | Without it |
|---------|--------------|----------------|-----------|
| Visibility (External/Internal) | Whether the op is callable from the wire | `OperationSpec.visibility` | Internal ops exposed to external callers |
| Composition authority | What authority internal calls run under | `HandlerRegistration.composition_authority` | ACL skipped or caller's scopes propagated (escalation) |
| Scoped env | What ops a handler can reach | `HandlerRegistration.scoped_env` | Handler can call anything in the registry (confused deputy) |

All three enter at registration. All three reach the dispatch path via the
registration bundle. The user's identity is the **gate credential** — checked
once at the External boundary. The composition authority is the **internal
credential** — used for all composition inside. The scoped env is the
**reachability boundary** — what the handler can even attempt to compose.

### 8. No intersection semantics

The user's authority does NOT limit internal calls. If the user has `chat` but
not `vastai:query`, `/agent/chat` composing `/vastai/listMachines` is NOT
denied because the user lacks `vastai:query`. The user's authority was
checked at the gate (`/agent/chat` requires `chat`, user has `chat`). Inside,
the handler runs under its own composition authority. The user's authority
does not propagate into internal calls.

This is the kernel/user model: `getaddrinfo` doesn't require the caller to
have `CAP_NET_RAW` to make DNS queries. The curated entry point exists
*because* it does things the user can't, on the user's behalf, under its own
authority.

If a handler *wants* to act on behalf of the user (e.g., a database proxy
that runs queries under the user's DB identity), that's a **handler-level
decision** — it reads `context.identity` and explicitly narrows its
behavior. That's delegated access, not automatic intersection. The system
shouldn't silently intersect; the handler should explicitly delegate.

## Consequences

**Positive:**

- The privilege model in ADR-015 is now implementable as specified. The
  composition authority, scoped env, and capabilities all have registration
  paths and dispatch-path wiring. No implementer has to invent the bridge.
- Leaves (`from_openapi`, `from_mcp`, `from_call`) don't get fake identities.
  The assembly layer doesn't have to invent `Identity { id:
  "vastai-listmachines-handler", scopes: [], resources: {} }` for forwarding
  stubs that will never compose. `composition_authority: None` is natural for
  leaves, not an oversight.
- External services can't self-grant composition authority. The OpenAPI spec
  defines the operation interface (name, schemas, access control). The
  *provenance* is set by the assembly layer when it runs `from_openapi`. The
  *composition authority* is `None` for imported ops — the external service
  can't grant itself scopes to compose into your registry. The assembly layer
  is the sole grantor, and only for `Local` and `Session` ops.
- Capabilities have one model: per-request on `OperationContext`, populated
  from the registration bundle. No closure-capture vs context duplication
  ambiguity. The three consuming crates (call, agent, napi) can't diverge
  because there's one wiring path.
- The graph model provides a precise structural framing without mandating a
  graph library for v1. The operation graph, scoped subgraph, and call graph
  are concepts that guide the API shape; HashMaps and HashSets are the v1
  implementation. A future `alknet-flowgraph` crate can reify these as
  petgraph structures when workflow templates and type-compatibility
  validation are needed.
- The kernel/user analogy makes the security model legible. The user's
  authority is the gate credential (checked once at External entry). The
  composition authority is the internal credential (used for all
  composition inside). The scoped env is the reachability boundary (what the
  handler can attempt to compose). This is the same model every OS uses, and
  it's been battle-tested.

**Negative:**

- The registration API changes from `register(spec, handler)` to
  `register(HandlerRegistration)`. This is a breaking change to the API
  surface shown in operation-registry.md, but since no implementation exists
  yet, it's a spec edit, not a migration.
- `CompositionAuthority` is a new type, distinct from `Identity`. This adds a
  type to alknet-call. It's not a peer identity — it's a declared authority
  bundle. The distinction from `Identity` is intentional and necessary (a
  handler is not a network peer), but it means the codebase has two
  scope-bearing types. Mitigated: they serve different roles and don't
  converge — `Identity` is inbound (resolved from credentials via
  `IdentityProvider`), `CompositionAuthority` is declared (set by the
  assembly layer at registration).
- The assembly layer has more registration-time responsibility: it must
  declare each handler's provenance, composition authority, and scoped env.
  This is expected — the assembly layer assembles everything (ADR-008), and
  forcing explicit declaration of privilege is a feature, not a bug. An
  `OperationRegistryBuilder` convenience API can reduce boilerplate for
  common cases (e.g., `.with_local(spec, handler, authority, env,
  capabilities)` vs `.with_leaf(spec, handler, capabilities)`).
- The dispatch path does a registry lookup per call (to fetch the
  registration bundle's composition authority, scoped env, and capabilities).
  This is a `HashMap` lookup — negligible cost. The alternative (baking
  everything into the handler closure) creates the C3 ambiguity. The lookup
  is the right trade.

**Validation strategy:**

The security model should be validated by fuzzing. A fuzzer that generates
call trees (valid and invalid compositions, different provenance mixes, edge
cases around the gate) and asserts "no path through the call graph lets a
user with scope X reach an operation requiring Y without going through a gate
that checks X" would catch the class of privilege-escalation bug this ADR is
designed to prevent. The typebox-rs fake data generator can produce valid and
invalid inputs from JSON Schemas; with minor edits it can output invalid
inputs or a mix of valid/invalid, enabling property-based testing of the ACL
model. This is a downstream concern — the spec needs to be right first, then
the fuzzer validates the implementation against the spec.

## Assumptions

1. **Internal calls should run under a different authority than external
   calls, not skip ACL entirely.** Inherited from ADR-015. The escalation
   vectors (buggy handler, parameterized dispatch) are real and must be
   prevented.

2. **Provenance is knowable at registration time.** The assembly layer knows
   whether an op is `Local`, `FromOpenAPI`, `FromMCP`, `FromCall`, or
   `Session` when it registers the op — the adapter that produced the
   `(OperationSpec, Handler)` pair knows its own type. If a future use case
   requires provenance to be discovered at call time, the model needs
   extension.

3. **Composition reachability is knowable at registration time.** The
   assembly layer can declare which operations a handler may compose when it
   registers the handler. If a use case requires fully dynamic scoping
   (handler discovers at call time what it can compose), the model needs
   extension — but the assumption is that composition reachability is
   knowable at registration time for `Local` ops, and at sandbox creation
   time for `Session` ops.

4. **The assembly layer is the trust boundary.** The assembly layer declares
   provenance, composition authority, and scoped env. If the assembly layer
   is compromised, all handler authority is compromised. This is the same
   trust boundary as ADR-008 and ADR-014.

5. **Leaves don't compose.** `FromOpenAPI`, `FromMCP`, and `FromCall` ops are
   forwarding stubs — they take input, forward it (over HTTP, MCP, or QUIC),
   and return output. They don't call `env.invoke()`. If a future use case
   requires an imported op to compose (e.g., a `from_call` op that locally
   composes other ops before forwarding), its provenance would need to change
   to `Local` (it's no longer a pure forwarding stub), or the model needs a
   hybrid provenance.

6. **`Session` ops compose under restricted authority.** Session ops
   (agent-written, OQ-19) get composition authority scoped down by the parent
   handler at sandbox creation (ADR-015's "dynamic scoping at sandbox
   creation"). The assembly layer grants the sandbox's parent handler a
   composition authority; the parent handler scopes it down further when
   creating the sandbox. The session op's composition authority is a subset
   of the parent's.

## References

- ADR-014: Secret material flow and capability injection (capabilities are
  orthogonal to identity — both set at registration; this ADR specifies the
  registration path ADR-014 left as a two-way door)
- ADR-015: Privilege model and authority context (this ADR refines
  Assumption 6 — composition authority is not a peer `Identity`; and wires
  the three controls that ADR-015 specified but left without registration
  paths)
- ADR-016: Abort cascade for nested calls (the call graph is the abort
  cascade tree; `parent_request_id` indexes it)
- ADR-017: Call protocol client and adapter contract (adapter-registered
  ops are `Internal` by default; this ADR's provenance makes that explicit)
- ADR-008: Vault integration point (assembly layer is the trust boundary)
- OQ-19: Session-scoped operation registries (session ops are `Session`
  provenance, always `Internal`, compose under restricted authority)
- docs/reviews/001-pre-implementation-architecture-sanity-check.md (findings
  C1–C4, which this ADR resolves)
- `/workspace/@alkdev/flowgraph/README.md` — operation graph, call graph, and
  scoped subgraph concepts (the graph model this ADR uses as framing)
- `/workspace/@alkdev/alknet-main/docs/architecture/flowgraph.md` — prior
  Rust speccing of flowgraph (incomplete; this ADR uses the model, not the
  crate)
- Kernel/user mode analogy: `getaddrinfo` runs under kernel authority, not
  the caller's `CAP_NET_RAW`; the curated entry point exists to do things the
  user can't, on the user's behalf