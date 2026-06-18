---
status: draft
last_updated: 2026-06-18
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
    pub input_schema: Value,      // JSON Schema for input
    pub output_schema: Value,     // JSON Schema for output
    pub access_control: AccessControl,
}

pub enum OperationType {
    Query,         // Read-only, idempotent (e.g., "fs/readFile", "services/list")
    Mutation,      // Side effects (e.g., "bash/exec", "github/authenticate")
    Subscription,  // Streaming (e.g., "agent/chat", "events/subscribe")
}
```

Operation names use slash-based paths without a leading slash, aligned with URL path conventions: `fs/readFile`, `agent/chat`, `services/list`. The leading slash is added when needed for display (`spec.path()` returns `/fs/readFile`) and for wire format (the `call.requested` payload uses `/fs/readFile`). See OQ-13 for the path format decision (single-node `service/op` vs head/worker `node/service/op`).

The `namespace` field is derived from the name: for `fs/readFile` it's `fs`, for `agent/chat` it's `agent`. It's a convenience accessor for ACL matching and service grouping.

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

Handlers are async — many operations (file I/O, HTTP service calls, irpc service calls) are inherently asynchronous. The handler receives an `async` runtime context and returns a `Future<Output = ResponseEnvelope>`.

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
    pub capabilities: Capabilities,
    pub metadata: HashMap<String, Value>,
    pub env: OperationEnv,
    pub trusted: bool,
}
```

