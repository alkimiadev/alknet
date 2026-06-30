---
status: draft
last_updated: 2026-06-30
---

# HTTP Server

The `HttpAdapter` — the `ProtocolHandler` for `h2` and `http/1.1` (and
WebSocket upgrade — see §"WebSocket browser path"). The `h3`/WebTransport
path is deferred per [ADR-044](../../decisions/044-defer-webtransport-browsers-use-websocket.md);
the deferred spec is at [webtransport.md](webtransport.md). This document
covers how axum is run over a QUIC bidirectional stream, Bearer auth
resolution, the HTTP-to-call dispatch, the `/healthz` raw route, stealth
decoy, and the WebSocket browser path.

## What

The `HttpAdapter` is constructed by the assembly layer with an
`Arc<dyn IdentityProvider>` (constructor injection, same pattern as
`SshAdapter` — see [auth.md](../core/auth.md)) and an
`Arc<OperationRegistry>` (for dispatching HTTP requests to call-protocol
operations). It implements `ProtocolHandler` for the standard HTTP ALPNs.

```rust
pub struct HttpAdapter {
    identity_provider: Arc<dyn IdentityProvider>,
    registry: Arc<OperationRegistry>,
    /// The default handler for paths that are not registered operations
    /// (stealth decoy). Configurable: a static site, a fake 404, a
    /// redirect. Two-way-door default (ADR-010).
    decoy: DecoyConfig,
    /// Deployment-specific routes added by the assembly layer (ADR-046).
    /// None = the default surface only. Custom routes are raw HTTP, not
    /// call-protocol operations; they coexist with the default surface and
    /// are not described by `to_openapi`.
    extra_routes: Option<Router>,
}

/// The stealth decoy surface for paths that are not registered
/// operations (and not `/healthz`, `/openapi.json`, the `to_openapi`
/// gateway endpoints `/search`/`/schema`/`/call`/`/batch`/`/subscribe`,
/// or the MCP route). Set by the assembly layer at `HttpAdapter`
/// construction. The existence of the decoy path is fixed by ADR-010;
/// the variant is a two-way-door config default.
pub enum DecoyConfig {
    /// Serve a fake `404 Not Found` (the default — matches the reference
    /// implementation's "fake nginx 404").
    NotFound,
    /// Serve a static site from a configured directory (the directory
    /// path is the payload). For deployments that want a real decoy
    /// website.
    StaticSite { root: PathBuf },
    /// Redirect to a configured URL.
    Redirect { to: String },
}

#[async_trait]
impl ProtocolHandler for HttpAdapter {
    fn alpn(&self) -> &'static [u8];   // returns the configured ALPN
    async fn handle(&self, connection: Connection, auth: &AuthContext) -> Result<(), HandlerError>;
}
```

The `HttpAdapter` registers for multiple ALPNs (`http/1.1`, `h2`).
The endpoint's `HandlerRegistry` maps each ALPN byte string to the same
adapter instance; `handle()` branches on `connection.remote_alpn()` to
pick the HTTP framing. For `http/1.1` and `h2`, the framing is hyper's
HTTP/1.1 or HTTP/2 over a QUIC bidirectional stream. WebSocket upgrade
(§"WebSocket browser path") layers on top of the same hyper connection
driver — a WS upgrade is an HTTP/1.1 or HTTP/2 request that switches
protocols. The `h3` ALPN is deferred (ADR-044); the deferred handler
design is at [webtransport.md](webtransport.md).

## Why

HTTP is the standard external interface. Browsers, curl, axios, API
gateways, and load balancers all speak HTTP. Serving HTTP on the standard
ALPNs means any HTTP client can connect without knowing about alknet —
the TLS handshake negotiates `h2` or `http/1.1` normally. This is the
stealth mapping (ADR-010): the HTTP surface is the decoy for clients that
don't offer alknet ALPNs, and the real external API surface for clients
that do know about alknet.

## Architecture

### Running axum over a QUIC stream

The `HttpAdapter::handle()` method for `h2`/`http/1.1`:

1. Accepts one bidirectional stream from the QUIC connection
   (`connection.accept_bi()` → `(SendStream, RecvStream)`).
