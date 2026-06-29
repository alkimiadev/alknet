---
status: draft
last_updated: 2026-06-29
---

# HTTP Server

The `HttpAdapter` — the `ProtocolHandler` for `h2` and `http/1.1` (and
`h3`, covered in [webtransport.md](webtransport.md)). This document
covers how axum is run over a QUIC bidirectional stream, Bearer auth
resolution, the HTTP-to-call dispatch, the `/healthz` raw route, and
stealth decoy.

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

The `HttpAdapter` registers for multiple ALPNs (`http/1.1`, `h2`, `h3`).
The endpoint's `HandlerRegistry` maps each ALPN byte string to the same
adapter instance; `handle()` branches on `connection.remote_alpn()` to
pick the HTTP framing. For `http/1.1` and `h2`, the framing is hyper's
HTTP/1.1 or HTTP/2 over a QUIC bidirectional stream; for `h3`, it's the
WebTransport/HTTP/3 path (see [webtransport.md](webtransport.md)).

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

- **The direct-call surface** (`POST /{service}/{op}` → `call.requested`
  dispatch — ADR-036). This is the HTTP projection of the call protocol's
  `/{service}/{op}` operation path; an HTTP client that knows the
  operation name calls it directly.
- **The `to_openapi` gateway endpoints** (`/search`, `/schema`, `/call`,
  `/batch`, `/subscribe` — ADR-042). These are the fixed 5-endpoint
  gateway that an OpenAPI consumer uses to discover and invoke
  operations without knowing operation names up front. `/call` and
  `/subscribe` dispatch through the same `OperationRegistry::invoke()`
  as the direct-call surface; `/search` and `/schema` dispatch the
  `services/list` / `services/schema` discovery ops. The gateway and
  the direct-call surface coexist on the same router — they are two
  projections of the same operation registry, not two registries.
- `GET /healthz` (raw route, no auth, no call protocol).
- `GET /openapi.json` (serves the `to_openapi` projection — the OpenAPI
  document that *describes* the 5 gateway endpoints. Post-ADR-042 this
  is the gateway's description doc, not a per-operation REST spec; the
  doc describes the 5 fixed endpoints, and the per-caller operation
  surface is discovered via `/search`, not preloaded into `paths`).
- The stealth decoy fallback (unknown paths).
- (Feature-gated) `POST /mcp` (the `to_mcp` streamable HTTP service —
  [http-mcp.md](http-mcp.md)).

A single HTTP/2 or HTTP/1.1 connection multiplexes multiple requests
over the one bidirectional stream (HTTP/2 multiplexing is native;
HTTP/1.1 is sequential). The axum router handles each request on a
tokio task; the hyper driver manages the connection lifetime.

### HTTP-to-call dispatch (ADR-036)

An HTTP request at `POST /fs/readFile` (or `GET /services/list`, or any
`/{service}/{op}` path matching a registered `External` operation) is
dispatched to the call protocol:

1. The axum route handler extracts the operation name from the path
   (`/fs/readFile` → `fs/readFile`, stripping the leading slash — the
   registry form).
2. It resolves the caller's identity from the `Authorization: Bearer`
   header via `identity_provider.resolve_from_token(&AuthToken { raw:
   token_bytes })`.
3. It parses the request body as the operation input (JSON).
4. It constructs the root `OperationContext` (caller identity, the
   registration bundle's capabilities, the connection's env composition)
   and dispatches through the `OperationRegistry::invoke()` — the same
   dispatch path the `CallAdapter` uses for `alknet/call` wire requests.
5. The response (`ResponseEnvelope`) is serialized as the HTTP response
   body (JSON). Errors map to HTTP status codes (see Error Mapping
   below).

`Internal` operations (ADR-015) return `404` on the HTTP handler,
matching the call protocol's `NOT_FOUND` for wire calls to Internal
ops — the HTTP handler dispatches only `External` operations.

### Streaming projection (SSE)

A `Subscription` operation served over `h2`/`http/1.1` projects its
`call.responded` stream as Server-Sent Events. The axum route handler:

- Sets `Content-Type: text/event-stream`.
- For each `call.responded` event, writes an SSE `data:` frame (the
  event's `output` serialized as JSON).
- On `call.completed`, closes the SSE stream (normal end).
- On `call.aborted`, closes the stream with an SSE error event.
- On HTTP client disconnect (detected as the response writer closing),
  sends `call.aborted` for the in-flight subscription, which cascades
  to descendants per ADR-016.

This is the HTTP/1.1 + HTTP/2 streaming projection. Over WebTransport
(`h3`), the subscription projects directly onto a WebTransport
bidirectional stream — no SSE framing (see [webtransport.md](webtransport.md)).

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
crate. WebTransport (`h3`) restores the bidirectional call model: a
WebTransport session is a long-lived connection over which either side
can open bidirectional streams and send `call.requested` events in
either direction — the call protocol's native bidirectionality applies
unchanged. See [webtransport.md](webtransport.md) and ADR-043. The
HTTP/1.1 + HTTP/2 surface is the projection for clients that only speak
HTTP; WebTransport is the surface for clients that can speak the call
protocol in both directions.

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

For paths that are not registered operations (and not `/healthz`,
`/openapi.json`, the `to_openapi` gateway endpoints `/search`/`/schema`/
`/call`/`/batch`/`/subscribe`, or the MCP route), the HTTP handler serves
a decoy. The decoy is configurable (`DecoyConfig`):

- A fake `404 Not Found` (the default — matches the reference
  implementation's "fake nginx 404").
- A static site (served from a configured directory).
- A redirect (to a configured URL).

The decoy is the stealth surface: a port scanner or a client that
doesn't offer alknet ALPNs connects on `h2`/`http/1.1` and sees the
decoy. Real services use `alknet/ssh`, `alknet/call`, etc. The decoy
config is a two-way-door default (an operator picks what to serve); the
*existence* of the stealth path is fixed by ADR-010.

## Constraints

- **The HTTP path IS the operation path on the direct-call surface.**
  `POST /fs/readFile` → `call.requested` for `fs/readFile`. No second
  routing table for the direct-call surface. See ADR-036. The
  `to_openapi` gateway (`/search`, `/schema`, `/call`, `/batch`,
  `/subscribe`) is a separate fixed-endpoint surface (ADR-042) that
  coexists with the direct-call surface on the same axum `Router`; it
  does not replace it.
- **`External` operations only.** `Internal` operations return `404`
  on the HTTP handler.
- **Bearer-only auth.** `Authorization: Bearer` →
  `resolve_from_token`. Other HTTP auth schemes are not implemented.
- **No secret material in HTTP responses.** The call protocol carries no
  secret material (ADR-014); the HTTP handler inherits this constraint.
  Capabilities are used for outbound calls (`from_openapi`), never
  serialized into HTTP response bodies.
- **`/healthz` is raw.** No auth, no call protocol. The one raw route.
- **The `h3` ALPN is a first-class transport.** The `HttpAdapter`
  registers for `h3` when the `h3` feature is enabled (ADR-038). The
  `h3` handler is covered in [webtransport.md](webtransport.md); this
  document covers the `h2`/`http/1.1` path.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| Direct path mapping (HTTP path = operation path) | [ADR-036](../../decisions/036-http-to-call-operation-mapping.md) | `POST /{service}/{op}` → `call.requested` (direct-call surface) |
| `to_openapi` gateway endpoints on the router | [ADR-042](../../decisions/042-openapi-gateway-pattern.md) | `/search`/`/schema`/`/call`/`/batch`/`/subscribe` coexist with the direct-call surface |
| SSE projection for subscriptions over h2/http1.1 | [ADR-036](../../decisions/036-http-to-call-operation-mapping.md) | `call.responded` stream → SSE frames |
| `/healthz` is a raw route | [ADR-036](../../decisions/036-http-to-call-operation-mapping.md) | No auth, no call protocol |
| Stealth decoy | [ADR-010](../../decisions/010-alpn-router-and-endpoint.md) | HTTP handler on standard ALPNs serves decoy |
| Bearer auth via `resolve_from_token` | [ADR-004](../../decisions/004-auth-as-shared-core.md) | HTTP handler credential source (settled) |
| `h3` is first-class (not deferred) | [ADR-038](../../decisions/038-http3-and-webtransport-as-first-class.md) | The `h3` ALPN handler lives in this crate |
| Error mapping (call codes → HTTP status) | [ADR-023](../../decisions/023-operation-error-schemas.md) | Protocol/operation codes distinct; `HTTP_<status>` prefix for imported |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-39** (open): `to_openapi` published-spec versioning — the
  generated OpenAPI spec is a compatibility contract (ADR-017
  Consequences); the versioning strategy needs specifying.
- **OQ-40** (open): reqwest client config and connection pooling —
  two-way-door config shape for the outbound HTTP client used by
  `from_openapi`/`from_mcp`.

## References

- [ADR-036](../../decisions/036-http-to-call-operation-mapping.md) — the
  HTTP-to-call mapping this server implements
- [ADR-038](../../decisions/038-http3-and-webtransport-as-first-class.md)
  — the `h3`/WebTransport companion to this server
- [overview.md](overview.md) — crate overview, adapter location map
- [webtransport.md](webtransport.md) — the `h3` ALPN handler
- [http-adapters.md](http-adapters.md) — `from_openapi`/`to_openapi`
- [../core/auth.md](../core/auth.md) — `IdentityProvider`, Bearer →
  `resolve_from_token`
- [../core/endpoint.md](../core/endpoint.md) — stealth mode as ALPN
  dispatch
- [../call/operation-registry.md](../call/operation-registry.md) —
  `OperationRegistry::invoke()`, the dispatch path HTTP requests hit