---
id: http/adapters/from-openapi
name: Implement from_openapi adapter (parse OpenAPI, reqwest forwarding handlers, no-env-vars injection)
status: pending
depends_on: [http/client/shared-http-client, http/gateway/error-mapping]
scope: broad
risk: medium
impact: component
level: implementation
---

## Description

Implement `from_openapi` in `src/adapters/from_openapi.rs`. This is the
OpenAPI-direction adapter: it parses an OpenAPI document, constructs a
`HandlerRegistration` bundle per OpenAPI operation with a forwarding
handler that calls the external HTTP endpoint via `reqwest`, and returns
the bundles for registration in the `OperationRegistry`. The adapter
implements `OperationAdapter` (the async trait from `alknet-call`,
ADR-017 §5).

### The adapter (http-adapters.md §"from_openapi")

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

### Type definitions (http-adapters.md §"Type definitions")

```rust
/// A parsed OpenAPI document. The concrete type is a two-way-door
/// implementation detail (openapiv3::OpenApi, a local alknet-http type,
/// or a serde_json::Value-based parse); the one-way constraint is that
/// from_openapi accepts a standard OpenAPI 3.x JSON/YAML doc and to_openapi
/// produces one. Both directions share the same Rust type, but not the
/// same document shape. Coordinate with the to-openapi task on the type.
pub struct OpenAPISpec {
    pub info: OpenAPIInfo,
    pub paths: BTreeMap<String, PathItem>,
    pub components: Option<Components>,
    // ... OpenAPI 3.x fields as needed
}

/// Configuration for an HTTP-backed adapter (from_openapi). Carries the
/// base URL, auth scheme (from Capabilities at registration, not env vars),
/// and optional headers. The auth field is the scheme the external API
/// expects; the credential itself is read from
/// OperationContext.capabilities at call time, not stored here.
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

### The import flow (http-adapters.md §"from_openapi")

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
     composition material, not directly callable from the wire — ADR-015).
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

### Forwarding handler (http-adapters.md §"Forwarding handler")

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
3. Sends the request via the shared HTTP client (`SharedHttpClient` —
   the `shared-http-client` task).
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

### No-Env-Vars credential injection (http-adapters.md §"No-Env-Vars credential injection")

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
source other than `OperationContext.capabilities`.

### Error Fidelity (ADR-023, http-adapters.md §"Error Fidelity")

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

## Acceptance Criteria

- [ ] `FromOpenAPI` struct with `spec: OpenAPISpec`, `config: HttpServiceConfig`
- [ ] `OperationAdapter` impl: `async fn import(&self) -> Result<Vec<HandlerRegistration>, AdapterError>`
- [ ] Parses OpenAPI doc (`paths`, `components`, `$ref` resolution)
- [ ] Parse failure → `AdapterError::SchemaParse`
- [ ] For each `(path, method, operation)`: constructs `HandlerRegistration`
- [ ] `spec.name` = `operationId` (or generated `${method}_${path_parts}`)
- [ ] `spec.namespace` = `config.namespace`
- [ ] `spec.op_type` = `Query`/`Mutation`/`Subscription` (detected from method + response content type)
- [ ] `spec.visibility` = `Internal` (ADR-015)
- [ ] `spec.input_schema` / `output_schema` from OpenAPI parameters/responses
- [ ] `spec.error_schemas` from non-2xx OpenAPI responses with `HTTP_<status>` prefix (ADR-023)
- [ ] `spec.access_control` = `AccessControl::default()`
- [ ] `provenance` = `FromOpenAPI`, `composition_authority: None`, `scoped_env: None` (ADR-022)
- [ ] `capabilities` = credentials for the external endpoint (injected at registration)
- [ ] Forwarding handler builds outbound HTTP request (path params, query, headers, body)
- [ ] Forwarding handler sends via `SharedHttpClient` (the shared client)
- [ ] `Query`/`Mutation`: parses response body (JSON/text/binary), wraps in `ResponseEnvelope`
- [ ] `Subscription` (`text/event-stream`): streams `call.responded` from SSE chunks, then `call.completed`
- [ ] HTTP error (non-2xx): maps to declared `ErrorDefinition` by status code, returns `CallError`
- [ ] No-env-vars: handler reads `context.capabilities.get("<service>")`, never `std::env::var` (ADR-014)
- [ ] Bearer/ApiKey/Basic auth injection from `Capabilities`
- [ ] `HttpAuthScheme` enum with `Bearer`, `ApiKey { header_name }`, `Basic`
- [ ] `HttpServiceConfig` with `namespace`, `base_url`, `auth`, `default_headers`
- [ ] Unit test: parse a minimal OpenAPI doc → one `HandlerRegistration`
- [ ] Unit test: parse failure → `AdapterError::SchemaParse`
- [ ] Unit test: `operationId` absent → generated name `${method}_${path_parts}`
- [ ] Unit test: GET → `Query`, POST → `Mutation`, SSE response → `Subscription`
- [ ] Unit test: error response 404 → `ErrorDefinition { code: "HTTP_404", http_status: Some(404) }`
- [ ] Unit test: forwarding handler injects Bearer token from `context.capabilities`
- [ ] Integration test: forwarding handler calls external endpoint via `SharedHttpClient`
- [ ] Integration test: SSE response streams `call.responded` events
- [ ] Integration test: no `std::env::var` reads in the forwarding handler
- [ ] `cargo test -p alknet-http` succeeds
- [ ] `cargo clippy -p alknet-http --all-targets` succeeds with no warnings

## References

- docs/architecture/crates/http/http-adapters.md — from_openapi (full spec)
- docs/architecture/decisions/017-call-protocol-client-and-adapter-contract.md — ADR-017 §5 (OperationAdapter trait)
- docs/architecture/decisions/015-privilege-model-and-authority-context.md — ADR-015 (Internal visibility)
- docs/architecture/decisions/022-handler-registration-provenance-and-composition-authority.md — ADR-022 (leaf provenance)
- docs/architecture/decisions/023-operation-error-schemas.md — ADR-023 (HTTP_<status> prefix, error fidelity)
- docs/architecture/decisions/014-secret-material-flow-and-capability-injection.md — ADR-014 (no env vars)
- /workspace/@alkdev/operations/src/from_openapi.ts — TypeScript prior art (parsing, SSE, auth headers, createHTTPOperation, parseSSEFrames)

## Notes

> from_openapi is the no-env-vars credential injection point. The
> forwarding handler reads context.capabilities, not std::env::var —
> this is the spec-level invariant (ADR-014). The handler is opaque to
> CallAdapter (Arc<dyn Handler>); alknet-call never sees reqwest. The
> error codes are prefixed HTTP_<status> to avoid collision with
> protocol-level codes (ADR-023, review #002 W20). The OpenAPISpec type
> is shared with to_openapi (coordinate on the type); the shape is not
> (from_openapi consumes per-operation-paths, to_openapi produces the
> 5-endpoint gateway doc). The TS prior art (@alkdev/operations/src/
> from_openapi.ts) shows the parsing patterns (resolveRef,
> buildInputSchema, buildOutputSchema, detectOperationType,
> createHTTPOperation, parseSSEFrames) — the SSE normalization patterns
> stay referenced, the client construction anti-patterns (env-var
> config, hand-rolled retry) are discarded.

## Summary

> To be filled on completion