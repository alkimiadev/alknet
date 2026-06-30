# ADR-042: OpenAPI Gateway Pattern for to_openapi

## Status

Proposed

## Context

The current `to_openapi` spec (`crates/http/http-adapters.md`) describes
`to_openapi` as generating a traditional OpenAPI document with one path
entry per alknet `External` operation — `POST /fs/readFile`,
`POST /agent/chat`, etc., each with parameters, request body, and
responses built from the operation's `input_schema`/`output_schema`/
`error_schemas`. This is the "inverse of `from_openapi`" framing: since
`from_openapi` merges OpenAPI path params / query params / request body
into a single flat JSON input schema, `to_openapi` should split them
back out.

### The flat→structured problem

The inverse is genuinely messy. The call protocol's input is a flat JSON
object (e.g., `{ path, content, encoding }` for `fs/writeFile`). To
generate a traditional OpenAPI path entry (`POST /fs/{path}` with path
param `path`, body `content`), `to_openapi` would need to know which
fields are path params, which are query params, and which is the body.
That information isn't in the flat schema — it's metadata the call
protocol doesn't carry because it doesn't care about HTTP parameter
structure. `to_openapi` would need either:

1. HTTP-specific metadata on `OperationSpec` (which fields are path
   params, etc.) — a leaky abstraction that puts HTTP concerns in the
   protocol-foundation crate (`alknet-call`), or
2. Heuristics (guess that fields named `id` are path params?) — fragile
   and wrong, or
3. Manual annotation per operation — boilerplate that defeats the "pure
   projection" promise.

All three are messy. The flat→structured split is the hard direction,
and it's the one `to_openapi` has to do.

### The per-caller API surface problem

A traditional OpenAPI document is static — it describes the full API
surface regardless of who's reading it. Real APIs have per-caller
authorization: an admin sees admin operations, a regular user sees a
subset. OpenAPI has no standard mechanism for "show me only what I have
access to." The Gitea API is a concrete failure case: its OpenAPI spec
dumps the full API (including admin operations) to every caller,
regardless of privilege. A user reading the spec can't tell which
endpoints they can actually call without trial-and-error `403`s.

The call protocol already has the per-caller filtering primitive:
`services/list` is `AccessControl::check(identity)`-filtered — the
caller sees only the operations they are authorized to call. A
`to_openapi` that generates a static full-surface doc loses this
property. A `to_openapi` that uses the gateway pattern preserves it.

### The pattern that works

The same tool-gateway pattern ADR-041 applies to `to_mcp` applies here:
`to_openapi` exposes a small fixed set of endpoints that gate access to
the full operation registry. The external client (a code generator, a
human developer, a `fetch`-based client) calls `search` to discover
operations, `schema` to learn an operation's input shape, `call` to
invoke. The input is always a flat JSON body — no path/query/body split
to reverse-engineer. JSON Schema for the input/output is already in the
`OperationSpec` — no conversion beyond wrapping it in OpenAPI's schema
format.

The OpenAPI gateway has one endpoint the MCP gateway doesn't:
`subscribe` (SSE). OpenAPI/SSE supports streaming; MCP tool calls don't.
So the OpenAPI gateway is 5 endpoints; the MCP gateway is 4.

## Decision

### 1. `to_openapi` exposes a fixed gateway endpoint set, not one path per operation

`to_openapi` generates an OpenAPI document with a small fixed set of
endpoints that gate access to the full operation registry. The external
client discovers and invokes operations through the gateway.

The gateway endpoint set (initial, two-way-door extensible):

