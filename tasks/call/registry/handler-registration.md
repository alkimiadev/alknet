---
id: call/registry/handler-registration
name: Implement Handler, HandlerRegistration, OperationProvenance, OperationRegistry, and OperationRegistryBuilder
status: completed
depends_on: [call/registry/operation-context]
scope: broad
risk: medium
impact: component
level: implementation
---

## Description

Implement the handler registration types and the operation registry in
`src/registry/registration.rs`. The registry maps operation names to
registration bundles and provides the dispatch entry point.

### Handler

```rust
pub type Handler = Arc<
    dyn Fn(Value, OperationContext) -> Pin<Box<dyn Future<Output = ResponseEnvelope> + Send>>
        + Send + Sync
>;
```

Handlers are async. They receive:
- `input: Value` — deserialized payload from `call.requested` (always `serde_json::Value`)
- `context: OperationContext` — request ID, identity, metadata, env

And return `ResponseEnvelope` (defined in protocol/wire task — use a forward
reference or define a minimal version here, full impl in the wire task).

### HandlerRegistration

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

The registration bundle carries everything the dispatch path needs to
construct an `OperationContext`. See ADR-022.

### OperationProvenance

```rust
pub enum OperationProvenance {
    Local,           // Assembly-written, trusted, can compose
    FromOpenAPI,     // HTTP forwarding stub, leaf
    FromMCP,         // MCP forwarding stub, leaf
    FromCall,        // QUIC forwarding stub, leaf locally
    FromJsonSchema,  // JSON Schema definition, no handler — schema only
    Session,         // Agent-written, sandboxed, can compose within sandbox
}
```

| Provenance | Can compose? | Has composition authority? | Default visibility |
|-----------|-------------|---------------------------|-------------------|
| `Local` | Yes | Yes | External or Internal (assembly declares) |
| `FromOpenAPI` | No (leaf) | No | Internal |
| `FromMCP` | No (leaf) | No | Internal |
| `FromCall` | No (leaf in local registry) | No | Internal |
| `FromJsonSchema` | N/A (no handler) | No | N/A |
| `Session` | Yes (within sandbox) | Yes | Internal always |

### OperationRegistry

```rust
pub struct OperationRegistry {
    operations: HashMap<String, HandlerRegistration>,
}
```

The curated layer (Layer 0) is a `HashMap<String, HandlerRegistration>`. Session
and connection overlays (Layers 1 and 2) are separate maps composed into the
per-call `OperationContext.env` by the CallAdapter (ADR-024).

Methods:
- `register(registration)`: add to curated layer at startup
- `registration(name)`: find by operation name (checks active overlays first,
  then curated base — ADR-024). Returns spec, handler, provenance, composition
  authority, scoped env, capabilities.
- `invoke(name, input, context)`: look up, check ACL, invoke handler, return result
- `list_operations()`: return all registered specs (for `/services/list` —
  returns curated + active overlay ops, External only)

### OperationRegistryBuilder

Fluent API with convenience methods:

```rust
pub struct OperationRegistryBuilder {
    operations: HashMap<String, HandlerRegistration>,
}

impl OperationRegistryBuilder {
    pub fn new() -> Self;

    // with_local: Local provenance, full bundle — all 5 args required
    pub fn with_local(
        mut self,
        spec: OperationSpec,
        handler: Handler,
        composition_authority: Option<CompositionAuthority>,
        scoped_env: Option<ScopedOperationEnv>,
        capabilities: Capabilities,
    ) -> Self;

    // with_leaf: leaf provenance (FromOpenAPI/FromMCP/FromCall), no authority, no scoped env
    pub fn with_leaf(
        mut self,
        spec: OperationSpec,
        handler: Handler,
        capabilities: Capabilities,
    ) -> Self;

    // with: full manual registration (any provenance)
    pub fn with(mut self, registration: HandlerRegistration) -> Self;

    pub fn build(self) -> OperationRegistry;
}
```

