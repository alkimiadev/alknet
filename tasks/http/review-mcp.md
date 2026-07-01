---
id: http/review-mcp
name: Review MCP adapters for ADR-037/041 conformance (streamable HTTP, 4-tool gateway, output handling)
status: pending
depends_on: [http/adapters/from-mcp, http/adapters/to-mcp]
scope: moderate
risk: low
impact: phase
level: review
---

## Description

Review the MCP adapters (`from_mcp`, `to_mcp`) for spec conformance,
pattern consistency, and correctness. This is the quality checkpoint
for the feature-gated MCP work.

### Review Checklist

1. **`from_mcp` conformance** (http-mcp.md):
   - `FromMCP` struct with `endpoint`, `auth_token`, `namespace`
   - `OperationAdapter` impl (`async fn import`)
   - Connects via rmcp `StreamableHttpClientTransport::from_uri(endpoint)`
   - Connection failure → `AdapterError::DiscoveryFailed`
   - 401 → `AdapterError::Unauthorized`
   - Calls `tools/list` → MCP tools
   - For each tool: `HandlerRegistration` with `FromMCP` provenance
   - `spec.name` = tool name (or `namespace/tool_name`)
   - `spec.op_type` = `Mutation` (MCP tools are call/response)
   - `spec.visibility` = `Internal` (ADR-015)
   - `spec.input_schema` = tool's `inputSchema`
   - `spec.output_schema` = declared `outputSchema` if present, else `ContentBlock` union
   - `provenance` = `FromMCP`, `composition_authority: None`, `scoped_env: None` (ADR-022)
   - `capabilities` = bearer token (injected at registration)

2. **`from_mcp` output handling** (http-mcp.md §"Output handling"):
   - `structured_content` present → use as result (validated against `output_schema`)
   - `structured_content` absent → map `content: Vec<ContentBlock>` to `ContentBlock` union
   - No heuristic `JSON.parse` of text blocks (carry as `ContentBlock`)
   - `isError: true` → `CallError` (not the output handling path)
   - rmcp client connection maintained for registration lifetime

3. **`from_mcp` no-env-vars** (ADR-014):
   - Handler reads `context.capabilities`, never `std::env::var`
   - Bearer token from `Capabilities`, not env vars

4. **`to_mcp` conformance** (http-mcp.md, ADR-041):
   - Implements rmcp `ServerHandler` trait (`call_tool`, `list_tools`)
   - `tools/list` returns 4 fixed gateway tools (`search`, `schema`, `call`, `batch`)
   - `tools/list` does NOT return registry's operations (discovered via `search`)
   - `search` → dispatches `services/list` via `GatewayDispatch::invoke`
   - `search` results are `AccessControl::check(identity)`-filtered
   - `Subscription` ops filtered out of `search` results (ADR-041 §2)
   - `schema` → dispatches `services/schema` via `GatewayDispatch::invoke`
   - `call` → dispatches via `GatewayDispatch::invoke` (shared spine)
   - `call` result → `CallToolResult::structured(value)` for `Ok`
   - `call` error → `CallToolResult::structured_error(details)` for `Err(CallError)`
   - `batch` → loop over `invoke`, returns array
   - `to_mcp` is a pure projection (consumes registry, does not produce entries)

