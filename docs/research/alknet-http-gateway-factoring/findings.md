# Research: alknet-http Gateway Factoring — Shared Dispatch Core vs Copy-Until-Third

**Status**: Complete
**Date**: 2026-07-01
**Scope**: Deep dive — architecture factoring decision for `to_mcp` / `to_openapi`
**Question**: Should the two `to_*` gateway projections share a common dispatch
core now, or remain separate implementations until a third gateway appears?

---

## 1. Summary

**Recommendation: conditional — extract a *thin* shared core now, but do not
build a `GatewayDispatch` trait or a gateway abstraction.**

The two gateways genuinely share a dispatch spine: resolve caller identity
(Bearer → `IdentityProvider::resolve_from_token`) → build a root
`OperationContext` → `OperationRegistry::invoke()` → return a
`ResponseEnvelope`. That spine is ~15–30 lines per gateway endpoint, and it is
*already mostly shared* through `OperationRegistry::invoke()` and the
`services/list` operation (which owns `AccessControl`-filtered listing for both
gateways). What is left to factor is a small `resolve_identity + build_context +
invoke` helper — a free `async fn` or a tiny struct holding
`Arc<OperationRegistry>` + `Arc<dyn IdentityProvider>`, returning a
`ResponseEnvelope`. Both gateways call it; each gateway then maps the
`ResponseEnvelope` to its own wire shape.

What is **not** worth sharing — and what a premature `GatewayDispatch` trait
would wrongly collapse — is the server-integration layer. `to_openapi` is five
axum route handlers (`POST /call`, `GET /search`, …); `to_mcp` is one rmcp
`ServerHandler` impl (`call_tool` / `list_tools`) wrapped by rmcp's
`StreamableHttpService`, a `tower::Service<Request<RequestBody>>` that owns
JSON-RPC framing, session management, SSE priming, and MCP-protocol-version
validation. These two shapes do not share a common trait surface, and forcing
them under one `GatewayDispatch` trait would either leak rmcp's
`CallToolResult`/`RequestContext` types into the shared core (wrong direction —
the core should be neutral) or require an adapter trait so abstract it has no
real methods. The wire-framing, discovery listing, streaming, and versioning
differences are all genuine and all live *outside* the dispatch spine.

The honest read: this is a "copy-until-third" situation *for the
server-integration and wire-framing layers*, and a "share now" situation *for
the dispatch spine*. The dispatch spine is small enough that the duplication
cost of *not* sharing it is also small — but the spine is also the one place
where a divergence bug (one gateway resolving identity differently, or building
`OperationContext` with a different `internal` flag, or mapping `CallError`
inconsistently) would be a real security/correctness issue. That asymmetry —
small to share, costly to diverge — is the case for extracting the thin helper
now and leaving the rest alone.

A third gateway (GraphQL, gRPC) is not on the horizon. If one appears, the
server-integration layer will need its own shape anyway (a GraphQL schema +
resolver tree, a gRPC service impl), and the thin shared spine will absorb
cleanly. Building a `GatewayDispatch` trait now, before a third shape exists to
validate the abstraction, is the classic premature-generalization failure mode.

---

## 2. The Shared Core Hypothesis — What the Two Gateways Genuinely Share

### 2.1 The dispatch spine, traced through both specs

**`to_openapi` `/call`** (`http-server.md:156-184`, `http-adapters.md:256-275`):

1. axum route handler for `POST /call` reads JSON body
   `{ "operation": "/fs/readFile", "input": {...} }`.
2. Resolves caller identity from `Authorization: Bearer` header via
   `identity_provider.resolve_from_token(&AuthToken { raw: token_bytes })`
   (`http-server.md:163-164`; `auth.md:211-214` defines the trait).
3. Constructs the root `OperationContext` (caller identity, registration
   bundle's capabilities, connection's env composition) and dispatches through
   `OperationRegistry::invoke()` — "the same dispatch path the `CallAdapter`
   uses for `alknet/call` wire requests" (`http-server.md:166-168`).
4. The response (`ResponseEnvelope`) is serialized as the HTTP response body
   (JSON). Errors map to HTTP status codes (`http-server.md:286-306`).

