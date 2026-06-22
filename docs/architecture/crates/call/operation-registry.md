---
status: draft
last_updated: 2026-06-22-22
---

# Operation Registry

OperationSpec, Handler, OperationRegistry, AccessControl, service discovery, and irpc integration.

## What

The operation registry maps operation names to specs and handlers. It is the dispatch core of the call protocol — when a `call.requested` event arrives, the registry looks up the operation by name, checks access control, invokes the handler, and returns the result.

The registry is **layered by trust boundary** (ADR-024): a static, immutable curated layer (`Local` provenance, registered at startup) plus dynamic overlays for session ops (`Session` provenance, per-session) and imported ops (`FromCall` etc., per-connection). The immutability claim that previously applied to the whole registry is now scoped to the curated layer — see ADR-024 for the layering model and the rationale for why immutability is the security control for composing ops but not for imported leaves.

## Why

The operation registry provides:
- **Discoverability**: Clients can query `/services/list` and `/services/schema` to learn what operations exist before calling them
- **Access control**: Each operation declares its required scopes and resources; the registry enforces ACL before invoking the handler
- **Type safety**: JSON Schema for input and output enables validation and client code generation
- **Composability**: Handlers can invoke other operations through `OperationEnv` (local dispatch — remote dispatch is a separate architectural concern, see Constraints)

The registry design is informed by the `@alkdev/operations` TypeScript package, which demonstrated the same capabilities in JavaScript runtimes. The Rust implementation in alknet-call is canonical — it preserves the behavioral contract (namespace + operation name → invoke with input, return output) while defining the adapter contract (from_*, to_*) in Rust (see ADR-013).

## Architecture

### OperationSpec

Every registered operation has a spec that declares its name, type, schemas, and access control:

```rust
pub struct OperationSpec {
    pub name: String,              // e.g., "fs/readFile", "agent/chat" (no leading slash)
    pub namespace: String,         // e.g., "fs", "agent"
    pub op_type: OperationType,   // Query, Mutation, Subscription
    pub visibility: Visibility,   // External (wire-callable) or Internal (composition-only)
    pub input_schema: Value,      // JSON Schema for input
    pub output_schema: Value,     // JSON Schema for output
    pub error_schemas: Vec<ErrorDefinition>,  // Declared domain errors (ADR-023)
    pub access_control: AccessControl,
}

pub enum OperationType {
    Query,         // Read-only, idempotent (e.g., "fs/readFile", "services/list")
    Mutation,      // Side effects (e.g., "bash/exec", "github/authenticate")
    Subscription,  // Streaming (e.g., "agent/chat", "events/subscribe")
}

pub enum Visibility {
    External,  // Callable from the wire (call.requested from a client)
    Internal,  // Composition-only (env.invoke from a handler)
}

/// A declared operation-level error. See ADR-023.
pub struct ErrorDefinition {
    pub code: String,           // e.g., "FILE_NOT_FOUND", "RATE_LIMITED"
    pub description: String,    // Human-readable description
    pub schema: Value,          // JSON Schema for the error detail payload
    pub http_status: Option<u16>,  // HTTP status for adapter projection (from_openapi/to_openapi)
}
```

Operation names use slash-based paths without a leading slash, aligned with URL path conventions: `fs/readFile`, `agent/chat`, `services/list`. The leading slash is added when needed for display (`spec.path()` returns `/fs/readFile`) and for wire format (the `call.requested` payload uses `/fs/readFile`). See OQ-13 for the path format decision (single-node `service/op` vs head/worker `node/service/op`).

The `namespace` field is derived from the name: for `fs/readFile` it's `fs`, for `agent/chat` it's `agent`. It's a convenience accessor for ACL matching and service grouping.

