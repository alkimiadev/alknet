---
status: draft
last_updated: 2026-06-29
---

# HTTP Adapters — from_openapi and to_openapi

The OpenAPI-direction adapters: `from_openapi` imports external HTTP APIs
as call-protocol operations (reqwest-backed forwarding handlers), and
`to_openapi` generates an OpenAPI spec from the local registry's
`External` operations. This document covers both, the error fidelity
(ADR-023), and the no-env-vars credential injection point.

## What

Two adapters, both in `alknet-http`:

1. **`from_openapi`** — parses an OpenAPI document, constructs a
   `HandlerRegistration` bundle per OpenAPI operation with a forwarding
   handler that calls the external HTTP endpoint via `reqwest`, and
   returns the bundles for registration in the `OperationRegistry`. The
   adapter implements `OperationAdapter` (the async trait from
   `alknet-call`, ADR-017 §5). Provenance is `FromOpenAPI` (leaf,
   `composition_authority: None`, `scoped_env: None`, `Internal` by
   default — ADR-015/022).
2. **`to_openapi`** — generates an OpenAPI document from the local
   registry's `External` operations. A pure projection: it consumes the
   registry, it does not produce entries for it (ADR-017 §5 — the `to_*`
   adapters are outbound projections, not `OperationAdapter`
   implementations). Served at `GET /openapi.json` by the HTTP server.

### from_openapi

```rust
pub struct FromOpenAPI {
    spec: OpenAPISpec,
    config: HttpServiceConfig,
}

#[async_trait]
impl OperationAdapter for FromOpenAPI {
    async fn import(&self) -> Result<Vec<HandlerRegistration>, AdapterError>;
}
```

#### Type definitions

```rust
/// A parsed OpenAPI document. The concrete type is a two-way-door
/// implementation detail (openapiv3::OpenApi, a local alknet-http
/// type, or a serde_json::Value-based parse); the one-way constraint is
/// that `from_openapi` accepts a standard OpenAPI 3.x JSON/YAML doc and
/// `to_openapi` produces one. Both directions share the same type.
pub struct OpenAPISpec {
    pub info: OpenAPIInfo,
    pub paths: BTreeMap<String, PathItem>,
    pub components: Option<Components>,
    // ... OpenAPI 3.x fields as needed
}

/// Configuration for an HTTP-backed adapter (`from_openapi`). Carries
/// the base URL, auth credentials (from `Capabilities` at registration,
/// not env vars — the no-env-vars invariant), and optional headers. The
/// `auth` field is the auth scheme the external API expects (bearer,
/// apiKey, basic); the credential itself is read from
/// `OperationContext.capabilities` at call time, not stored here.
pub struct HttpServiceConfig {
    pub namespace: String,
    pub base_url: String,
    pub auth: Option<HttpAuthScheme>,
    pub default_headers: HashMap<String, String>,
}

pub enum HttpAuthScheme {
    Bearer,                          // Authorization: Bearer <token>
    ApiKey { header_name: String },  // e.g., X-API-Key: <key>
    Basic,                           // Authorization: Basic <credentials>
}
```

The adapter:

1. Parses the OpenAPI document (`OpenAPISpec` — `paths`, `components`,
   `$ref` resolution). On parse failure, returns
   `AdapterError::SchemaParse`. The TS prior art
   (`@alkdev/operations/src/from_openapi.ts`) shows the parsing patterns:
   `resolveRef` for `$ref`, `resolveRefsRecursive` for nested refs,
   `buildInputSchema` (parameters + request body → input JSON Schema),
   `buildOutputSchema` (200/201 response → output JSON Schema),
   `detectOperationType` (SSE response → `Subscription`, GET → `Query`,
   else `Mutation`).
2. For each `(path, method, operation)` in `spec.paths`, constructs a
   `HandlerRegistration`:
   - `spec.name` = the `operationId` (or a generated
     `${method}_${path_parts}` name if `operationId` is absent — same
     normalization as the TS `normalizeOperationId`).
   - `spec.namespace` = the `config.namespace` (the importing
     deployment's name for the service, not the OpenAPI doc's `info.title`).
   - `spec.op_type` = `Query` / `Mutation` / `Subscription` (detected
     from the method + response content type, same as TS).
   - `spec.visibility` = `Internal` (adapter-registered ops are
     composition material, not directly callable from the wire —
     ADR-015).
   - `spec.input_schema` / `output_schema` = the JSON Schemas built
     from the OpenAPI parameters/responses.
   - `spec.error_schemas` = the `ErrorDefinition`s built from the
     non-2xx OpenAPI responses (ADR-023 §5 — see Error Fidelity below).
   - `spec.access_control` = `AccessControl::default()` (the adapter
     doesn't declare scopes; the composing handler that reaches the
     imported op gates access).
   - `handler` = a forwarding handler (see Forwarding Handler below).
   - `provenance` = `FromOpenAPI`, `composition_authority: None`,
     `scoped_env: None` (leaf — ADR-022).
   - `capabilities` = the credentials the forwarding handler needs (the
     bearer token / API key for the external HTTP endpoint, injected by
     the assembly layer at registration — see No-Env-Vars below).
