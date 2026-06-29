# ADR-036: HTTP-to-Call Operation Mapping

## Status

Proposed

## Context

`alknet-http` implements `ProtocolHandler` for the standard HTTP ALPNs (`h2`,
`http/1.1`, `h3`). An inbound HTTP request that targets an alknet operation
must become a call-protocol `call.requested` dispatch — the HTTP handler is a
*projection* of the call protocol, not a parallel routing layer. The
question is how an HTTP request maps to an operation invocation.

Three options were considered in the alknet-http Phase 0 research
(`docs/research/alknet-http/phase-0-findings.md`, decision point DH-3):

- **(a) Direct path mapping.** `POST /{service}/{op}` → `call.requested` for
  `/{service}/{op}`. The HTTP handler parses the request body as the
  operation input, sends `call.requested`, and returns the response as JSON.
  The HTTP surface is a thin projection of the call protocol's
  `/{service}/{op}` operation path format (resolved by OQ-13).
- **(b) OpenAPI-defined routes.** The HTTP surface is defined by the
  `to_openapi` projection — routes, methods, schemas are generated from the
  registry's `External` operations, and the HTTP handler dispatches based on
  the generated OpenAPI spec's path mapping.
- **(c) Explicit route registration.** The assembly layer registers HTTP
  routes explicitly, mapping URL paths to operations. Most flexible, most
  boilerplate.

This is a load-bearing architectural choice. Once the HTTP surface's routing
contract is published and external clients build against it, changing the
mapping (e.g., from "the HTTP path IS the operation path" to "the HTTP path
is a generated alias") is a one-way door: every client breaks. It needs an
ADR before implementation.

The call protocol's operation path format is `/{service}/{op}` (OQ-13,
resolved). The HTTP handler serves these operations over HTTP. The mapping
must be a *projection* of that single operation surface, not a second
routing table that has to be kept in sync with the registry.

## Decision

**Direct path mapping is the default HTTP surface; `to_openapi` is the
discovery/projection layer, not a parallel router.**

The `HttpAdapter` receives an HTTP request whose path is `/{service}/{op}`
(e.g., `POST /fs/readFile`, `POST /agent/chat`), constructs a
`call.requested` dispatch with `operationId: /{service}/{op}` and `input:
<parsed body>`, and returns the operation's response as JSON. The HTTP path
IS the operation path — one routing surface, the call protocol's.

`to_openapi` generates the OpenAPI spec that *describes* this surface for
external consumers (route paths, methods, request/response schemas, error
schemas per ADR-023). It does not define separate routes — the generated
spec's `paths` mirror the `/{service}/{op}` operation paths. An external
client reading the OpenAPI doc learns the same routes the HTTP handler
serves; there is no second mapping.

> **Amendment (superseded by [ADR-042](042-openapi-gateway-pattern.md) on
> the `to_openapi` clause):** The paragraph above described the original
> "per-operation-paths projection" — `to_openapi` generating one OpenAPI
> path entry per `External` operation, mirroring `/{service}/{op}`. ADR-042
> replaces this with the **gateway pattern**: `to_openapi` generates 5
> fixed gateway endpoints (`/search`, `/schema`, `/call`, `/batch`,
> `/subscribe`) instead of one path per operation. The "no second routing
> table" property is preserved (the gateway endpoints are fixed; the
> per-caller operation surface is discovered via `/search`, not preloaded
> into a generated path set). The direct-call surface (`POST
> /{service}/{op}`) that this ADR defines is **unchanged** — ADR-042 only
> changes what `to_openapi` *describes*, not what the HTTP handler
> *serves*. A traditional per-operation-paths OpenAPI projection remains
> available as an additive alternative (ADR-042 §5).

### HTTP method semantics

The call protocol's `OperationType` (`Query`, `Mutation`, `Subscription`,
per operation-registry.md) maps to HTTP methods on the default surface:

| `OperationType` | Default HTTP method | Notes |
|-----------------|----------------------|-------|
| `Query` | `GET` | Read-only, idempotent. Input from query parameters + optional body. |
| `Mutation` | `POST` (or `PUT`/`PATCH`/`DELETE` if the operation declares it) | Default `POST`; the op may declare a specific mutation method in its spec metadata. |
| `Subscription` | `GET` with `Accept: text/event-stream` | Streaming — the HTTP handler projects the subscription's `call.responded` stream as SSE chunks. |

The default method for an `External` operation with no explicit HTTP method
declared is `POST` for `Mutation`, `GET` for `Query`. This is the
least-surprise default; an operation that wants a specific HTTP verb
declares it. The method-to-`OperationType` mapping is a two-way-door
default (changing it later is additive — a new method is added, existing
methods keep working).

### Streaming projection (SSE)

A `Subscription` operation served over HTTP/1.1 or HTTP/2 projects its
`call.responded` stream as Server-Sent Events. Each `call.responded` event
becomes an SSE `data:` frame; `call.completed` closes the SSE stream;
`call.aborted` closes the stream with an SSE error event. This is the
HTTP/1.1 + HTTP/2 streaming projection. Over WebTransport (`h3`), the
subscription projects directly onto a WebTransport bidirectional stream —
no SSE framing is needed (see ADR-038 for the WebTransport path).

### Auth

