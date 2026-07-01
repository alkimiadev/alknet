---
id: http/server/bearer-auth-middleware
name: Implement shared Bearer auth middleware (resolve_from_token, stash Identity in request extensions)
status: pending
depends_on: [http/server/http-adapter]
scope: narrow
risk: medium
impact: component
level: implementation
---

## Description

Implement the shared Bearer auth axum middleware in
`src/server/auth.rs`. This is the auth layer shared by the HTTP gateway
endpoints AND the `to_mcp` rmcp service (research §4.4: "the auth
middleware is shareable now"). One axum layer resolves the bearer token
and stashes `Option<Identity>` in request extensions; the `to_openapi`
route handlers read it from axum state/extractors, and the `to_mcp`
`call_tool` handler reads it from rmcp's `RequestContext.extensions`
(rmcp injects `http::request::Parts` into extensions — research §4.4,
`tower.rs:487-521, 1086-1097`).

### The middleware (http-server.md §"Auth")

Inbound HTTP auth is `Authorization: Bearer <token>`, resolved via
`IdentityProvider::resolve_from_token()` (the auth.md handler table:
`HttpAdapter`, Bearer header, `resolve_from_token`). Bearer-only is the
auth mechanism for the default surface; other HTTP auth schemes (Basic,
API key in query param) are not implemented and would be added as axum
middleware (two-way door).

```rust
/// Axum middleware that resolves the `Authorization: Bearer` header via
/// `IdentityProvider::resolve_from_token()` and stashes the resolved
/// `Option<Identity>` in request extensions. Shared by the HTTP gateway
/// endpoints and the to_mcp rmcp service (research §4.4).
pub async fn bearer_auth_middleware(
    State(identity_provider): State<Arc<dyn IdentityProvider>>,
    mut request: Request,
    next: Next,
) -> Response {
    let identity = extract_bearer_identity(&request, &identity_provider);
    request.extensions_mut().insert(identity);
    next.run(request).await
}

/// Extract the `Authorization: Bearer <token>` header and resolve it to
/// an `Option<Identity>`. Returns `None` if no token is present (the
/// request proceeds unauthenticated; the route handler / AccessControl
/// decides whether to reject). Returns `None` if the token is present
/// but resolution fails (treat as unauthenticated, not as an error —
/// matches the CallAdapter's per-request identity resolution behavior).
pub fn extract_bearer_identity(
    request: &Request,
    identity_provider: &dyn IdentityProvider,
) -> Option<Identity> {
    let header = request.headers().get(AUTHORIZATION)?;
    let token_str = header.to_str().ok()?.strip_prefix("Bearer ")?;
    let token = AuthToken { raw: token_str.as_bytes().to_vec() };
    identity_provider.resolve_from_token(&token)
}
```

### Auth resolution behavior

- An unauthenticated request to an operation with `AccessControl`
  restrictions returns `401` (no token) or `403` (token present but
  insufficient scopes). The call protocol's `FORBIDDEN` protocol code
  maps to `403`; `NOT_FOUND` (Internal op) maps to `404`. (The
  `error-mapping` task owns the status mapping; this task resolves the
  identity and stashes it.)
- The HTTP handler stores the resolved identity on the `Connection` for
  observability (`connection.set_identity(identity)`), same as the call
  protocol handler (OQ-11 resolved).
- Bearer-only is the auth mechanism. Basic auth, API keys in query
  params, and other HTTP auth schemes are not implemented. A deployment
  that needs a different auth scheme adds it as axum middleware
  (two-way door), but the default surface is Bearer-only.

### The `Identity` extractor

Provide an axum extractor so route handlers can declare `identity:
Option<Identity>` as a parameter and get the resolved identity from
extensions:

```rust
/// Axum extractor: the resolved bearer identity (or None if
/// unauthenticated). Read from request extensions (stashed by
/// `bearer_auth_middleware`).
#[derive(Clone, Debug)]
pub struct ResolvedIdentity(pub Option<Identity>);

#[async_trait]
impl FromRequestParts<AppState> for ResolvedIdentity {
    type Rejection = Infallible;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        Ok(ResolvedIdentity(parts.extensions.get::<Option<Identity>>().cloned().flatten_or(None)))
    }
}
```

### Shared with `to_mcp` (research §4.4)

The `to_mcp` rmcp service is nested into the axum router via
`Router::nest_service("/mcp", mcp_service)` (the `to-mcp` task). The
Bearer auth middleware is applied as an axum layer *around* the nested
service (the rmcp `simple_auth_streamhttp.rs` example shows the pattern:
`middleware::from_fn_with_state` around `Router::nest_service`). The
`to_mcp` `call_tool` handler reads the `Identity` from
`RequestContext<RoleServer>.extensions` (rmcp injects
`http::request::Parts` into extensions — `tower.rs:487-521, 1086-1097`).

A spike should confirm this extension-survives-the-rmcp-framing path
works end-to-end — it is the load-bearing assumption for sharing the auth
middleware (research §6 open question #2). The `Identity` stashed by the
axum middleware into `Parts.extensions` should be retrievable via
`ctx.extensions.get::<Identity>()` inside `call_tool`.

### What this task does NOT do

- **No AccessControl enforcement.** The middleware resolves identity;
  the route handlers / `GatewayDispatch::invoke()` enforce
  `AccessControl::check(identity)`. This task stashes the identity; it
  does not reject requests (except for malformed `Authorization` headers,
  which are treated as no-token, not as errors).
- **No error response mapping.** The `401`/`403`/`404` status mapping is
  the `error-mapping` task. This task resolves identity; the route
  handler produces the `CallError`, and the error-mapping task maps it.
- **No `to_mcp` service.** The rmcp service is the `to-mcp` task. This
  task provides the middleware that wraps it.

## Acceptance Criteria

- [ ] `bearer_auth_middleware` axum middleware in `src/server/auth.rs`
- [ ] Extracts `Authorization: Bearer <token>` header
- [ ] Resolves via `identity_provider.resolve_from_token(&AuthToken { raw })`
- [ ] Stashes `Option<Identity>` in request extensions
- [ ] No token present → `None` identity (request proceeds, route handler decides)
- [ ] Malformed `Authorization` header → `None` identity (not an error)
- [ ] Token present but resolution fails → `None` identity (treat as unauthenticated)
- [ ] `ResolvedIdentity` axum extractor reads from extensions
- [ ] Middleware is `pub` and re-exported from `lib.rs`
- [ ] Middleware applicable to both HTTP routes and nested rmcp service (research §4.4)
- [ ] `connection.set_identity(identity)` called for observability (OQ-11)
- [ ] No `std::env::var` reads (no-env-vars invariant)
- [ ] Unit test: request with valid Bearer token → `Some(identity)` in extensions
- [ ] Unit test: request with no `Authorization` header → `None` in extensions
- [ ] Unit test: request with malformed `Authorization` → `None` in extensions
- [ ] Unit test: request with `Basic` auth → `None` (Bearer-only, not an error)
- [ ] Unit test: `ResolvedIdentity` extractor retrieves stashed identity
- [ ] `cargo test -p alknet-http` succeeds
- [ ] `cargo clippy -p alknet-http --all-targets` succeeds with no warnings

## References

- docs/architecture/crates/http/http-server.md — Auth (§"Auth")
- docs/research/alknet-http-gateway-factoring/findings.md — §4.4 (auth-extraction convergence, shareable now)
- docs/architecture/crates/core/auth.md — IdentityProvider, resolve_from_token
- docs/architecture/decisions/004-auth-as-shared-core.md — ADR-004 (Bearer → resolve_from_token)
- /workspace/rust-sdk/examples/servers/src/simple_auth_streamhttp.rs — rmcp axum middleware pattern

## Notes

> The auth middleware is the second small shared piece (alongside the
> dispatch spine). It is shareable between the HTTP gateway routes and
> the to_mcp rmcp service because both use axum middleware — the rmcp
> service is nested via Router::nest_service, and the middleware is
> applied around it. The load-bearing assumption is that the Identity
> stashed in Parts.extensions survives the rmcp framing and is
> retrievable via ctx.extensions.get::<Identity>() inside call_tool
> (research §6 open question #2 — confirm with a spike). This task
> resolves identity and stashes it; it does not enforce AccessControl
> (that's the route handler / GatewayDispatch's job) or map errors
> (that's the error-mapping task).

## Summary

> To be filled on completion