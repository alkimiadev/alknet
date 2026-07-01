---
id: http/adapters/from-mcp
name: Implement from_mcp adapter (rmcp streamable HTTP client, tools/list discovery, structuredContent handling)
status: completed
depends_on: [http/client/shared-http-client, http/gateway/error-mapping]
scope: broad
risk: medium
impact: component
level: implementation
---

## Description

Implement `from_mcp` in `src/adapters/from_mcp.rs` (feature-gated behind
`mcp`). This is the MCP-direction adapter: it discovers remote MCP tools
via the MCP `tools/list` call over streamable HTTP, and registers each
as a `HandlerRegistration` bundle with a forwarding handler that calls
the remote tool via `tools/call`. Uses rmcp's
`StreamableHttpClientTransport` (reqwest-based). Implements
`OperationAdapter` (ADR-017 §5).

### Streamable HTTP only (ADR-037)

MCP defines two transports: streamable HTTP and stdio. **alknet-http
supports only streamable HTTP.** Stdio is not built — it is the spawn-
arbitrary-executable RCE vector that the rest of the architecture is
designed to avoid (ADR-037). The `mcp` feature gate pulls in rmcp with
the streamable HTTP transport features only; the stdio transport
(`transport-child-process`) is not a dependency, not optional, not
behind a separate feature.

### The adapter (http-mcp.md §"from_mcp")

```rust
pub struct FromMCP {
    /// The MCP server's streamable HTTP endpoint URL.
    endpoint: String,
    /// Bearer token for the MCP server (from Capabilities at registration).
    auth_token: Option<String>,
    /// The importing deployment's name for this MCP server (becomes the
    /// operation namespace).
    namespace: String,
}

#[async_trait]
impl OperationAdapter for FromMCP {
    async fn import(&self) -> Result<Vec<HandlerRegistration>, AdapterError>;
}
```

The adapter:

1. Connects to the MCP server's streamable HTTP endpoint using rmcp's
   `StreamableHttpClientTransport::from_uri(endpoint)` (the rmcp
   `streamable_http.rs` client example shows the pattern: `client_info
   .serve(transport).await`, then `client.list_tools()`,
   `client.call_tool()`). On connection failure, returns
   `AdapterError::DiscoveryFailed`; on 401, `AdapterError::Unauthorized`.

2. Calls `tools/list` → the list of MCP tools (name, description,
   `inputSchema`, optional `outputSchema`).

