---
id: http/adapters/to-openapi
name: Implement to_openapi gateway projection (5-endpoint OpenAPI doc, info.version semver, ADR-042/045)
status: completed
depends_on: [http/server/gateway-endpoints, http/gateway/gateway-dispatch-spine]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Implement `to_openapi` in `src/adapters/to_openapi.rs`. This is the
OpenAPI gateway projection: it generates an OpenAPI document with a
**fixed 5-endpoint gateway set** that gates access to the full operation
registry — not one path per operation (ADR-042). The external client (a
code generator, a human developer, a `fetch`-based client) calls
`/search` to discover operations, `/schema` to learn an operation's
input shape, `/call` to invoke. Served at `GET /openapi.json` by the HTTP
server.

### Pure projection (ADR-017 §5)

`to_openapi` is a pure projection — it consumes the registry and produces
a spec. It does not modify the registry; it does not register
operations; it is not an `OperationAdapter`. The HTTP server serves the
generated spec at `GET /openapi.json`.

```rust
/// Generate an OpenAPI document describing the 5 gateway endpoints.
/// Pure projection: consumes the registry, does not produce entries.
/// The per-caller operation surface is discovered via /search, not
/// preloaded into the doc (ADR-042 §3).
pub fn to_openapi(registry: &OperationRegistry) -> OpenAPISpec;
```

### The gateway endpoint set (http-adapters.md §"The gateway endpoint set")

`to_openapi` generates 5 fixed endpoints:

| OpenAPI path | Call protocol | HTTP method | Purpose |
|--------------|--------------|-------------|---------|
| `/search` | `services/list` | `GET` | List/search operations (AccessControl-filtered). Names + descriptions. |
| `/schema` | `services/schema` | `GET` | Get an operation's full `OperationSpec`. |
| `/call` | `call.requested` (Query/Mutation) | `POST` | Invoke an operation. Flat JSON body `{ operation, input }`. |
| `/batch` | multiple `call.requested` | `POST` | Invoke multiple operations. Array of `{ operation, input }`. |
| `/subscribe` | `call.requested` (Subscription) | `POST` (SSE) | Invoke a streaming operation. Body `{ operation, input }`; response `text/event-stream`. |

The input is always a flat JSON body — no path/query/body split to
reverse-engineer. JSON Schema for the input/output is already in the
`OperationSpec`; the gateway wraps it in OpenAPI's schema format without
splitting parameters.

`/subscribe` is the one endpoint the MCP gateway excludes (ADR-041 —
MCP tool calls are request/response). OpenAPI/SSE supports streaming;
the gateway's `/subscribe` uses the SSE projection — `call.responded` →
SSE `data:` frames, `call.completed` → stream close.

### Per-caller API surface (http-adapters.md §"Per-caller API surface")

The `/search` endpoint's results are `AccessControl::check(identity)`-
filtered — the client sees only the operations it is authorized to call.
The generated OpenAPI doc describes the 5 gateway endpoints (stable,
same for every caller); the per-caller operation surface is discovered
through `/search`, not preloaded into the doc. This is the key
advantage over a traditional per-operation-paths OpenAPI doc: the
per-caller API surface is the default (the Gitea failure mode — dumping
admin ops to every caller — is structurally impossible).

### `info.version` semver (ADR-045, OQ-39 resolved)

The generated gateway doc carries `info.version` (semver) tracking the
**gateway endpoint contract**, not the operation set — per-caller
operation changes (add/remove/modify, schema changes) do not bump the
version (the operation set is discovered via `/search`, not preloaded
into the doc). Consumers detect breaking changes via the major version.

- **Major** = breaking gateway change (an endpoint removed, a request
  field removed, a status code changed meaning).
- **Minor** = additive (a new endpoint, a new optional request field).
- **Patch** = wording (doc clarifications, description tweaks).

The version is a constant in `to_openapi` (bumped manually when the
gateway contract changes), not derived from the registry's operation
set. The initial version is `1.0.0`.

### Error fidelity (ADR-023, http-adapters.md §"Error Fidelity")

`to_openapi` projects `error_schemas` to the gateway endpoint's response
definitions. The `/call` endpoint's responses include the
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
dropping of error contracts.

### The `OpenAPISpec` type

The concrete type is a two-way-door implementation detail
(`openapiv3::OpenApi`, a local alknet-http type, or a
`serde_json::Value`-based parse); the one-way constraint is that
`from_openapi` accepts a standard OpenAPI 3.x JSON/YAML doc and
`to_openapi` produces one. Both directions share the same Rust type, but
not the same document shape: `from_openapi` consumes traditional
per-operation-paths docs (one path per operation), while `to_openapi`
produces the 5-endpoint gateway doc (ADR-042). The type is shared; the
shape is not. Coordinate with the `from-openapi` task on the shared type.