| OpenAPI path | Call protocol operation | HTTP method | Purpose |
|--------------|------------------------|-------------|---------|
| `/search` | `services/list` | `GET` | List/search available operations (filtered by the caller's `AccessControl`). Returns names + descriptions. |
| `/schema` | `services/schema` | `GET` | Get an operation's full `OperationSpec` (input/output JSON Schemas, error schemas). |
| `/call` | `call.requested` (Query/Mutation) | `POST` | Invoke an operation by name with a JSON input. Returns the output or a typed error (ADR-023). |
| `/batch` | multiple `call.requested` | `POST` | Invoke multiple operations in one request (correlated request IDs, OQ-14). Returns an array of results. |
| `/subscribe` | `call.requested` (Subscription) | `GET` (SSE) | Invoke a streaming operation. Returns `text/event-stream` — each `call.responded` is an SSE frame, `call.completed` closes the stream. |

Five endpoints. The client calls `/search` to find operations, `/schema`
to learn the input shape, `/call` (or `/subscribe` for streaming) to
invoke. The input is always a flat JSON body (`{ operation:
"/fs/readFile", input: { ... } }`); the output is the operation's result
as JSON. No path/query/body split to reverse-engineer.

### 2. `subscribe` is the OpenAPI gateway's streaming endpoint (SSE)

The OpenAPI gateway includes `subscribe` (which the MCP gateway excludes
— ADR-041, MCP tool calls are request/response). The `subscribe`
endpoint maps `Subscription` operations onto SSE: `GET /subscribe` with
`Accept: text/event-stream`, each `call.responded` event is an SSE
`data:` frame, `call.completed` closes the stream, `call.aborted` closes
with an error frame. This is the same SSE projection ADR-036 describes
for `h2`/`http/1.1` clients — the gateway's `subscribe` endpoint is the
single SSE entry point instead of per-operation SSE streams.

### 3. The generated OpenAPI doc is per-caller (AccessControl-filtered)

The `/search` endpoint's results are filtered by the caller's
`AccessControl::check(identity)` — the client sees only the operations
it is authorized to call. The `/call` and `/subscribe` endpoints run the
same `AccessControl` check on dispatch. The generated OpenAPI doc
describes the gateway endpoints (5 fixed paths); the per-caller
operation surface is discovered through `/search`, not preloaded into
the doc.

This is the key advantage over a traditional per-operation-paths OpenAPI
doc: the per-caller API surface is the default, not an afterthought. A
client reading the gateway OpenAPI doc learns the gateway's shape (5
endpoints, stable); a client calling `/search` learns what *it* can call
(per-caller, AccessControl-filtered). The Gitea failure mode (dumping
admin ops to every caller) is structurally impossible — `/search` doesn't
return operations the caller can't call.

### 4. The gateway OpenAPI doc is a compatibility contract

Once published, the gateway endpoint set (5 endpoints) and the
request/response shapes are a compatibility contract (ADR-017
Consequences). Adding endpoints is additive (non-breaking); removing or
renaming is a one-way door. The initial 5-endpoint set is the published
contract. The versioning strategy for the generated doc was tracked as
OQ-39 (now **resolved by [ADR-045](045-to-openapi-gateway-spec-versioning.md)**:
`info.version` semver tracks the gateway endpoint contract, not the
operation set) — the gateway pattern simplifies versioning to 5 stable
endpoints instead of a per-operation surface.

### 5. A traditional per-operation-paths projection is additive, not replacement

A deployment that wants a traditional REST OpenAPI doc (per-operation
paths with split parameters) can build it as a separate projection with
the HTTP-specific metadata (which fields are path params, etc.). The
gateway pattern is the default `to_openapi` projection; the traditional
projection is an additive alternative for deployments that need it. The
gateway does not foreclose the traditional projection — it just doesn't
require it for the common case.

## Consequences

**Positive:**
- No flat→structured split. The gateway's input is always a flat JSON
  body (`{ operation, input }`); the operation's input/output schemas
  are already JSON Schemas in the `OperationSpec`. No reverse-
  engineering of path/query/body semantics. The messy direction of the
  `from_openapi` inverse is sidestepped.
- Per-caller API surface by default. `/search` is
  `AccessControl`-filtered; the client sees only what it can call. The
  Gitea failure mode (dumping admin ops to every caller) is structurally
  impossible. This is a property the traditional per-operation-paths
  OpenAPI doc cannot provide (OpenAPI has no per-caller filtering
  concept).
- Easy to build clients for. Any language's `fetch` + JSON Schema
  libraries can call the gateway: `POST /call` with a JSON body, get a
  JSON result. No code generator needed for the common case; a code
  generator produces a `CallClient` (call/search/schema/batch/
  subscribe) instead of typed per-operation methods.
- 5 stable endpoints instead of a per-operation surface. The
  versioning concern (OQ-39) is simpler — 5 endpoints that rarely
  change vs. a per-operation surface that changes on every operation
  addition/modification.