3. For each tool, constructs a `HandlerRegistration`:
   - `spec.name` = the tool name (or `namespace/tool_name` if a
     namespace prefix is configured — same local-naming sugar as
     `from_call`'s `FromCallConfig::namespace_prefix`, ADR-029 §5).
   - `spec.namespace` = the configured `namespace`.
   - `spec.op_type` = `Mutation` (MCP tools are call/response; the MCP
     spec doesn't have a native streaming/tool-subscription distinction
     — `tools/call` returns a result. If MCP adds a streaming-tool
     extension, a `Subscription` mapping would be added.)
   - `spec.visibility` = `Internal` (adapter-registered, ADR-015).
   - `spec.input_schema` = the tool's `inputSchema` (JSON Schema).
   - `spec.output_schema` = depends on whether the tool declares
     `outputSchema` (MCP 2025-06-18+):
     - **`outputSchema` present** → `output_schema` = the declared
       schema (converted from JSON Schema). The result arrives in
       `CallToolResult.structured_content` and is composable with
       local operations.
     - **`outputSchema` absent** (older MCP servers) → `output_schema`
       = the MCP `ContentBlock` union (`text | image | audio | resource
       | resource_link` — a well-defined MCP type, *not*
       `Type.Unknown()`). The result arrives in
       `CallToolResult.content` as a `Vec<ContentBlock>`. See "Output
       handling" below.
   - `spec.error_schemas` = the MCP tool's error description mapped to
     `ErrorDefinition` (ADR-023 — MCP tool definitions carry error
     descriptions; the adapter maps them).
   - `spec.access_control` = `AccessControl::default()`.
   - `handler` = a forwarding handler (see Forwarding Handler below).
   - `provenance` = `FromMCP`, `composition_authority: None`,
     `scoped_env: None` (leaf — ADR-022).
   - `capabilities` = the bearer token for the MCP server (injected by
     the assembly layer at registration — see No-Env-Vars below).

4. Returns the bundles. The caller (the assembly layer) registers them
   in the `OperationRegistry`.

### Forwarding handler (http-mcp.md §"Forwarding handler")

At call time, the `from_mcp` forwarding handler:

1. Reads the call input (`serde_json::Value` — the tool arguments).
2. Calls `client.call_tool({ name: tool_name, arguments: input })` via
   the rmcp client (the `streamable_http.rs` example shows
   `client.call_tool(CallToolRequestParams::new(name).with_arguments(...))`).
3. On success: extracts the result from the `CallToolResult`, following
   the `structuredContent`-preferred-over-content-blocks rule (see
   "Output handling" below), wraps in a `ResponseEnvelope`, returns.
4. On `result.isError`: maps to a `CallError` with the MCP error content
   (the TS `from_mcp.ts` handler shows the error mapping), returns.
5. The rmcp client connection is maintained for the lifetime of the
   registration (the MCP server is a persistent streamable HTTP
   endpoint, not a per-call connection).

The handler is opaque to the `CallAdapter` — `Arc<dyn Handler>` the
registry dispatches. `alknet-call` never sees rmcp.

### Output handling: structuredContent vs content blocks (http-mcp.md §"Output handling")

MCP `CallToolResult` (rmcp `model.rs`) carries two result fields:
`content: Vec<ContentBlock>` (always present, defaults to `[]`) and
`structured_content: Option<Value>` (present when the tool declared
`outputSchema`). The `from_mcp` handler follows the same rule the TS
adapter (`@alkdev/operations/src/from_mcp.ts`) and the rmcp SDK
(`CallToolResult::into_typed`) use:

- **`structured_content` present** (tool declared `outputSchema`): the
  handler uses `structured_content` as the result, validated/cast
  against the declared `output_schema`. This is the composable case —
  the data matches the declared type, so a composing handler can use it
  as a typed value.
- **`structured_content` absent** (older server, no `outputSchema`):
  the handler maps `content: Vec<ContentBlock>` to the
  `ContentBlock`-union `output_schema` (text/image/audio/resource/
  resource_link). The TS `mapMCPContentBlocks` shows the mapping; the
  Rust `ContentBlock` enum (`rmcp/src/model/content.rs`) is the same
  shape. The common sub-case is a single `Text` block — older servers
  often JSON-stringify structured data into the `text` field. The
  adapter does *not* attempt to `JSON.parse` the text heuristically
  (fragile, not the adapter's concern); it carries the `ContentBlock`
  union as the typed result. A consumer that knows the text is JSON can
  parse it downstream.

The `isError: true` case is handled separately (step 4 above) — it
maps to a `CallError`, not to the output handling path.

### No-Env-Vars (http-mcp.md §"No-Env-Vars")

The `from_mcp` forwarding handler reads the MCP server's bearer token
from `context.capabilities` (the same injection path as `from_openapi`),
not from `std::env::var`. The assembly layer injects the token at
registration; the handler reads it per-call. This is the no-env-vars
invariant (ADR-014).

## Acceptance Criteria

- [ ] `FromMCP` struct with `endpoint`, `auth_token`, `namespace`
- [ ] `OperationAdapter` impl: `async fn import(&self) -> Result<Vec<HandlerRegistration>, AdapterError>`
- [ ] Connects via rmcp `StreamableHttpClientTransport::from_uri(endpoint)`
- [ ] Connection failure → `AdapterError::DiscoveryFailed`
- [ ] 401 → `AdapterError::Unauthorized`
- [ ] Calls `tools/list` → MCP tools (name, description, inputSchema, outputSchema)
- [ ] For each tool: constructs `HandlerRegistration`
- [ ] `spec.name` = tool name (or `namespace/tool_name` with prefix)
- [ ] `spec.namespace` = configured `namespace`
- [ ] `spec.op_type` = `Mutation` (MCP tools are call/response)
- [ ] `spec.visibility` = `Internal` (ADR-015)
- [ ] `spec.input_schema` = tool's `inputSchema`
- [ ] `spec.output_schema` = declared `outputSchema` if present, else `ContentBlock` union
- [ ] `spec.error_schemas` from MCP tool error descriptions (ADR-023)
- [ ] `spec.access_control` = `AccessControl::default()`
- [ ] `provenance` = `FromMCP`, `composition_authority: None`, `scoped_env: None` (ADR-022)
- [ ] `capabilities` = bearer token for MCP server (injected at registration)
- [ ] Forwarding handler calls `client.call_tool({ name, arguments })`
- [ ] `structured_content` present → use as result (validated against `output_schema`)
- [ ] `structured_content` absent → map `content: Vec<ContentBlock>` to `ContentBlock` union
- [ ] No heuristic `JSON.parse` of text blocks (carry as `ContentBlock`)
- [ ] `isError: true` → `CallError` with MCP error content
- [ ] rmcp client connection maintained for registration lifetime
- [ ] No-env-vars: handler reads `context.capabilities`, never `std::env::var` (ADR-014)
- [ ] Feature-gated behind `mcp` (no compile without `mcp` feature)
- [ ] stdio transport NOT built (ADR-037 — streamable HTTP only)
- [ ] Unit test: `import()` with mock MCP server → `HandlerRegistration` bundles
- [ ] Unit test: `outputSchema` present → `output_schema` = declared schema
- [ ] Unit test: `outputSchema` absent → `output_schema` = `ContentBlock` union
- [ ] Unit test: `structured_content` present → used as result
- [ ] Unit test: `structured_content` absent → `content` blocks mapped to union
- [ ] Unit test: `isError: true` → `CallError`
- [ ] Integration test: forwarding handler calls remote MCP tool via rmcp
- [ ] Integration test: no `std::env::var` reads in the forwarding handler
- [ ] `cargo test -p alknet-http --features mcp` succeeds
- [ ] `cargo clippy -p alknet-http --features mcp --all-targets` succeeds with no warnings
- [ ] `cargo check -p alknet-http` (no `mcp` feature) succeeds — from_mcp not compiled

## References

- docs/architecture/crates/http/http-mcp.md — from_mcp (full spec)
- docs/architecture/decisions/037-mcp-stdio-transport-exclusion.md — ADR-037 (streamable HTTP only)
- docs/architecture/decisions/017-call-protocol-client-and-adapter-contract.md — ADR-017 §5 (OperationAdapter)
- docs/architecture/decisions/015-privilege-model-and-authority-context.md — ADR-015 (Internal)
- docs/architecture/decisions/022-handler-registration-provenance-and-composition-authority.md — ADR-022 (leaf)
- docs/architecture/decisions/023-operation-error-schemas.md — ADR-023 (error fidelity)
- docs/architecture/decisions/014-secret-material-flow-and-capability-injection.md — ADR-014 (no env vars)
- /workspace/rust-sdk/crates/rmcp/src/model.rs — CallToolResult (content + structured_content)
- /workspace/rust-sdk/crates/rmcp/src/model/content.rs — ContentBlock enum
- /workspace/rust-sdk/examples/clients/src/streamable_http.rs — streamable HTTP MCP client pattern
- /workspace/@alkdev/operations/src/from_mcp.ts — TypeScript prior art (mapMCPContentBlocks, structuredContent logic)

## Notes

> from_mcp is feature-gated behind mcp (rmcp dependency). Streamable HTTP
> only — stdio is NOT built (ADR-037). The output handling follows the
> structuredContent-preferred-over-content-blocks rule (same as the TS
> adapter and rmcp's into_typed). The adapter does NOT heuristically
> JSON.parse text blocks — it carries the ContentBlock union as the
> typed result; a downstream consumer that knows the text is JSON can
> parse it. The no-env-vars invariant applies (handler reads
> context.capabilities, not std::env::var). The rmcp client connection
> is maintained for the registration lifetime (persistent streamable HTTP
> endpoint, not per-call). The handler is opaque to CallAdapter
> (Arc<dyn Handler>); alknet-call never sees rmcp.

## Summary

> Implemented FromMCP adapter (feature-gated behind mcp) in src/adapters/from_mcp/.
> rmcp StreamableHttpClientTransport connects to MCP endpoint, calls tools/list,
> builds HandlerRegistration bundles (provenance FromMCP, leaf, Internal, Mutation,
> capabilities=bearer token). Forwarding handler calls client.call_tool, maps
> CallToolResult per structuredContent-preferred-over-content-blocks rule (declared
> outputSchema → structuredContent; absent → ContentBlock union; no heuristic
> JSON.parse; isError→CallError). No-env-vars (reads context.capabilities). Streamable
> HTTP only (ADR-037). 19 unit + 5 integration tests (real rmcp streamable HTTP server).
> cargo test/clippy --features mcp pass clean; cargo check (no mcp) compiles with
> from_mcp excluded.