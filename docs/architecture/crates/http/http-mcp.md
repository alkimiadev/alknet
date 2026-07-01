---
status: draft
last_updated: 2026-07-01
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
    - `spec.output_schema` = depends on whether the tool declares
      `outputSchema` (MCP 2025-06-18+):
      - **`outputSchema` present** → `output_schema` = the declared
        schema (converted from JSON Schema). The result arrives in
        `CallToolResult.structured_content` and is composable with
        local operations (the data matches the declared type).
      - **`outputSchema` absent** (older MCP servers) → `output_schema`
        = the MCP `ContentBlock` union (`text | image | audio |
        resource | resource_link` — a well-defined MCP type, *not*
        `Type.Unknown()`). The result arrives in
        `CallToolResult.content` as a `Vec<ContentBlock>`. The common
        sub-case is a single `Text` block (which older servers often
        fill with JSON-stringified data), but the *type* is the
        `ContentBlock` union regardless of what the text contains.
        See "Output handling" below.
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

### Output handling (structuredContent vs content blocks)

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

### to_mcp

```rust
pub fn to_mcp_service(
    registry: Arc<OperationRegistry>,
    identity_provider: Arc<dyn IdentityProvider>,
) -> StreamableHttpService<...>;
```

`to_mcp` exposes the local registry's operations as a **fixed gateway
tool set** over streamable HTTP — not one MCP tool per operation. This
is the tool-gateway pattern (ADR-041): the LLM has a few tools in
context (search, schema, call, batch), not hundreds, and discovers
operations on demand through the gateway. See
[ADR-041](../../decisions/041-mcp-tool-gateway-pattern.md) for the
rationale (the tool-bloat problem, the `memory`/`worktree` tool pattern
that informed the design).

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

`alknet-http`'s `to_mcp` follows the same axum integration pattern,
but the rmcp `Service` impl is a gateway service (4 fixed tools) rather
than a per-operation tool registry.

#### The gateway tool set

`to_mcp` exposes four MCP tools that gate access to the full operation
registry:

