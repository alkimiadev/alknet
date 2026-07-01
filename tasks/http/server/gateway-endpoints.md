---
id: http/server/gateway-endpoints
name: Implement 5 gateway endpoints (search/schema/call/batch/subscribe) — axum route handlers
status: completed
depends_on: [http/server/http-adapter, http/gateway/gateway-dispatch-spine, http/gateway/error-mapping, http/server/bearer-auth-middleware]
scope: broad
risk: medium
impact: component
level: implementation
---

## Description

Implement the 5 fixed gateway endpoints in `src/server/gateway_routes.rs`.
These are the sole invoke path over HTTP (ADR-042, ADR-047): an HTTP
client invokes an operation via `POST /call` with
`{ "operation": "/fs/readFile", "input": {...} }`, discovers what it can
call via the `AccessControl`-filtered `GET /search`, and learns an
operation's shape via `GET /schema`. There is no per-operation
`POST /{service}/{op}` direct-call surface — the gateway is the invoke
path (ADR-047 supersedes ADR-036's direct-call surface).

### The 5 endpoints (http-server.md §"HTTP-to-call dispatch", http-adapters.md §"The gateway endpoint set")

| Endpoint | Call protocol | HTTP method | Purpose |
|----------|--------------|-------------|---------|
| `/search` | `services/list` | `GET` | List/search operations (AccessControl-filtered). Names + descriptions. |
| `/schema` | `services/schema` | `GET` | Get an operation's full `OperationSpec`. |
| `/call` | `call.requested` (Query/Mutation) | `POST` | Invoke an operation. Flat JSON body `{ operation, input }`. |
| `/batch` | multiple `call.requested` | `POST` | Invoke multiple operations. Array of `{ operation, input }`. |
| `/subscribe` | `call.requested` (Subscription) | `POST` (SSE) | Invoke a streaming operation. Body `{ operation, input }`; response `text/event-stream`. |

### `POST /call` dispatch (http-server.md §"HTTP-to-call dispatch")

1. The axum route handler reads the JSON body
   `{ "operation": "/fs/readFile", "input": {...} }`.
2. Resolves the caller's identity from the `Authorization: Bearer` header
   (via the shared `bearer_auth_middleware` — stashed in extensions as
   `ResolvedIdentity`).
3. Calls `GatewayDispatch::invoke(identity, operation, input)` — the
   shared dispatch spine (the `gateway-dispatch-spine` task). This builds
   the root `OperationContext` (`internal: false`, `forwarded_for: None`)
   and dispatches through `OperationRegistry::invoke()`.
4. The response (`ResponseEnvelope`) is serialized as the HTTP response
   body (JSON). Errors map to HTTP status codes via the `error-mapping`
   task (`call_error_to_http_response`).

`Internal` operations (ADR-015) return `404` (`NOT_FOUND`) — the gateway
dispatches only `External` operations, and the caller discovers which
`External` operations it can call via the `AccessControl`-filtered
`/search` endpoint. This is the per-caller API surface property: an HTTP
client cannot stub its toe on a path for an operation it can't call,
because there is no per-operation path — `/search` tells it what it can
call, `/call` invokes it, and the `AccessControl` check runs on `/call`
regardless.

### `GET /search` (AccessControl-filtered discovery)

Dispatches `services/list` through `GatewayDispatch::invoke()` with the
resolved caller identity. The `services/list` handler (already in
`OperationRegistry`) filters by `AccessControl::check(identity)` — the
client sees only the operations it is authorized to call. Returns
operation names + descriptions (not full schemas). Query parameters for
filtering/searching are a two-way-door extension (the v1 shape is "list
all I can call"; search/filter sugar is additive).

### `GET /schema`

Dispatches `services/schema` through `GatewayDispatch::invoke()` with
the resolved caller identity. Returns the operation's full
`OperationSpec` (input/output JSON Schemas, error schemas). The
`AccessControl` check runs (an unauthorized caller gets `FORBIDDEN`, not
the schema).

### `POST /batch`

Follows the same dispatch path as `/call` with an array of
`{ operation, input }` pairs (OQ-14). `batch` is a loop over
`GatewayDispatch::invoke()` in the gateway (research §6 open question
#3 — confirm `batch` is genuinely just a loop, no shared batch-specific
state, no transactional semantics). Returns an array of results (or
errors), one per entry, in order.

### `POST /subscribe` (SSE streaming projection)

A `Subscription` operation invoked via the gateway's `POST /subscribe`
endpoint projects its `call.responded` stream as Server-Sent Events. The
request body is `{ operation, input }` (the same flat JSON shape as
`/call`); the response is `text/event-stream` (negotiated via
`Accept: text/event-stream` on the `POST`). The axum route handler:

- Sets `Content-Type: text/event-stream`.
- For each `call.responded` event, writes an SSE `data:` frame (the
  event's `output` serialized as JSON).
- On `call.completed`, closes the SSE stream (normal end).
- On `call.aborted`, closes the stream with an SSE error event.
- On HTTP client disconnect (detected as the response writer closing),
  sends `call.aborted` for the in-flight subscription, which cascades
  to descendants per ADR-016.

This is the HTTP/1.1 + HTTP/2 streaming projection. Over WebSocket
(websocket.md), the subscription projects directly onto the WS
connection — `call.responded` events as binary WS messages, no SSE
framing. WebTransport (`h3`, deferred per ADR-044) would project onto
WebTransport bidirectional streams.

### One-directional projection (http-server.md §"One-directional projection")

The HTTP/1.1 + HTTP/2 surface is a **lossy, one-directional projection**
of the call protocol. HTTP is request/response: the client initiates, the
server responds. The call protocol is bidirectional — both sides can
initiate calls. The HTTP projection carries only the client→server call
direction; the server→client call direction has no HTTP expression.
`Subscription` streaming is the one partial exception — the server
streams `call.responded` frames back over the SSE response — but even
there, the *call* is client-initiated; only the *results* flow
server→client. WebSocket restores the bidirectional call model for
browsers (the `websocket/` tasks).

### Constraints

- **The gateway is the sole invoke path over HTTP (ADR-042, ADR-047).**
  No per-operation `POST /{service}/{op}` direct-call surface.
- **`External` operations only.** `Internal` operations return `404` on
  the gateway's `/call`, matching the call protocol's `NOT_FOUND`.
- **Bearer-only auth.** Via the shared `bearer_auth_middleware`.
- **No secret material in HTTP responses.** Capabilities are used for
  outbound calls (`from_openapi`), never serialized into HTTP response
  bodies (ADR-014).

## Acceptance Criteria

- [ ] `POST /call` route handler reads `{ operation, input }` JSON body
- [ ] `/call` resolves identity via `ResolvedIdentity` extractor (shared middleware)
- [ ] `/call` dispatches via `GatewayDispatch::invoke(identity, operation, input)`
- [ ] `/call` response is `ResponseEnvelope` serialized as JSON
- [ ] `/call` errors mapped via `call_error_to_http_response` (error-mapping task)
- [ ] `Internal` op on `/call` → `404 NOT_FOUND`
- [ ] `External` op with `AccessControl` restrictions + unauthorized → `403 FORBIDDEN`
- [ ] `External` op with `AccessControl` restrictions + no identity → `401`
- [ ] `GET /search` dispatches `services/list` via `GatewayDispatch::invoke`
- [ ] `/search` results are `AccessControl::check(identity)`-filtered
- [ ] `/search` returns operation names + descriptions (not full schemas)
- [ ] `GET /schema` dispatches `services/schema` via `GatewayDispatch::invoke`
- [ ] `/schema` returns the operation's full `OperationSpec`
- [ ] `/schema` for unauthorized op → `403 FORBIDDEN`
- [ ] `POST /batch` dispatches an array of `{ operation, input }` via loop over `invoke`
- [ ] `/batch` returns an array of results (or errors), one per entry, in order
- [ ] `POST /subscribe` sets `Content-Type: text/event-stream`
- [ ] `/subscribe` writes `call.responded` events as SSE `data:` frames
- [ ] `/subscribe` closes stream on `call.completed`
- [ ] `/subscribe` writes SSE error event on `call.aborted`
- [ ] `/subscribe` sends `call.aborted` on HTTP client disconnect (ADR-016 cascade)
- [ ] No per-operation `POST /{service}/{op}` direct-call surface (ADR-047)
- [ ] No secret material in HTTP response bodies (ADR-014)
- [ ] Integration test: `/call` round-trip (External op → 200 + JSON body)
- [ ] Integration test: `/call` Internal op → 404
- [ ] Integration test: `/call` unauthorized → 403
- [ ] Integration test: `/call` unauthenticated + restricted op → 401
- [ ] Integration test: `/search` returns only AccessControl-allowed ops
- [ ] Integration test: `/schema` returns full spec for authorized op
- [ ] Integration test: `/batch` returns array of results in order
- [ ] Integration test: `/subscribe` streams SSE events until completed
- [ ] Integration test: `/subscribe` client disconnect → abort cascade
- [ ] `cargo test -p alknet-http` succeeds
- [ ] `cargo clippy -p alknet-http --all-targets` succeeds with no warnings

## References

- docs/architecture/crates/http/http-server.md — HTTP-to-call dispatch, SSE projection, one-directional projection
- docs/architecture/crates/http/http-adapters.md — The gateway endpoint set, per-caller API surface
- docs/architecture/decisions/042-openapi-gateway-pattern.md — ADR-042 (5 fixed gateway endpoints)
- docs/architecture/decisions/047-remove-direct-call-http-surface.md — ADR-047 (gateway is sole invoke path)
- docs/architecture/decisions/015-privilege-model-and-authority-context.md — ADR-015 (Internal → 404)
- docs/architecture/decisions/016-abort-cascade-for-nested-calls.md — ADR-016 (disconnect → abort cascade)
- docs/architecture/decisions/014-secret-material-flow-and-capability-injection.md — ADR-014 (no secrets in responses)

## Notes

> The 5 gateway endpoints are the sole HTTP invoke path (ADR-047). The
> /call handler delegates to GatewayDispatch::invoke (the shared spine);
> the error mapping is the error-mapping task; the auth is the shared
> bearer-auth-middleware. /subscribe is the SSE streaming projection —
> the one to_openapi-specific piece that does not go through the shared
> spine's request/response invoke (research §6 open question #5 —
> /subscribe is to_openapi-owned, not in the shared core). /batch is a
> loop over invoke (research §6 open question #3 — confirm no
> batch-specific shared state). The one-directional projection is a
> structural property of HTTP; WebSocket restores bidirectionality for
> browsers.

## Summary

> Implemented 5 fixed gateway endpoints in src/server/gateway_routes.rs: POST /call,
> GET /search, GET /schema, POST /batch, POST /subscribe (SSE). All delegate to
> GatewayDispatch::invoke; auth via ResolvedIdentity extractor; errors mapped via
> call_error_to_http_response (identity-aware 401/403 split). Internal ops → 404.
> /schema adds ACL pre-check. /subscribe projects ResponseEnvelope as SSE. /batch
> loops over invoke returning array. Wired into adapter.rs replacing placeholder 501s.
> 188 tests pass. Clippy clean.
>
> Note: /subscribe SSE completes after single event (registry invoke returns single
> ResponseEnvelope, no streaming subscription handler yet — research §6 OQ#5).