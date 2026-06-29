---
status: draft
last_updated: 2026-06-29
---

# HTTP MCP — from_mcp and to_mcp

The MCP-direction adapters (feature-gated behind `mcp`): `from_mcp`
imports remote MCP tools as call-protocol operations over streamable
HTTP (reqwest client), and `to_mcp` exposes local operations as MCP
tools over streamable HTTP (axum server). This document covers both, the
rmcp integration, and the stdio exclusion (ADR-037).

## What

Two adapters, both in `alknet-http`, both behind the `mcp` feature gate:

1. **`from_mcp`** — discovers remote MCP tools via the MCP
   `tools/list` call over streamable HTTP, and registers each as a
   `HandlerRegistration` bundle with a forwarding handler that calls the
   remote tool via `tools/call`. Uses rmcp's
   `StreamableHttpClientTransport` (reqwest-based). Provenance is
   `FromMCP` (leaf, `composition_authority: None`, `scoped_env: None`,
   `Internal` by default — ADR-015/022). Implements `OperationAdapter`.
2. **`to_mcp`** — exposes the local registry's `External` operations as
   MCP tools over streamable HTTP, using rmcp's `StreamableHttpService`
   (an axum-compatible tower service). An external MCP client (an editor,
   an AI tool) discovers and calls alknet operations through the MCP
   protocol. A pure projection (consumes the registry, does not produce
   entries — ADR-017 §5).

### Streamable HTTP only (ADR-037)

MCP defines two transports: streamable HTTP and stdio. **alknet-http
supports only streamable HTTP.** Stdio is not built — it is the spawn-
arbitrary-executable RCE vector that the rest of the architecture is
designed to avoid (ADR-037). The `mcp` feature gate pulls in rmcp with
the streamable HTTP transport features only; the stdio transport
(`transport-child-process`) is not a dependency, not optional, not
behind a separate feature.

If an operator wants a stdio-only MCP server, they run a small
streamable-HTTP-to-stdio bridge themselves, outside alknet. The bridge
is where the RCE risk lives, explicitly in the operator's hands. See
ADR-037.

### from_mcp

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
   - `spec.output_schema` = the tool's `outputSchema`, or
     `Type.Unknown()` if absent (the TS `from_mcp.ts` shows this
     fallback).
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

### Forwarding handler

At call time, the `from_mcp` forwarding handler:

1. Reads the call input (`serde_json::Value` — the tool arguments).
2. Calls `client.call_tool({ name: tool_name, arguments: input })` via
   the rmcp client (the `streamable_http.rs` example shows
   `client.call_tool(CallToolRequestParams::new(name).with_arguments(...))`).
3. On success: extracts `structuredContent` (if present) or maps the
   `content` blocks (the TS `mapMCPContentBlocks` shows the mapping:
   text/image/audio/resource/resource_link → `MCPContentBlock`),
   wraps in a `ResponseEnvelope`, returns.
4. On `result.isError`: maps to a `CallError` with the MCP error content
   (the TS `from_mcp.ts` handler shows the error mapping), returns.
5. The rmcp client connection is maintained for the lifetime of the
   registration (the MCP server is a persistent streamable HTTP
   endpoint, not a per-call connection).

The handler is opaque to the `CallAdapter` — `Arc<dyn Handler>` the
registry dispatches. `alknet-call` never sees rmcp.

### to_mcp

```rust
pub fn to_mcp_service(
    registry: Arc<OperationRegistry>,
    identity_provider: Arc<dyn IdentityProvider>,
) -> StreamableHttpService<...>;
```

`to_mcp` exposes the local registry's `External` operations as MCP tools
over streamable HTTP, using rmcp's `StreamableHttpService` (an
axum-compatible tower service). The rmcp
`simple_auth_streamhttp.rs` server example shows the pattern:

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

`alknet-http`'s `to_mcp` follows the same pattern: the local operations
are exposed as an MCP server (an rmcp `Service` impl that wraps the
`OperationRegistry`), the `StreamableHttpService` nests into the axum
`Router` at `/mcp`, and a Bearer auth middleware gates access (the
`simple_auth_streamhttp.rs` `auth_middleware` + `extract_token` pattern).

The `to_mcp` service:

1. On MCP `tools/list`: returns the local registry's `External`
   operations as MCP tools (name, description, `inputSchema`).
2. On MCP `tools/call`: dispatches to the `OperationRegistry::invoke()`
   — the same dispatch path the HTTP server uses for HTTP requests
   (ADR-036). The MCP tool call becomes a `call.requested` internally.
   The result is mapped back to the MCP `tools/call` response shape
   (`structuredContent` or `content` blocks).