| MCP tool | Call protocol operation | Purpose |
|----------|------------------------|---------|
| `search` | `services/list` | List/search available operations (filtered by the caller's `AccessControl`). Returns names + descriptions, not full schemas. |
| `schema` | `services/schema` | Get an operation's full `OperationSpec` (input/output JSON Schemas, error schemas). |
| `call` | `call.requested` (Query/Mutation) | Invoke an operation by name with a JSON input. Returns the output or a typed error (ADR-023). |
| `batch` | multiple `call.requested` | Invoke multiple operations in one tool call (correlated request IDs, OQ-14). |

The LLM calls `search` to discover operations, `schema` to learn an
operation's input shape, `call` to invoke. Same pattern as `man
<command>` — discover on demand, don't preload. See ADR-041 for the
rationale.

#### `Subscription` exclusion

The gateway exposes only `Query` and `Mutation` operations
(request/response). `Subscription` operations (streaming) are filtered
out of `search` results and cannot be invoked via `call` — MCP tool
calls are request/response by protocol design; streaming subscriptions
don't fit the LLM tool-call pattern. See ADR-041 §2.

#### `to_mcp` service behavior

1. On MCP `tools/list`: returns the fixed gateway tool set (4 tools:
   `search`, `schema`, `call`, `batch`), not the registry's
   operations. The gateway tools have stable names and schemas; the
   registry's operations are discovered through `search`.
2. On MCP `tools/call`:
   - `search` → dispatches `services/list` (filtered by the caller's
     `AccessControl`), returns operation names + descriptions.
   - `schema` → dispatches `services/schema`, returns the
     `OperationSpec`.
   - `call` → dispatches `OperationRegistry::invoke()` (the same
     dispatch path the HTTP server uses, ADR-036). The result is
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

#### Shared dispatch spine with `to_openapi`

`to_mcp`'s `call` tool and `to_openapi`'s `/call` endpoint share the
same dispatch spine: resolve caller identity (Bearer →
`IdentityProvider::resolve_from_token`) → build a root
`OperationContext` → `OperationRegistry::invoke()` → map the
`ResponseEnvelope` to the gateway's wire shape (`CallToolResult` for
MCP, HTTP JSON for OpenAPI). The wire framing, discovery listing
(`tools/list` vs `/search`), streaming (excluded vs `/subscribe` SSE),
and server integration (rmcp `StreamableHttpService` tower service vs
axum route handlers) are genuinely per-gateway and are not shared.

Research findings
(`docs/research/alknet-http-gateway-factoring/findings.md`) recommend
extracting a **thin shared spine** (a concrete struct holding
`Arc<OperationRegistry>` + `Arc<dyn IdentityProvider>` with a
`resolve + build_context + invoke` method returning a
`ResponseEnvelope`), **not** a `GatewayDispatch` trait or gateway
abstraction. The spine is small (~15–30 lines per endpoint), but it is
the one place where a divergence bug (identity resolved differently,
`OperationContext.internal` set inconsistently, `CallError` mapped
asymmetrically) would be a security/correctness issue. The
server-integration and wire-framing layers stay per-gateway; a third
gateway (GraphQL, gRPC) is not on the horizon, and if one appears its
server-integration layer needs its own shape anyway. This is an
implementation factoring note, not an ADR — the decision is internal to
`alknet-http` and does not cross crate boundaries.

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

`to_mcp` uses the **tool-gateway pattern** (ADR-041): a fixed set of
meta-tools (`search`, `schema`, `call`, `batch`) gates access to the
full operation registry, so the LLM has a few tools in context instead
of hundreds. This addresses the tool-bloat problem — an LLM connecting
to a node with 200 operations gets 4 MCP tools, not 200, and discovers
operations on demand through `search` + `schema`. Same pattern as the
`memory` and `worktree` tools (one entry point, large dataset behind
it), and the same principle as Linux's `man` command (don't preload all
documentation; query on demand).

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
| `to_mcp` tool-gateway pattern | [ADR-041](../../decisions/041-mcp-tool-gateway-pattern.md) | 4 fixed gateway tools (search/schema/call/batch), not one tool per operation; Subscription excluded |
| `from_mcp` is an `OperationAdapter` | [ADR-017](../../decisions/017-call-protocol-client-and-adapter-contract.md) | Async trait; produces `HandlerRegistration` bundles |
| `to_mcp` is a projection | [ADR-017](../../decisions/017-call-protocol-client-and-adapter-contract.md) | Consumes the registry, doesn't produce entries |
| Adapter-registered ops are `Internal` | [ADR-015](../../decisions/015-privilege-model-and-authority-context.md) | `from_mcp` ops are composition material |
| `from_mcp` provenance is a leaf | [ADR-022](../../decisions/022-handler-registration-provenance-and-composition-authority.md) | `composition_authority: None`, `scoped_env: None` |
| Error fidelity | [ADR-023](../../decisions/023-operation-error-schemas.md) | MCP tool errors mapped to `ErrorDefinition`s |
| No-env-vars credential injection | [ADR-014](../../decisions/014-secret-material-flow-and-capability-injection.md) | Handler reads `context.capabilities`, not env vars |
| MCP clients are not alknet peers | [ADR-034](../../decisions/034-outgoing-only-x509-and-three-peer-roles.md) | Bearer token, no `PeerId` |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-40** (resolved): reqwest client config — the shared
  `ClientWithMiddleware` used by `from_mcp` (same client as
  `from_openapi`).

## References

- [ADR-037](../../decisions/037-mcp-stdio-transport-exclusion.md) — the
  stdio exclusion this document enforces
- [overview.md](overview.md) — adapter location map, feature gates
- [../call/client-and-adapters.md](../call/client-and-adapters.md) —
  `OperationAdapter` trait, `AdapterError` variants
- `/workspace/rust-sdk/` — MCP Rust SDK (rmcp v1.8.0); streamable HTTP
  transport
- `/workspace/rust-sdk/crates/rmcp/src/model/tool.rs` — `Tool` with
  `output_schema: Option<Arc<JsonObject>>` (the `outputSchema` field)
- `/workspace/rust-sdk/crates/rmcp/src/model/content.rs` — `ContentBlock`
  enum (text/image/audio/resource/resource_link — the fallback
  `output_schema` type when `outputSchema` is absent)
- `/workspace/rust-sdk/crates/rmcp/src/model.rs` (~line 2868) —
  `CallToolResult` with `content: Vec<ContentBlock>` and
  `structured_content: Option<Value>` (the two result fields); see also
  `into_typed` (~line 3057) for the SDK's own
  structured-content-preferred-over-text-block fallback logic
- `/workspace/rust-sdk/examples/servers/src/simple_auth_streamhttp.rs`
  — streamable HTTP MCP server with Bearer auth (the `to_mcp` pattern)
- `/workspace/rust-sdk/examples/clients/src/streamable_http.rs` —
  streamable HTTP MCP client (the `from_mcp` pattern)
- `/workspace/@alkdev/operations/src/from_mcp.ts` — TypeScript prior art
  (`createMCPClient`, `mapMCPContentBlocks`, the `MCPClientLoader`; the
  `structuredContent`-preferred-over-content-blocks logic)
- `/workspace/@alkdev/operations/docs/architecture/adapters.md` —
  TypeScript adapter architecture doc (the `from_mcp` `outputSchema`/
  `structuredContent` handling, the `MUTATION` tool-type decision)
- `docs/research/alknet-http-gateway-factoring/findings.md` — research
  on the shared dispatch spine between `to_mcp` and `to_openapi`
  (recommendation: thin shared struct, not a trait)