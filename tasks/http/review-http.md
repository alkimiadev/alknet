---
id: http/review-http
name: Review alknet-http server surface + OpenAPI adapters for spec conformance
status: completed
depends_on: [http/server/gateway-endpoints, http/adapters/to-openapi, http/adapters/from-openapi, http/server/healthz-decoy]
scope: broad
risk: low
impact: phase
level: review
---

## Description

Review the alknet-http server surface and OpenAPI adapters for spec
conformance, pattern consistency, and correctness. This is the quality
checkpoint for the HTTP server + gateway + OpenAPI adapter work — the
core of the crate.

### Review Checklist

1. **HttpAdapter conformance** (http-server.md):
   - `HttpAdapter` struct with `identity_provider`, `registry`, `decoy`, `extra_routes`
   - `DecoyConfig` enum with `NotFound` (default), `StaticSite`, `Redirect`
   - `ProtocolHandler::alpn()` returns `http/1.1` or `h2`
   - `handle()` branches on `connection.remote_alpn()` for HTTP framing
   - axum over QUIC bidirectional stream (`accept_bi` → hyper `TokioIo` → axum router)
   - Router built once at construction, cloned per connection (Arc clone)
   - `h3` ALPN not registered (deferred per ADR-044)
   - Custom routes (`extra_routes`) merged via `Router::merge` (ADR-046)
   - Default surface reserved paths take precedence on collision

2. **Gateway endpoints conformance** (http-server.md, http-adapters.md):
   - 5 fixed gateway endpoints (`/search`, `/schema`, `/call`, `/batch`, `/subscribe`)
   - No per-operation `POST /{service}/{op}` direct-call surface (ADR-047)
   - `/call` body is flat JSON `{ operation, input }`
   - `/call` dispatches via `GatewayDispatch::invoke` (shared spine)
   - `Internal` ops → 404 on `/call` (ADR-015)
   - `External` op + unauthorized → 403; + no identity → 401
   - `/search` dispatches `services/list` (AccessControl-filtered)
   - `/schema` dispatches `services/schema`
   - `/batch` is a loop over `invoke` (array of results in order)
   - `/subscribe` is SSE (`text/event-stream`, `call.responded` → `data:` frames)
   - `/subscribe` disconnect → `call.aborted` cascade (ADR-016)

3. **Error mapping conformance** (http-server.md, ADR-023):
   - `NOT_FOUND` → 404, `FORBIDDEN` → 401/403, `INVALID_INPUT` → 422, `TIMEOUT` → 504, `INTERNAL` → 500
   - Operation-level code with `http_status` → declared status
   - Operation-level code without `http_status` → 500
   - `HTTP_<status>` prefix for imported codes (no collision with protocol codes)
   - `retryable` → `Retry-After` hint for 503/429-class

4. **Auth conformance** (http-server.md, ADR-004):
   - Bearer-only (`Authorization: Bearer` → `resolve_from_token`)
   - Shared middleware stashes `Option<Identity>` in request extensions
   - `ResolvedIdentity` extractor reads from extensions
   - `connection.set_identity(identity)` for observability (OQ-11)
   - No `std::env::var` reads (no-env-vars invariant)

5. **`/healthz` and decoy conformance** (http-server.md):
   - `/healthz` is raw (no auth, no call protocol, no OperationContext)
   - `/healthz` returns 200 + "ok"
   - Decoy fallback for unknown paths
   - `DecoyConfig::NotFound` default (fake nginx 404, no alknet leak)
   - Custom routes take precedence over decoy

6. **`to_openapi` conformance** (http-adapters.md, ADR-042/045):
   - 5 fixed gateway endpoints in the doc (not per-operation paths)
   - `info.version` semver tracks gateway contract (initial 1.0.0)
   - Per-caller operation surface NOT preloaded (discovered via `/search`)
   - `/call` responses include protocol-level + operation-level errors
   - `HTTP_<status>`-prefixed codes projected correctly
   - Pure projection (consumes registry, does not produce entries)

7. **`from_openapi` conformance** (http-adapters.md, ADR-017/023):
   - `OperationAdapter` impl (`async fn import`)
   - Parse OpenAPI doc (`$ref` resolution, buildInputSchema/buildOutputSchema)
   - `operationId` (or generated name) → `spec.name`
   - `op_type` detected from method + response content type
   - `visibility` = `Internal` (ADR-015)
   - `provenance` = `FromOpenAPI`, leaf (ADR-022)
   - Error codes prefixed `HTTP_<status>` (ADR-023)
   - Forwarding handler: reqwest via `SharedHttpClient`
   - No-env-vars: reads `context.capabilities`, never `std::env::var` (ADR-014)
   - SSE parsing for `Subscription` forwarding
   - `HttpAuthScheme` (Bearer, ApiKey, Basic)

8. **Shared HTTP client conformance** (http-adapters.md, OQ-40):
   - `ClientWithMiddleware` (not bare `reqwest::Client`)
   - `RetryTransientMiddleware` + inlined `RetryAfterMiddleware`
   - Bounded `RetryAfterMiddleware` storage (no unbounded growth)
   - ArcSwap hot-reload (rebuild-and-swap)
   - Per-request credential injection (not at construction)
   - No env-var-based client config