Visibility (ADR-015) controls whether an operation is callable from the wire. `External` operations are wire-facing — they appear in `services/list` and accept `call.requested` from clients. `Internal` operations are composition-only — they return `NOT_FOUND` (not `FORBIDDEN`) when called from the wire, and do not appear in `services/list`. The assembly layer declares visibility at registration. All import adapters (`from_openapi`, `from_mcp`, `from_jsonschema`, `from_call`) register operations as `Internal` by default (they're composition material, not directly callable); the handler that composes them is `External`.

### AccessControl

```rust
pub struct AccessControl {
    pub required_scopes: Vec<String>,          // AND-checked: caller must have ALL
    pub required_scopes_any: Option<Vec<String>>, // OR-checked: caller must have at LEAST ONE
    pub resource_type: Option<String>,          // e.g., "service"
    pub resource_action: Option<String>,        // e.g., "read"
}
```

When a `call.requested` event arrives:
1. The `CallAdapter` resolves the caller's `Identity` from `AuthContext` (and possibly an `AuthToken` in the payload)
2. The registry checks operation **visibility** — if the operation is `Internal`, returns `call.error` with code `NOT_FOUND` (does not leak existence)
3. The registry checks `access_control.check(identity)` — for external calls (`internal: false`), ACL runs against the **caller's identity**; for internal calls (`internal: true`), ACL runs against the **handler's identity** (ADR-015)
4. If access is denied, the adapter returns `call.error` with code `FORBIDDEN`
5. If the relevant identity is `None` and the operation has restrictions, the adapter returns `call.error` with code `FORBIDDEN` and message `"authentication required"`

Operations with empty `AccessControl` (no required scopes, no resource checks) are accessible to all callers, including unauthenticated ones.

**Internal calls and authority context**: When a handler invokes another operation through `OperationEnv`, the nested call is marked `internal: true`, meaning it originated from composition (not from a wire request). The `internal` flag switches the authority context: the ACL check runs against the composing handler's `handler_identity` (set at registration), not the caller's identity and not as a blanket skip. This prevents privilege escalation through composition — a handler can only compose operations its own identity is authorized for. See ADR-015.

### Handler

```rust
pub type Handler = Arc<dyn Fn(Value, OperationContext) -> Pin<Box<dyn Future<Output = ResponseEnvelope> + Send>> + Send + Sync>;
```

Handlers are async — many operations (file I/O, HTTP service calls, irpc service calls) are inherently asynchronous. The handler receives an `async` runtime context and returns a `Future<Output = ResponseEnvelope>`.

A handler receives:
- `input: Value` — the deserialized `payload` from the `call.requested` event (always `serde_json::Value`)
- `context: OperationContext` — request ID, identity, metadata, env

And returns a `ResponseEnvelope` containing the result or an error. `ResponseEnvelope` is defined in [call-protocol.md](call-protocol.md#responseenvelope) — it carries the request ID and a `Result<Value, CallError>`. Local dispatch produces it with no serialization overhead; the `CallAdapter` converts it to `EventEnvelope` for the wire.

When a handler returns an error, the `CallError.code` is matched against the operation's declared `error_schemas` (ADR-023). If the code matches a declared `ErrorDefinition`, the `call.error` event carries that code and the error's detail payload. If it doesn't match, the `call.error` carries `INTERNAL`. This is how handler failures become typed errors on the wire instead of string-matched messages.

### OperationContext

```rust
pub struct OperationContext {
    pub request_id: String,
    pub parent_request_id: Option<String>,
    pub identity: Option<Identity>,                       // Caller's identity (inbound — who invoked me)
    pub handler_identity: Option<CompositionAuthority>,    // Handler's composition authority (ADR-022)
    pub capabilities: Capabilities,
    pub metadata: HashMap<String, Value>,
    /// Reachability set — the operations this handler may compose.
    /// Populated from the registration bundle's `scoped_env` (ADR-022).
    /// The reachability check in `OperationEnv::invoke()` consults
    /// `scoped_env.allows(&name)`. This is data, not a dispatch trait.
    pub scoped_env: ScopedOperationEnv,
    /// Composition dispatch trait. A handler calls `env.invoke(...)` to
    /// compose child operations. This is `Arc<dyn OperationEnv>` (a trait
    /// object), not a concrete struct — the trait-object design is what
    /// enables registry layering (ADR-024): the CallAdapter composes the
    /// root env per call from the active layers (curated base + connection
    /// overlay + session overlay), and session/connection overlays wrap
    /// the base via trait layering. Same pattern as `IdentityProvider`
    /// (ADR-004). See ADR-024.
    pub env: Arc<dyn OperationEnv + Send + Sync>,
    /// Abort policy for this call's descendants (ADR-016 Decision 6).
    /// Default `AbortDependents` — aborting this request aborts all
    /// non-terminal descendants. `ContinueRunning` is an opt-in for
    /// long-running work that should survive a parent's abort. Set by the
    /// composing handler via `OperationEnv::invoke()` (or
    /// `invoke_with_policy()`), not by the wire caller.
    pub abort_policy: AbortPolicy,
    /// Composition-origin flag. Set by `OperationEnv::invoke()` (true) or the
    /// `CallAdapter` dispatch path (false) — never by handlers. Module-private
    /// for writes; read via `is_internal()`. See ADR-015.
    pub(crate) internal: bool,
}

/// Abort cascade policy for a call's descendants (ADR-016).
///
/// `AbortDependents` (default): aborting this call cascades to all
/// non-terminal descendants.
///
/// `ContinueRunning` (opt-in): descendants that have already started
/// continue to completion; descendants that haven't started are aborted;
/// no new descendants start.
pub enum AbortPolicy {
    AbortDependents,
    ContinueRunning,
}

impl Default for AbortPolicy {
    fn default() -> Self { Self::AbortDependents }
}

impl OperationContext {
    pub fn is_internal(&self) -> bool { self.internal }
}
```

- `request_id`: Correlates with the `call.requested` event's `id` field
- `parent_request_id`: Set when this call was initiated by another operation (via `OperationEnv`). Records the agency chain — the call tree is the principal→agent chain (ADR-015)
- `identity`: The authenticated caller (from `IdentityProvider`) — inbound auth (who is calling me). For external calls, this is who sent the `call.requested`. For internal calls, this is the parent handler's `handler_identity` (propagated through `OperationEnv::invoke()`)
- `handler_identity`: The composition authority of the handler processing this call. `None` for leaves (`FromOpenAPI`, `FromMCP`, `FromCall`) — they don't compose. `Some(...)` for `Local` and `Session` ops that can compose children. For internal calls (`internal: true`), the ACL check runs against this authority (ADR-015, ADR-022). This is NOT a peer `Identity` — it's a declared authority bundle set at registration by the assembly layer
- `capabilities`: Outbound credentials the handler may use (decrypted API keys, scoped vault access) — see [Capability Injection](#capability-injection) below
- `metadata`: Request-scoped context (tracing IDs, connection info). **Must not hold secret material** — see ADR-014. **Does not propagate through `OperationEnv::invoke()`** — nested calls get fresh metadata. The tracing link between parent and child is `parent_request_id`, not metadata propagation. Anything a handler needs to pass to a child goes in the call `input`.
- `scoped_env`: The reachability set — the operations this handler may compose. Populated from the registration bundle's `scoped_env` (ADR-022). The reachability check in `OperationEnv::invoke()` consults `scoped_env.allows(&name)`. This is *data* (a `ScopedOperationEnv` struct), not a dispatch trait. `None`/empty for leaves.
- `env`: The composition dispatch trait (`Arc<dyn OperationEnv + Send + Sync>`). A handler calls `context.env.invoke(...)` to compose child operations. This is a trait object, not a concrete struct — the trait-object design enables registry layering (ADR-024): the CallAdapter composes the root env per call from the active layers (curated base + connection overlay + session overlay), and overlays wrap the base via trait layering. Same pattern as `IdentityProvider` (ADR-004). See ADR-024.
- `internal`: When `true`, this call originated from composition (a handler calling another operation via `OperationEnv`), not from a wire request. This switches the authority context: ACL runs against `handler_identity`, not `identity`. The `internal` field uses module-private construction — handlers construct `OperationContext` through `OperationEnv::invoke()` which sets `internal: true`, or through the `CallAdapter` dispatch path which sets `internal: false`. The field is not `pub` for writes; only `pub fn is_internal(&self) -> bool` is exposed for reads. See ADR-015.

`identity` and `capabilities` are orthogonal: identity is inbound (who is calling me), capabilities are outbound (what credentials I can use). `identity` and `handler_identity` are the principal/agent pair: `identity` is the principal (who delegated), `handler_identity` is the agent (who is acting). See ADR-014 for capabilities, ADR-015 for the privilege model, and ADR-022 for the composition authority type.

### OperationRegistry

```rust
pub struct OperationRegistry {
    operations: HashMap<String, HandlerRegistration>,
}
```

The registry maps operation names to `HandlerRegistration` bundles. The curated layer (Layer 0) is a `HashMap<String, HandlerRegistration>`; session and connection overlays (Layers 1 and 2) are separate maps that the `CallAdapter` composes into the per-call `OperationContext.env` (ADR-024). See ADR-022 for the full registration model and ADR-024 for the layering model. Key methods:

- `register(registration)`: Add an operation to the curated layer at startup
- `registration(name)`: Find a registration by operation name (checks active overlays first, then curated base — ADR-024). Returns spec, handler, provenance, composition authority, scoped env, capabilities.
- `invoke(name, input, context)`: Look up, check ACL, invoke handler, return result
- `list_operations()`: Return all registered specs (for `/services/list` — returns curated + active overlay ops)

### HandlerRegistration

The registration bundle carries everything the dispatch path needs to construct an `OperationContext`. See ADR-022 for the full rationale.

```rust
pub struct HandlerRegistration {
    pub spec: OperationSpec,
    pub handler: Handler,
    pub provenance: OperationProvenance,
    pub composition_authority: Option<CompositionAuthority>, // None for leaves
    pub scoped_env: Option<ScopedOperationEnv>,               // None for leaves
    pub capabilities: Capabilities,
}
```

- `provenance`: Where the op came from (`Local`, `FromOpenAPI`, `FromMCP`, `FromCall`, `FromJsonSchema`, `Session`). Determines composition capability, default visibility, and trust model. Only `Local` and `Session` ops can compose; leaves get `composition_authority: None` and `scoped_env: None`.
- `composition_authority`: The declared authority (label + scopes + resources) the handler operates under when composing children. `None` for leaves. This replaces ADR-015's `handler_identity: Identity` — it's not a peer identity, it's a declared authority bundle. See ADR-022.
- `scoped_env`: The set of operations this handler may reach via `env.invoke()`. `None` for leaves (empty env). The reachability control from ADR-015.
- `capabilities`: Outbound credentials (decrypted API keys, signing keys). Populated by the assembly layer from the vault at registration time. See [Capability Injection](#capability-injection).

The `OperationRegistryBuilder` provides a fluent API with convenience methods for common cases:

```rust
let registry = OperationRegistryBuilder::new()
    // Built-in service discovery (Local, no composition)
    .with_local(services_list_spec(), Arc::new(services_list_handler),
                CompositionAuthority::none(), ScopedOperationEnv::empty())
    .with_local(services_schema_spec(), Arc::new(schema_handler),
                CompositionAuthority::none(), ScopedOperationEnv::empty())
    // Agent handler (Local, composes — has authority + scoped env)
    .with_local(agent_chat_spec(), Arc::new(agent_chat_handler),
                CompositionAuthority::new("agent-chat", ["llm:call", "fs:read", "vastai:query"]),
                ScopedOperationEnv::new(["fs/readFile", "vastai/listMachines", "llm/generate"]))
    // Imported ops (leaves — no authority, no scoped env)
    .with_leaf(vastai_listMachines_spec(), Arc::new(vastai_handler), vastai_credentials)
    .build();
```

The CLI binary (or assembly layer) constructs the registry and passes it to the `CallAdapter`. Once built, the **curated layer** (Layer 0 — `Local` provenance ops) is immutable. Session and imported overlays are dynamic at their respective scopes (per-session, per-connection) per ADR-024. The `CallAdapter` composes the root `OperationContext.env` per incoming call from the active layers.

### OperationEnv

The `OperationEnv` trait is the universal composition mechanism. A handler calls `context.env.invoke("fs", "readFile", input, &context)` and gets a `ResponseEnvelope` back — regardless of whether the operation runs locally, via an irpc service, or on a remote node.

```rust
/// The composition dispatch trait. A handler composes child operations
/// through its `OperationContext.env` (which implements this trait).
///
/// This must remain a trait, not a concrete type — session-scoped
/// registries (OQ-19) depend on wrapping the global env via trait
/// layering. Making `OperationEnv` concrete or hardcoding the global
/// registry into the dispatch path would close the session-overlay
/// pattern.
#[async_trait]
pub trait OperationEnv: Send + Sync {
    /// Compose a child operation. The child's `OperationContext` is
    /// constructed with `internal: true`, inheriting the parent's
    /// composition authority as the child's caller identity. The abort
    /// policy defaults to the parent's (ADR-016 Decision 6).
    async fn invoke(
        &self,
        namespace: &str,
        operation: &str,
        input: Value,
        parent: &OperationContext,
    ) -> ResponseEnvelope;

    /// Compose a child with an explicit abort policy (ADR-016 Decision 6).
    /// Use `AbortPolicy::ContinueRunning` for long-running work that
    /// should survive a parent's abort. The default `invoke()` inherits
    /// the parent's policy; this method overrides it for this child.
    async fn invoke_with_policy(
        &self,
        namespace: &str,
        operation: &str,
        input: Value,
        parent: &OperationContext,
        policy: AbortPolicy,
    ) -> ResponseEnvelope;
}
```

The `parent` parameter propagates the calling context: the nested call gets `parent_request_id: Some(parent.request_id)`, inherits `parent.handler_identity` as the caller identity, and is marked `internal: true`.

**Metadata does not propagate through composition.** Nested calls get fresh metadata (`HashMap::new()`), not the parent's metadata bag. This is a security constraint (ADR-014): `metadata: HashMap<String, Value>` accepts any `serde_json::Value`, including secret material. If metadata propagated through `env.invoke()`, a handler that accidentally placed a secret in metadata would leak it to every child operation — and if a child is a `from_call` operation (ADR-017), the metadata would cross the wire to the remote node. The tracing link between parent and child is `parent_request_id`, not metadata propagation. Anything a handler needs to pass to a child goes in the call `input`, not in ambient context.

**Local dispatch only.** The initial `OperationEnv` implementation for the
curated layer (Layer 0) dispatches directly through the local
`OperationRegistry`. The composite env (curated + session + connection
overlays) is a separate type built by the `CallAdapter` per call — see
ADR-024 and the `CompositeOperationEnv` sketch below.

```rust
/// Layer 0 dispatch — the curated registry. This is the base env that
/// overlays wrap. See ADR-024 for the layering model.
pub struct LocalOperationEnv {
    registry: Arc<OperationRegistry>,
}

#[async_trait]
impl OperationEnv for LocalOperationEnv {
    async fn invoke(&self, namespace: &str, operation: &str, input: Value, parent: &OperationContext) -> ResponseEnvelope {
        let name = format!("{namespace}/{operation}");

        // Reachability check (ADR-015, ADR-022): is this op in the parent's
        // scoped env? If not, return NOT_FOUND. This bounds the
        // parameterized-dispatch attack surface — a handler (or an LLM
        // picking tools) can only reach declared ops. The reachability set
        // is on `parent.scoped_env` (data), not on `parent.env` (dispatch
        // trait) — see ADR-024 for the split.
        if !parent.scoped_env.allows(&name) {
            return ResponseEnvelope::not_found(name);
        }

        let registration = self.registry.registration(&name);
        let context = OperationContext {
            // Unique per invocation — a counter, UUID, or parent_id + suffix.
            // A deterministic ID (e.g. format!("env-{name}")) collides across
            // concurrent invocations of the same operation, which corrupts
            // PendingRequestMap correlation and the abort-cascade tree
            // (ADR-016), which is indexed by parent_request_id.
            request_id: generate_request_id(),
            parent_request_id: Some(parent.request_id.clone()),
            // Parent's composition authority becomes the caller for the child.
            // This is the authority switch: the child's ACL checks against
            // the parent's authority, not the original wire caller's identity.
            identity: parent.handler_identity.as_identity(),
            // Child's own composition authority (from its registration).
            // None for leaves — they don't compose, so this is never used
            // for ACL on a grandchild.
            handler_identity: registration.composition_authority.clone(),
            capabilities: parent.capabilities.clone(),       // Inherit caller's capabilities
            metadata: HashMap::new(),                         // Fresh — does NOT propagate parent metadata (ADR-014)
            scoped_env: registration.scoped_env.clone()
                .unwrap_or_else(ScopedOperationEnv::empty),  // Child's own scoped env (empty for leaves)
            // Dispatch trait: the child inherits the parent's env (the same
            // composite of curated base + active overlays). See ADR-024.
            env: parent.env.clone(),
            // Abort policy: inherit the parent's policy by default (ADR-016).
            // The parent handler can override via `invoke_with_policy()`.
            abort_policy: parent.abort_policy.clone(),
            internal: true,                                   // Nested calls use handler authority
        };
        self.registry.invoke(&name, input, context).await
    }

    // invoke_with_policy() delegates to invoke() with the policy set on the
    // child context (ADR-016 Decision 6). See the trait definition above.
}
```

The composite env (built by the `CallAdapter` per incoming call) wraps the
curated base and any active overlays:

```rust
/// Per-call composite env (ADR-024). Built by the CallAdapter in
/// build_root_context from the active layers. The child inherits this by
/// Arc::clone through invoke().
pub struct CompositeOperationEnv {
    session: Option<Arc<dyn OperationEnv + Send + Sync>>,    // Layer 1 — active session, if any
    connection: Option<Arc<dyn OperationEnv + Send + Sync>>, // Layer 2 — this connection's imported ops
    base: Arc<dyn OperationEnv + Send + Sync>,               // Layer 0 — curated registry (LocalOperationEnv)
}

#[async_trait]
impl OperationEnv for CompositeOperationEnv {
    async fn invoke(&self, namespace: &str, operation: &str, input: Value, parent: &OperationContext) -> ResponseEnvelope {
        let name = format!("{namespace}/{operation}");
        // Reachability check against parent.scoped_env (same as LocalOperationEnv).
        if !parent.scoped_env.allows(&name) {
            return ResponseEnvelope::not_found(name);
        }
        // Dispatch in overlay order: session → connection → curated base.
        // First match wins. Each overlay is an OperationEnv impl that knows
        // its own registry; the composite routes to the right one.
        if let Some(session) = &self.session {
            // session impl checks its own registry; if not found, falls
            // through (returns a sentinel or the composite continues).
            // Implementation detail: the session impl's `invoke` either
            // dispatches or returns a "not in this overlay" signal.
        }
        if let Some(connection) = &self.connection {
            // same pattern
        }
        self.base.invoke(namespace, operation, input, parent).await
    }
}
```

The exact "first match wins" mechanism (sentinel return, a separate
`contains` check, or a try/else pattern) is a two-way door for
implementation — the structural decision (composite trait object, overlay
order, `Arc::clone` inheritance) is what ADR-024 locks.
```

Two things happen in `invoke()`:

1. **Reachability check**: before constructing the child context, `invoke()` checks whether the requested op is in the parent's scoped env. If not, `NOT_FOUND`. This is the reachability control — a handler can only compose declared ops.
2. **Authority propagation**: the child's `identity` is the parent's `handler_identity` (the parent's composition authority becomes the caller). The child's `handler_identity` is the child's own registration's `composition_authority` — so if the child itself composes further, its children inherit the child's authority. This is the principal/agent chain from ADR-015, now wired via ADR-022.

Future work may add irpc service dispatch and remote call protocol dispatch as additional backends. The handler-facing API stays the same.

**`OperationEnv` must remain a trait.** This is a constraint, not a suggestion. The trait-based design enables registry layering (ADR-024): the CallAdapter composes the root env per call from the curated base + active connection/session overlays, and overlays wrap the base via trait layering. Session-scoped registries (OQ-19) and connection-scoped remote imports (ADR-017 `from_call`) are both overlays on the same base, using the same mechanism. Making `OperationEnv` concrete or hardcoding the global registry into the dispatch path would close both the session-overlay and connection-overlay patterns. This is the same integration-point pattern as `IdentityProvider` (ADR-004). See OQ-19 and ADR-024.

### Service Discovery

Two built-in operations expose what the node offers:

| Operation name | Display path | Type | Description |
|---------------|-------------|------|-------------|
| `services/list` | `/services/list` | Query | List registered operation names and metadata |
| `services/schema` | `/services/schema` | Query | Get the `OperationSpec` for a specific operation |

These are read-only — no admin operations are exposed through the call protocol itself.

`services/list` only returns `External` operations to remote callers. `Internal` operations are not part of the wire-facing API surface — they're implementation details of composition. A remote client cannot enumerate the internal call tree. See ADR-015.

`services/list` returns:

```json
{
  "operations": [
    { "name": "fs/readFile", "namespace": "fs", "op_type": "query" },
    { "name": "agent/chat", "namespace": "agent", "op_type": "subscription" },
    { "name": "events/subscribe", "namespace": "events", "op_type": "subscription" }
  ]
}
```

`services/schema` accepts `{ "name": "fs/readFile" }` (no leading slash —
registry form, same as `OperationSpec.name`) and returns the full
`OperationSpec` including input/output JSON Schemas and declared
`error_schemas` (ADR-023). The `CallAdapter` normalizes the leading slash
from wire `operationId`s before lookup, so `services/schema` accepts both
`fs/readFile` and `/fs/readFile`. This enables client code generation: a
client reading the schema can produce typed error enums instead of generic
error handling.

### irpc Integration

irpc and the operation registry serve different scopes:

| Layer | Mechanism | Serialization | Scope |
|-------|-----------|---------------|-------|
| Call protocol (external) | `EventEnvelope` over QUIC streams | JSON | Cross-language, cross-node |
| irpc services (internal) | `VaultProtocol` derive macro, `Service` trait | postcard (binary) | Rust-to-Rust, in-process or in-cluster |
| Local dispatch (in-process) | Direct function call through `OperationRegistry` | None | Same process |

irpc services are an internal dispatch mechanism — they are not directly exposed on the call protocol. The vault's `VaultProtocol` uses irpc for in-process, type-safe dispatch via `VaultServiceHandle` (postcard serialization for in-cluster, direct calls for in-process). The vault is accessed by the assembly layer (CLI binary) at startup, not by handlers at call time. See ADR-008 and ADR-014.

If a handler internally uses an irpc-based service, the handler bridges the two: it receives JSON input from the call protocol, calls the irpc service in-process (postcard, type-safe), and serializes the result back to JSON for the call protocol response. This layering preserves irpc's type safety for internal calls while keeping the external interface cross-language.

### Operation Registration at Startup

The CLI binary (or assembly layer) constructs `HandlerRegistration` bundles with provenance, composition authority, scoped env, and capabilities (from the vault — see [Capability Injection](#capability-injection)), then registers them before starting the endpoint:

```rust
// Assembly layer: unlock vault, derive credentials
let vault = VaultServiceHandle::new();
vault.unlock(&mnemonic, passphrase.as_deref())?;
let google_api_key = vault.decrypt(&google_key_blob)?;
let github_signing_key = vault.derive_ed25519(PATHS::GITHUB_SIGNING)?;
let vastai_credentials = Capabilities::new().with_http_token("vastai", vastai_token);

// Register operations — vault operations are NOT registered here
let registry = OperationRegistryBuilder::new()
    // Built-in service discovery (Local, no composition)
    .with_local(services_list_spec(), Arc::new(services_list_handler),
                CompositionAuthority::none(), ScopedOperationEnv::empty())
    .with_local(services_schema_spec(), Arc::new(schema_handler),
                CompositionAuthority::none(), ScopedOperationEnv::empty())
    // Agent handler (Local, composes — has authority + scoped env + capabilities)
    .with(HandlerRegistration {
        spec: agent_chat_spec(),
        handler: Arc::new(agent_chat_handler),
        provenance: OperationProvenance::Local,
        composition_authority: Some(CompositionAuthority::new(
            "agent-chat", ["llm:call", "fs:read", "vastai:query"])),
        scoped_env: Some(ScopedOperationEnv::new(
            ["fs/readFile", "vastai/listMachines", "llm/generate"])),
        capabilities: Capabilities::new().with_api_key("google", google_api_key),
    })
    // Vastai ops (FromOpenAPI, leaves — no authority, no scoped env)
    .with_leaf(vastai_listMachines_spec(), Arc::new(vastai_listMachines_handler),
               vastai_credentials.clone())
    .build();

let call_adapter = CallAdapter::new(Arc::new(registry), identity_provider);
```

The vault is used at construction time to populate `capabilities` in the registration bundle, not registered as call protocol operations. The curated layer (Layer 0) is immutable after construction — adding a `Local` op requires restarting the process. Session and imported overlays are dynamic at their respective scopes (ADR-024). This is consistent with OQ-04 (scoped to the `HandlerRegistry` by ADR-024), ADR-008, ADR-014, and ADR-022.

### Capability Injection

Handlers that need outbound credentials (LLM provider API keys, signing keys, HTTP service tokens) receive them through the `Capabilities` type on `OperationContext`, not by calling vault operations over the wire and not from environment variables. This is the mechanism that ADR-008 described in prose ("derived keys and decrypted credentials are injected into operation contexts at the assembly layer") and that ADR-014 specifies as a one-way door. ADR-022 specifies the registration path: capabilities live on the `HandlerRegistration` bundle, and the dispatch path populates `OperationContext.capabilities` from the bundle at call time.

The flow is:

```
Assembly layer (CLI startup):
  1. Unlock vault (local, mnemonic from secure prompt or file)
  2. Derive / decrypt the credentials each handler needs
  3. Construct HandlerRegistration bundles with capabilities from the vault
  4. Register the bundles in the OperationRegistry
  5. Start the endpoint

Handler invocation (at call time):
  call.requested → CallAdapter looks up registration by op name
  → build_root_context populates OperationContext.capabilities from registration.capabilities
  → handler reads context.capabilities → uses the credential for its outbound call
```

The handler closure does **not** capture capabilities — that was the pre-ADR-022 "Model A" that created a circular dependency with per-request `OperationContext.capabilities`. Capabilities live on the registration bundle, and the dispatch path populates the context from the bundle. One model, one wiring path. See ADR-022 Decision 6.

The `Capabilities` type holds non-serializable, zeroized secret material. It does not implement `Serialize` — it cannot cross the call protocol wire even by accident. The concrete shape of the type (a typed map, a struct with named fields, a trait object) is a two-way door for implementation. The one-way constraints are fixed by ADR-014:

- Capabilities are populated by the assembly layer at registration (on the `HandlerRegistration` bundle). They are never populated from call protocol inputs.
- Capabilities hold secret material that does not implement `Serialize` and does not appear in `EventEnvelope` payloads.
- The call protocol carries no secret material. See [call-protocol.md](call-protocol.md) for the wire-level constraint.
- **Capabilities are `Clone` and cloned through composition.** `OperationEnv::invoke()` calls `parent.capabilities.clone()` to pass capabilities to nested calls. This is intentional: a child handler needs the same outbound credentials as its parent (e.g., the `/agent/chat` handler composing `/fs/readFile` may need the same API key for an outbound LLM call). The security implication is that each composition step duplicates the secret material reference — but capabilities are scoped (the handler can only use what the assembly layer declared on the registration bundle), and children run under the parent's composition authority (ADR-015, ADR-022). A clone is the same scoped handle, not a widening of scope. The concrete cloning semantics (reference-counted `Arc` vs deep copy of zeroized material) is a two-way door for implementation, but `Capabilities: Clone` is required by the composition model.

**No vault operations are registered in the call protocol.** The vault is assembly-layer only (ADR-008, ADR-014). A handler that needs a child key for a specific operation (e.g., signing for GitHub auth) receives a scoped capability that performs the derivation in-process — it never holds the master seed and never calls a network-exposed vault operation.

**Adapters take credential sources.** All import adapters (`from_openapi`, `from_mcp`, `from_jsonschema`, `from_call` — see ADR-017, constrained by ADR-014) register HTTP-backed, MCP-backed, or remote-call-backed operations. The credential each service needs (bearer token, API key, TLS identity for the remote connection) is provided by the assembly layer at registration time — the adapter receives a credential source, not a static token string. This is the integration point where the vault feeds credentials into backed operations, including LLM providers that expose OpenAPI-compatible endpoints. Adapter-registered operations are `Internal` by default (ADR-015) — they're composition material, not directly callable from the wire.

**`from_call` imports remote operations.** The `from_call` adapter (ADR-017) discovers operations on a remote call protocol endpoint via `services/list` and `services/schema`, then registers them with handlers that forward calls over the QUIC connection. This makes cross-node composition transparent — a handler calling `env.invoke("worker", "exec", ...)` doesn't know whether the operation is local or remote. Connection direction (who opened the QUIC connection) is independent of call direction (who calls whom) — both sides can call each other once connected.

**`from_call` trust is transitive.** A `from_call`-imported operation executes the remote node's code, not yours. The scoped env (ADR-015) bounds *which* operations are reachable, but not *what* they do. A compromised remote node can do anything its operations are declared to do (and anything its handler bugs allow). This is inherent to remote composition — same as trusting any RPC endpoint — but it must be explicit in the threat model. `from_call` means "I trust the remote node as much as my own handlers." The scoping protects the caller from reaching arbitrary ops; it does not protect against what the reached op does.

**Scoped composition env.** The `OperationEnv` given to a handler is scoped — it can only invoke a declared set of operations, set at registration on the `HandlerRegistration` bundle by the assembly layer (ADR-022). This bounds the parameterized-dispatch attack surface: a handler (or an LLM picking tools, or a quickjs sandbox) can only reach declared operations, not the entire registry. The scoped env is the reachability control; the composition authority is the authority control. Both are needed for least privilege. See ADR-015 and ADR-022.

## Constraints

- The registry is **layered by trust boundary** (ADR-024). The curated layer (`Local` provenance) is immutable after construction — adding a `Local` op requires restarting the process, which re-enters the startup trust boundary. Session (`Session`) and imported (`FromCall` etc.) ops are dynamic at their respective scopes (per-session, per-connection). The pre-ADR-024 blanket immutability claim was inherited by analogy from ADR-010's `HandlerRegistry` (ALPN-level) and did not apply to the operation registry — the TLS-config argument that justifies `HandlerRegistry` immutability does not touch the operation registry, which lives behind the single ALPN `alknet/call`.
- Operation specs use JSON Schema. The call protocol's external interface is always JSON. irpc's postcard serialization is internal only.
- `OperationEnv::invoke()` dispatches through the local registry. Remote dispatch (federation, head/worker routing) would be a separate mechanism at a different layer — not a prefix added to operation paths. irpc service dispatch is contracted but not built.
- The call protocol does not depend on any database. Operation specs are in-memory, populated at startup.
- `OperationContext.internal` is set by `OperationEnv`, not by callers. A handler cannot mark its own call as internal. The `internal` flag switches authority context (composition authority for ACL), it does not skip ACL — see ADR-015, ADR-022.
- **Operations have External/Internal visibility.** `Internal` operations return `NOT_FOUND` when called from the wire and are excluded from `services/list`. The assembly layer declares visibility at registration. See ADR-015.
- **The composition env is scoped.** A handler can only invoke operations declared in its scoped env (on the `HandlerRegistration` bundle). This bounds parameterized-dispatch attack surface. See ADR-015, ADR-022.
- **No vault operations are registered in the call protocol.** The vault is assembly-layer only (ADR-008, ADR-014). Handlers receive secret material through `OperationContext.capabilities`, not by calling vault operations over the wire.
- **The call protocol carries no secret material.** Secret material (private keys, API keys, mnemonics, decrypted credentials) must not appear in `call.requested` payloads, `call.responded` payloads, or `OperationContext.metadata`. See ADR-014.
- **Metadata does not propagate through composition.** `OperationEnv::invoke()` constructs fresh metadata for nested calls (`HashMap::new()`), not the parent's metadata. This prevents a handler that accidentally places a secret in metadata from leaking it to child operations — and if a child is a `from_call` operation (ADR-017), across the wire to a remote node. The tracing link is `parent_request_id`, not metadata propagation. See ADR-014.
- **Provenance determines composition capability.** Only `Local` and `Session` ops can compose. Leaves (`FromOpenAPI`, `FromMCP`, `FromCall`) get `composition_authority: None` and `scoped_env: None` — they don't compose, so they don't need authority or reachability bounds. See ADR-022.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| irpc as call protocol foundation | [ADR-005](../../decisions/005-irpc-as-call-protocol-foundation.md) | irpc provides framing and service dispatch |
| Call protocol stream model | [ADR-012](../../decisions/012-call-protocol-stream-model.md) | Bidirectional streams, EventEnvelope, ID-based correlation |
| Static handler registration | [ADR-010](../../decisions/010-alpn-router-and-endpoint.md) | `HandlerRegistry` (ALPN-level) immutable after construction; `OperationRegistry` layered by ADR-024 (curated immutable, session/imported dynamic) |
| Vault integration via assembly layer | [ADR-008](../../decisions/008-secret-service-integration.md) | Vault is a capability source, accessed at assembly time |
| Secret material flow and capability injection | [ADR-014](../../decisions/014-secret-material-flow-and-capability-injection.md) | Capabilities carry outbound credentials; call protocol carries no secret material |
| Privilege model and authority context | [ADR-015](../../decisions/015-privilege-model-and-authority-context.md) | `internal` = authority switch not ACL skip; External/Internal visibility; composition authority + scoped env |
| Handler registration, provenance, and composition authority | [ADR-022](../../decisions/022-handler-registration-provenance-and-composition-authority.md) | Registration bundle carries provenance, composition authority, scoped env, capabilities; dispatch path reads from bundle |
| Operation registry layering | [ADR-024](../../decisions/024-operation-registry-layering.md) | Curated (static, immutable) + session and connection overlays (dynamic); `OperationEnv` as trait-object integration point; `OperationContext.env` split into `scoped_env` (data) and `env` (dispatch trait) |
| Operation error schemas | [ADR-023](../../decisions/023-operation-error-schemas.md) | Operations declare domain errors; `call.error` carries typed `details`; adapter fidelity for `from_openapi`/`to_openapi` |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-13** (resolved): Operation path format is `/{service}/{op}`. Remote dispatch is a separate mechanism, not a path prefix.
- **OQ-14** (resolved): Batch is a client-side pattern of correlated `call.requested` events, not a protocol primitive.
- **OQ-16** (resolved by ADR-014): No vault operations are exposed over the call protocol for now.
- **OQ-19** (resolved): Session-scoped operation registries — agent-written operations overlaid on the curated registry via `OperationEnv` trait layering. Protocol doesn't need changes; `OperationEnv` must remain a trait. Session ops are `Session` provenance (ADR-022) — always `Internal`, compose under restricted authority scoped down at sandbox creation. Generalized by ADR-024 to cover connection-scoped overlays as well.

## References

- [call-protocol.md](call-protocol.md) — CallAdapter, EventEnvelope, stream model, PendingRequestMap
- ADR-005: irpc as call protocol foundation
- ADR-008: Vault integration point
- ADR-010: ALPN router and endpoint (static registration — applies to the `HandlerRegistry`, not the `OperationRegistry`; see ADR-024 for the distinction)
- ADR-012: Call protocol stream model
- ADR-024: Operation registry layering (curated + session/connection overlays; `OperationEnv` as trait-object integration point)
- Reference implementation: `/workspace/@alkdev/alknet-main/crates/alknet-core/src/call/`