- `subscribe` maps cleanly onto SSE — the same projection ADR-036
  describes, just as a single gateway entry point instead of per-
  operation SSE streams.
- A deployment that wants the traditional REST surface can build it
  additively. The gateway doesn't foreclose it.

**Negative:**
- The generated OpenAPI doc is not a "nice UI" by default. A Swagger UI
  rendering shows 5 generic endpoints instead of a REST tree. This is
  the tradeoff for avoiding the flat→structured split and gaining per-
  caller filtering. A deployment that wants the nice UI builds the
  traditional projection (additive, with metadata).
- A code generator reading the gateway OpenAPI doc produces a
  `CallClient` (generic call/search/schema methods), not typed per-
  operation methods. Typed methods require the traditional projection
  (with metadata) or a client that reads `/search` + `/schema` and
  generates typed wrappers at build time. The gateway is optimized for
  the `fetch`-and-JSON-Schema use case, not the code-generation use
  case.
- The gateway doc is less "traditional" — a developer expecting a
  REST OpenAPI doc sees a small RPC-style surface instead. This is
  honest (the call protocol is a flat JSON RPC, not a REST API), but
  it's a departure from OpenAPI conventions.

## Assumptions

1. **The gateway endpoint set is stable.** Once external clients build
   against the 5-endpoint gateway, changing the endpoint set (renaming,
   removing) breaks them. Adding endpoints is additive (non-breaking).
   The initial 5-endpoint set is the published contract.

2. **`AccessControl` filtering is the right per-caller mechanism.** The
   client sees the operations it's authorized to call. If an operation's
   existence is itself sensitive, `Visibility::Internal` (ADR-015) is
   the mechanism — Internal ops are excluded from `services/list` and
   therefore from `/search` results. The gateway does not add a
   separate visibility layer.

3. **The common case is `fetch` + JSON Schema, not code generation.**
   The gateway is optimized for the developer who calls `POST /call`
   with a JSON body and parses the result. The code-generation case
   (typed per-operation methods) is served by the traditional projection
   (additive) or a client that generates wrappers from `/search` +
   `/schema` at build time.

4. **`subscribe` (SSE) is the streaming projection for the gateway.**
   Over `h2`/`http/1.1`, subscriptions are SSE. Over WebSocket (the v1
   browser bidirectional path, ADR-044), subscriptions project onto the
   WS connection directly as binary messages — the gateway's `/subscribe`
   is the `h2`/`http/1.1` SSE path; the WebSocket path is the native
   call-protocol session (`websocket.md`; the gateway shape does not
   appear on WS per [ADR-048](048-websocket-native-session-not-gateway.md)).
   WebTransport (`h3`, deferred per ADR-044) would project onto
   WebTransport streams; the deferred design is at
   `webtransport.md`.

## References

- [ADR-015](015-privilege-model-and-authority-context.md) —
  External/Internal visibility (Internal ops excluded from
  `services/list`, therefore from `/search`)
- [ADR-017](017-call-protocol-client-and-adapter-contract.md) —
  `to_*` adapters are projections; published-spec compatibility contract
- [ADR-023](023-operation-error-schemas.md) — typed error `details`
  mapped to OpenAPI error responses
- [ADR-036](036-http-to-call-operation-mapping.md) — the SSE projection
  for subscriptions over `h2`/`http/1.1` (the gateway's `/subscribe`
  endpoint uses the same SSE framing)
- [ADR-044](044-defer-webtransport-browsers-use-websocket.md) —
  WebSocket is the v1 browser bidirectional path; `h3`/WebTransport
  deferred (the gateway's `/subscribe` is the `h2`/`http/1.1` SSE path;
  the WS path is the native call-protocol session). ADR-038 is
  superseded by ADR-044.
- [ADR-041](041-mcp-tool-gateway-pattern.md) — the sibling gateway
  pattern for `to_mcp` (4 tools; `subscribe` excluded because MCP tool
  calls are request/response)
- OQ-39 — `to_openapi` published-spec versioning (simplified by the
  gateway pattern to 5 stable endpoints; **resolved by
  [ADR-045](045-to-openapi-gateway-spec-versioning.md)**)
- `crates/http/http-adapters.md` — the spec that implements the gateway