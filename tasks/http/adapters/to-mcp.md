---
id: http/adapters/to-mcp
name: Implement to_mcp gateway projection (4-tool gateway, rmcp StreamableHttpService, ADR-041)
status: completed
depends_on: [http/gateway/gateway-dispatch-spine, http/server/bearer-auth-middleware]
scope: broad
risk: medium
impact: component
level: implementation
---

## Description

Implement `to_mcp` in `src/adapters/to_mcp.rs` (feature-gated behind
`mcp`). This is the MCP-direction gateway projection: it exposes the
local registry's `External` operations as a **fixed gateway tool set**
over streamable HTTP — not one MCP tool per operation. This is the
tool-gateway pattern (ADR-041): the LLM has a few tools in context
(search, schema, call, batch), not hundreds, and discovers operations
on demand through the gateway.

### Pure projection (ADR-017 §5)

`to_mcp` is a pure projection — it consumes the registry and does not
produce entries for it. It is not an `OperationAdapter`. An external MCP
client (an editor, an AI tool) discovers and calls alknet operations
through the MCP protocol.

### The gateway tool set (http-mcp.md §"The gateway tool set")

`to_mcp` exposes four MCP tools that gate access to the full operation
registry:

| MCP tool | Call protocol operation | Purpose |
|----------|------------------------|---------|
| `search` | `services/list` | List/search available operations (filtered by the caller's `AccessControl`). Returns names + descriptions, not full schemas. |
| `schema` | `services/schema` | Get an operation's full `OperationSpec` (input/output JSON Schemas, error schemas). |
| `call` | `call.requested` (Query/Mutation) | Invoke an operation by name with a JSON input. Returns the output or a typed error (ADR-023). |
| `batch` | multiple `call.requested` | Invoke multiple operations in one tool call (correlated request IDs, OQ-14). |

The LLM calls `search` to discover operations, `schema` to learn an
operation's input shape, `call` to invoke. Same pattern as
`man <command>` — discover on demand, don't preload. See ADR-041 for
the rationale (the tool-bloat problem).

### `Subscription` exclusion (http-mcp.md §"Subscription exclusion")

The gateway exposes only `Query` and `Mutation` operations
(request/response). `Subscription` operations (streaming) are filtered
out of `search` results and cannot be invoked via `call` — MCP tool
calls are request/response by protocol design; streaming subscriptions
don't fit the LLM tool-call pattern. See ADR-041 §2.

### `to_mcp` service behavior (http-mcp.md §"to_mcp service behavior")

1. On MCP `tools/list`: returns the fixed gateway tool set (4 tools:
   `search`, `schema`, `call`, `batch`), not the registry's
   operations. The gateway tools have stable names and schemas; the
   registry's operations are discovered through `search`.

2. On MCP `tools/call`:
   - `search` → dispatches `services/list` (filtered by the caller's
     `AccessControl`), returns operation names + descriptions.
   - `schema` → dispatches `services/schema`, returns the
     `OperationSpec`.
   - `call` → dispatches `OperationRegistry::invoke()` (via the shared
     `GatewayDispatch::invoke()` — the dispatch spine). The result is
     mapped to an MCP `CallToolResult` (`structuredContent` for the
     output, or `isError: true` for a `CallError` with typed
     `details` per ADR-023).
   - `batch` → dispatches multiple `call.requested` events, returns
     an array of results.

3. Auth: the Bearer middleware resolves the token via
   `IdentityProvider::resolve_from_token()`, same as the HTTP server's
   auth (ADR-004). The MCP client authenticates by bearer token; no
   `PeerId` (browsers and MCP clients are not alknet peers — ADR-034 §4).
   `AccessControl` gates `search` results and `call` dispatch — the
   LLM sees only what it's authorized to call.

### rmcp integration (http-mcp.md §"to_mcp", research §4)

The rmcp `simple_auth_streamhttp.rs` server example shows the
streamable-HTTP-service-into-axum-`Router` pattern:

```rust
// From the rmcp example:
let mcp_service: StreamableHttpService<Counter, LocalSessionManager> =
    StreamableHttpService::new(
        || Ok(Counter::new()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    );

let protected_mcp_router = Router::new()
    .nest_service("/mcp", mcp_service)
    .layer(middleware::from_fn_with_state(token_store, auth_middleware));
```

`alknet-http`'s `to_mcp` follows the same axum integration pattern, but
the rmcp `Service` impl is a gateway service (4 fixed tools) rather than
a per-operation tool registry. The `to_mcp` gateway implements rmcp's
`ServerHandler` trait (`call_tool` / `list_tools`) and is wrapped by
`StreamableHttpService` (a `tower::Service<Request<RequestBody>>`).

### Shared dispatch spine with `to_openapi` (http-mcp.md §"Shared dispatch spine")

`to_mcp`'s `call` tool and `to_openapi`'s `/call` endpoint share the
same dispatch spine: resolve caller identity (Bearer →
`IdentityProvider::resolve_from_token`) → build a root
`OperationContext` → `OperationRegistry::invoke()` → map the
`ResponseEnvelope` to the gateway's wire shape (`CallToolResult` for
MCP, HTTP JSON for OpenAPI). The wire framing, discovery listing
(`tools/list` vs `/search`), streaming (excluded vs `/subscribe` SSE),
and server integration (rmcp `StreamableHttpService` tower service vs
axum route handlers) are genuinely per-gateway and are not shared.

