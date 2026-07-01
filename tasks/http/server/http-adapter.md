---
id: http/server/http-adapter
name: Implement HttpAdapter (ProtocolHandler for h2/http1.1) — axum over QUIC stream, ALPN branching, custom routes
status: completed
depends_on: [http/crate-init, http/gateway/gateway-dispatch-spine]
scope: broad
risk: high
impact: component
level: implementation
---

## Description

Implement `HttpAdapter` in `src/server/adapter.rs`. This is the
`ProtocolHandler` implementation for the standard HTTP ALPNs (`h2`,
`http/1.1`) — the highest-risk task in the http crate. It ties together
the axum-over-QUIC-stream integration, ALPN branching, the router
construction (gateway endpoints + `/healthz` + `/openapi.json` + MCP +
custom routes + decoy), and the `extra_routes: Option<Router>` extension
point (ADR-046).

### The struct (http-server.md §"What")

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
    /// are not described by to_openapi.
    extra_routes: Option<Router>,
}

pub enum DecoyConfig {
    /// Serve a fake `404 Not Found` (the default — "fake nginx 404").
    NotFound,
    /// Serve a static site from a configured directory.
    StaticSite { root: PathBuf },
    /// Redirect to a configured URL.
    Redirect { to: String },
}
```

### ProtocolHandler impl

```rust
#[async_trait]
impl ProtocolHandler for HttpAdapter {
    fn alpn(&self) -> &'static [u8];   // returns the configured ALPN
    async fn handle(&self, connection: Connection, auth: &AuthContext) -> Result<(), HandlerError>;
}
```

The `HttpAdapter` registers for multiple ALPNs (`http/1.1`, `h2`). The
endpoint's `HandlerRegistry` maps each ALPN byte string to the same
adapter instance; `handle()` branches on `connection.remote_alpn()` to
pick the HTTP framing. For `http/1.1` and `h2`, the framing is hyper's
HTTP/1.1 or HTTP/2 over a QUIC bidirectional stream. WebSocket upgrade
layers on top of the same hyper connection driver — a WS upgrade is an
HTTP/1.1 or HTTP/2 request that switches protocols (the WS handler is
the `websocket/upgrade-handler` task; this task hosts the route).

### Running axum over a QUIC stream (http-server.md §"Running axum over a QUIC stream")

The `HttpAdapter::handle()` method for `h2`/`http/1.1`:

1. Accepts one bidirectional stream from the QUIC connection
   (`connection.accept_bi()` → `(SendStream, RecvStream)`).
2. Wraps the `(SendStream, RecvStream)` pair as a hyper
   `TokioIo`-compatible duplex stream — the same byte stream hyper
   expects for an HTTP connection.
3. Constructs the axum `Router` (built once at adapter construction,
   cloned per connection — axum `Router` is `Clone` and cheap to clone).
4. Hands the duplex stream + the axum router to hyper's connection driver
   (`hyper::server::conn::http1::Builder` or
   `http2::Builder::serve_connection`), which reads HTTP frames, parses
   them, dispatches to axum routes, and writes HTTP responses.
5. Returns when the HTTP connection closes (the client disconnects or
   the stream ends).

The axum `Router` is built once at adapter construction with the
`Arc<OperationRegistry>` and `Arc<dyn IdentityProvider>` embedded in its
state; cloning the `Router` per connection clones the `Arc`s (cheap,
shared state), so every request handler has access to the registry and
identity provider through the router's state.

### The router surface (http-server.md §"Architecture")

The axum `Router` is the single routing surface for HTTP requests. It
contains:

- **The `to_openapi` gateway endpoints** (`/search`, `/schema`, `/call`,
  `/batch`, `/subscribe` — ADR-042). These 5 fixed endpoints are the sole
  invoke path over HTTP. (Route handlers are the `gateway-endpoints`
  task; this task wires the router.)
- `GET /healthz` (raw route, no auth, no call protocol). (The
  `healthz-decoy` task; this task wires the route.)
- `GET /openapi.json` (serves the `to_openapi` projection). (The
  `to-openapi` task; this task wires the route.)
- The stealth decoy fallback (unknown paths). (The `healthz-decoy`
  task; this task wires the fallback.)
- (Feature-gated) `POST /mcp` (the `to_mcp` streamable HTTP service). (The
  `to-mcp` task; this task wires the route behind the `mcp` feature gate.)
- **Deployment-specific custom routes** (ADR-046). The assembly layer
  may inject an `axum::Router` of extra routes at `HttpAdapter`
  construction. (This task implements the `extra_routes` merge.)

### Custom routes (ADR-046)

Custom routes:

- Are **raw HTTP**, not call-protocol operations — not registered in the
  `OperationRegistry`, not discoverable via `/search`, not in the
  `to_openapi` gateway doc.
- **May** dispatch into the registry via
  `OperationRegistry::invoke()` with a proper `OperationContext`
  (caller identity from the resolved bearer token) — the OAI proxy
  pattern. Or they may be pure HTTP (a webhook receiver, a static asset
  server) that never touches the registry.
- Run under the **default Bearer-auth middleware**; a route that wants
  different auth applies its own axum middleware (the deployment owns
  its custom routes' middleware stack).
- **Do not collide** with the reserved default-surface paths
  (`/search`, `/schema`, `/call`, `/batch`, `/subscribe`, `/healthz`,
  `/openapi.json`, the MCP route) — the default surface wins on
  collision; custom routes namespace away naturally (`/v1/...`).
- Are **not versioned** by `to_openapi` (ADR-045 versions the gateway
  contract, not custom routes).
- Are **immutable after construction** (matches OQ-04 / ADR-010's
  static-registration constraint; the `HttpAdapter` router is built once
  at startup).

The extension point is additive: a deployment that passes `None` gets
exactly the default surface. The mechanism (the constructor parameter)
is the one-way door — once downstream deployments build against it, it's
a contract (ADR-046). The specific routes a deployment adds are a
two-way door (add/remove freely).

### ALPN branching

The `HttpAdapter` registers for `http/1.1` and `h2`. The endpoint's
`HandlerRegistry` maps each ALPN to the same `HttpAdapter` instance; the
handler branches on `connection.remote_alpn()` to pick the right
framing. The `h3` ALPN is not registered in v1 (deferred per ADR-044).

### What this task does NOT do

- **No gateway route handlers.** The 5 gateway endpoints' handler logic
  is the `gateway-endpoints` task. This task wires the routes into the
  router and provides the router state.
- **No `/healthz` or decoy logic.** The `healthz-decoy` task implements
  the healthz handler and the decoy fallback. This task wires the
  routes.
- **No `/openapi.json` generation.** The `to-openapi` task implements
  the OpenAPI doc generation. This task wires the route.
- **No MCP route.** The `to-mcp` task implements the rmcp service. This
  task wires the route behind the `mcp` feature gate.
- **No WebSocket upgrade handler.** The `websocket/upgrade-handler` task
  implements the WS upgrade. This task hosts the WS upgrade route on the
  router (the WS task depends on this task's router).

## Acceptance Criteria

- [ ] `HttpAdapter` struct with `identity_provider`, `registry`, `decoy`, `extra_routes`
- [ ] `DecoyConfig` enum with `NotFound`, `StaticSite { root }`, `Redirect { to }`
- [ ] `HttpAdapter::new(identity_provider, registry)` constructor
- [ ] `HttpAdapter::with_decoy(self, decoy)` builder
- [ ] `HttpAdapter::with_extra_routes(self, routes: Router)` builder (ADR-046)
- [ ] `ProtocolHandler::alpn()` returns the configured ALPN (`http/1.1` or `h2`)
- [ ] `handle()` branches on `connection.remote_alpn()` for HTTP framing
- [ ] `handle()` accepts a QUIC bidirectional stream via `connection.accept_bi()`
- [ ] `handle()` wraps the stream as a hyper `TokioIo`-compatible duplex
- [ ] `handle()` drives hyper's `http1::Builder` or `http2::Builder::serve_connection`
- [ ] axum `Router` built once at construction, cloned per connection
- [ ] Router state holds `Arc<OperationRegistry>` + `Arc<dyn IdentityProvider>`
- [ ] Custom routes (`extra_routes`) merged via `Router::merge` (ADR-046)
- [ ] Default surface reserved paths take precedence on collision with custom routes
- [ ] `h3` ALPN is not registered (deferred per ADR-044)
- [ ] `handle()` returns when the HTTP connection closes
- [ ] Unit test: `alpn()` returns `http/1.1` or `h2` per config
- [ ] Unit test: `DecoyConfig::default()` is `NotFound`
- [ ] Unit test: `with_extra_routes` merges routes without collision on reserved paths
- [ ] Integration test: `handle()` serves an HTTP request over a mock QUIC stream
- [ ] Integration test: custom route (`/v1/foo`) coexists with default surface
- [ ] Integration test: reserved path (`/healthz`) wins over a custom route collision
- [ ] `cargo test -p alknet-http` succeeds
- [ ] `cargo clippy -p alknet-http --all-targets` succeeds with no warnings

## References

- docs/architecture/crates/http/http-server.md — HttpAdapter, axum over QUIC, custom routes
- docs/architecture/decisions/010-alpn-router-and-endpoint.md — ADR-010 (stealth, ALPN dispatch)
- docs/architecture/decisions/046-assembly-layer-custom-http-routes.md — ADR-046 (extra_routes)
- docs/architecture/decisions/044-defer-webtransport-browsers-use-websocket.md — ADR-044 (h3 deferred)
- docs/architecture/decisions/039-http-server-and-client-host-colocated.md — ADR-039 (one crate)

## Notes

> This is the highest-risk task in the http crate — the axum-over-QUIC
> integration is the merge point. The router is built once at
> construction with the registry and identity provider in its state;
> cloning per connection is cheap (Arc clone). The extra_routes
> extension point (ADR-046) is additive: None = default surface only;
> Some(routes) = default + custom, with default winning on collision.
> The h3 ALPN is not registered (deferred per ADR-044). This task wires
> the router and provides the QUIC-to-hyper bridge; the gateway
> endpoints, healthz, decoy, openapi.json, MCP, and WS upgrade route are
> wired by their respective tasks (which depend on this task's router).

## Summary

> Implemented HttpAdapter (ProtocolHandler for h2/http1.1) in src/server/adapter.rs.
> Axum-over-QUIC bridge via hyper-util auto Builder. DecoyConfig enum
> (NotFound/StaticSite/Redirect). with_extra_routes merge (ADR-046). Router state
> holds Arc<OperationRegistry> + Arc<dyn IdentityProvider>. Placeholder 501 handlers
> for gateway endpoints/healthz/openapi.json/MCP (later tasks fill in). h3 ALPN not
> registered (ADR-044). 78 tests pass. Clippy clean on default + no-default-features.