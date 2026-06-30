---
status: draft
last_updated: 2026-06-30
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
3. Sends the request via the shared HTTP client (see HTTP Client
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

`alknet-http` maintains a shared HTTP client, constructed once and reused
across all `from_openapi`/`from_mcp` forwarding handlers. The client owns
connection pooling, keep-alive, TLS, and a retry stack. The shared type is
`reqwest_middleware::ClientWithMiddleware`, not a bare `reqwest::Client` —
both retry and Retry-After are middleware on the stack, and middleware
requires the `ClientWithMiddleware` wrapper.

The middleware stack has two layers:

1. **`RetryTransientMiddleware`** (from `reqwest-retry`) — exponential
   backoff on transient failures (connection errors, 5xx). The "retry N
   times with increasing intervals" part. Configured via an
   `ExponentialBackoff` policy at client construction.
2. **Inlined `RetryAfterMiddleware`** — parses the `Retry-After` header
   on 429/503 and sleeps before the next request to that URL. The
   "respect what the server told you" part. Inlined (MIT, ~50 lines of
   real logic) from `melotic/reqwest-retry-after`, not pulled as a
   dependency: the crate is complementary to `reqwest-retry` (whose
   default strategy does not honor `Retry-After`), and inlining lets
   the upstream's unbounded `HashMap<Url, SystemTime>` storage be
   bounded for a long-running process.

Pooling, keep-alive, and TLS come from `reqwest::ClientBuilder` defaults;
outbound TLS uses the system trust store (standard HTTPS to external APIs
like OpenAI, Anthropic). Custom CA bundle + client certs are an optional
config for self-hosted API gateways (two-way-door implementation detail;
the credential comes from `Capabilities`, the TLS trust comes from the
system).

Credential injection happens per-request (from
`OperationContext.capabilities`), not at client construction — the client
is shared across all operations, the credentials are per-call.

Hot-reload of the pooling/retry config is **rebuild-and-swap**: a config
change rebuilds the `ClientWithMiddleware` and swaps it via `ArcSwap`
(the same pattern `ConfigIdentityProvider` uses, ADR-035). A rebuild
drops the connection pool / keep-alive state, which is acceptable — a
config change wanting a fresh pool is the case that triggers it. The
retry policy is baked into the middleware at `ClientBuilder::build()`
time; live policy mutation is not supported by `reqwest-retry`, so cheap
per-policy updates are not part of the model.

The exact pooling/retry config (pool size, retry count, timeout
defaults, hot-reloadability via `DynamicConfig`) is a two-way-door
implementation detail (OQ-40, now resolved); the one-way constraint is
that `alknet-http` owns its HTTP client (no env-var-based client config,
no shared global client).

**Downstream layering boundary.** The agent crate's provider SSE
normalization (replicating the solid part of aisdk's pattern — the
Vercel-UI-message normalization that maps different providers' SSE to a
common shape) sits on top of this `ClientWithMiddleware`: it consumes the
`reqwest::Response` stream the forwarding handler produces and emits
`call.responded` events. It does not replace the client or own
transport/pooling/retry. `alknet-http` owns transport; the agent crate
owns provider-specific SSE → Vercel-UI-message mapping. The aisdk
`core/client.rs` reference for HTTP client construction is *not* carried
forward — its env-var config and hand-rolled retry are the anti-patterns
discarded in favor of the middleware stack above. The
`@alkdev/operations/src/from_openapi.ts` SSE *normalization* pattern is
separate and stays referenced in the Forwarding Handler section above
(the `parseSSEFrames`, `createHTTPOperation`, content-type branching
patterns).

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

`to_openapi` generates an OpenAPI document with a **fixed gateway
endpoint set** that gates access to the full operation registry — not
one path per operation. This is the OpenAPI gateway pattern (ADR-042):
the same principle as the MCP gateway (ADR-041) applied to OpenAPI. The
external client (a code generator, a human developer, a `fetch`-based
client) calls `/search` to discover operations, `/schema` to learn an
operation's input shape, `/call` to invoke. See
[ADR-042](../../decisions/042-openapi-gateway-pattern.md) for the
rationale (the flat→structured split problem, the per-caller API
surface problem).

#### The gateway endpoint set

`to_openapi` generates 5 fixed endpoints:

| OpenAPI path | Call protocol | HTTP method | Purpose |
|--------------|--------------|-------------|---------|
| `/search` | `services/list` | `GET` | List/search operations (AccessControl-filtered). Names + descriptions. |
| `/schema` | `services/schema` | `GET` | Get an operation's full `OperationSpec`. |
| `/call` | `call.requested` (Query/Mutation) | `POST` | Invoke an operation. Flat JSON body `{ operation, input }`. |
| `/batch` | multiple `call.requested` | `POST` | Invoke multiple operations. Array of `{ operation, input }`. |
| `/subscribe` | `call.requested` (Subscription) | `GET` (SSE) | Invoke a streaming operation. `text/event-stream`. |

The input is always a flat JSON body — no path/query/body split to
reverse-engineer. JSON Schema for the input/output is already in the
`OperationSpec`; the gateway wraps it in OpenAPI's schema format without
splitting parameters.

`/subscribe` is the one endpoint the MCP gateway excludes (ADR-041 —
MCP tool calls are request/response). OpenAPI/SSE supports streaming;
the gateway's `/subscribe` uses the same SSE projection ADR-036
describes — `call.responded` → SSE `data:` frames, `call.completed` →
stream close.

#### Per-caller API surface

The `/search` endpoint's results are `AccessControl::check(identity)`-
filtered — the client sees only the operations it is authorized to call.
The generated OpenAPI doc describes the 5 gateway endpoints (stable,
same for every caller); the per-caller operation surface is discovered
through `/search`, not preloaded into the doc. This is the key
advantage over a traditional per-operation-paths OpenAPI doc: the
per-caller API surface is the default (the Gitea failure mode — dumping
admin ops to every caller — is structurally impossible). See ADR-042 §3.

#### Pure projection

`to_openapi` is a pure projection — it consumes the registry and
produces a spec. It does not modify the registry; it does not register
operations; it is not an `OperationAdapter`. The HTTP server serves the
generated spec at `GET /openapi.json` (or a configured path).

#### Traditional per-operation-paths projection (additive)

A deployment that wants a traditional REST OpenAPI doc (per-operation
paths with split parameters) can build it as a separate projection with
HTTP-specific metadata (which fields are path params, etc.). The
gateway pattern is the default `to_openapi` projection; the traditional
projection is additive, not a replacement. See ADR-042 §5.

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

`to_openapi` projects `error_schemas` to the gateway endpoint's
response definitions. The `/call` endpoint's responses include the
operation-level errors (mapped by `http_status`), plus the protocol-
level errors:

```yaml
# /call endpoint responses
responses:
  '200': { schema: <output_schema for the called operation> }
  '400': { schema: <INVALID_INPUT error> }
  '401': { schema: <no bearer token> }
  '403': { schema: <FORBIDDEN — insufficient scopes> }
  '404': { schema: <NOT_FOUND — operation not registered or Internal> }
  '422': { schema: <operation-level error with http_status=422> }
  '429': { schema: <operation-level error with http_status=429> }
  '500': { schema: <INTERNAL> }
  '504': { schema: <TIMEOUT> }
```

The operation-level errors (with `http_status`) are surfaced on the
`/call` endpoint's response — the gateway propagates the called
operation's `error_schemas` as response definitions. This makes the
adapter contract from ADR-017 faithful on the error axis — no silent
dropping of error contracts. See ADR-023.

## Why

`from_openapi` is how alknet composes external HTTP APIs (OpenAI,
Anthropic, vast.ai, GitHub) into the call protocol. An operation
imported via `from_openapi` is a first-class operation: it has a spec,
it's discoverable via `services/list`, it can be composed by handlers,
its errors are typed. The agent crate's LLM provider calls go through
`from_openapi`-imported operations — that's how the no-env-vars
invariant makes aisdk's env-var reads unreachable.

`to_openapi` is how external systems discover the alknet operation
surface. A client generator, a human developer, or a `fetch`-based
client reads the OpenAPI doc to learn the gateway's shape (5 fixed
endpoints), then calls `/search` to discover what *it* can call
(per-caller, AccessControl-filtered) and `/schema` to learn an
operation's input shape. The gateway pattern avoids the flat→structured
split that a traditional per-operation-paths projection would require,
and makes the per-caller API surface the default (the Gitea failure
mode — dumping admin ops to every caller — is structurally impossible).
See [ADR-042](../../decisions/042-openapi-gateway-pattern.md). The
generated spec is a compatibility contract (ADR-017 Consequences) —
once published, the 5-endpoint gateway shape is one-way.

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
- **`alknet-http` owns its HTTP client.** Shared across all forwarding
  handlers, constructed once. The shared type is
  `reqwest_middleware::ClientWithMiddleware` (middleware stack:
  `RetryTransientMiddleware` + inlined `RetryAfterMiddleware`). No
  env-var-based client config. Pooling/retry config is a two-way door,
  resolved in OQ-40.
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
| HTTP path = operation path (direct-call surface) | [ADR-036](../../decisions/036-http-to-call-operation-mapping.md) | `POST /{service}/{op}` → `call.requested` (the direct-call surface; not what `to_openapi` describes) |
| `to_openapi` gateway pattern | [ADR-042](../../decisions/042-openapi-gateway-pattern.md) | 5 fixed gateway endpoints (search/schema/call/batch/subscribe), not one path per operation; per-caller AccessControl-filtered. Supersedes ADR-036's original `to_openapi` "paths mirror `/{service}/{op}`" clause |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-39** (open): `to_openapi` published-spec versioning — the
  versioning strategy for generated OpenAPI specs (tied to the
  registry's `External` operation set version). One-way after first
  publication.
- **OQ-40** (resolved): reqwest client config and connection pooling —
  `ClientWithMiddleware` + `RetryTransientMiddleware` + inlined
  `RetryAfterMiddleware`; rebuild-and-swap hot-reload; per-request
  credential injection. Two-way-door config shape, now resolved.

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
  art (parsing, SSE, auth headers, `createHTTPOperation`,
  `parseSSEFrames` — the SSE normalization patterns, not the client
  construction)
- `reqwest-retry` crate (https://docs.rs/reqwest-retry/) —
  `RetryTransientMiddleware` / `ExponentialBackoff` retry policy
- `melotic/reqwest-retry-after`
  (https://github.com/melotic/reqwest-retry-after) — `RetryAfterMiddleware`
  source (MIT, inlined, not a dependency)