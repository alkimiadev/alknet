# ADR-047: Remove the Direct-Call HTTP Surface; Gateway Is the Sole Invoke Path

## Status

Proposed

## Supersedes

The "direct path mapping" clause of [ADR-036](036-http-to-call-operation-mapping.md)
§Decision ("Direct path mapping is the default HTTP surface") and §HTTP
method semantics. ADR-036's other clauses (SSE projection, Bearer auth,
`/healthz`, stealth decoy, error mapping, `External`-only dispatch)
remain in force — they are independent of the routing decision and are
reaffirmed by this ADR (see §"What survives from ADR-036").

## Context

ADR-036 defined the HTTP surface as **direct path mapping**:
`POST /{service}/{op}` → `call.requested` for every `External`
operation. An operation `fs/readFile` was served at `POST /fs/readFile`,
one HTTP path per operation — a REST-like surface mirroring the call
protocol's `/{service}/{op}` operation paths. This was the original HTTP
contract, decided before the simplified-contract / gateway-pattern
work landed.

Since then, three shifts made the direct-call surface a contradiction
with the architecture's settled model:

1. **ADR-042** replaced `to_openapi`'s per-operation-paths projection
   with the **gateway pattern** — 5 fixed endpoints (`/search`,
   `/schema`, `/call`, `/batch`, `/subscribe`) where the per-caller
   operation surface is discovered via `AccessControl`-filtered
   `/search`, not preloaded into a static doc. The gateway's `/call`
   endpoint is the invoke path: `POST /call` with
   `{ operation: "/fs/readFile", input: {...} }`. This is the same
   RPC-shape pattern MCP uses (`tools/call` with a tool name, ADR-041).

2. **The simplified contract is the few-fixed-endpoints model**, not a
   per-operation REST tree. The whole point of the gateway pattern
   (ADR-042) was to escape the "static full-surface dump" failure mode
   (the Gitea anti-pattern: every operation gets a path, every caller
   sees the full surface, per-caller access is an afterthought). The
   direct-call surface is that anti-pattern at the HTTP level: every
   `External` operation gets an HTTP path, the path exists regardless
   of the caller's privilege, and the caller discovers what it can call
   by trial-and-error `403`s. The gateway's `/search` exists precisely
   to make the per-caller surface the default; the direct-call surface
   re-introduces the problem the gateway solved.

3. **ADR-046** added the custom-routes extension point, so a
   deployment that genuinely wants a REST-like per-operation HTTP
   surface (e.g., to match a legacy API shape) builds it as a custom
   route projection (additive, deployment-owned, not the alknet
   default contract). The direct-call surface is no longer the only
   way to get per-operation HTTP paths; it's the *default* way, and
   it's the wrong default.

The result: the HTTP router currently has **two ways to invoke an
operation** — the direct-call surface (`POST /fs/readFile`) and the
gateway (`POST /call` with the operation name in the body). That is the
contradiction: the simplified contract says "a few core endpoints,"
and the direct-call surface is a second, per-operation invoke path that
duplicates the gateway's `/call` with a scheme the gateway was built
to replace. ADR-042's amendment explicitly preserved the direct-call
surface ("unchanged"); that preservation was a leftover from before
the simplified contract was fully thought through, not a deliberate
endorsement of two invoke paths.

### The clean-up

The direct-call surface is residual from early-stage planning, the
same way the pre-ADR-042 `to_openapi` per-operation-paths projection
was residual. ADR-042 cleaned up `to_openapi`; this ADR cleans up the
HTTP handler's routing. The gateway becomes the sole invoke path; the
per-operation HTTP paths go away.

### What about HTTP clients that knew operation names?

A client that previously called `POST /fs/readFile` now calls
`POST /call` with `{ "operation": "/fs/readFile", "input": {...} }`. The
operation name is still the call protocol's `/{service}/{op}` form
(OQ-13, unchanged) — it moves from the HTTP path to the request body.
The gateway's `/call` is the standard invoke endpoint; the direct path
was a REST-like affordance that the simplified contract deliberately
drops. This is a breaking change for any HTTP client built against the
direct-call surface, which is exactly why it needs an ADR — but the
direct-call surface has not been implemented or published yet (the
alknet-http crate is specced, not shipped), so the "break" is
paper-only: no external client depends on it.

## Decision

### 1. The gateway is the sole invoke path; the direct-call surface is removed

The `HttpAdapter`'s router serves the **5 fixed gateway endpoints**
(`/search`, `/schema`, `/call`, `/batch`, `/subscribe` — ADR-042) as
the only way to invoke operations over HTTP. There is no
`POST /{service}/{op}` direct-call surface. An HTTP client invokes an
operation by `POST /call` with
`{ "operation": "/{service}/{op}", "input": {...} }`.

