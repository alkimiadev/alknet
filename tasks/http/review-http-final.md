---
id: http/review-http-final
name: Final review of alknet-http crate — all components, feature gates, pattern consistency
status: pending
depends_on: [http/review-http, http/review-websocket, http/review-mcp]
scope: broad
risk: low
impact: project
level: review
---

## Description

Final review of the entire `alknet-http` crate — all components together,
feature gate isolation, pattern consistency, and cross-cutting concerns.
This is the last quality checkpoint before the crate is considered
complete. It runs after the three phase reviews (server surface,
WebSocket, MCP) and verifies the crate as a whole.

### Review Checklist

1. **Crate structure**:
   - `crates/alknet-http/Cargo.toml` with all dependencies and feature gates
   - `src/lib.rs` with module declarations and re-exports
   - Module structure: `server/`, `gateway/`, `client/`, `adapters/`, `websocket/`
   - Root `Cargo.toml` `members` includes `crates/alknet-http`
   - Workspace path deps for `alknet-core`, `alknet-call`

2. **Feature gate isolation**:
   - `default = ["h2", "http1"]` — the HTTP surface (incl. WebSocket upgrade)
   - `mcp = ["dep:rmcp"]` — from_mcp/to_mcp (streamable HTTP only, ADR-037)
   - `h3`/WebTransport feature gate is ABSENT (deferred per ADR-044)
   - `cargo check -p alknet-http` (default features) succeeds
   - `cargo check -p alknet-http --features mcp` succeeds
   - `cargo check -p alknet-http --no-default-features` succeeds (if meaningful)
   - MCP code (from_mcp, to_mcp) not compiled without `mcp` feature
   - stdio transport (`transport-child-process`) is NOT a dependency

3. **Dependency correctness**:
   - `alknet-core` (ProtocolHandler, Connection, IdentityProvider, Capabilities)
   - `alknet-call` (OperationAdapter, AdapterError, OperationRegistry, HandlerRegistration, Dispatcher, CallConnection, EventEnvelope, ResponseEnvelope, CallError)
   - `axum` (HTTP server, WebSocket upgrade)
   - `reqwest` + `reqwest-middleware` + `reqwest-retry` (HTTP client)
   - `hyper` (HTTP/1.1 + HTTP/2 framing)
   - `rmcp` (MCP, feature-gated, streamable HTTP only)
   - No `wtransport` / `h3` dependency (deferred per ADR-044)
   - No env-var-based config (no-env-vars invariant, ADR-014)

4. **Cross-cutting conformance**:
   - **No-env-vars invariant** (ADR-014): no `std::env::var` in any handler
     (from_openapi, from_mcp forwarding handlers read context.capabilities)
   - **No secret material in responses** (ADR-014): Capabilities not serialized
     into HTTP response bodies or CallToolResult
   - **AccessControl is the sole gate** (ADR-029 §3): no remote_safe/trusted_peer
   - **Internal ops → NOT_FOUND** (ADR-015): don't leak existence from wire
   - **Error fidelity** (ADR-023): HTTP_<status> prefix for imported codes,
     no collision with protocol codes
   - **Browsers/MCP clients are not peers** (ADR-034 §4, ADR-044 §5):
     no PeerId, connection-local overlay