**`to_mcp` `call` tool** (`http-mcp.md:187-210`, ADR-041 §4 lines 105-113):

1. rmcp `ServerHandler::call_tool` receives `CallToolRequestParams { name,
   arguments, .. }` (`server.rs:303-309`; `model.rs:3098-3110`).
2. Auth: the Bearer middleware resolves the token via
   `IdentityProvider::resolve_from_token()`, "same as the HTTP server's auth
   (ADR-004)" (`http-mcp.md:205-207`). The rmcp example
   (`simple_auth_streamhttp.rs:73-89, 147-153`) confirms this is axum
   middleware layered *around* the nested `StreamableHttpService`, not inside
   it.
3. `call` → "dispatches `OperationRegistry::invoke()` (the same dispatch path
   the HTTP server uses, ADR-036)" (`http-mcp.md:199-200`; ADR-041 §4).
4. The result is mapped to an MCP `CallToolResult` (`structuredContent` for the
   output, or `isError: true` for a `CallError` with typed `details` per
   ADR-023) (`http-mcp.md:200-202`; ADR-041 §4 lines 110-113).

**The shared spine is explicit in both specs.** Both resolve identity the same
way (`resolve_from_token`), both build a root `OperationContext`, both dispatch
through `OperationRegistry::invoke()`, both get back a `ResponseEnvelope`
(`call-protocol.md:491-501`: `ResponseEnvelope { request_id, result:
Result<Value, CallError> }`). The only divergence in the spine itself is the
*output mapping*: `ResponseEnvelope` → HTTP `Response` (JSON body + status
code) vs `ResponseEnvelope` → `CallToolResult` (`structured_content` /
`is_error` / `content`).

### 2.2 `AccessControl`-filtered listing is *already* shared

The hypothesis in the research brief asks whether `AccessControl`-filtered
listing belongs in the shared core or the gateway. The specs answer: it is
already in the shared core — it is the `services/list` operation.

- `OperationRegistry` has built-in `services/list` and `services/schema`
  operations (`operation-registry.md:610-642`). `services/list` "only returns
  `External` operations to remote callers" and is `AccessControl::check`-
  filtered (`operation-registry.md:621`, `client-and-adapters.md:187-196`).
- `to_openapi` `/search` dispatches `services/list` (`http-adapters.md:260`).
- `to_mcp` `search` tool dispatches `services/list` (`http-mcp.md:194-195`,
  ADR-041 §1 lines 70-71).

Both gateways invoke the *same operation* for listing. The filtering logic lives
in the `services_list_handler`, not in either gateway. A `GatewayDispatch`
abstraction would not centralize listing — it is already centralized in the
registry. The gateway's only listing-specific job is to frame the
`services/list` result (OpenAPI JSON array vs MCP `ListToolsResult`-shaped
tool-list entries), which is wire framing, not dispatch.

### 2.3 The `OperationContext` construction is shared in *shape*, divergent in *one field*