The shared spine is the `GatewayDispatch` struct (the
`gateway-dispatch-spine` task). `to_mcp` holds an `Arc<GatewayDispatch>`
(or it lives in the rmcp service state) and calls
`GatewayDispatch::invoke()` for the `call` tool. The
`ResponseEnvelope` → `CallToolResult` mapping is `to_mcp`-specific.

### Auth: shared middleware (research §4.4)

The Bearer auth middleware (the `bearer-auth-middleware` task) is applied
as an axum layer *around* the nested `StreamableHttpService` (the rmcp
example shows the pattern: `middleware::from_fn_with_state` around
`Router::nest_service`). The `to_mcp` `call_tool` handler reads the
`Identity` from `RequestContext<RoleServer>.extensions` (rmcp injects
`http::request::Parts` into extensions — `tower.rs:487-521, 1086-1097`).

### `CallToolResult` mapping

The `ResponseEnvelope` → `CallToolResult` mapping uses rmcp's
`IntoCallToolResult` trait (`tool.rs:78-113`):

- `Ok(value)` → `CallToolResult::structured(value)` (`model.rs:3006`).
- `Err(call_error)` → `CallToolResult::structured_error(error.details)`
  (`model.rs:3032`) or `CallToolResult::error(error_data)`.

## Acceptance Criteria