3. Returns the bundles. The caller (the assembly layer) registers them
   in the `OperationRegistry`.

### Forwarding handler

The forwarding handler is the `Arc<dyn Handler>` stored in the
`HandlerRegistration`. At call time, it:

1. Reads the call input (`serde_json::Value`).
2. Builds the outbound HTTP request:
   - URL path: substitutes path parameters (`{id}` → input value),
     appends query parameters from input fields not in the path.
   - Method: the OpenAPI operation's method.
   - Headers: `Content-Type: application/json` + the auth header built
     from `context.capabilities` (see No-Env-Vars below).
   - Body: the `body` field of the input (for `Mutation`/`Subscription`).
3. Sends the request via the shared `reqwest::Client` (see HTTP Client
   below).
4. For a `Query`/`Mutation`: parses the response body (JSON, text, or
   binary — same content-type branching as the TS `createHTTPOperation`),
   wraps it in a `ResponseEnvelope`, returns.
5. For a `Subscription` (`text/event-stream` response): streams
   `call.responded` events as the SSE chunks arrive (same SSE parsing as
   the TS `parseSSEFrames`), then `call.completed` on stream end.
6. On HTTP error (non-2xx): maps to the declared `ErrorDefinition` by
   HTTP status code (see Error Fidelity below), returns a `CallError`.

The handler is opaque to the `CallAdapter` — it's an `Arc<dyn Handler>`
the registry dispatches. `alknet-call` never sees `reqwest`.

### HTTP client (reqwest)

`alknet-http` maintains a shared `reqwest::Client` (constructed once,
reused across all `from_openapi`/`from_mcp` forwarding handlers). The
client handles connection pooling, keep-alive, and TLS. The aisdk
`core/client.rs` reference shows the pattern worth referencing: a shared
client with `OnceLock<reqwest::Client>`, retry logic (exponential
backoff, `Retry-After` header), and separate streaming vs non-streaming
clients. `alknet-http` owns its HTTP client; it does not inherit aisdk's.

The retry/pooling config comes from `StaticConfig` or `DynamicConfig`
(hot-reloadable). The credential injection happens per-request (from
`OperationContext.capabilities`), not at client construction — the
client is shared across all operations, the credentials are per-call.

The exact pooling/retry config is a two-way-door implementation detail
(OQ-40); the one-way constraint is that `alknet-http` owns its `reqwest`
client (no env-var-based client config, no shared global client).

### No-Env-Vars credential injection

The forwarding handler is the **credential injection point** for the
no-env-vars architecture. The handler reads
`context.capabilities.get("<service>")` (e.g., `"openai"`, `"vastai"`,
`"github"`), extracts the credential, and injects it into the outbound
HTTP request:

- Bearer token → `Authorization: Bearer <token>`.
- API key → the header the OpenAPI spec declares (e.g., `X-API-Key:
  <key>`, or `Authorization: ApiKey <key>` — the `HTTPServiceConfig.auth`
  in the TS prior art shows the three auth types: `bearer`, `apiKey`,
  `basic`).
- Basic auth → `Authorization: Basic <credentials>`.

The credential comes from `Capabilities`, which was populated by the
dispatch path from the `HandlerRegistration.capabilities` bundle
(ADR-022 §6), which was populated by the assembly layer from the vault
(ADR-014). The handler never reads `std::env::var`. This is the
spec-level invariant: no handler reads outbound credentials from any
source other than `OperationContext.capabilities`. See
[overview.md](overview.md) and
[client-and-adapters.md](../call/client-and-adapters.md).

### to_openapi

```rust
pub fn to_openapi(registry: &OperationRegistry) -> OpenAPISpec;
```

`to_openapi` generates an OpenAPI document from the local registry's
`External` operations:

1. For each `External` operation in the registry, generate an OpenAPI
   path entry:
   - Path: `/{service}/{op}` (the operation path, ADR-036 — the HTTP
     path IS the operation path).
   - Method: the operation's `OperationType` → HTTP method (`Query`→GET,
     `Mutation`→POST by default, `Subscription`→GET with
     `text/event-stream` response).
   - `operationId`: the operation name.
   - `parameters` / `requestBody` / `responses`: built from the
     operation's `input_schema` / `output_schema` / `error_schemas`.
2. The `components.schemas` section holds the reusable schemas
   referenced by `$ref` from the paths.
3. The `info` section carries the API title, version, and description.

This is a pure projection — it consumes the registry and produces a
spec. It does not modify the registry; it does not register operations;
it is not an `OperationAdapter`. The HTTP server serves the generated
spec at `GET /openapi.json` (or a configured path).

### Error Fidelity (ADR-023)