5. **`to_mcp` auth** (http-mcp.md, research §4.4):
   - Bearer auth via shared `bearer_auth_middleware` (applied around `nest_service`)
   - `Identity` read from `RequestContext.extensions` inside `call_tool`
   - Identity survives rmcp framing (research §6 #2 — confirmed by spike)
   - MCP client has no `PeerId` (not an alknet peer, ADR-034 §4)
   - `AccessControl` gates `search` results and `call` dispatch

6. **`to_mcp` rmcp integration** (http-mcp.md, research §4):
   - `StreamableHttpService` nested into axum `Router` at `/mcp`
   - `ServerHandler` impl is a gateway service (4 fixed tools)
   - rmcp owns JSON-RPC framing, session management, SSE priming
   - `to_mcp` owns `call_tool`/`list_tools` dispatch + `CallToolResult` mapping

7. **Streamable HTTP only** (ADR-037):
   - stdio transport NOT built (not a dependency, not optional, not behind a feature)
   - `mcp` feature pulls in rmcp with streamable HTTP transport features only
   - `transport-child-process` is not a dependency

8. **Feature gate isolation**:
   - `from_mcp`/`to_mcp` compile only with `mcp` feature
   - `cargo check -p alknet-http` (no `mcp`) succeeds — MCP code not compiled
   - `cargo check -p alknet-http --features mcp` succeeds

9. **Shared dispatch spine** (research §5.1):
   - `to_mcp` `call` uses `GatewayDispatch::invoke` (same spine as `to_openapi`)
   - `ResponseEnvelope` → `CallToolResult` mapping is `to_mcp`-specific
   - No `GatewayDispatch` trait (concrete struct)
   - Identity resolution, context construction, invoke are shared (security axis)
   - Wire framing, discovery, streaming are per-gateway (presentation axis)

10. **Error fidelity** (ADR-023):
    - `from_mcp` error_schemas from MCP tool error descriptions
    - `to_mcp` `call` error → `CallToolResult::structured_error` with typed `details`
    - No collision with protocol-level codes

11. **Security constraints**:
    - No-env-vars invariant (from_mcp reads context.capabilities, ADR-014)
    - MCP clients are not peers (no PeerId, ADR-034 §4)
    - AccessControl gates search results and call dispatch
    - No secret material in CallToolResult

12. **Test coverage**:
    - Unit tests for from_mcp (import, outputSchema present/absent, isError)
    - Unit tests for to_mcp (tools/list returns 4, search/schema/call/batch)
    - Integration test: MCP client → search → schema → call round-trip
    - Integration test: Bearer auth gates to_mcp service
    - Integration test: no-env-vars (no std::env::var in from_mcp)
    - Feature gate: cargo check without mcp succeeds

## Acceptance Criteria

- [ ] from_mcp matches http-mcp.md (OperationAdapter, tools/list, outputSchema handling)
- [ ] from_mcp output handling: structuredContent-preferred, ContentBlock fallback, no JSON.parse
- [ ] from_mcp no-env-vars: reads context.capabilities, never std::env::var
- [ ] to_mcp matches ADR-041 (4-tool gateway, not one tool per operation)
- [ ] to_mcp Subscription excluded from search results
- [ ] to_mcp uses GatewayDispatch::invoke (shared spine)
- [ ] to_mcp CallToolResult mapping correct (structured/structured_error)
- [ ] to_mcp auth: shared middleware, Identity survives rmcp framing
- [ ] Streamable HTTP only (ADR-037 — stdio NOT built)
- [ ] Feature gate isolation (mcp code not compiled without mcp feature)
- [ ] No GatewayDispatch trait (concrete struct, research recommendation)
- [ ] Error fidelity (ADR-023 — no collision with protocol codes)
- [ ] MCP clients are not peers (no PeerId, ADR-034 §4)
- [ ] AccessControl gates search and call
- [ ] No secret material in CallToolResult
- [ ] Test coverage adequate for all MCP functionality
- [ ] `cargo fmt --check -p alknet-http --features mcp` passes
- [ ] `cargo clippy -p alknet-http --features mcp` passes with no warnings
- [ ] `cargo test -p alknet-http --features mcp` passes
- [ ] `cargo check -p alknet-http` (no mcp) succeeds

## References

- docs/architecture/crates/http/http-mcp.md — full MCP spec
- docs/research/alknet-http-gateway-factoring/findings.md — §4 (rmcp constraints), §4.4 (auth sharing)
- docs/architecture/decisions/037-mcp-stdio-transport-exclusion.md — ADR-037
- docs/architecture/decisions/041-mcp-tool-gateway-pattern.md — ADR-041
- docs/architecture/decisions/017-call-protocol-client-and-adapter-contract.md — ADR-017 §5
- docs/architecture/decisions/023-operation-error-schemas.md — ADR-023
- docs/architecture/decisions/014-secret-material-flow-and-capability-injection.md — ADR-014
- docs/architecture/decisions/034-outgoing-only-x509-and-three-peer-roles.md — ADR-034 §4
- /workspace/rust-sdk/ — rmcp SDK (ServerHandler, StreamableHttpService, CallToolResult)

## Notes

> The MCP adapters are feature-gated and the most SDK-coupled part of
> the crate (rmcp integration). The review should verify: (1) streamable
> HTTP only (no stdio, ADR-037), (2) the 4-tool gateway pattern (ADR-041,
> not one tool per operation), (3) the output handling
> (structuredContent-preferred, ContentBlock fallback, no JSON.parse),
> (4) the shared dispatch spine is used for to_mcp's call (same spine as
> to_openapi), (5) the auth middleware is shared (Identity survives rmcp
> framing — research §6 #2), (6) the no-env-vars invariant holds, (7)
> feature gate isolation (MCP code not compiled without mcp). If
> deviations are found, document and fix before considering the MCP
> adapters complete.

## Summary

> To be filled on completion