The router's operation-invoke surface is the gateway's `/call`
endpoint, not a per-operation path set. The operation name is in the
request body, not the HTTP path — same shape as MCP's `tools/call`
(ADR-041) and the call protocol's own `call.requested`
(`operationId` + `input`).

### 2. The HTTP method semantics move to the gateway endpoints

ADR-036's `OperationType` → HTTP method mapping (`Query`→`GET`,
`Mutation`→`POST`, `Subscription`→`SSE`) no longer applies per-operation
at the HTTP path level, because there are no per-operation HTTP paths.
The gateway endpoints have fixed methods (ADR-042's table):
`/search` `GET`, `/schema` `GET`, `/call` `POST`, `/batch` `POST`,
`/subscribe` `GET` (SSE). The `OperationType` of the *called operation*
is carried in the request/result, not expressed in the HTTP verb — the
client calls `/call` with the operation name; the operation's type is
the registry's concern, not the HTTP method's. A `Query` operation and a
`Mutation` operation both go through `POST /call`; the distinction is
in the operation spec (discovered via `/schema`), not the HTTP surface.

### 3. What survives from ADR-036

ADR-036's routing decision is superseded, but its other clauses are
independent of routing and remain in force:

- **SSE projection for subscriptions over `h2`/`http/1.1`** (§Streaming
  projection). The gateway's `/subscribe` endpoint uses this SSE
  projection (ADR-042 §2). The framing (`call.responded` → SSE `data:`
  frame, `call.completed` → stream close, `call.aborted` → error frame)
  is unchanged; it is now the `/subscribe` endpoint's behavior, not a
  per-operation SSE stream.
- **Bearer auth** (§Auth). `Authorization: Bearer` →
  `resolve_from_token` on every gateway endpoint. Unchanged.
- **`/healthz`** (§`/healthz` and operational endpoints). Raw route, no
  auth, no call protocol. Unchanged.
- **Stealth decoy** (§Stealth mode). Unknown paths get the decoy.
  Unchanged — and now *all* operation invocations go through the 5
  gateway paths, so the "unknown path" surface is larger (anything not
  `/search`, `/schema`, `/call`, `/batch`, `/subscribe`, `/healthz`,
  `/openapi.json`, the MCP route, or a custom route per ADR-046 is
  decoy).
- **Error mapping** (the call `code` → HTTP status table in
  http-server.md, ADR-023). The gateway's `/call` endpoint returns the
  same error mapping. Unchanged in mechanism; the entry point is
  `/call` instead of `/{service}/{op}`.
- **`External`-only dispatch** (Assumption 2). The gateway's `/call`
  returns `404` (`NOT_FOUND`) for `Internal` operations, same as the
  direct-call surface did. The `AccessControl` check runs on the called
  operation regardless of the entry point.
- **Abort cascade on HTTP disconnect** (Consequences, citing ADR-016).
  An HTTP client disconnecting mid-`/subscribe` is detected as a stream
  close and sends `call.aborted`, cascading to descendants. Unchanged.

### 4. A deployment that wants per-operation HTTP paths builds them as custom routes (ADR-046)

A deployment that genuinely needs a REST-like per-operation HTTP
surface (to match a legacy API shape, to serve clients that can't
adapt to the gateway) builds it as a **custom route projection**
(ADR-046): the assembly layer injects an `axum::Router` with
`POST /{service}/{op}` handlers that dispatch into
`OperationRegistry::invoke()`. This is deployment-owned, additive, and
explicitly *not* the alknet default contract — the same status as an
OAI-compatible proxy. The direct-call surface is no longer a built-in
default; it's a projection a deployment can build if it needs it, on
the same extension point as any other custom HTTP surface.

This keeps the default surface small (5 gateway endpoints) while
preserving the *capability* for REST-like access — it just isn't free
by default, which is correct, because the per-operation path surface
has real costs (the static-surface problem) that the gateway avoids.

### 5. `to_openapi` describes the gateway, unchanged

`to_openapi` (ADR-042, ADR-045) already describes the 5 gateway
endpoints, not per-operation paths. Removing the direct-call surface
does not change what `to_openapi` generates — it already generated the
gateway doc. The `info.version` semver (ADR-045) tracks the gateway
contract; the direct-call surface was never in that contract. No change
to `to_openapi` or its versioning.

## Consequences

**Positive:**
- One invoke path over HTTP, not two. The HTTP surface is the 5 gateway
  endpoints — exactly the "few core endpoints" of the simplified
  contract. The contradiction with the gateway pattern is resolved.
- The per-caller API surface is the default, structurally. An HTTP
  client cannot stub its toe on `POST /admin/deleteUser` because that
  path does not exist; it calls `/call` with the operation name, and
  `/search` tells it what it can call. The Gitea failure mode is
  structurally impossible at the HTTP level, not just at the discovery
  level.
- The HTTP surface is honest about what the call protocol is: an RPC,
  not a REST API. The gateway's `/call` with `{ operation, input }` is
  the call protocol's own shape; the direct path mapping was a REST
  disguise that didn't fit (the flat JSON input, no path/query/body
  split — ADR-042 §"The flat→structured problem").
