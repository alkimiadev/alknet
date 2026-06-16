---
status: draft
last_updated: 2026-06-17
---

# Operation Registry

OperationSpec, Handler, OperationRegistry, AccessControl, service discovery, and irpc integration.

## What

The operation registry maps operation names to specs and handlers. It is the dispatch core of the call protocol — when a `call.requested` event arrives, the registry looks up the operation by name, checks access control, invokes the handler, and returns the result.

The registry is populated at startup by the CLI binary (or by the assembly layer in embedded contexts). Operations cannot be added or removed at runtime. This is consistent with OQ-04 (static registration at startup) and the `HandlerRegistry` model in alknet-core.

## Why

The operation registry provides:
- **Discoverability**: Clients can query `/services/list` and `/services/schema` to learn what operations exist before calling them
- **Access control**: Each operation declares its required scopes and resources; the registry enforces ACL before invoking the handler
- **Type safety**: JSON Schema for input and output enables validation and client code generation
- **Composability**: Handlers can invoke other operations through `OperationEnv` (local dispatch in Phase 1)

The registry design is derived from the `@alkdev/operations` TypeScript package, which provides the same capabilities in JavaScript runtimes. The Rust implementation preserves the behavioral contract: namespace + operation name → invoke with input, return output.

## Architecture

### OperationSpec

Every registered operation has a spec that declares its name, type, schemas, and access control:

```rust
pub struct OperationSpec {
    pub name: String,              // e.g., "fs/readFile", "vault/derive" (no leading slash)
    pub namespace: String,         // e.g., "fs", "vault"
    pub op_type: OperationType,   // Query, Mutation, Subscription
    pub input_schema: Value,      // JSON Schema for input
    pub output_schema: Value,     // JSON Schema for output
    pub access_control: AccessControl,
}

pub enum OperationType {
    Query,         // Read-only, idempotent (e.g., "fs/readFile", "services/list")
    Mutation,      // Side effects (e.g., "bash/exec", "vault/unlock")
    Subscription,  // Streaming (e.g., "events/subscribe")
}
```

Operation names use slash-based paths without a leading slash, aligned with URL path conventions: `fs/readFile`, `vault/derive`, `services/list`. The leading slash is added when needed for display (`spec.path()` returns `/fs/readFile`) and for wire format (the `call.requested` payload uses `/fs/readFile`). See OQ-13 for the path format decision (single-node `service/op` vs head/worker `node/service/op`).

The `namespace` field is derived from the name: for `fs/readFile` it's `fs`, for `vault/derive` it's `vault`. It's a convenience accessor for ACL matching and service grouping.

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
2. The registry checks `access_control.check(identity)` before invoking the handler
3. If access is denied, the adapter returns `call.error` with code `FORBIDDEN`
4. If the identity is `None` and the operation has restrictions, the adapter returns `call.error` with code `FORBIDDEN` and message `"authentication required"`

Operations with empty `AccessControl` (no required scopes, no resource checks) are accessible to all callers, including unauthenticated ones.

**Trusted calls skip ACL**: When a handler invokes another operation through `OperationEnv`, the nested call is marked `trusted: true` and skips access control checks. This prevents double-checking: if `/agent/chat` is allowed and it internally calls `/auth/verify`, the auth check is trusted.

### Handler

```rust
pub type Handler = Arc<dyn Fn(Value, OperationContext) -> Pin<Box<dyn Future<Output = ResponseEnvelope> + Send>> + Send + Sync>;
```

Handlers are async — many operations (vault key derivation, file I/O, irpc service calls) are inherently asynchronous. The handler receives an `async` runtime context and returns a `Future<Output = ResponseEnvelope>`.

A handler receives:
- `input: Value` — the deserialized `payload` from the `call.requested` event (always `serde_json::Value`)
- `context: OperationContext` — request ID, identity, metadata, env

And returns a `ResponseEnvelope` containing the result or an error.

### OperationContext

```rust
pub struct OperationContext {
    pub request_id: String,
    pub parent_request_id: Option<String>,
    pub identity: Option<Identity>,
    pub metadata: HashMap<String, Value>,
    pub env: OperationEnv,
    pub trusted: bool,
}
```