The root `OperationContext` for a wire-ingress call is built by the dispatch
path with `internal: false` (`operation-registry.md:148-152`:
`internal` is "Set by `OperationEnv::invoke()` (true) or the `CallAdapter`
dispatch path (false) — never by handlers"). Both gateways build a root context
for a wire-ingress call, so both set `internal: false`. There is no
gateway-specific authority switch — the caller's `identity` is the resolved
bearer identity, `handler_identity` comes from the registration bundle,
`forwarded_for: None` (wire-ingress only, `operation-registry.md:180`).

The one field that differs: `request_id`. For `to_openapi` it is generated by
the HTTP handler (or the wire `call.requested` id, if the gateway is framed as
a call); for `to_mcp` it is the rmcp `RequestId` from the JSON-RPC request
(`tool.rs:36`, `tool.rs:206-213` passes `name`/`arguments` but the request id
lives on the `RequestContext`). This is a trivial divergence — a UUID v4 from
`generate_request_id()` (`operation-registry.md:204-223`) works for both. It is
not a factoring blocker.

### 2.4 Error mapping: shared *input*, divergent *output*

Both gateways consume the same `CallError { code, message, retryable, details }`
(`call-protocol.md:496-501`) and map it to their wire shape:

- `to_openapi`: `CallError.code` → HTTP status (`http-server.md:288-306`:
  `NOT_FOUND`→404, `FORBIDDEN`→401/403, `INVALID_INPUT`→422, `TIMEOUT`→504,
  `INTERNAL`→500, operation-level `http_status` → declared status).
- `to_mcp`: `CallError` → `CallToolResult` with `is_error: Some(true)` and
  typed `details` as `structured_content` (ADR-041 §4 lines 110-113;
  `model.rs:3014-3039` shows `CallToolResult::structured_error`).

The *input* (`CallError`) is shared; the *output* (HTTP status table vs
`CallToolResult` builder) is gateway-specific. The error-mapping code is ~15
lines per gateway and is genuinely different (an HTTP status is not a
`CallToolResult`). This belongs in each gateway, not in a shared core.

---

## 3. The Divergences That Resist Sharing

Five genuine divergences, each tied to a specific spec/SDK location:

### 3.1 Wire framing (HTTP JSON vs MCP `CallToolResult`)

- `to_openapi` `/call` returns an HTTP `Response` with a JSON body
  (`http-server.md:169-171`: "The response (`ResponseEnvelope`) is serialized
  as the HTTP response body (JSON)").
- `to_mcp` `call` returns `Result<CallToolResult, McpError>`
  (`server.rs:303-309`). `CallToolResult` is `{ content: Vec<ContentBlock>,
  structured_content: Option<Value>, is_error: Option<bool>, meta:
  Option<Meta> }` (`model.rs:2868-2881`). The success path uses
  `CallToolResult::structured(value)` (`model.rs:3006-3013`); the error path
  uses `CallToolResult::structured_error` or `CallToolResult::error`
  (`model.rs:2984-3039`).

These are different types with different serialization. A shared core that
produced `CallToolResult` would leak rmcp into `to_openapi`; a shared core that
produced HTTP `Response` would be useless to `to_mcp`. The neutral type is
`ResponseEnvelope`, and the `ResponseEnvelope` → wire-shape mapping is the
gateway's job.

### 3.2 Discovery shape (OpenAPI `/search` endpoint vs MCP `tools/list`)

- `to_openapi` exposes a `GET /search` HTTP endpoint
  (`http-adapters.md:258-260`) that returns operation names + descriptions as
  JSON. The OpenAPI doc *describes* the 5 gateway endpoints
  (`http-adapters.md:277-286`); the per-caller operation surface is discovered
  via `/search`.
- `to_mcp` exposes a `search` *MCP tool* (`http-mcp.md:167-172`) and relies on
  rmcp's `tools/list` (`server.rs:310-316, 541-547`) to advertise the *4 fixed
  gateway tools* (`http-mcp.md:189-192`: "On MCP `tools/list`: returns the
  fixed gateway tool set (4 tools: `search`, `schema`, `call`, `batch`), not
  the registry's operations").

The discovery models are structurally different: OpenAPI's is "one doc + one
`/search` endpoint"; MCP's is "`tools/list` returns the 4 meta-tools, and the
`search` meta-tool returns the registry's operations." A shared discovery
abstraction would have to model both, which is more complexity than the two
separate implementations. The `services/list` operation is the shared backend;
the discovery *framing* is gateway-specific.

### 3.3 Streaming (`/subscribe` SSE vs excluded)

- `to_openapi` includes `/subscribe` (SSE): `GET /subscribe` with
  `text/event-stream`, `call.responded` → SSE `data:` frame, `call.completed`
  → stream close (`http-adapters.md:264`, `http-server.md:186-206`, ADR-042 §2).
- `to_mcp` excludes streaming: "MCP tool calls are request/response by
  protocol design; streaming subscriptions don't fit the LLM tool-call
  pattern" (ADR-041 §2 lines 79-93). `Subscription` operations are filtered
  out of `search` results and cannot be invoked via `call`
  (`http-mcp.md:179-185`).

This is a one-sided divergence — `to_openapi` has a streaming endpoint,
`to_mcp` does not. A shared core that included streaming would force `to_mcp`
to carry dead code; a shared core that excluded it would force `to_openapi` to
own streaming entirely outside the core. The latter is correct: streaming is
`to_openapi`-specific.

### 3.4 Versioning (`info.version` semver vs none)

- `to_openapi` carries `info.version` (semver) tracking the gateway endpoint
  contract (ADR-045 §1: major = breaking gateway change, minor = additive,
  patch = wording). Per-caller operation changes do not bump the version
  (ADR-045 §1 lines 80-85).
- `to_mcp` has no versioning. The MCP `tools/list` returns the 4 fixed tools;
  there is no published-spec version field. (MCP's `protocolVersion` is the
  MCP-protocol version, negotiated via `initialize`, not an alknet gateway
  contract version — `tower.rs:183-212` validates the
  `MCP-Protocol-Version` header.)

Versioning is purely a `to_openapi` concern. It does not belong in a shared
core.

### 3.5 Server integration (axum routes vs rmcp `StreamableHttpService`)

This is the divergence that most constrains the factoring. See §4.

---

## 4. rmcp `StreamableHttpService` Constraints

### 4.1 The tower-service shape

`StreamableHttpService<S, M>` implements
`tower_service::Service<Request<RequestBody>>` (`tower.rs:570-594`):

```rust
impl<RequestBody, S, M> tower_service::Service<Request<RequestBody>> for StreamableHttpService<S, M>
where
    RequestBody: Body + Send + 'static,
    S: crate::Service<RoleServer> + Send + 'static,
    M: SessionManager,
    ...
{
    type Response = BoxResponse;
    type Error = Infallible;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;
    fn call(&mut self, req: http::Request<RequestBody>) -> Self::Future { ... }
}
```

It is nested into an axum `Router` via `Router::nest_service("/mcp",
mcp_service)` (`simple_auth_streamhttp.rs:147-153`). The service owns, *inside*
its `call` method:

- DNS-rebinding / Host / Origin validation (`tower.rs:411-461, 867-871`).
- `MCP-Protocol-Version` header validation (`tower.rs:183-212, 954, 1084`).
- JSON-RPC message deserialization (`tower.rs:1048-1053`).
- Session management (stateful mode: `create_session`, `has_session`,
  `restore_session`, `accept_message` — `tower.rs:1055-1220`; stateless mode:
  `serve_directly` — `tower.rs:1221-1302`).
- SSE priming events and keep-alive (`tower.rs:995-1003, 1196-1212`).
- The `initialize` handshake replay for cross-instance session restore
  (`tower.rs:703-861`).
- Response framing as SSE or JSON-direct (`tower.rs:1254-1292`,
  `json_response` config).

The `to_mcp` gateway does **not** write axum route handlers for
`search`/`schema`/`call`/`batch`. It implements rmcp's `ServerHandler` trait
(`server.rs:424-432`) — specifically `call_tool` (`server.rs:303-309, 533-539`)
and `list_tools` (`server.rs:310-316, 541-547`) — and `StreamableHttpService`
frames the wire. The gateway tools are MCP tools, not HTTP endpoints.

### 4.2 What this means for a shared core

The server-integration shapes are *different abstraction levels*:

- **`to_openapi`**: the gateway *is* the axum route layer. Five `async fn`
  handlers, each with axum extractors, each calling the dispatch spine and
  mapping to `Response`.
- **`to_mcp`**: the gateway *is* an rmcp `ServerHandler` impl. The axum route
  layer is `StreamableHttpService` (rmcp's code), and the gateway's `call_tool`
  / `list_tools` methods are called by rmcp's `serve_directly` /
  `serve_server` machinery (`tower.rs:1249, 671`).

A `GatewayDispatch` trait that abstracted over both would need to either:

1. **Be a tower `Service`** — but `to_openapi`'s five route handlers are not
   one `Service<Request>`; they are five separate `async fn`s composed by
   axum's `Router`. Forcing them into one `Service` would reimplement routing
   inside the service, duplicating axum.
2. **Be an async `fn`-shaped trait** (e.g., `async fn dispatch(...) ->
   ResponseEnvelope`) — but `to_mcp`'s `call_tool` returns
   `Result<CallToolResult, McpError>`, not `ResponseEnvelope`. The trait would
   need an associated output type, and each gateway would provide a different
   one, at which point the trait has no shared methods and is not an
   abstraction.
3. **Produce a neutral `ResponseEnvelope` and let each gateway wrap it** —
   this works, but it is not a *trait*; it is a *free function* (or a struct
   holding `Arc<OperationRegistry>` + `Arc<dyn IdentityProvider>` with a
   method like `async fn invoke_as(&self, identity, op, input) ->
   ResponseEnvelope`). Both gateways call it as a library function, not through
   a polymorphic trait.

Option 3 is the viable one, and it is exactly the "thin shared core" the
recommendation endorses. The shared core is a *library*, not a *trait*. It
produces `ResponseEnvelope` (the neutral type both gateways already consume),
and each gateway owns the `ResponseEnvelope` → wire-shape mapping.

### 4.3 Can a neutral result type feed both axum routes and a tower `Service`?

Yes. The neutral type is `ResponseEnvelope`, which already exists
(`call-protocol.md:491-501`). The flow:

- **Shared core** (a `GatewayDispatch` struct or free `fn`):
  `async fn invoke(&self, identity: Option<Identity>, op: &str, input: Value)
  -> ResponseEnvelope`. Internally: build root `OperationContext` (`internal:
  false`, `identity` from the resolved bearer, `handler_identity` from the
  registration, `forwarded_for: None`, fresh `request_id`), call
  `OperationRegistry::invoke()`, return the `ResponseEnvelope`.
- **`to_openapi` `/call` handler**: `async fn` with axum extractors → call
  shared core → match `ResponseEnvelope.result`: `Ok(v)` → `Json(v)` with 200;
  `Err(e)` → map `CallError.code` to HTTP status (`http-server.md:288-306`),
  body = `e.details` or error JSON.
- **`to_mcp` `call` tool**: `ServerHandler::call_tool` → dispatch on
  `params.name` ("call" → call shared core; "search" → invoke `services/list`;
  "schema" → invoke `services/schema`; "batch" → loop) → match
  `ResponseEnvelope.result`: `Ok(v)` →
  `CallToolResult::structured(v).into_call_tool_result()` (`model.rs:3006`,
  `tool.rs:82-86`); `Err(e)` →
  `CallToolResult::structured_error(e.details.unwrap_or(json!({}))).
  into_call_tool_result()` (`model.rs:3032`, `tool.rs:100-113`).

The `IntoCallToolResult` trait (`tool.rs:78-113`) is the bridge on the `to_mcp`
side — it converts `CallToolResult` / `ErrorData` / `Result<T,E>` into
`Result<CallToolResult, ErrorData>`. The shared core does not need to know
about it; the `to_mcp` gateway calls `.into_call_tool_result()` on the
`CallToolResult` it builds from the `ResponseEnvelope`.

### 4.4 The auth-extraction convergence (a point *for* sharing, not against)

Both gateways resolve the bearer token via axum middleware, not inside the
dispatch logic:

- `to_openapi`: axum middleware extracts `Authorization: Bearer`, calls
  `resolve_from_token`, stashes `Identity` in request state for the route
  handlers.
- `to_mcp`: the rmcp example (`simple_auth_streamhttp.rs:73-89, 147-153`)
  applies axum `middleware::from_fn_with_state` *around* the nested
  `StreamableHttpService`. The `to_mcp` spec confirms: "Auth: the Bearer
  middleware resolves the token via `IdentityProvider::resolve_from_token()`,
  same as the HTTP server's auth (ADR-004)" (`http-mcp.md:205-207`).

This means the *auth middleware* is shareable now — one axum layer that
resolves the bearer and stashes `Option<Identity>` in request extensions. The
`to_mcp` `call_tool` handler reads the `Identity` from
`RequestContext<RoleServer>.extensions` (rmcp injects `http::request::Parts`
into extensions — `tower.rs:487-521, 1086-1097`); the `to_openapi` handler
reads it from axum state/extractors. The *extraction* differs, but the
*resolution* is the same and can be one middleware. This is a second small
shared piece (alongside the dispatch spine).

---

## 5. Recommendation

### 5.1 Share the thin dispatch spine now; do not build a `GatewayDispatch` trait

Extract a small, concrete struct (not a trait) in `alknet-http`:

```rust
/// Shared dispatch spine for the `to_*` gateway projections.
/// Resolves identity, builds a root OperationContext, invokes the registry,
/// returns the neutral ResponseEnvelope. Each gateway maps the envelope to
/// its own wire shape.
pub struct GatewayDispatch {
    registry: Arc<OperationRegistry>,
    identity_provider: Arc<dyn IdentityProvider>,
}

impl GatewayDispatch {
    /// Resolve a bearer token to an Identity (shared by both gateways'
    /// axum auth middleware).
    pub fn resolve_bearer(&self, token: &AuthToken) -> Option<Identity> {
        self.identity_provider.resolve_from_token(token)
    }

    /// Invoke an operation as a wire-ingress caller. `internal: false`,
    /// `forwarded_for: None`, fresh request_id. Returns the neutral
    /// ResponseEnvelope; the gateway maps it to its wire shape.
    pub async fn invoke(
        &self,
        identity: Option<Identity>,
        op: &str,
        input: Value,
    ) -> ResponseEnvelope {
        // build root OperationContext, call self.registry.invoke(op, input, ctx)
        // ...
    }
}
```

Both `to_openapi` and `to_mcp` hold an `Arc<GatewayDispatch>` (or it lives in
axum state / rmcp service state). Each gateway owns:

- **Wire framing**: `ResponseEnvelope` → `axum::Response` (JSON + status) vs
  `ResponseEnvelope` → `CallToolResult` (`structured` / `structured_error`).
- **Discovery framing**: `/search` HTTP endpoint vs `tools/list` + `search`
  tool.
- **Streaming**: `/subscribe` SSE (`to_openapi` only).
- **Versioning**: `info.version` semver (`to_openapi` only).
- **Server integration**: 5 axum route handlers vs 1 rmcp `ServerHandler` impl
  wrapped by `StreamableHttpService`.

The shared core owns:

- Identity resolution (`resolve_bearer` — used by both gateways' axum
  middleware).
- Root `OperationContext` construction (`internal: false`, `forwarded_for:
  None`, fresh `request_id`, `identity` from the resolved bearer,
  `handler_identity` from the registration bundle).
- `OperationRegistry::invoke()` call.
- Returning the neutral `ResponseEnvelope`.

`AccessControl`-filtered listing stays in the `services/list` operation (where
it already is — `operation-registry.md:610-642`). The gateways invoke
`services/list` through the shared core; they do not reimplement filtering.

### 5.2 Why not a `GatewayDispatch` trait

A trait would need an associated output type (HTTP `Response` vs
`CallToolResult`), at which point it has no shared method bodies. Or it would
produce `ResponseEnvelope` — but then it is a struct, not a trait (there is one
implementation). The third gateway, if it ever appears, will have its own
output type (GraphQL response, gRPC message); a trait generalized now over two
output types will almost certainly not generalize over the third without
redesign. A concrete struct with a `ResponseEnvelope`-returning method absorbs
the third gateway cleanly (it calls the same method, maps to its own shape).

The rule of three applies: two gateways do not justify a trait abstraction for
the wire-framing layer. The dispatch spine is shared because it is *the same
code* (not the same shape, different code) — that is a struct, not a trait.

### 5.3 Why share the spine now rather than copy-until-third

The spine is small (~15–30 lines per endpoint), so the duplication cost of
copying is also small. The case for sharing now rests on the *asymmetry of
divergence cost*:

- A divergence in identity resolution (one gateway resolves the bearer
  differently) is a security bug — the two gateways would enforce different
  auth.
- A divergence in `OperationContext` construction (one gateway sets `internal:
  true` by mistake, or populates `forwarded_for` from an untrusted source) is
  a privilege-escalation or metadata-leak bug.
- A divergence in `invoke()` call shape (one gateway bypasses
  `AccessControl`, or skips the reachability check) is an authorization bug.

These are exactly the bugs that are easy to introduce by copy-paste and hard to
catch in review. A shared spine makes the two gateways *provably* identical on
the security-relevant axis (auth, authority, ACL), and lets them diverge only
on the wire-framing axis (where divergence is correct). That is worth the small
extraction cost now.

The wire-framing, discovery, streaming, and versioning layers are *not*
security-relevant in the same way — they are presentation, and copying them
is fine. A divergence in HTTP status mapping is a compatibility bug, not a
security bug.

### 5.4 What this rules out

- **No `GatewayDispatch` trait.** A concrete struct, not a polymorphic trait.
- **No shared `into_wire()` method.** The `ResponseEnvelope` → wire mapping is
  per-gateway; do not parameterize the core over it.
- **No shared streaming abstraction.** `/subscribe` SSE is `to_openapi`-only;
  do not build a `GatewayStream` trait for one implementation.
- **No shared discovery abstraction.** `services/list` is the shared backend;
  the discovery *framing* (OpenAPI `/search` vs MCP `tools/list` + `search`
  tool) is per-gateway.
- **No shared versioning.** `info.version` is `to_openapi`-only.

---

## 6. Open Questions (Spike-Needed)

These need a concrete implementation spike to confirm, not just spec reading:

1. **`OperationContext` construction ergonomics.** The root-context
   construction is currently specced as living in the `CallAdapter` dispatch
   path (`operation-registry.md:148-152`, `call-protocol.md` `build_root_
   context`). Extracting it into `GatewayDispatch::invoke` requires either
   (a) calling a shared `build_root_context` helper from both `CallAdapter`
   and `GatewayDispatch`, or (b) duplicating the construction logic. A spike
   should confirm `build_root_context` is reusable as a free function (it
   should be — it takes `identity`, `capabilities`, `env`, `deadline` and
   returns an `OperationContext`), and that `GatewayDispatch` can call it
   without re-implementing the `internal: false` / `forwarded_for: None`
   invariants. If `build_root_context` is tangled with `CallAdapter`-specific
   state (`PendingRequestMap`, the `Dispatcher`), the extraction is larger
   than this research assumes.

2. **rmcp `RequestContext` → `Identity` extraction.** The `to_mcp` `call_tool`
   handler receives `RequestContext<RoleServer>` (`server.rs:305`). The
   resolved `Identity` needs to flow from the axum auth middleware (which
   stashes it in request extensions) into the rmcp handler. rmcp injects
   `http::request::Parts` into extensions (`tower.rs:487-521, 1086-1097`), so
   the `Identity` (stashed by the axum middleware into `Parts.extensions`)
   should be retrievable via `ctx.extensions.get::<Identity>()` inside
   `call_tool`. A spike should confirm this extension-survives-the-rmcp-framing
   path works end-to-end — it is the load-bearing assumption for sharing the
   auth middleware.

3. **`batch` semantics.** Both gateways have a `batch` endpoint/tool
   (`http-adapters.md:263`, `http-mcp.md:172`, ADR-041 §1). The spec notes
   "correlated request IDs, OQ-14" — OQ-14 is open. The shared core's
   `invoke()` is per-operation; `batch` is a loop over `invoke()` in both
   gateways. A spike should confirm `batch` is genuinely just a loop (no
   shared batch-specific state, no transactional semantics) — if it is, `batch`
   stays in each gateway as a thin loop over the shared `invoke`. If OQ-14
   resolves to something more structured (atomic batch, partial-failure
   semantics), the shared core may need a `invoke_batch` method.

4. **`to_mcp` `search`/`schema` tool dispatch.** The `to_mcp` `call_tool`
   handler dispatches on `params.name` (`search`/`schema`/`call`/`batch`).
   `search` and `schema` invoke `services/list` / `services/schema` — through
   the shared `GatewayDispatch::invoke`, or directly? The shared core's
   `invoke()` calls `OperationRegistry::invoke()`, which works for
   `services/list` and `services/schema` (they are registered operations). A
   spike should confirm the `services/list` handler's `AccessControl`-
   filtering works when invoked through `GatewayDispatch::invoke` with the
   resolved bearer `Identity` — i.e., that the filtering sees the *caller's*
   identity, not a synthetic one. (It should — `services/list` is
   `AccessControl::check(identity)`-filtered, and `GatewayDispatch` passes the
   resolved identity as the caller.)

5. **`to_openapi` `/subscribe` and the shared core.** `/subscribe` is a
   streaming `Subscription` invocation — it produces a *stream* of
   `call.responded` events, not a single `ResponseEnvelope`. The shared
   `GatewayDispatch::invoke()` returns one `ResponseEnvelope` (the
   request/response shape). A spike should confirm `/subscribe` either (a)
   calls a different shared method (`invoke_subscribe` → returns a stream), or
   (b) is entirely `to_openapi`-specific and does not touch the shared core.
   Hypothesis: (b) is cleaner — `/subscribe` is SSE framing over a
   `Subscription` invoke, and the `Subscription` invoke path is already in
   `OperationRegistry` (the handler returns a stream via the `Handler` type,
   `operation-registry.md:94-96`). The shared core stays request/response;
   `/subscribe` is `to_openapi`-owned. Confirm with a spike.

---

## References

### alknet specs
- `docs/architecture/crates/http/http-adapters.md` — `to_openapi` spec
  (gateway endpoints §254-301, error fidelity §303-340, versioning §378-384)
- `docs/architecture/crates/http/http-mcp.md` — `to_mcp` spec (gateway tools
  §162-210, auth §205-207, subscription exclusion §179-185)
- `docs/architecture/crates/http/http-server.md` — `HttpAdapter`, axum router
  §86-149, `/call` dispatch §156-184, SSE §186-206, error mapping §286-306
- `docs/architecture/decisions/041-mcp-tool-gateway-pattern.md` — MCP gateway
  ADR (4 tools §1, subscription exclusion §2, AccessControl §5)
- `docs/architecture/decisions/042-openapi-gateway-pattern.md` — OpenAPI
  gateway ADR (5 endpoints §1, subscribe §2, per-caller §3)
- `docs/architecture/decisions/045-to-openapi-gateway-spec-versioning.md` —
  `info.version` semver §1
- `docs/architecture/crates/call/operation-registry.md` —
  `OperationRegistry::invoke()` (line 201), `OperationContext` (lines 110-174),
  `services/list`/`services/schema` (lines 610-642), `AccessControl` (lines
  72-90)
- `docs/architecture/crates/call/client-and-adapters.md` — `OperationAdapter`
  trait (lines 397-409), adapter location map (lines 432-455), `to_*` are
  projections (lines 427-429)
- `docs/architecture/crates/call/call-protocol.md` — `ResponseEnvelope` /
  `CallError` (lines 491-501)
- `docs/architecture/crates/core/auth.md` — `IdentityProvider` trait (lines
  211-214), `resolve_from_token` used by HTTP + call (line 218)

### rmcp SDK
- `rust-sdk/crates/rmcp/src/transport/streamable_http_server/tower.rs` —
  `StreamableHttpService` (lines 546-594), `Service<Request<RequestBody>>` impl
  (lines 570-594), DNS-rebinding/Host/Origin validation (lines 411-461),
  MCP-Protocol-Version validation (lines 183-212), session management (lines
  1055-1220), stateless mode (lines 1221-1302), `http::request::Parts`
  injection into extensions (lines 487-521, 1086-1097)
- `rust-sdk/crates/rmcp/src/handler/server.rs` — `ServerHandler` trait (lines
  424-432), `call_tool` (lines 303-309, 533-539), `list_tools` (lines 310-316,
  541-547)
- `rust-sdk/crates/rmcp/src/handler/server/tool.rs` — `IntoCallToolResult`
  trait (lines 78-113), `ToolCallContext` (lines 33-66), `CallToolHandler`
  (lines 151-156)
- `rust-sdk/crates/rmcp/src/model.rs` — `CallToolResult` (lines 2868-2881,
  2925-3039: `success`/`error`/`structured`/`structured_error`), 
  `CallToolRequestParams` (lines 3098-3110)
- `rust-sdk/crates/rmcp/src/service/tower.rs` — `TowerHandler` (lines 8-54),
  the rmcp-internal tower-Service-to-`Service<RoleServer>` adapter
- `rust-sdk/examples/servers/src/simple_auth_streamhttp.rs` — axum middleware
  around nested `StreamableHttpService` (lines 73-89, 147-153)

### Prior art
- `@alkdev/operations/docs/architecture/adapters.md` — TypeScript prior art
  (`from_openapi`, `from_mcp`, `FromSchema`, scanner)