2. Wraps the `(SendStream, RecvStream)` pair as a hyper
   `TokioIo`-compatible duplex stream — the same byte stream hyper
   expects for an HTTP connection.
3. Constructs the axum `Router` (built once at adapter construction,
   cloned per connection — axum `Router` is `Clone` and cheap to clone).
4. Hands the duplex stream + the axum router to hyper's connection
   driver (`hyper::server::conn::http1::Builder` or
   `http2::Builder::serve_connection`), which reads HTTP frames, parses
   them, dispatches to axum routes, and writes HTTP responses.
5. Returns when the HTTP connection closes (the client disconnects or
   the stream ends).

The axum `Router` is built once at adapter construction with the
`Arc<OperationRegistry>` and `Arc<dyn IdentityProvider>` embedded in its
state; cloning the `Router` per connection clones the `Arc`s (cheap,
shared state), so every request handler has access to the registry and
identity provider through the router's state.

The axum `Router` is the single routing surface for HTTP requests. It
contains:

- **The `to_openapi` gateway endpoints** (`/search`, `/schema`, `/call`,
  `/batch`, `/subscribe` — ADR-042). These 5 fixed endpoints are the
  sole invoke path over HTTP: an HTTP client invokes an operation via
  `POST /call` with `{ "operation": "/{service}/{op}", "input": {...} }`,
  discovers available operations via `GET /search`
  (`AccessControl`-filtered), and learns an operation's shape via `GET
  /schema`. `/subscribe` is the SSE streaming invoke path. There is no
  per-operation `POST /{service}/{op}` direct-call surface — the
  gateway is the invoke path (ADR-047 supersedes ADR-036's direct-call
  surface; the simplified contract is a few fixed endpoints, not a
  per-operation REST tree). `/call` and `/subscribe` dispatch through
  `OperationRegistry::invoke()`; `/search` and `/schema` dispatch the
  `services/list` / `services/schema` discovery ops.
- `GET /healthz` (raw route, no auth, no call protocol).
- `GET /openapi.json` (serves the `to_openapi` projection — the OpenAPI
  document that *describes* the 5 gateway endpoints. The doc describes
  the 5 fixed endpoints, and the per-caller operation surface is
  discovered via `/search`, not preloaded into `paths`. The doc carries
  `info.version` (semver) tracking the gateway endpoint contract —
  consumers detect breaking changes via the major version (ADR-045)).
- The stealth decoy fallback (unknown paths).
- (Feature-gated) `POST /mcp` (the `to_mcp` streamable HTTP service —
  [http-mcp.md](http-mcp.md)).
- **Deployment-specific custom routes** (ADR-046). The assembly layer
  may inject an `axum::Router` of extra routes at `HttpAdapter`
  construction — e.g., an OpenAI-compatible proxy at
  `/v1/chat/completions` that dispatches into the registry. These are
  raw HTTP, not call-protocol operations: not in the
  `OperationRegistry`, not discoverable via `/search`, not described
  by `to_openapi`. The default surface's reserved paths take precedence
  on collision; custom routes namespace away from the reserved set
  naturally (`/v1/...`). A deployment that passes no extra routes gets
  exactly the default surface above. A deployment that wants a
  REST-like per-operation HTTP surface (the former direct-call shape)
  builds it as a custom route projection (ADR-047 §4). See ADR-046 and
  §"Custom routes" below.

A single HTTP/2 or HTTP/1.1 connection multiplexes multiple requests
over the one bidirectional stream (HTTP/2 multiplexing is native;
HTTP/1.1 is sequential). The axum router handles each request on a
tokio task; the hyper driver manages the connection lifetime.

### HTTP-to-call dispatch (the gateway's `/call`; ADR-042, ADR-047)

An HTTP client invokes an operation via the gateway's `/call` endpoint:

1. The axum route handler for `POST /call` reads the JSON body
   `{ "operation": "/fs/readFile", "input": {...} }`.
2. It resolves the caller's identity from the `Authorization: Bearer`
   header via `identity_provider.resolve_from_token(&AuthToken { raw:
   token_bytes })`.
3. It constructs the root `OperationContext` (caller identity, the
   registration bundle's capabilities, the connection's env composition)
   and dispatches through the `OperationRegistry::invoke()` — the same
   dispatch path the `CallAdapter` uses for `alknet/call` wire requests.
4. The response (`ResponseEnvelope`) is serialized as the HTTP response
   body (JSON). Errors map to HTTP status codes (see Error Mapping
   below).

`Internal` operations (ADR-015) return `404` (`NOT_FOUND`) — the gateway
dispatches only `External` operations, and the caller discovers which
`External` operations it can call via the `AccessControl`-filtered
`/search` endpoint. This is the per-caller API surface property that
the direct-call surface (removed, ADR-047) lacked: an HTTP client cannot
stub its toe on a path for an operation it can't call, because there is
no per-operation path — `/search` tells it what it can call, `/call`
invokes it, and the `AccessControl` check runs on `/call` regardless.

`/batch` follows the same dispatch path with an array of
`{ operation, input }` pairs (OQ-14); `/subscribe` follows it with the
SSE streaming projection (below).

### Streaming projection (SSE — the gateway's `/subscribe`)

A `Subscription` operation invoked via the gateway's `/subscribe`
endpoint projects its `call.responded` stream as Server-Sent Events.
The axum route handler:

- Sets `Content-Type: text/event-stream`.
- For each `call.responded` event, writes an SSE `data:` frame (the
  event's `output` serialized as JSON).
- On `call.completed`, closes the SSE stream (normal end).
- On `call.aborted`, closes the stream with an SSE error event.
- On HTTP client disconnect (detected as the response writer closing),
  sends `call.aborted` for the in-flight subscription, which cascades
  to descendants per ADR-016.

This is the HTTP/1.1 + HTTP/2 streaming projection. Over WebSocket
(§"WebSocket browser path" below), the subscription projects directly
onto the WS connection — `call.responded` events as binary WS messages,
no SSE framing. WebTransport (`h3`, deferred per ADR-044) would project
onto WebTransport bidirectional streams; see
[webtransport.md](webtransport.md).

### One-directional projection (HTTP request/response)

The HTTP/1.1 + HTTP/2 surface is a **lossy, one-directional projection**
of the call protocol. HTTP is request/response: the client initiates,
the server responds. The call protocol is bidirectional — both sides can
initiate calls (see
[../call/call-protocol.md](../call/call-protocol.md) §"Bidirectional
Calls": the server can call operations on the client just as the client
calls operations on the server). The HTTP projection carries only the
client→server call direction; the server→client call direction has no
HTTP expression (there is no HTTP mechanism for the server to initiate a
request to the client). `Subscription` streaming is the one partial
exception — the server streams `call.responded` frames back over the
SSE response — but even there, the *call* is client-initiated; only the
*results* flow server→client.

This is a structural property of HTTP, not a design choice in this
crate. **WebSocket restores the bidirectional call model for browsers**
(see §"WebSocket browser path" below): a WS connection is a long-lived
full-duplex channel over which either side can send `call.requested`
frames in either direction — the call protocol's native bidirectionality
applies unchanged (ADR-012 — stream-agnostic correlation; a WS message
stream is another `BiStream`-satisfying transport). WebTransport (`h3`)
would restore it via native multi-stream multiplexing, but WebTransport
is deferred per ADR-044 — WebSocket is the v1 browser bidirectional path.
The HTTP/1.1 + HTTP/2 surface is the projection for clients that only
speak HTTP; WebSocket is the surface for browser clients that speak the
call protocol in both directions.

### WebSocket browser path (ADR-044)

A browser connecting to a hub upgrades an HTTP/1.1 or HTTP/2 request to
WebSocket (RFC 6455). The resulting full-duplex WS connection carries
call-protocol `EventEnvelope` frames as binary WebSocket messages — one
envelope per message. The browser authenticates by bearer token on the
upgrade request (the HTTP `Authorization` header), resolved by the hub's
`IdentityProvider::resolve_from_token`, same as any HTTP request. The WS
connection is then a **bidirectional call-protocol session**:

- The browser opens the WS connection to `/alknet/call` (or `/`).
- The handler hands the WS message stream to the call protocol's
  `Dispatcher` — the same dispatch loop the `CallAdapter` uses for
  `alknet/call` QUIC connections (ADR-012, stream-agnostic correlation).
- The browser writes `EventEnvelope` frames as binary WS messages; the
  handler reads them and dispatches via `OperationRegistry::invoke()`.
- Responses (`call.responded`, `call.error`, `call.completed`,
  `call.aborted`) are written back as binary WS messages.

**Bidirectionality:** the WS call-protocol session inherits the call
protocol's native bidirectionality — both sides can initiate calls
(ADR-043 §2, transferred to WebSocket per ADR-044 §3). The browser calls
operations on the hub; the hub can call operations registered on the
browser's side, over the same session, using the same `PendingRequestMap`
and `EventEnvelope` framing as `alknet/call`. The browser case where the
client registers no operations of its own is the common case — the
server→client call direction is unused because the browser has nothing
to call. That is a use-case scoping, not an architectural limitation.

**No SSE translation.** A `Subscription` operation served over WebSocket
projects its `call.responded` stream directly as binary WS messages — no
SSE `data:` framing. `call.completed` closes the stream; `call.aborted`
closes it with an error frame. This is the native streaming projection
for the WS path; SSE (ADR-036) is the projection for `h2`/`http/1.1`
clients that don't upgrade to WebSocket.

**Browsers are not alknet peers.** A browser over WebSocket authenticates
by bearer token, gets no `PeerId`, does not enter `PeerCompositeEnv`, and
its registered ops (if any) land in a connection-local Layer 2 overlay —
the inbound mirror of ADR-034 §2. The rationale (addressability vs.
bidirectionality) is stated in ADR-044 §5 and amends ADR-034 §4 by
reference. In short: "peer" means an addressable node in the
call-protocol peer graph (stable `PeerId`, `PeerRef::Specific`-reachable,
identity stable across reconnects), not "any endpoint that exchanges
calls during a live session." A browser is the second thing but not the
first — it has no stable cryptographic identity of its own (it presents
a bearer token the hub issued; nothing to pin), it is ephemeral (close
the tab → connection dies → the connection-local overlay dies with it),
and it is not addressable from other nodes (another alknet node has no
way to reach "the browser currently connected to hub-A"; the hub holds
it as a live `CallConnection` handle, not a peer-graph entry). The
connection-local overlay is what gives the browser bidirectional-call
capability *without* peer-graph membership.

**What WebSocket does not provide (deferred to WebTransport, ADR-044):**
the ALPN-stream-proxy (ADR-040) — a browser running a WASM parser for
SSH/SFTP/git to reach a non-call ALPN — requires WebTransport's
multi-stream model and is the speculative use case whose deferral is
ADR-044's reversal trigger. WebSocket carries the call protocol from a
browser; it does not carry the non-call-ALPN substrate. A browser cannot
reach SSH/SFTP/git ALPNs in the v1 release. See ADR-044.

### Auth

Inbound HTTP auth is `Authorization: Bearer <token>`, resolved via
`IdentityProvider::resolve_from_token()` (the auth.md handler table:
`HttpAdapter`, Bearer header, `resolve_from_token`). Bearer-only is the
auth mechanism for the default surface; other HTTP auth schemes (Basic,
API key in query param) are not implemented and would be added as axum
middleware (two-way door). This is recorded in
[ADR-036](../../decisions/036-http-to-call-operation-mapping.md) §Auth;
the resolution mechanism (`resolve_from_token`) is from
[ADR-004](../../decisions/004-auth-as-shared-core.md), and the
connection-level observability (`set_identity`) is OQ-11 (resolved).

- Bearer-only is the auth mechanism. Basic auth, API keys in query
  params, and other HTTP auth schemes are not implemented. A deployment
  that needs a different auth scheme adds it as axum middleware
  (two-way door), but the default surface is Bearer-only.
- The `HttpAdapter` constructor-injects `Arc<dyn IdentityProvider>`,
  same pattern as `SshAdapter`.
- An unauthenticated request to an operation with `AccessControl`
  restrictions returns `401` (no token) or `403` (token present but
  insufficient scopes). The call protocol's `FORBIDDEN` protocol code
  maps to `403`; `NOT_FOUND` (Internal op) maps to `404`.
- The HTTP handler stores the resolved identity on the `Connection` for
  observability (`connection.set_identity(identity)`), same as the call
  protocol handler.

### Error Mapping

Call-protocol `CallError` codes (ADR-023) map to HTTP status codes:

| Call `code` | HTTP status | Notes |
|-------------|-------------|-------|
| `NOT_FOUND` (operation not registered, or Internal op) | `404` | |
| `FORBIDDEN` (insufficient scopes, or unauthenticated) | `401` (no token) / `403` (token present) | |
| `INVALID_INPUT` (schema mismatch) | `422` | |
| `TIMEOUT` | `504` | `retryable: true` |
| `INTERNAL` | `500` | |
| Operation-level domain code with `http_status` (ADR-023) | the declared `http_status` | `from_openapi`-imported ops carry the original status |
| Operation-level domain code without `http_status` | `500` | |

The `retryable` field from `CallError` maps to an HTTP `Retry-After`
hint for `503`/`429`-class errors. The mapping is a two-way-door
default (the exact status for ambiguous codes can be refined
additively); the one-way constraint is that protocol-level and
operation-level codes are distinct (ADR-023) and `from_openapi`-imported
codes are prefixed `HTTP_<status>` to avoid collision with protocol
codes.

### `/healthz` (raw route)

`GET /healthz` is a raw HTTP route outside the call protocol — no auth,
no operation registration, no `OperationContext`. It returns `200 OK`
with a plain-text body (e.g., `"ok"`) if the endpoint is healthy. This
is the infrastructure endpoint load balancers and orchestrators call;
it must work before identity is resolvable.

Other operational endpoints (metrics, dashboard) are call-protocol
operations if built (`/metrics/list`, `/dashboard/view`), not raw HTTP
routes. `healthz` is the one exception. See ADR-036.

### Stealth decoy

For paths that are not the gateway endpoints (`/search`, `/schema`,
`/call`, `/batch`, `/subscribe`), `/healthz`, `/openapi.json`, the MCP
route, or a custom route per ADR-046), the HTTP handler serves a decoy.
The decoy is configurable (`DecoyConfig`):

- A fake `404 Not Found` (the default — matches the reference
  implementation's "fake nginx 404").
- A static site (served from a configured directory).
- A redirect (to a configured URL).

The decoy is the stealth surface: a port scanner or a client that
doesn't offer alknet ALPNs connects on `h2`/`http/1.1` and sees the
decoy. Real services use `alknet/ssh`, `alknet/call`, etc. The decoy
config is a two-way-door default (an operator picks what to serve); the
*existence* of the stealth path is fixed by ADR-010. Custom routes
(ADR-046) take precedence over the decoy — a path matched by a custom
route is served by it, not the decoy; the decoy is the fallback for
paths matched by neither the default surface nor a custom route.

### Custom routes (ADR-046)

A deployment that needs HTTP endpoints outside the default surface
(direct-call + gateway + `/healthz` + `/openapi.json` + MCP) injects
them as an `axum::Router` at `HttpAdapter` construction. The classic use
case: an OpenAI-compatible proxy at `/v1/chat/completions` that wraps a
call-protocol operation (the deployment parses the OAI request, invokes
an `openai/chat` or `agent/chat` op via `OperationRegistry::invoke()`,
reformats the response as an OAI response). The hub is a standard
alknet node *plus* a deployment-specific HTTP surface.

Custom routes:

- Are **raw HTTP**, not call-protocol operations — not registered in the
  `OperationRegistry`, not discoverable via `/search`, not in the
  `to_openapi` gateway doc.
- **May** dispatch into the registry via
  `OperationRegistry::invoke()` with a proper `OperationContext`
  (caller identity from the resolved bearer token) — the OAI proxy
  does this. Or they may be pure HTTP (a webhook receiver, a static
  asset server) that never touches the registry.
- Run under the **default Bearer-auth middleware**; a route that wants
  different auth applies its own axum middleware (the deployment owns
  its custom routes' middleware stack).
- **Do not collide** with the reserved default-surface paths
  (`/{service}/{op}`, `/search`, `/schema`, `/call`, `/batch`,
  `/subscribe`, `/healthz`, `/openapi.json`, the MCP route) — the
  default surface wins on collision; custom routes namespace away
  naturally (`/v1/...`).
- Are **not versioned** by `to_openapi` (ADR-045 versions the gateway
  contract, not custom routes). The deployment versions its own custom
  routes however it wants.
- Are **immutable after construction** (matches OQ-04 / ADR-010's
  static-registration constraint; the `HttpAdapter` router is built once
  at startup).

The extension point is additive: a deployment that passes `None` gets
exactly the default surface. The mechanism (the constructor parameter)
is the one-way door — once downstream deployments build against it, it's
a contract (ADR-046). The specific routes a deployment adds are a
two-way door (add/remove freely). See
[ADR-046](../../decisions/046-assembly-layer-custom-http-routes.md).

## Constraints

- **The gateway is the sole invoke path over HTTP (ADR-042, ADR-047).**
  The 5 gateway endpoints (`/search`, `/schema`, `/call`, `/batch`,
  `/subscribe`) are the only way to invoke operations over HTTP. There
  is no per-operation `POST /{service}/{op}` direct-call surface — the
  simplified contract is a few fixed endpoints, not a per-operation
  REST tree. A client invokes an operation via `POST /call` with
  `{ "operation": "/{service}/{op}", "input": {...} }`; it discovers
  what it can call via the `AccessControl`-filtered `/search`. The
  per-caller API surface is the default (the Gitea failure mode — every
  operation gets a path, every caller sees the full surface — is
  structurally impossible). A deployment that wants a REST-like
  per-operation HTTP surface builds it as a custom route projection
  (ADR-046, ADR-047 §4).
- **`External` operations only.** `Internal` operations return `404`
  on the gateway's `/call`, matching the call protocol's `NOT_FOUND`.
- **Bearer-only auth.** `Authorization: Bearer` →
  `resolve_from_token`. Other HTTP auth schemes are not implemented.
- **No secret material in HTTP responses.** The call protocol carries no
  secret material (ADR-014); the HTTP handler inherits this constraint.
  Capabilities are used for outbound calls (`from_openapi`), never
  serialized into HTTP response bodies.
- **`/healthz` is raw.** No auth, no call protocol. The one raw route.
- **WebSocket is the browser bidirectional path (ADR-044).** A browser
  upgrades an HTTP request to WS and speaks the call protocol over binary
  messages. `h3`/WebTransport is deferred (ADR-044); the ALPN-stream-proxy
  (ADR-040) is not available in v1. The `h3` ALPN and its feature gate are
  not implemented in the initial release.
- **Custom routes are raw HTTP, not call-protocol operations
  (ADR-046).** The assembly layer injects an `axum::Router` of extra
  routes at `HttpAdapter` construction. They are not in the
  `OperationRegistry`, not discoverable via `/search`, not in the
  `to_openapi` doc. They may dispatch into the registry via
  `OperationRegistry::invoke()` (the OAI-compatible proxy pattern) or
  be pure HTTP. The default surface's reserved paths take precedence on
  collision. A deployment that passes no extra routes gets the default
  surface unchanged.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| ~~Direct path mapping (HTTP path = operation path)~~ | ~~[ADR-036](../../decisions/036-http-to-call-operation-mapping.md)~~ | **Superseded by ADR-047** — direct-call surface removed; gateway `/call` is the sole invoke path |
| Gateway is the sole invoke path over HTTP | [ADR-042](../../decisions/042-openapi-gateway-pattern.md), [ADR-047](../../decisions/047-remove-direct-call-http-surface.md) | 5 fixed gateway endpoints (`/search`/`/schema`/`/call`/`/batch`/`/subscribe`); `POST /call` with `{ operation, input }` is the invoke path; per-caller `AccessControl`-filtered `/search` is the discovery; no per-operation HTTP paths |
| `to_openapi` published-spec versioning | [ADR-045](../../decisions/045-to-openapi-gateway-spec-versioning.md) | `/openapi.json` carries `info.version` (semver) tracking the gateway contract, not the operation set |
| SSE projection for subscriptions (`/subscribe`) | [ADR-036](../../decisions/036-http-to-call-operation-mapping.md) §Streaming, [ADR-042](../../decisions/042-openapi-gateway-pattern.md) §2 | `call.responded` stream → SSE frames; the gateway's `/subscribe` endpoint is the entry point |
| `/healthz` is a raw route | [ADR-036](../../decisions/036-http-to-call-operation-mapping.md) | No auth, no call protocol |
| Stealth decoy | [ADR-010](../../decisions/010-alpn-router-and-endpoint.md) | HTTP handler on standard ALPNs serves decoy for non-gateway, non-custom, non-`/healthz` paths |
| Bearer auth via `resolve_from_token` | [ADR-004](../../decisions/004-auth-as-shared-core.md) | HTTP handler credential source (settled) |
| WebSocket is the browser bidirectional path | [ADR-044](../../decisions/044-defer-webtransport-browsers-use-websocket.md) | Browsers upgrade to WS; `EventEnvelope` over binary messages; `h3`/WebTransport deferred |
| Browsers are not alknet peers | [ADR-034](../../decisions/034-outgoing-only-x509-and-three-peer-roles.md) §4 (amended by ADR-044 §5) | Bearer token, no `PeerId`, connection-local overlay (addressability vs. bidirectionality) |
| Error mapping (call codes → HTTP status) | [ADR-023](../../decisions/023-operation-error-schemas.md) | Protocol/operation codes distinct; `HTTP_<status>` prefix for imported |
| Custom HTTP routes from the assembly layer | [ADR-046](../../decisions/046-assembly-layer-custom-http-routes.md) | `extra_routes: Option<Router>` at construction; raw HTTP, not operations; default surface takes precedence on collision |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-39** (resolved): `to_openapi` published-spec versioning —
  resolved by [ADR-045](../../decisions/045-to-openapi-gateway-spec-versioning.md):
  `info.version` semver tracks the gateway endpoint contract (not the
  operation set); the per-caller operation surface is discovered via
  `/search` and does not bump the version.
- **OQ-40** (resolved): reqwest client config and connection pooling —
  `ClientWithMiddleware` + middleware stack; the outbound HTTP client
  used by `from_openapi`/`from_mcp`.

## References

- [ADR-036](../../decisions/036-http-to-call-operation-mapping.md) — the
  HTTP-to-call mapping this server implements
- [ADR-044](../../decisions/044-defer-webtransport-browsers-use-websocket.md)
  — WebSocket is the v1 browser bidirectional path; `h3`/WebTransport
  deferred. States the "browser is not a peer" rationale (addressability
  vs. bidirectionality) that ADR-034 §4 closes without arguing.
  References the `@alkdev/pubsub` WebSocket prior art (the
  `EventEnvelope { type, id, payload }` client/server the call
  protocol's envelope was derived from).
- [overview.md](overview.md) — crate overview, adapter location map
- [webtransport.md](webtransport.md) — the deferred `h3` ALPN handler
  (kept intact for revival)
- [http-adapters.md](http-adapters.md) — `from_openapi`/`to_openapi`
- [../call/call-protocol.md](../call/call-protocol.md) — `EventEnvelope`
  wire format, `Dispatcher` (stream-agnostic; runs over WS unchanged),
  the `@alkdev/pubsub` prior-art note
- `/workspace/@alkdev/pubsub/src/event-target-websocket-client.ts`,
  `/workspace/@alkdev/pubsub/src/event-target-websocket-server.ts` —
  TypeScript prior art for the WS browser path (the
  `EventEnvelope { type, id, payload }` over WS binary messages)
- [../core/auth.md](../core/auth.md) — `IdentityProvider`, Bearer →
  `resolve_from_token`
- [../core/endpoint.md](../core/endpoint.md) — stealth mode as ALPN
  dispatch
- [../call/operation-registry.md](../call/operation-registry.md) —
  `OperationRegistry::invoke()`, the dispatch path HTTP requests hit