### Traditional per-operation-paths projection (additive, out of scope)

A deployment that wants a traditional REST OpenAPI doc (per-operation
paths with split parameters) can build it as a separate projection with
HTTP-specific metadata (which fields are path params, etc.). The gateway
pattern is the default `to_openapi` projection; the traditional
projection is additive, not a replacement (ADR-042 §5). This task
implements the gateway projection only; the traditional projection is
out of scope.

## Acceptance Criteria

- [ ] `to_openapi(registry: &OperationRegistry) -> OpenAPISpec` implemented
- [ ] Generates 5 fixed gateway endpoints (`/search`, `/schema`, `/call`, `/batch`, `/subscribe`)
- [ ] No per-operation paths (the gateway is the surface, ADR-042)
- [ ] `/call` request body is flat JSON `{ operation, input }` (no path/query/body split)
- [ ] `/subscribe` response is `text/event-stream`
- [ ] `info.version` is semver tracking the gateway contract (initial `1.0.0`, ADR-045)
- [ ] Per-caller operation surface NOT preloaded into the doc (discovered via `/search`)
- [ ] `/call` responses include protocol-level errors (400, 401, 403, 404, 500, 504)
- [ ] `/call` responses include operation-level errors (mapped by `http_status`, ADR-023)
- [ ] `HTTP_<status>`-prefixed error codes projected correctly (no collision with protocol codes)
- [ ] `to_openapi` is a pure projection (does not modify registry, not an OperationAdapter)
- [ ] `GET /openapi.json` route serves the generated spec (wired by http-adapter task)
- [ ] Unit test: generated doc has exactly 5 paths (the gateway endpoints)
- [ ] Unit test: `/call` request schema is `{ operation: string, input: object }`
- [ ] Unit test: `/subscribe` response content type is `text/event-stream`
- [ ] Unit test: `info.version` is `1.0.0`
- [ ] Unit test: `/call` responses include all protocol-level error statuses
- [ ] Unit test: operation with `error_schemas` → those errors projected on `/call`
- [ ] Unit test: operation with `HTTP_404` error code → projected as 404 response
- [ ] `cargo test -p alknet-http` succeeds
- [ ] `cargo clippy -p alknet-http --all-targets` succeeds with no warnings

## References

- docs/architecture/crates/http/http-adapters.md — to_openapi (§"to_openapi", §"The gateway endpoint set", §"Per-caller API surface", §"Error Fidelity")
- docs/architecture/decisions/042-openapi-gateway-pattern.md — ADR-042 (5 fixed gateway endpoints)
- docs/architecture/decisions/045-to-openapi-gateway-spec-versioning.md — ADR-045 (info.version semver)
- docs/architecture/decisions/047-remove-direct-call-http-surface.md — ADR-047 (gateway is sole invoke path)
- docs/architecture/decisions/023-operation-error-schemas.md — ADR-023 (error fidelity, HTTP_<status> prefix)
- docs/architecture/decisions/017-call-protocol-client-and-adapter-contract.md — ADR-017 §5 (to_* are projections)

## Notes

> to_openapi is a pure projection — it consumes the registry, does not
> produce entries. The generated doc describes the 5 fixed gateway
> endpoints (stable, same for every caller); the per-caller operation
> surface is discovered via /search, not preloaded. The info.version
> semver tracks the gateway endpoint contract, not the operation set
> (ADR-045) — per-caller operation changes do not bump the version. The
> error fidelity (ADR-023) projects operation-level errors (with
> http_status) onto /call's responses, plus the protocol-level errors.
> The OpenAPISpec type is shared with from_openapi (coordinate on the
> type); the shape is not (from_openapi consumes per-operation-paths,
> to_openapi produces the 5-endpoint gateway doc). The traditional
> per-operation-paths projection is additive (ADR-042 §5) and out of
> scope.

## Summary

> Implemented to_openapi(registry: &OperationRegistry) -> OpenAPISpec in src/adapters/
> to_openapi.rs — pure projection generating fixed 5-endpoint gateway doc (/search,
> /schema, /call, /batch, /subscribe) with info.version = 1.0.0 (ADR-045). /call responses
> carry protocol-level errors (400/401/403/404/500/504) + operation-level errors from
> registry error_schemas mapped by http_status (ADR-023). Per-caller operation surface
> NOT preloaded (discovered via /search, ADR-042). /subscribe response is text/event-stream.
> Wired GET /openapi.json in adapter.rs replacing placeholder 501. 16 new tests. 230
> total tests pass. Clippy clean. Formatting fixed during merge.