9. **GatewayDispatch conformance** (research §5.1):
   - Concrete struct (not a trait)
   - `resolve_bearer` + `invoke` → `ResponseEnvelope`
   - Root `OperationContext`: `internal: false`, `forwarded_for: None`, fresh `request_id`
   - `handler_identity` from registration bundle
   - No `into_wire()` method (per-gateway mapping stays out)
   - No streaming abstraction (per-gateway)

10. **Security constraints**:
    - No secret material in HTTP response bodies (ADR-014)
    - Capabilities not serialized into responses
    - No-env-vars invariant (from_openapi reads context.capabilities)
    - Internal ops → 404 (don't leak existence)
    - AccessControl is the sole authorization gate

11. **Pattern consistency**:
    - GatewayDispatch is a struct, not a trait (research recommendation)
    - Auth middleware shared between HTTP routes and to_mcp (research §4.4)
    - Error mapping is a free function (not a trait method)
    - SharedHttpClient is ArcSwap-wrapped (same pattern as ConfigIdentityProvider)

12. **Test coverage**:
    - Unit tests for error mapping (all codes, 401/403 split)
    - Unit tests for auth middleware (valid/absent/malformed Bearer)
    - Unit tests for GatewayDispatch (invoke, services/list filtering)
    - Unit tests for to_openapi (5 paths, info.version, error projection)
    - Unit tests for from_openapi (parse, operationId, op_type, error codes)
    - Integration tests for gateway endpoints (call, search, schema, batch, subscribe)
    - Integration tests for from_openapi forwarding (no-env-vars, SSE)

## Acceptance Criteria

- [ ] HttpAdapter matches http-server.md (struct, DecoyConfig, ProtocolHandler, axum over QUIC)
- [ ] Gateway endpoints match http-server.md (5 endpoints, no direct-call surface, ADR-047)
- [ ] Error mapping matches ADR-023 (all codes, HTTP_<status> prefix, 401/403 split)
- [ ] Auth matches ADR-004 (Bearer-only, shared middleware, set_identity)
- [ ] /healthz is raw; decoy fallback works; custom routes take precedence
- [ ] to_openapi matches ADR-042/045 (5 endpoints, info.version, per-caller via /search)
- [ ] from_openapi matches http-adapters.md (OperationAdapter, no-env-vars, HTTP_<status>)
- [ ] SharedHttpClient matches OQ-40 (ClientWithMiddleware, retry, RetryAfter, ArcSwap)
- [ ] GatewayDispatch is a concrete struct (not a trait), shared spine correct
- [ ] No secret material in HTTP responses (ADR-014)
- [ ] No-env-vars invariant verified (no std::env::var in from_openapi)
- [ ] Internal ops → 404 (don't leak existence)
- [ ] AccessControl is the sole authorization gate
- [ ] Test coverage adequate for all functionality
- [ ] `cargo fmt --check -p alknet-http` passes
- [ ] `cargo clippy -p alknet-http` passes with no warnings
- [ ] All tests pass

## References

- docs/architecture/crates/http/README.md
- docs/architecture/crates/http/overview.md
- docs/architecture/crates/http/http-server.md
- docs/architecture/crates/http/http-adapters.md
- docs/research/alknet-http-gateway-factoring/findings.md
- docs/architecture/decisions/ (relevant ADRs: 004, 010, 014, 015, 017, 022, 023, 036, 039, 042, 045, 046, 047)

## Notes

> This is the quality checkpoint for the HTTP server + gateway + OpenAPI
> adapter work — the core of the crate. The review should verify that
> the gateway is the sole invoke path (ADR-047), the error mapping is
> faithful (ADR-023), the no-env-vars invariant holds (ADR-014), and the
> GatewayDispatch shared spine is a concrete struct (not a trait, per
> the research recommendation). If deviations are found, document and
> fix before considering the server surface complete.

## Summary

> HTTP server surface + OpenAPI adapters reviewed against all 12 checklist items. All
> conformance criteria pass: HttpAdapter (struct, DecoyConfig, ProtocolHandler, axum
> over QUIC, h3 not registered), gateway endpoints (5 fixed, no direct-call ADR-047,
> flat JSON /call, GatewayDispatch::invoke, Internal → 404, 401/403 split, SSE /subscribe),
> error mapping (all codes, HTTP_<status> prefix, 401/403 split, Retry-After), auth
> (Bearer-only, shared middleware, ResolvedIdentity, no env vars), /healthz raw + decoy
> fallback, to_openapi (5 endpoints, info.version 1.0.0, per-caller via /search, error
> fidelity ADR-023), from_openapi (OperationAdapter, no-env-vars, HTTP_<status>, SSE),
> SharedHttpClient (ClientWithMiddleware, retry, RetryAfter, ArcSwap), GatewayDispatch
> (concrete struct, not trait), security constraints (no secrets in responses, no env
> vars, Internal → 404, AccessControl sole gate). 230 default + 265 mcp tests pass.
> Clippy clean on both feature configs. Fmt clean.