- `request_id`: Correlates with the `call.requested` event's `id` field
- `parent_request_id`: Set when this call was initiated by another operation (via `OperationEnv`)
- `identity`: The authenticated identity making the call (from `IdentityProvider`) — inbound auth (who is calling me)
- `capabilities`: Outbound credentials the handler may use (decrypted API keys, scoped vault access) — see [Capability Injection](#capability-injection) below
- `metadata`: Additional context (connection info, tracing IDs). **Must not hold secret material** — see ADR-014
- `env`: The operation environment for composing calls to other operations
- `trusted`: When `true`, ACL checks are skipped (set by `OperationEnv`, not by callers). The `trusted` field uses module-private construction — handlers construct `OperationContext` through `OperationEnv::invoke()` which sets `trusted: true`, or through the `CallAdapter` dispatch path which sets `trusted: false`. The field is not `pub` for writes; only `pub fn is_trusted(&self) -> bool` is exposed for reads.

`identity` and `capabilities` are orthogonal: identity is inbound (resolved per-request from the caller's credentials), capabilities are outbound (provisioned by the assembly layer from the vault). See ADR-014 for the full rationale.

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
    .with(agent_chat_spec(), Arc::new(agent_chat_handler))
    .build();
```

The CLI binary (or assembly layer) constructs the registry and passes it to the `CallAdapter`. Handlers are constructed with injected capabilities (see [Capability Injection](#capability-injection)) before registration. Once built, the registry is immutable.

### OperationEnv

```rust
#[async_trait]
pub trait OperationEnv: Send + Sync {
    async fn invoke(&self, namespace: &str, operation: &str, input: Value, parent: &OperationContext) -> ResponseEnvelope;
}
```

`OperationEnv` is the universal composition mechanism. A handler calls `context.env.invoke("fs", "readFile", input, &context)` and gets a `ResponseEnvelope` back — regardless of whether the operation runs locally, via an irpc service, or on a remote node.

The `parent` parameter propagates the calling context: the nested call gets `parent_request_id: Some(parent.request_id)`, inherits `parent.identity`, and is marked `trusted: true`.

**Local dispatch only.** The initial `OperationEnv` implementation dispatches directly through the local `OperationRegistry`:

```rust
pub struct LocalOperationEnv {
    registry: Arc<OperationRegistry>,
}

#[async_trait]
impl OperationEnv for LocalOperationEnv {
    async fn invoke(&self, namespace: &str, operation: &str, input: Value, parent: &OperationContext) -> ResponseEnvelope {
        let name = format!("{namespace}/{operation}");
        let context = OperationContext {
            request_id: format!("env-{name}"),
            parent_request_id: Some(parent.request_id.clone()),
            identity: parent.identity.clone(),       // Inherit caller's identity
            capabilities: parent.capabilities.clone(), // Inherit caller's capabilities
            metadata: parent.metadata.clone(),       // Inherit caller's metadata
            env: self.clone(),
            trusted: true,                           // Nested calls skip ACL
        };
        self.registry.invoke(&name, input, context).await
    }
}
```

Future work may add irpc service dispatch and remote call protocol dispatch as additional backends. The handler-facing API stays the same.

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
    { "name": "agent/chat", "namespace": "agent", "op_type": "subscription" },
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

irpc services are an internal dispatch mechanism — they are not directly exposed on the call protocol. The vault's `VaultProtocol` uses irpc for in-process, type-safe dispatch via `VaultServiceHandle` (postcard serialization for in-cluster, direct calls for in-process). The vault is accessed by the assembly layer (CLI binary) at startup, not by handlers at call time. See ADR-008 and ADR-014.

If a handler internally uses an irpc-based service, the handler bridges the two: it receives JSON input from the call protocol, calls the irpc service in-process (postcard, type-safe), and serializes the result back to JSON for the call protocol response. This layering preserves irpc's type safety for internal calls while keeping the external interface cross-language.

### Operation Registration at Startup

The CLI binary (or assembly layer) constructs handlers with the credentials they need (from the vault — see [Capability Injection](#capability-injection)), then registers them before starting the endpoint:

```rust
// Assembly layer: unlock vault, derive credentials, construct handlers
let vault = VaultServiceHandle::new();
vault.unlock(&mnemonic, passphrase.as_deref())?;
let google_api_key = vault.decrypt(&google_key_blob)?;
let github_signing_key = vault.derive_ed25519(PATHS::GITHUB_SIGNING)?;

// Construct handlers with injected capabilities
let agent_handler = Arc::new(agent_chat_handler(Capabilities::new()
    .with_api_key("google", google_api_key)));
let github_handler = Arc::new(github_authenticate_handler(Capabilities::new()
    .with_signing_key(github_signing_key)));

// Register operations — vault operations are NOT registered here
let registry = OperationRegistryBuilder::new()
    // Built-in service discovery
    .with(services_list_spec(), Arc::new(services_list_handler))
    .with(services_schema_spec(), Arc::new(schema_handler))
    // Agent and GitHub handlers (constructed with injected capabilities)
    .with(agent_chat_spec(), agent_handler)
    .with(github_authenticate_spec(), github_handler)
    .build();

let call_adapter = CallAdapter::new(Arc::new(registry), identity_provider);
```

The vault is used at construction time, not registered as call protocol operations. The registry is immutable after construction. Adding operations requires restarting the process. This is consistent with OQ-04, ADR-008, and ADR-014.

### Capability Injection

Handlers that need outbound credentials (LLM provider API keys, signing keys, HTTP service tokens) receive them through the `Capabilities` type on `OperationContext`, not by calling vault operations over the wire and not from environment variables. This is the mechanism that ADR-008 described in prose ("derived keys and decrypted credentials are injected into operation contexts at the assembly layer") and that ADR-014 specifies as a one-way door.

The flow is:

```
Assembly layer (CLI startup):
  1. Unlock vault (local, mnemonic from secure prompt or file)
  2. Derive / decrypt the credentials each handler needs
  3. Construct handlers with those credentials
  4. Register operations with the constructed handlers
  5. Start the endpoint

Handler invocation (at call time):
  call.requested → OperationContext { capabilities, identity, ... }
  handler reads capabilities → uses the credential for its outbound call
```

The `Capabilities` type holds non-serializable, zeroized secret material. It does not implement `Serialize` — it cannot cross the call protocol wire even by accident. The concrete shape of the type (a typed map, a struct with named fields, a trait object) is a two-way door for implementation. The one-way constraints are fixed by ADR-014:

- Capabilities are populated by the assembly layer at handler construction (the common case: a static decrypted API key) or scoped per-request for trusted-internal-only flows. They are never populated from call protocol inputs.
- Capabilities hold secret material that does not implement `Serialize` and does not appear in `EventEnvelope` payloads.
- The call protocol carries no secret material. See [call-protocol.md](call-protocol.md) for the wire-level constraint.

**No vault operations are registered in the call protocol.** The vault is assembly-layer only (ADR-008, ADR-014). A handler that needs a child key for a specific operation (e.g., signing for GitHub auth) receives a scoped capability that performs the derivation in-process — it never holds the master seed and never calls a network-exposed vault operation.

**Adapters take credential sources.** The `from_openapi` and `from_jsonschema` adapter patterns (see OQ-15, constrained by ADR-014) register HTTP-backed operations. The credential the HTTP service needs (bearer token, API key) is provided by the assembly layer at registration time — the adapter receives a credential source, not a static token string. This is the integration point where the vault feeds credentials into HTTP-backed operations, including LLM providers that expose OpenAPI-compatible endpoints.

## Constraints

- The registry is immutable after construction. No runtime registration or deregistration. Two-way door — `ArcSwap<OperationRegistry>` can be added later.
- Operation specs use JSON Schema. The call protocol's external interface is always JSON. irpc's postcard serialization is internal only.
- `OperationEnv::invoke()` dispatches through the local registry. Remote dispatch (federation, head/worker routing) would be a separate mechanism at a different layer — not a prefix added to operation paths. irpc service dispatch is contracted but not built.
- The call protocol does not depend on any database. Operation specs are in-memory, populated at startup.
- `OperationContext.trusted` is set by `OperationEnv`, not by callers. A handler cannot mark its own call as trusted.
- **No vault operations are registered in the call protocol.** The vault is assembly-layer only (ADR-008, ADR-014). Handlers receive secret material through `OperationContext.capabilities`, not by calling vault operations over the wire.
- **The call protocol carries no secret material.** Secret material (private keys, API keys, mnemonics, decrypted credentials) must not appear in `call.requested` payloads, `call.responded` payloads, or `OperationContext.metadata`. See ADR-014.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| irpc as call protocol foundation | [ADR-005](../../decisions/005-irpc-as-call-protocol-foundation.md) | irpc provides framing and service dispatch |
| Call protocol stream model | [ADR-012](../../decisions/012-call-protocol-stream-model.md) | Bidirectional streams, EventEnvelope, ID-based correlation |
| Static handler registration | [ADR-010](../../decisions/010-alpn-router-and-endpoint.md) | Registry is immutable after construction |
| Vault integration via assembly layer | [ADR-008](../../decisions/008-secret-service-integration.md) | Vault is a capability source, accessed at assembly time |
| Secret material flow and capability injection | [ADR-014](../../decisions/014-secret-material-flow-and-capability-injection.md) | Capabilities carry outbound credentials; call protocol carries no secret material |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-13** (resolved): Operation path format is `/{service}/{op}`. Remote dispatch is a separate mechanism, not a path prefix.
- **OQ-14** (resolved): Batch is a client-side pattern of correlated `call.requested` events, not a protocol primitive.
- **OQ-15** (open): Call protocol client and adapter contract. ADR-014 constrains the adapter contract: adapters take credential sources from the assembly layer, not static tokens.
- **OQ-16** (resolved by ADR-014): No vault operations are exposed over the call protocol for now.

## References

- [call-protocol.md](call-protocol.md) — CallAdapter, EventEnvelope, stream model, PendingRequestMap
- ADR-005: irpc as call protocol foundation
- ADR-008: Vault integration point
- ADR-010: ALPN router and endpoint (static registration)
- ADR-012: Call protocol stream model
- Reference implementation: `/workspace/@alkdev/alknet-main/crates/alknet-core/src/call/`