`with_local` sets `provenance: Local`. `with_leaf` sets `provenance: FromOpenAPI`
(or a parameter), `composition_authority: None`, `scoped_env: None`. `with` takes
the full bundle for any provenance.

### Registry invoke flow

```rust
impl OperationRegistry {
    pub async fn invoke(&self, name: &str, input: Value, context: OperationContext) -> ResponseEnvelope {
        // 1. Look up registration by name
        // 2. Check visibility: if Internal and context is external (internal: false), return NOT_FOUND
        // 3. Check ACL: access_control.check(identity or handler_identity depending on internal flag)
        // 4. If denied: return FORBIDDEN
        // 5. Invoke handler: (handler)(input, context).await
        // 6. Return ResponseEnvelope
    }
}
```

The ACL authority depends on `context.internal`:
- `internal: false` (wire call): check against `context.identity` (caller)
- `internal: true` (composition): check against `context.handler_identity.as_identity()`

### Layer 0 immutability

The curated layer (Layer 0 — `Local` provenance ops) is immutable after
construction. Adding a `Local` op requires restarting the process. Session and
imported overlays are dynamic at their respective scopes (ADR-024). The
`OperationRegistryBuilder` is Layer-0-only; runtime overlay registration uses
`CallConnection::register_imported()` (in the protocol/connection task).

## Acceptance Criteria

- [ ] `Handler` type alias (async closure returning ResponseEnvelope)
- [ ] `HandlerRegistration` struct with all 6 fields
- [ ] `OperationProvenance` enum with all 6 variants
- [ ] `OperationRegistry` struct with operations HashMap
- [ ] `OperationRegistry::register()` adds to curated layer
- [ ] `OperationRegistry::registration()` looks up by name
- [ ] `OperationRegistry::invoke()` checks visibility, ACL, invokes handler
- [ ] `OperationRegistry::list_operations()` returns External specs only
- [ ] `OperationRegistryBuilder` with `new()`, `with_local()`, `with_leaf()`, `with()`, `build()`
- [ ] `with_local` sets provenance Local, requires all 5 args
- [ ] `with_leaf` sets provenance leaf, composition_authority None, scoped_env None
- [ ] invoke: Internal op called externally → NOT_FOUND (not FORBIDDEN)
- [ ] invoke: ACL denied → FORBIDDEN
- [ ] invoke: internal: true → ACL against handler_identity, not identity
- [ ] invoke: internal: false → ACL against identity
- [ ] Unit test: register and invoke a simple operation
- [ ] Unit test: Internal op returns NOT_FOUND from external call
- [ ] Unit test: ACL check with sufficient scopes → Allowed
- [ ] Unit test: ACL check with insufficient scopes → Forbidden
- [ ] Unit test: builder with_local and with_leaf produce correct provenance
- [ ] `cargo test -p alknet-call` succeeds
- [ ] `cargo clippy -p alknet-call` succeeds with no warnings

## References

- docs/architecture/crates/call/operation-registry.md — Handler, HandlerRegistration, OperationRegistry, builder
- docs/architecture/decisions/022-handler-registration-provenance-and-composition-authority.md — ADR-022
- docs/architecture/decisions/024-operation-registry-layering.md — ADR-024 (layering, immutability)

## Notes

> The registry is the dispatch core. The ACL authority switch (internal: true
> → handler_identity, internal: false → identity) is the ADR-015 privilege
> model — get this right. Internal ops return NOT_FOUND from the wire (don't
> leak existence), not FORBIDDEN. The builder is Layer-0-only; runtime overlay
> registration is via CallConnection (protocol task).

## Summary

Implemented `Handler`, `HandlerRegistration`, `OperationProvenance`,
`OperationRegistry` (register/registration/invoke/list_operations), and
`OperationRegistryBuilder` (new/with_local/with_leaf/with_leaf_provenance/with/build)
in `registry/registration.rs`. `invoke` enforces visibility (Internal from wire →
NOT_FOUND), ACL with authority switch (internal: true → handler_identity.as_identity(),
internal: false → caller identity), and handler dispatch. 21 unit tests pass; clippy
clean. Merged to develop.