5. **Pattern consistency across the crate**:
   - `GatewayDispatch` is a concrete struct (not a trait) — shared by to_openapi, to_mcp
   - Auth middleware is shared (HTTP routes + to_mcp rmcp service)
   - `SharedHttpClient` is ArcSwap-wrapped (rebuild-and-swap, same pattern as ConfigIdentityProvider)
   - Error mapping is a free function (not a trait method)
   - `OperationAdapter` implementations (from_openapi, from_mcp) follow the same shape:
     parse/discover → construct HandlerRegistration bundles → return
   - `to_*` projections (to_openapi, to_mcp) are pure (consume registry, don't produce entries)
   - `from_*` adapters (from_openapi, from_mcp) are OperationAdapter impls (produce entries)

6. **ADR conformance (full list)**:
   - ADR-003 Am. 1: alknet-http depends on alknet-call (protocol-foundation)
   - ADR-004: Bearer auth via resolve_from_token
   - ADR-010: stealth mode (HTTP handler on standard ALPNs, decoy)
   - ADR-012: stream-agnostic correlation (Dispatcher over WS)
   - ADR-014: no-env-vars, no secret material in responses
   - ADR-015: Internal ops → 404 from wire
   - ADR-016: abort cascade on disconnect
   - ADR-017: OperationAdapter trait, to_* are projections
   - ADR-022: leaf provenance (from_openapi, from_mcp)
   - ADR-023: error fidelity, HTTP_<status> prefix
   - ADR-024: Layer 2 connection-local overlay (WS browser ops)
   - ADR-029 §3: AccessControl is sole gate (no remote_safe)
   - ADR-032: forwarded_for: None for wire-ingress
   - ADR-034 §4: browsers/MCP clients are not peers
   - ADR-036: /healthz raw route, stealth, error mapping (non-routing clauses survive)
   - ADR-037: MCP streamable HTTP only (no stdio)
   - ADR-039: one crate for server + client host
   - ADR-041: to_mcp 4-tool gateway
   - ADR-042: to_openapi 5-endpoint gateway
   - ADR-044: h3 deferred, WS is v1 browser path, no length prefix
   - ADR-045: to_openapi info.version semver
   - ADR-046: extra_routes custom routes
   - ADR-047: gateway is sole invoke path (no direct-call surface)
   - ADR-048: WS carries native session, not gateway shape

7. **What's NOT in the crate** (verify absence):
   - No `h3`/WebTransport handler (deferred per ADR-044)
   - No `from_wss` adapter (out of scope, websocket.md §"Future")
   - No stdio MCP transport (ADR-037)
   - No per-operation `POST /{service}/{op}` direct-call surface (ADR-047)
   - No traditional per-operation-paths OpenAPI projection (additive, ADR-042 §5, out of scope)
   - No env-var-based client config (ADR-014)

8. **Test coverage (full crate)**:
   - All unit tests pass
   - All integration tests pass
   - `cargo test -p alknet-http` (default features) succeeds
   - `cargo test -p alknet-http --features mcp` succeeds
   - `cargo test -p alknet-call` succeeds (no regressions from transport abstraction)

9. **Build cleanliness**:
   - `cargo fmt --check -p alknet-http` passes
   - `cargo clippy -p alknet-http` passes with no warnings
   - `cargo clippy -p alknet-http --features mcp --all-targets` passes with no warnings
   - `cargo build -p alknet-http` succeeds
   - `cargo build -p alknet-http --features mcp` succeeds

## Acceptance Criteria

- [ ] Crate structure complete (Cargo.toml, lib.rs, all modules)
- [ ] Feature gates correct (default h2+http1, mcp optional, h3 absent)
- [ ] Feature gate isolation verified (MCP code not compiled without mcp)
- [ ] All dependencies correct (alknet-core, alknet-call, axum, reqwest, hyper, rmcp)
- [ ] No wtransport/h3 dependency (deferred per ADR-044)
- [ ] No env-var-based config (ADR-014)
- [ ] No-env-vars invariant holds across all handlers
- [ ] No secret material in responses (ADR-014)
- [ ] AccessControl is the sole gate (ADR-029 §3)
- [ ] Internal ops → NOT_FOUND from wire (ADR-015)
- [ ] Error fidelity (ADR-023 — HTTP_<status> prefix, no collision)
- [ ] Browsers/MCP clients are not peers (ADR-034 §4, ADR-044 §5)
- [ ] GatewayDispatch is a concrete struct (not a trait)
- [ ] Auth middleware shared (HTTP routes + to_mcp)
- [ ] SharedHttpClient is ArcSwap-wrapped
- [ ] All ADRs conformed to (003-048, full list in checklist)
- [ ] No h3/WebTransport handler (deferred)
- [ ] No from_wss adapter (out of scope)
- [ ] No stdio MCP transport (ADR-037)
- [ ] No direct-call surface (ADR-047)
- [ ] No traditional per-operation-paths OpenAPI (out of scope)
- [ ] `cargo fmt --check -p alknet-http` passes
- [ ] `cargo clippy -p alknet-http` passes with no warnings
- [ ] `cargo clippy -p alknet-http --features mcp --all-targets` passes with no warnings
- [ ] `cargo test -p alknet-http` passes
- [ ] `cargo test -p alknet-http --features mcp` passes
- [ ] `cargo test -p alknet-call` passes (no regressions)

## References

- docs/architecture/crates/http/README.md — crate index
- docs/architecture/crates/http/overview.md — overview, dependencies, feature gates
- docs/architecture/crates/http/http-server.md — HttpAdapter
- docs/architecture/crates/http/http-adapters.md — from_openapi, to_openapi
- docs/architecture/crates/http/http-mcp.md — from_mcp, to_mcp
- docs/architecture/crates/http/websocket.md — WebSocket path
- docs/architecture/crates/http/webtransport.md — deferred (verify absence)
- docs/research/alknet-http-gateway-factoring/findings.md — gateway factoring
- docs/architecture/decisions/ (all relevant ADRs: 003-048)

## Notes

> This is the final quality checkpoint. It runs after the three phase
> reviews (review-http, review-websocket, review-mcp) and verifies the
> crate as a whole. The key cross-cutting concerns: (1) the no-env-vars
> invariant holds across ALL handlers (from_openapi, from_mcp), (2) the
> GatewayDispatch shared spine is a concrete struct used by both
> to_openapi and to_mcp, (3) the auth middleware is shared between HTTP
> routes and the to_mcp rmcp service, (4) feature gate isolation is
> correct (MCP code not compiled without mcp, h3 absent), (5) no
> regressions in alknet-call from the transport abstraction. Verify the
> absence of the deferred/out-of-scope items (h3, from_wss, stdio,
> direct-call surface, traditional per-operation-paths OpenAPI). If
> deviations are found, document and fix before considering the crate
> complete.

## Summary

> To be filled on completion