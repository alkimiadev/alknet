---
id: http/server/healthz-decoy
name: Implement /healthz raw route and stealth decoy fallback (DecoyConfig)
status: completed
depends_on: [http/server/http-adapter]
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Implement the `/healthz` raw route and the stealth decoy fallback in
`src/server/healthz.rs` and `src/server/decoy.rs`. These are the two
non-gateway HTTP surfaces on the `HttpAdapter` router: the one raw
operational endpoint (`/healthz`) and the stealth fallback for unknown
paths (the decoy).

### `GET /healthz` (raw route, http-server.md §"/healthz (raw route)")

`GET /healthz` is a raw HTTP route outside the call protocol — no auth,
no operation registration, no `OperationContext`. It returns `200 OK`
with a plain-text body (e.g., `"ok"`) if the endpoint is healthy. This is
the infrastructure endpoint load balancers and orchestrators call; it
must work before identity is resolvable.

```rust
/// GET /healthz — raw health check. No auth, no call protocol.
/// Returns 200 OK with plain-text body "ok" if the endpoint is healthy.
async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, [("content-type", "text/plain"), "ok"])
}
```

Other operational endpoints (metrics, dashboard) are call-protocol
operations if built (`/metrics/list`, `/dashboard/view`), not raw HTTP
routes. `healthz` is the one exception (ADR-036).

### Stealth decoy (http-server.md §"Stealth decoy")

For paths that are not the gateway endpoints (`/search`, `/schema`,
`/call`, `/batch`, `/subscribe`), `/healthz`, `/openapi.json`, the MCP
route, the WS upgrade route, or a custom route per ADR-046, the HTTP
handler serves a decoy. The decoy is configurable (`DecoyConfig`):

- `NotFound` — A fake `404 Not Found` (the default — matches the
  reference implementation's "fake nginx 404").
- `StaticSite { root }` — Serve a static site from a configured
  directory. For deployments that want a real decoy website.
- `Redirect { to }` — Redirect to a configured URL.

The decoy is the stealth surface: a port scanner or a client that
doesn't offer alknet ALPNs connects on `h2`/`http/1.1` and sees the
decoy. Real services use `alknet/ssh`, `alknet/call`, etc. The decoy
config is a two-way-door default (an operator picks what to serve); the
*existence* of the stealth path is fixed by ADR-010.

Custom routes (ADR-046) take precedence over the decoy — a path matched
by a custom route is served by it, not the decoy; the decoy is the
fallback for paths matched by neither the default surface nor a custom
route.

### The fallback handler

```rust
/// Fallback handler for unknown paths (stealth decoy). Serves the
/// configured DecoyConfig: fake 404 (default), static site, or redirect.
async fn decoy_fallback(
    State(decoy): State<DecoyConfig>,
    request: Request,
) -> Response {
    match decoy {
        DecoyConfig::NotFound => fake_nginx_404(),
        DecoyConfig::StaticSite { root } => serve_static(root, request).await,
        DecoyConfig::Redirect { to } => redirect(to),
    }
}
```

The `NotFound` variant should match the reference implementation's
"fake nginx 404" — a realistic 404 page that looks like a generic web
server, not an alknet-specific error. The exact body is a two-way-door
implementation detail; the one-way constraint is that it does not leak
alknet's presence (no alknet headers, no alknet error format).

### Wiring into the router

The `healthz` route and the `decoy_fallback` are wired into the axum
`Router` by the `http-adapter` task. This task implements the handlers;
the `http-adapter` task's router construction calls them:

```rust
// In the http-adapter task's router construction:
let router = Router::new()
    .route("/healthz", get(healthz))           // this task
    .fallback(decoy_fallback)                  // this task
    // ... gateway endpoints, /openapi.json, MCP, WS upgrade ...
```

## Acceptance Criteria

- [ ] `GET /healthz` handler returns `200 OK` with plain-text body `"ok"`
- [ ] `/healthz` requires no auth (no Bearer token check)
- [ ] `/healthz` does not construct an `OperationContext` (raw route)
- [ ] `DecoyConfig::NotFound` serves a fake 404 (no alknet-specific headers/format)
- [ ] `DecoyConfig::StaticSite { root }` serves static files from `root`
- [ ] `DecoyConfig::Redirect { to }` returns an HTTP redirect to `to`
- [ ] `DecoyConfig::default()` returns `NotFound`
- [ ] Decoy fallback serves for paths not matched by any other route
- [ ] Custom routes (ADR-046) take precedence over decoy (decoy is fallback only)
- [ ] Gateway endpoints, `/healthz`, `/openapi.json`, MCP route, WS upgrade take precedence over decoy
- [ ] Decoy does not leak alknet presence (no alknet headers, no alknet error format)
- [ ] Unit test: `/healthz` returns 200 + "ok"
- [ ] Unit test: unknown path with `NotFound` decoy → 404
- [ ] Unit test: unknown path with `StaticSite` decoy → static file
- [ ] Unit test: unknown path with `Redirect` decoy → redirect
- [ ] Unit test: `/healthz` works with no `Authorization` header
- [ ] Integration test: custom route matched → custom handler (not decoy)
- [ ] Integration test: unknown path not matched by custom route → decoy
- [ ] `cargo test -p alknet-http` succeeds
- [ ] `cargo clippy -p alknet-http --all-targets` succeeds with no warnings

## References

- docs/architecture/crates/http/http-server.md — /healthz (§"/healthz (raw route)"), Stealth decoy (§"Stealth decoy")
- docs/architecture/decisions/010-alpn-router-and-endpoint.md — ADR-010 (stealth, decoy existence)
- docs/architecture/decisions/036-http-to-call-operation-mapping.md — ADR-036 (/healthz is the one raw route)
- docs/architecture/decisions/046-assembly-layer-custom-http-routes.md — ADR-046 (custom routes take precedence over decoy)

## Notes

> /healthz is the one raw route — no auth, no call protocol, no
> OperationContext. It must work before identity is resolvable (load
> balancers call it). The decoy is the stealth surface: a port scanner
> sees the decoy, not alknet. The decoy config is a two-way-door
> (operator picks NotFound/StaticSite/Redirect); the existence of the
> stealth path is fixed by ADR-010. The NotFound variant should look
> like a generic web server's 404, not an alknet error — no alknet
> headers, no alknet format. Custom routes take precedence over the
> decoy; the decoy is the fallback for paths matched by neither the
> default surface nor a custom route.

## Summary

> Implemented GET /healthz raw route (src/server/healthz.rs, 200 OK text/plain 'ok',
> no auth, no OperationContext) and stealth decoy fallback (src/server/decoy.rs:
> DecoyConfig NotFound=nginx 404 / StaticSite=serve files / Redirect). Wired real
> handlers into HttpAdapter router (adapter.rs) replacing placeholder 501s, using
> FromRef<RouterState> for DecoyConfig substate. 125 tests pass. Clippy clean.