- [ ] `to_mcp` implements rmcp `ServerHandler` trait (`call_tool`, `list_tools`)
- [ ] `tools/list` returns 4 fixed gateway tools (`search`, `schema`, `call`, `batch`)
- [ ] `tools/list` does NOT return the registry's operations (discovered via `search`)
- [ ] `search` tool → dispatches `services/list` via `GatewayDispatch::invoke`
- [ ] `search` results are `AccessControl::check(identity)`-filtered
- [ ] `search` results are names + descriptions (not full schemas)
- [ ] `Subscription` ops filtered out of `search` results (ADR-041 §2)
- [ ] `schema` tool → dispatches `services/schema` via `GatewayDispatch::invoke`
- [ ] `call` tool → dispatches via `GatewayDispatch::invoke` (shared spine)
- [ ] `call` result → `CallToolResult::structured(value)` for `Ok`
- [ ] `call` error → `CallToolResult::structured_error(details)` for `Err(CallError)`
- [ ] `batch` tool → loop over `GatewayDispatch::invoke`, returns array
- [ ] Bearer auth via shared `bearer_auth_middleware` (applied around `nest_service`)
- [ ] `Identity` read from `RequestContext.extensions` inside `call_tool`
- [ ] MCP client has no `PeerId` (not an alknet peer, ADR-034 §4)
- [ ] `AccessControl` gates `search` results and `call` dispatch
- [ ] `to_mcp` is a pure projection (consumes registry, does not produce entries)
- [ ] `StreamableHttpService` nested into axum `Router` at `/mcp`
- [ ] Feature-gated behind `mcp` (no compile without `mcp` feature)
- [ ] stdio transport NOT built (ADR-037)
- [ ] Unit test: `tools/list` returns exactly 4 gateway tools
- [ ] Unit test: `search` returns AccessControl-filtered ops (no Subscriptions)
- [ ] Unit test: `schema` returns full OperationSpec
- [ ] Unit test: `call` → `CallToolResult::structured` for success
- [ ] Unit test: `call` → `CallToolResult::structured_error` for CallError
- [ ] Unit test: `batch` returns array of results
- [ ] Integration test: MCP client calls `search` → `schema` → `call` round-trip
- [ ] Integration test: Bearer auth middleware gates `to_mcp` service
- [ ] Integration test: `Identity` survives rmcp framing (research §6 #2)
- [ ] `cargo test -p alknet-http --features mcp` succeeds
- [ ] `cargo clippy -p alknet-http --features mcp --all-targets` succeeds with no warnings
- [ ] `cargo check -p alknet-http` (no `mcp` feature) succeeds — to_mcp not compiled

## References

- docs/architecture/crates/http/http-mcp.md — to_mcp (full spec)
- docs/research/alknet-http-gateway-factoring/findings.md — §4 (rmcp StreamableHttpService constraints), §4.4 (auth middleware sharing)
- docs/architecture/decisions/041-mcp-tool-gateway-pattern.md — ADR-041 (4-tool gateway, Subscription exclusion)
- docs/architecture/decisions/037-mcp-stdio-transport-exclusion.md — ADR-037 (streamable HTTP only)
- docs/architecture/decisions/017-call-protocol-client-and-adapter-contract.md — ADR-017 §5 (to_* are projections)
- docs/architecture/decisions/023-operation-error-schemas.md — ADR-023 (error fidelity, CallToolResult mapping)
- docs/architecture/decisions/034-outgoing-only-x509-and-three-peer-roles.md — ADR-034 §4 (MCP clients are not peers)
- /workspace/rust-sdk/examples/servers/src/simple_auth_streamhttp.rs — axum middleware around nested StreamableHttpService
- /workspace/rust-sdk/crates/rmcp/src/handler/server.rs — ServerHandler trait (call_tool, list_tools)
- /workspace/rust-sdk/crates/rmcp/src/handler/server/tool.rs — IntoCallToolResult trait
- /workspace/rust-sdk/crates/rmcp/src/model.rs — CallToolResult (structured, structured_error)

## Notes

> to_mcp is the 4-tool gateway (ADR-041): search/schema/call/batch, not
> one tool per operation. The LLM discovers operations on demand through
> search + schema, same as man <command>. Subscription ops are excluded
> (MCP tool calls are request/response). The shared dispatch spine
> (GatewayDispatch) is used for the call tool; the ResponseEnvelope →
> CallToolResult mapping is to_mcp-specific. The Bearer auth middleware
> is shared with the HTTP routes (research §4.4 — applied around
> nest_service). The load-bearing assumption is that Identity survives
> the rmcp framing (research §6 #2 — confirm with a spike that
> ctx.extensions.get::<Identity>() works inside call_tool). to_mcp is a
> pure projection (consumes registry, does not produce entries). The mcp
> feature gate is optional; stdio is NOT built (ADR-037).

## Summary

> Implemented src/adapters/to_mcp.rs: ToMcpGateway rmcp ServerHandler with 4 fixed
> gateway tools (search/schema/call/batch). search dispatches services/list (ACL-
> filtered, excludes Subscriptions ADR-041 §2), schema dispatches services/schema,
> call/batch dispatch via GatewayDispatch::invoke with ResponseEnvelope→CallToolResult
> mapping (structured for Ok, structured_error for Err). Bearer auth via shared
> middleware around nest_service. Identity survives rmcp framing (research §6 #2
> confirmed via test). Feature-gated behind mcp; stdio NOT built (ADR-037). Pure
> projection. StreamableHttpService nested at /mcp. 16 unit tests. 223 mcp-feature
> tests + 5 integration tests pass. Clippy clean on both feature configs.