Inbound HTTP auth is `Authorization: Bearer <token>`, resolved via
`IdentityProvider::resolve_from_token()` (auth.md's handler table —
`HttpAdapter`, Bearer header, `resolve_from_token`). This is settled by
ADR-004 and OQ-11; this ADR does not change it. Bearer-only is the auth
mechanism; other HTTP auth schemes (Basic, API key in query param) are not
implemented. An unauthenticated request to an operation with
`AccessControl` restrictions returns `401`/`403` (mapped from the call
protocol's `FORBIDDEN` protocol code).

### Stealth mode

The HTTP handler on `h2`/`http/1.1` serves a decoy (configurable: fake
404, a static site, a redirect) for paths that are not registered
operations. This is the ALPN-based stealth mapping from endpoint.md —
clients that don't offer alknet ALPNs get the HTTP handler, and unknown
HTTP paths get the decoy. The decoy is a two-way-door config default (an
operator picks what to serve); the *existence* of the stealth path is fixed
by ADR-010.

### `/healthz` and operational endpoints

`GET /healthz` is a raw HTTP route outside the call protocol — no auth, no
operation registration. It exists for infrastructure (load balancers,
orchestrators). Other operational endpoints (metrics, dashboard) are
call-protocol operations if built (`/metrics/list`, `/dashboard/view`),
not raw HTTP routes. `healthz` is the one exception: it must be callable
without auth before identity is resolvable.

## Consequences

**Positive:**
- One routing surface. The HTTP handler does not maintain a second routing
  table; it projects the call protocol's `/{service}/{op}` paths directly.
  No sync drift between the operation registry and the HTTP routes.
- `to_openapi` is a pure projection (generate a spec that *describes* the
  existing surface), not a routing authority. The generated spec is always
  consistent with what the handler actually serves because they're the same
  paths.
- External HTTP clients (curl, axios, browser `fetch`) can call alknet
  operations without knowing about the call protocol — the HTTP surface is
  a standard REST-like API.
- The abort cascade (ADR-016) is preserved: an HTTP client disconnecting
  mid-subscription is detected as a stream close, and the HTTP handler
  sends `call.aborted` for the in-flight subscription, which cascades to
  descendants.
- The HTTP method mapping (`Query`→`GET`, `Mutation`→`POST`,
  `Subscription`→`SSE`) is the standard REST projection — no surprise
  verbs, no exotic method semantics.

**Negative:**
- The HTTP surface inherits the call protocol's `/{service}/{op}` path
  shape. An operation named `fs/readFile` is served at `POST /fs/readFile`,
  not at a REST-nested `POST /fs/files/:id/read` or any other
  REST-conventional path. Operations that want a REST-nested HTTP path
  must declare it in spec metadata (a two-way-door extension); the
  default is the operation path verbatim. This is a deliberate
  least-surprise-for-alknet choice, not a REST-purist choice.
- HTTP request/response semantics don't map cleanly onto every call
  protocol operation. A `Query` with a large input has to put the input in
  the body (GET-with-body is non-standard). A `Mutation` that is
  idempotent doesn't get `PUT` semantics unless it declares them. The
  projection is lossy at the edges; operations that need precise HTTP
  semantics declare them.
- `to_openapi` is a published compatibility contract (ADR-017 Consequences:
  once external clients build against the generated spec, the mapping is
  one-way). The generated spec's versioning (tied to the registry's
  `External` operation set version) must be emitted as a spec marker so
  consumers can detect mapping changes. This is OQ-17's published-artifact
  concern, applied to the HTTP projection.

## Assumptions

1. **The operation path IS the HTTP path.** An operation `fs/readFile` is
   served at `/fs/readFile`. There is no separate HTTP path mapping layer.
   If a deployment wants different HTTP paths (e.g., a REST-nested
   convention), that's a future projection layer, not a change to this
   mapping.

2. **`External` operations are the HTTP surface.** `Internal` operations
   (composition-only, ADR-015) are not served over HTTP — they return `404`
   on the HTTP handler, matching the call protocol's `NOT_FOUND` for wire
   calls to Internal ops. The HTTP handler dispatches only `External`
   operations.

3. **HTTP auth is Bearer-only.** The HTTP handler resolves identity from
   the `Authorization: Bearer` header via `resolve_from_token`. Basic auth,
   API keys in query params, and other HTTP auth schemes are not
   implemented. A deployment that needs a different auth scheme adds it as
   middleware (two-way door), but the default surface is Bearer-only.

## References

- [ADR-004](004-auth-as-shared-core.md) — `IdentityProvider`, Bearer →
  `resolve_from_token` (the auth model this ADR uses, unchanged)
- [ADR-010](010-alpn-router-and-endpoint.md) — stealth mode as ALPN
  dispatch (the HTTP handler on standard ALPNs serves the decoy)
- [ADR-015](015-privilege-model-and-authority-context.md) — External/Internal
  visibility (Internal ops are not served over HTTP)
- [ADR-016](016-abort-cascade-for-nested-calls.md) — abort cascade (HTTP
  client disconnect → `call.aborted` → cascade to descendants)
- [ADR-017](017-call-protocol-client-and-adapter-contract.md) —
  `to_openapi` as a projection; published-spec compatibility contract
- [ADR-023](023-operation-error-schemas.md) — error schema fidelity in
  `from_openapi`/`to_openapi`; HTTP status mapping
- [ADR-042](042-openapi-gateway-pattern.md) — supersedes this ADR's
  `to_openapi` clause (the per-operation-paths projection is replaced by
  the 5-endpoint gateway pattern; the direct-call surface this ADR
  defines is unchanged)
- OQ-13 (resolved) — operation path format `/{service}/{op}`
- `docs/research/alknet-http/phase-0-findings.md` DH-3 — the decision this
  ADR resolves
- `crates/http/http-server.md` — the spec that implements this mapping