- `request_id`: Correlates with the `call.requested` event's `id` field
- `parent_request_id`: Set when this call was initiated by another operation (via `OperationEnv`)
- `identity`: The authenticated identity making the call (from `IdentityProvider`)
- `metadata`: Additional context (connection info, tracing IDs)
- `env`: The operation environment for composing calls to other operations
- `trusted`: When `true`, ACL checks are skipped (set by `OperationEnv`, not by callers). The `trusted` field uses module-private construction — handlers construct `OperationContext` through `OperationEnv::invoke()` which sets `trusted: true`, or through the `CallAdapter` dispatch path which sets `trusted: false`. The field is not `pub` for writes; only `pub fn is_trusted(&self) -> bool` is exposed for reads.

### OperationRegistry

```rust
pub struct OperationRegistry {
    operations: HashMap<String, (OperationSpec, Handler)>,
}
```

The registry maps operation names to `(OperationSpec, Handler)` pairs. Key methods:

- `register(spec, handler)`: Add an operation at startup
- `lookup(name)`: Find an operation by name, returning spec and handler
- `invoke(name, input, context)`: Look up, check ACL, invoke handler, return result
- `list_operations()`: Return all registered specs (for `/services/list`)

The `OperationRegistryBuilder` provides a fluent API for constructing the registry at startup:

```rust
let registry = OperationRegistryBuilder::new()
    .with(services_list_spec(), Arc::new(services_list_handler))
    .with(services_schema_spec(), Arc::new(schema_handler))
    .with(vault_derive_spec(), Arc::new(vault_derive_handler))
    .with(vault_unlock_spec(), Arc::new(vault_unlock_handler))
    .build();
```

The CLI binary (or assembly layer) constructs the registry and passes it to the `CallAdapter`. Once built, the registry is immutable.

### OperationEnv

```rust
#[async_trait]
pub trait OperationEnv: Send + Sync {
    async fn invoke(&self, namespace: &str, operation: &str, input: Value, parent: &OperationContext) -> ResponseEnvelope;
}
```

`OperationEnv` is the universal composition mechanism. A handler calls `context.env.invoke("vault", "derive", input, &context)` and gets a `ResponseEnvelope` back — regardless of whether the operation runs locally, via an irpc service, or on a remote node.

The `parent` parameter propagates the calling context: the nested call gets `parent_request_id: Some(parent.request_id)`, inherits `parent.identity`, and is marked `trusted: true`.

**Phase 1: Local dispatch only.** The initial `OperationEnv` implementation dispatches directly through the local `OperationRegistry`:

```rust
pub struct LocalOperationEnv {
    registry: Arc<OperationRegistry>,
}

#[async_trait]
impl OperationEnv for LocalOperationEnv {
    async fn invoke(&self, namespace: &str, operation: &str, input: Value, parent: &OperationContext) -> ResponseEnvelope {
        let name = format!("/{namespace}/{operation}");
        let context = OperationContext {
            request_id: format!("env-{name}"),
            parent_request_id: Some(parent.request_id.clone()),
            identity: parent.identity.clone(),  // Inherit caller's identity
            metadata: parent.metadata.clone(),   // Inherit caller's metadata
            env: self.clone(),
            trusted: true,                       // Nested calls skip ACL
        };
        self.registry.invoke(&name, input, context).await
    }
}
```

Future phases add irpc service dispatch and remote call protocol dispatch as additional backends. The handler-facing API stays the same.

### Service Discovery

Two built-in operations expose what the node offers:

| Operation name | Display path | Type | Description |
|---------------|-------------|------|-------------|
| `services/list` | `/services/list` | Query | List registered operation names and metadata |
| `services/schema` | `/services/schema` | Query | Get the `OperationSpec` for a specific operation |

These are read-only — no admin operations are exposed through the call protocol itself.

`services/list` returns:

```json
{
  "operations": [
    { "name": "fs/readFile", "namespace": "fs", "op_type": "query" },
    { "name": "vault/derive", "namespace": "vault", "op_type": "mutation" },
    { "name": "events/subscribe", "namespace": "events", "op_type": "subscription" }
  ]
}
```

`services/schema` accepts `{ "name": "fs/readFile" }` and returns the full `OperationSpec` including input/output JSON Schemas.

### irpc Integration

irpc and the operation registry serve different scopes:

| Layer | Mechanism | Serialization | Scope |
|-------|-----------|---------------|-------|
| Call protocol (external) | `EventEnvelope` over QUIC streams | JSON | Cross-language, cross-node |
| irpc services (internal) | `VaultProtocol` derive macro, `Service` trait | postcard (binary) | Rust-to-Rust, in-process or in-cluster |
| Local dispatch (in-process) | Direct function call through `OperationRegistry` | None | Same process |

The call protocol can wrap irpc services. When `/vault/derive` receives a `call.requested` event, the handler:
1. Deserializes the JSON payload
2. Calls `VaultProtocol::DeriveEd25519` via irpc (in-process, type-safe, postcard)
3. Serializes the result back to JSON
4. Returns `call.responded` on the stream

This layering preserves irpc's type safety for internal calls while keeping the external interface cross-language.

### Operation Registration at Startup

The CLI binary (or assembly layer) registers operations before starting the endpoint:

```rust
let registry = OperationRegistryBuilder::new()
    // Built-in service discovery
    .with(services_list_spec(), Arc::new(services_list_handler))
    .with(services_schema_spec(), Arc::new(services_schema_handler))
    // Vault operations (exposed via call protocol, backed by irpc)
    .with(vault_derive_spec(), Arc::new(vault_derive_handler))
    .with(vault_unlock_spec(), Arc::new(vault_unlock_handler))
    .with(vault_lock_spec(), Arc::new(vault_lock_handler))
    .build();

let call_adapter = CallAdapter::new(Arc::new(registry), identity_provider);
```

The registry is immutable after construction. Adding operations requires restarting the process. This is consistent with OQ-04 and the `HandlerRegistry` model in alknet-core.

## Constraints

- The registry is immutable after construction. No runtime registration or deregistration. Two-way door — `ArcSwap<OperationRegistry>` can be added later.
- Operation specs use JSON Schema. The call protocol's external interface is always JSON. irpc's postcard serialization is internal only.
- Phase 1 is local dispatch only. `OperationEnv::invoke()` goes through the local registry. irpc service dispatch and remote call protocol dispatch are contracted but not built.
- The call protocol does not depend on any database. Operation specs are in-memory, populated at startup.
- `OperationContext.trusted` is set by `OperationEnv`, not by callers. A handler cannot mark its own call as trusted.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| irpc as call protocol foundation | [ADR-005](../../decisions/005-irpc-as-call-protocol-foundation.md) | irpc provides framing and service dispatch |
| Call protocol stream model | [ADR-012](../../decisions/012-call-protocol-stream-model.md) | Bidirectional streams, EventEnvelope, ID-based correlation |
| Static handler registration | [ADR-010](../../decisions/010-alpn-router-and-endpoint.md) | Registry is immutable after construction |
| Vault integration via call protocol | [ADR-008](../../decisions/008-secret-service-integration.md) | Vault ops exposed as call protocol operations |

## Open Questions

- **OQ-13**: Operation path format — `/{service}/{op}` for Phase 1 (single-node), with the node prefix `/{node}/{service}/{op}` added when remote dispatch is implemented. Two-way door — the prefix can be added later without breaking existing operations.
- **OQ-14**: Batch operation semantics — whether to add batch-specific event types or rely on the "multiple call.requested with correlated IDs" pattern. Two-way door — can be added later.

## References

- [call-protocol.md](call-protocol.md) — CallAdapter, EventEnvelope, stream model, PendingRequestMap
- ADR-005: irpc as call protocol foundation
- ADR-008: Vault integration point
- ADR-010: ALPN router and endpoint (static registration)
- ADR-012: Call protocol stream model
- Reference implementation: `/workspace/@alkdev/alknet-main/crates/alknet-core/src/call/`