3. Auth: the Bearer middleware resolves the token via
   `IdentityProvider::resolve_from_token()`, same as the HTTP server's
   auth (ADR-004). The MCP client authenticates by bearer token; no
   `PeerId` (browsers and MCP clients are not alknet peers — ADR-034 §4).

### No-Env-Vars

The `from_mcp` forwarding handler reads the MCP server's bearer token
from `context.capabilities` (the same injection path as `from_openapi`),
not from `std::env::var`. The assembly layer injects the token at
registration; the handler reads it per-call. This is the no-env-vars
invariant (ADR-014, [overview.md](overview.md)).

## Why

MCP is the protocol editors and AI tools use to discover and call tools.
`from_mcp` lets alknet compose external MCP servers (a remote tool
server, a third-party MCP endpoint) into the call protocol — the same
composition pattern as `from_openapi` and `from_call`. `to_mcp` lets
external MCP clients (an editor, an AI tool) discover and call alknet
operations through the MCP protocol, without those clients needing to
speak EventEnvelope.

The streamable-HTTP-only constraint (ADR-037) is a security position:
alknet does not import the MCP stdio RCE vector. The streamable HTTP
path is network-isolated, auth-gatable, and runs under alknet's
auth/identity/capabilities machinery — the same machinery that gates
every other HTTP request.

## Constraints

- **Streamable HTTP only.** Stdio is not built (ADR-037). The `mcp`
  feature pulls in rmcp with streamable HTTP transport features only.
- **`from_mcp`-registered ops are `Internal` by default.** Composition
  material, not directly callable from the wire (ADR-015).
- **`from_mcp` handlers read credentials from
  `OperationContext.capabilities`.** No env vars (ADR-014).
- **`to_mcp` is a pure projection.** Consumes the registry, does not
  produce entries. Not an `OperationAdapter`.
- **MCP clients are not alknet peers.** A browser or MCP client
  connecting to `to_mcp` authenticates by bearer token, gets no
  `PeerId`, is not in the peer graph (ADR-034 §4).
- **The `mcp` feature is optional.** A deployment that doesn't need MCP
  doesn't compile rmcp. The default feature set is `h2` + `http1`.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| MCP stdio transport excluded | [ADR-037](../../decisions/037-mcp-stdio-transport-exclusion.md) | Streamable HTTP only; stdio is not built |
| `from_mcp` is an `OperationAdapter` | [ADR-017](../../decisions/017-call-protocol-client-and-adapter-contract.md) | Async trait; produces `HandlerRegistration` bundles |
| `to_mcp` is a projection | [ADR-017](../../decisions/017-call-protocol-client-and-adapter-contract.md) | Consumes the registry, doesn't produce entries |
| Adapter-registered ops are `Internal` | [ADR-015](../../decisions/015-privilege-model-and-authority-context.md) | `from_mcp` ops are composition material |
| `from_mcp` provenance is a leaf | [ADR-022](../../decisions/022-handler-registration-provenance-and-composition-authority.md) | `composition_authority: None`, `scoped_env: None` |
| Error fidelity | [ADR-023](../../decisions/023-operation-error-schemas.md) | MCP tool errors mapped to `ErrorDefinition`s |
| No-env-vars credential injection | [ADR-014](../../decisions/014-secret-material-flow-and-capability-injection.md) | Handler reads `context.capabilities`, not env vars |
| MCP clients are not alknet peers | [ADR-034](../../decisions/034-outgoing-only-x509-and-three-peer-roles.md) | Bearer token, no `PeerId` |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-40** (open): reqwest client config — the shared `reqwest::Client`
  used by `from_mcp` (same client as `from_openapi`).

## References

- [ADR-037](../../decisions/037-mcp-stdio-transport-exclusion.md) — the
  stdio exclusion this document enforces
- [overview.md](overview.md) — adapter location map, feature gates
- [../call/client-and-adapters.md](../call/client-and-adapters.md) —
  `OperationAdapter` trait, `AdapterError` variants
- `/workspace/rust-sdk/` — MCP Rust SDK (rmcp v1.8.0); streamable HTTP
  transport
- `/workspace/rust-sdk/examples/servers/src/simple_auth_streamhttp.rs`
  — streamable HTTP MCP server with Bearer auth (the `to_mcp` pattern)
- `/workspace/rust-sdk/examples/clients/src/streamable_http.rs` —
  streamable HTTP MCP client (the `from_mcp` pattern)
- `/workspace/@alkdev/operations/src/from_mcp.ts` — TypeScript prior art
  (`createMCPClient`, `mapMCPContentBlocks`, the `MCPClientLoader`)