- A deployment that wants REST-like per-operation paths still can, via
  custom routes (ADR-046) — it's an explicit choice with its own costs,
  not a default that leaks the static-surface problem into every
  deployment.
- No change to `to_openapi` (already described the gateway), to the
  SSE projection (now on `/subscribe`), to Bearer auth, to `/healthz`,
  to stealth, or to error mapping. The cleanup is narrow: the routing
  decision only.

**Negative:**
- An HTTP client that knew an operation name can no longer call it at
  a predictable HTTP path. It must call `/call` with the operation name
  in the body. This is one layer of indirection, but it's the same
  indirection MCP uses and the same shape the call protocol uses
  natively. The operation name (OQ-13's `/{service}/{op}` form) is
  unchanged — it moves from the path to the body.
- The HTTP surface is RPC-shaped, not REST-shaped. A developer
  expecting `POST /fs/readFile` sees `POST /call` with a body instead.
  This is honest (the call protocol is a flat JSON RPC, ADR-042 §3), but
  it's a departure from the REST conventions ADR-036's direct-call
  surface offered. A deployment that needs the REST shape builds it as a
  custom route projection (ADR-046).
- The `OperationType` → HTTP method mapping (`Query`→`GET` etc.) no
  longer applies at the HTTP level. A `Query` operation and a
  `Mutation` operation both go through `POST /call`. The distinction is
  in the operation spec (visible via `/schema`), not the HTTP verb. This
  loses a small amount of HTTP-level signal (a load balancer can't tell
  a read from a write by method), but the call protocol's
  `OperationType` was always a registry concern, not an HTTP concern —
  the direct-call surface borrowed HTTP verbs to express it, and the
  gateway doesn't.

## Assumptions

1. **No external client depends on the direct-call surface.** The
   alknet-http crate is specced, not shipped; the direct-call surface
   has not been published. Removing it is a paper-only break — no
   deployed client breaks. This is why the cleanup is cheap now and
   would be expensive after implementation.

2. **The gateway's `/call` is a sufficient invoke path for HTTP
   clients.** Any operation callable via `POST /{service}/{op}` is
   callable via `POST /call` with the operation name in the body. The
   operation name form (`/{service}/{op}`, OQ-13) is unchanged. The
   input/output shapes are unchanged. The only difference is where the
   operation name lives (path vs body).

3. **A deployment needing REST-like per-operation paths builds them
   explicitly.** Via ADR-046 custom routes. This is not a common need —
   the gateway's `/call` covers the standard invoke case, and the
   OAI-compatible-proxy pattern (ADR-046) covers the "match an external
   API shape" case. The direct-call surface was a default that served
   neither case particularly well (it wasn't REST-conventional, per
   ADR-036 §Negative, and it leaked the static-surface problem).

4. **The gateway endpoints are stable (ADR-042 Assumption 1).**
   Removing the direct-call surface does not change the gateway
   endpoint set; the 5 endpoints are the published contract. This ADR
   narrows the HTTP surface *to* that contract, it does not modify the
   contract itself.

## References

- [ADR-036](036-http-to-call-operation-mapping.md) — the ADR whose
  routing decision this supersedes (§Decision, §HTTP method semantics);
  its other clauses survive (§"What survives from ADR-036")
- [ADR-042](042-openapi-gateway-pattern.md) — the gateway pattern that
  made the direct-call surface redundant; its amendment to ADR-036
  preserved the direct-call surface, which this ADR reverses
- [ADR-044](044-defer-webtransport-browsers-use-websocket.md) —
  WebSocket is the browser bidirectional path (the direct-call surface
  was the `h2`/`http/1.1` one-directional path; removing it does not
  affect WebSocket, which carries the call protocol natively)
- [ADR-046](046-assembly-layer-custom-http-routes.md) — the extension
  point a deployment uses to build a per-operation HTTP surface if it
  needs one (the direct-call surface's replacement for the rare case)
- [ADR-045](045-to-openapi-gateway-spec-versioning.md) — `to_openapi`
  versions the gateway contract (unchanged; the direct-call surface
  was never in the contract)
- OQ-13 (resolved) — operation path format `/{service}/{op}` is
  unchanged; it moves from the HTTP path to the `/call` request body
- `crates/http/http-server.md` — the spec whose router surface this ADR
  narrows to the gateway endpoints