`from_openapi` maps OpenAPI non-2xx response status codes to
`ErrorDefinition`s (ADR-023 §5). The normative rule (review #002 W20):
`from_openapi` must not produce error codes that collide with the five
protocol-level codes (`NOT_FOUND`, `FORBIDDEN`, `INVALID_INPUT`,
`INTERNAL`, `TIMEOUT`). The adapter prefixes imported error codes with
`HTTP_` and the status number:

```rust
// OpenAPI: 404: { schema: NotFoundError }
// → ErrorDefinition { code: "HTTP_404", http_status: Some(404), schema: NotFoundError }
```

`to_openapi` projects `error_schemas` back to OpenAPI response
definitions:

```yaml
responses:
  '200': { schema: <output_schema> }
  '404': { schema: <error_schemas[i].schema> }   # where http_status = 404
  '429': { schema: <error_schemas[j].schema> }   # where http_status = 429
```

This makes the adapter contract from ADR-017 faithful on the error axis —
no silent dropping of error contracts. See ADR-023.

## Why

`from_openapi` is how alknet composes external HTTP APIs (OpenAI,
Anthropic, vast.ai, GitHub) into the call protocol. An operation
imported via `from_openapi` is a first-class operation: it has a spec,
it's discoverable via `services/list`, it can be composed by handlers,
its errors are typed. The agent crate's LLM provider calls go through
`from_openapi`-imported operations — that's how the no-env-vars
invariant makes aisdk's env-var reads unreachable.

`to_openapi` is how external systems discover the alknet operation
surface. An API gateway, a client generator, or a human developer reads
the OpenAPI doc to learn what operations exist and how to call them.
The generated spec is a compatibility contract (ADR-017 Consequences) —
once published, the mapping is one-way.

## Constraints

- **`from_openapi`/`from_mcp` handlers read credentials from
  `OperationContext.capabilities`, not `std::env::var`.** This is the
  no-env-vars invariant (ADR-014). The handler implementations are
  verified against this invariant.
- **`from_openapi`-registered ops are `Internal` by default.** They are
  composition material, not directly callable from the wire (ADR-015).
  The handler that composes them is `External`.
- **`from_openapi` error codes are prefixed `HTTP_<status>`.** No
  collision with protocol-level codes (ADR-023, review #002 W20).
- **`to_openapi` is a pure projection.** It consumes the registry, does
  not produce entries for it. Not an `OperationAdapter`.
- **Published `to_openapi` specs are compatibility contracts.** The
  generated spec's versioning (tied to the registry's `External`
  operation set version) must be emitted so consumers can detect mapping
  changes (ADR-017 Consequences, OQ-39).
- **`alknet-http` owns its `reqwest::Client`.** Shared across all
  forwarding handlers, constructed once. No env-var-based client config.
  Pooling/retry config is a two-way door (OQ-40).
- **TLS for outbound calls uses the system trust store by default.**
  Standard HTTPS to external APIs (OpenAI, Anthropic). Custom CA bundle
  + client certs are an optional config for self-hosted API gateways.
  This is a two-way-door implementation detail; the credential (API
  key/token) comes from `Capabilities`, the TLS trust comes from the
  system.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| `from_openapi` is an `OperationAdapter` | [ADR-017](../../decisions/017-call-protocol-client-and-adapter-contract.md) | Async trait; produces `HandlerRegistration` bundles |
| `to_openapi` is a projection, not an adapter | [ADR-017](../../decisions/017-call-protocol-client-and-adapter-contract.md) | Consumes the registry, doesn't produce entries |
| Adapter-registered ops are `Internal` | [ADR-015](../../decisions/015-privilege-model-and-authority-context.md) | `from_openapi` ops are composition material |
| `from_openapi` provenance is a leaf | [ADR-022](../../decisions/022-handler-registration-provenance-and-composition-authority.md) | `composition_authority: None`, `scoped_env: None` |
| Error fidelity (`HTTP_<status>` codes) | [ADR-023](../../decisions/023-operation-error-schemas.md) | No collision with protocol codes; `to_openapi` projects back |
| No-env-vars credential injection | [ADR-014](../../decisions/014-secret-material-flow-and-capability-injection.md) | Handler reads `context.capabilities`, not env vars |
| HTTP path = operation path | [ADR-036](../../decisions/036-http-to-call-operation-mapping.md) | `to_openapi` paths mirror `/{service}/{op}` |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-39** (open): `to_openapi` published-spec versioning — the
  versioning strategy for generated OpenAPI specs (tied to the
  registry's `External` operation set version). One-way after first
  publication.
- **OQ-40** (open): reqwest client config and connection pooling —
  two-way-door: the exact pooling/retry config shape, hot-reloadable
  via `DynamicConfig`.

## References

- [ADR-017](../../decisions/017-call-protocol-client-and-adapter-contract.md)
  — `OperationAdapter` trait, `to_*` are projections
- [ADR-023](../../decisions/023-operation-error-schemas.md) — error
  fidelity, `HTTP_<status>` prefix rule
- [overview.md](overview.md) — adapter location map, no-env-vars
  invariant
- [../call/client-and-adapters.md](../call/client-and-adapters.md) —
  `OperationAdapter` trait, `AdapterError` variants (OQ-26), no-env-vars
  invariant
- `/workspace/@alkdev/operations/src/from_openapi.ts` — TypeScript prior
  art (parsing, SSE, auth headers, `createHTTPOperation`)
- `/workspace/aisdk/src/core/client.rs` — HTTP client reference (pooling,
  retry, streaming vs non-streaming)