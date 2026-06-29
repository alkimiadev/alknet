# ADR-041: MCP Tool-Gateway Pattern for to_mcp

## Status

Proposed

## Context

The current `to_mcp` spec (`crates/http/http-mcp.md`) describes
`to_mcp` as "exposes the local registry's `External` operations as MCP
tools" — one MCP tool per alknet operation. An LLM connecting to an
alknet node with 200 registered operations gets 200 MCP tools dumped
into its context. This is the **tool-bloat problem**: the LLM's context
is bloated with tools that are irrelevant to the current task, degrading
its reasoning and wasting context budget.

### The problem in concrete terms

The MCP `tools/list` response returns every tool the server exposes.
An MCP client (an editor, an AI tool) loads all of them into the LLM's
context as tool definitions. An alknet node exposing 200 operations
produces a `tools/list` response with 200 `Tool` structs, each with a
name, description, and `inputSchema` (JSON Schema). The LLM sees 200
tool definitions whether it needs them or not. This is the same anti-
pattern as loading every man page into a shell's environment —
absurd, but it's what the naive one-tool-per-operation mapping produces.

### The pattern that works

The project already has two examples of a better pattern:

1. **The `memory` tool** (opencode): read-only access to the underlying
   session database. The LLM doesn't load all past sessions into
   context — it calls `memory` with a search query when it needs to
   recall something. One tool, access to a large dataset on demand.
2. **The `worktree` tool** (opencode): gates 8-10 sub-tools behind a
   single `worktree` entry point. The LLM has one tool in context; the
   sub-tools are discovered and invoked through it.

The general principle (same as Linux's `man` command): **don't load all
documentation/tools into context 24/7; expose a small fixed set of
meta-tools that gate access to the full set on demand.**

### The call protocol's discovery surface

The call protocol already has the discovery primitives that make this
work:

- `services/list` — lists registered operations (filtered by
  `AccessControl`).
- `services/schema` — returns an operation's `OperationSpec`
  (input/output JSON Schemas, error schemas).

The `to_mcp` gateway exposes these primitives (plus invocation) as a
small fixed set of MCP tools. The LLM searches for what it needs, learns
the schema, then calls — instead of having every operation pre-loaded.

## Decision

### 1. `to_mcp` exposes a fixed gateway tool set, not one tool per operation

`to_mcp` exposes a small fixed set of MCP tools that gate access to the
full operation registry. The LLM has a few tools in context (not
hundreds); it discovers and invokes operations through the gateway.

The gateway tool set (initial, two-way-door extensible):

| MCP tool | Call protocol operation | Purpose |
|----------|------------------------|---------|
| `search` | `services/list` | List/search available operations (filtered by the caller's `AccessControl`). The LLM discovers what it can call. |
| `schema` | `services/schema` | Get an operation's `OperationSpec` (input/output JSON Schemas, error schemas). The LLM learns how to call a specific operation. |
| `call` | `call.requested` (Query/Mutation) | Invoke an operation by name with a JSON input. Returns the operation's output (or a typed error per ADR-023). |
| `batch` | multiple `call.requested` | Invoke multiple operations in one tool call (correlated request IDs, OQ-14). The LLM batches independent calls. |

Four tools. The LLM calls `search` to find operations relevant to its
task, `schema` to learn the input shape, `call` to invoke. Same pattern
as `man <command>` — discover on demand, don't preload.

### 2. `Subscription` operations are excluded from the MCP gateway

MCP tool calls are request/response — an LLM invokes a tool and
receives a result. The call protocol's `Subscription` type
(streaming, many `call.responded` events) does not map onto the MCP
tool-call model. The gateway exposes only `Query` and `Mutation`
operations (request/response). `Subscription` operations are filtered
out of `search` results and cannot be invoked via `call`.

This is a deliberate scoping decision, not a deferral: MCP tool calls
are request/response by protocol design; streaming subscriptions are a
different interaction model that doesn't fit the LLM tool-call pattern.
If a future MCP extension adds streaming tool calls, the gateway could
expose `Subscription` operations through it — but that's a future MCP
spec question, not an alknet decision.

### 3. `search` returns names + descriptions, not full schemas

The `search` tool (backed by `services/list`) returns operation names,
namespaces, types, and short descriptions — not the full input/output
JSON Schemas. This keeps the search result small (the LLM is choosing
what to call, not how to call it yet). The LLM calls `schema` for the
specific operation it wants to invoke, getting the full `OperationSpec`
only when needed. Two-step discovery: search (cheap, list) → schema
(targeted, full spec).

### 4. `call` maps to the call protocol's request/response dispatch

The `call` tool takes `{ operation: "/fs/readFile", input: { ... } }`
and dispatches through the `OperationRegistry::invoke()` — the same
dispatch path the HTTP server uses (ADR-036). The result is mapped to
an MCP `CallToolResult` (`structuredContent` for the output, or
`isError: true` for a `CallError` with the typed `details` payload per
ADR-023). The `batch` tool takes an array of `{ operation, input }`
pairs and returns an array of results.

### 5. `AccessControl` gates the gateway

The `search` tool's results are filtered by the caller's
`AccessControl::check(identity)` — the LLM (authenticated by bearer
token, ADR-034 §4) sees only the operations it is authorized to call.
The `call` tool's dispatch runs the same `AccessControl` check. An
LLM that calls `call` with an operation it isn't authorized for gets
`FORBIDDEN` (mapped to an MCP error result). The gateway does not
bypass the call protocol's authorization — it's the same dispatch
path, just reached through an MCP tool call instead of an HTTP request.

## Consequences

**Positive:**
- The LLM has 4 tools in context, not hundreds. Context budget is
  preserved for the actual task; the LLM discovers operations on
  demand through `search` + `schema`. This is the same pattern that
  makes the `memory` and `worktree` tools effective.
- The gateway maps onto the call protocol's existing discovery
  primitives (`services/list`, `services/schema`) and dispatch
  (`OperationRegistry::invoke`). No new call-protocol mechanisms
  needed — `to_mcp` is a thin wrapper around the existing surface.
- `AccessControl` gates the gateway. An LLM sees only what it's
  authorized to call; the gateway doesn't leak operation existence or
  schemas to unauthorized callers.
- `Subscription` exclusion is explicit. The LLM tool-call model is
  request/response; streaming doesn't fit, and pretending it does
  would produce a broken mapping.

**Negative:**
- The LLM needs two round-trips to call an operation it hasn't seen
  before (`search` → `schema` → `call`). A one-tool-per-operation
  mapping would let it call directly. The tradeoff: 4 tools in context
  + 2 discovery round-trips vs. 200 tools in context + 0 round-trips.
  The context budget is the scarcer resource; the round-trips are
  cheap (the MCP server is local or nearby).
- The `search` tool's result format (names + descriptions, not full
  schemas) means the LLM may need to call `schema` for multiple
  operations before finding the right one. Mitigated: `search` can
  accept a query/filter (namespace, keyword) to narrow results.
- The gateway tool set is fixed (4 tools). An operation that wants a
  custom MCP tool (e.g., a specialized `git_clone` tool with a curated
  input schema, not the generic `call` wrapper) is not exposed through
  the gateway. A future "custom tool" extension could allow operations
  to declare an MCP tool projection — but the gateway pattern is the
  default, and the custom-tool path is additive (not a replacement).

## Assumptions

1. **The LLM context budget is the scarcer resource.** The tradeoff
   favoring 4 tools + discovery round-trips over 200 preloaded tools
   assumes the LLM's context window is more valuable than the network
   round-trips. This holds for current LLMs (context windows are
   large but not unlimited; tool definitions consume context
   proportionally to their schemas).

2. **`Query` and `Mutation` cover the LLM tool-call use case.**
   LLMs invoke tools in a request/response pattern: call a tool,
   receive a result, reason about it. Streaming subscriptions
   (`call.responded` events over time) don't fit this pattern — the
   LLM expects one result per tool call. The assumption is that the
   operations an LLM wants to call are `Query`/`Mutation`, not
   `Subscription`.

3. **The gateway tool set is stable.** Once LLM clients build
   prompts/workflows against the `search`/`schema`/`call`/`batch`
   tool set, changing the tool surface (renaming, removing) breaks
   them. Adding tools is additive (non-breaking); removing or renaming
   is a one-way door. The initial 4-tool set is the published contract.

4. **`AccessControl` filtering is sufficient for `search`.** The LLM
   sees the operations it's authorized to call. If an operation's
   existence is itself sensitive (the LLM shouldn't know it exists
   even if it can't call it), `Visibility::Internal` (ADR-015) is the
   mechanism — Internal ops are excluded from `services/list` and
   therefore from `search` results. The gateway does not add a
   separate visibility layer.

## References

- [ADR-015](015-privilege-model-and-authority-context.md) —
  External/Internal visibility (Internal ops excluded from
  `services/list`, therefore from `search`)
- [ADR-017](017-call-protocol-client-and-adapter-contract.md) —
  `to_*` adapters are projections (consume the registry, don't
  produce entries)
- [ADR-023](023-operation-error-schemas.md) — typed error `details`
  mapped to MCP error results
- [ADR-034](034-outgoing-only-x509-and-three-peer-roles.md) §4 —
  browsers/MCP clients are not alknet peers (bearer token, no
  `PeerId`)
- [ADR-036](036-http-to-call-operation-mapping.md) — the HTTP-to-call
  dispatch path the `call` tool reuses
- [ADR-037](037-mcp-stdio-transport-exclusion.md) — streamable HTTP
  only (the transport `to_mcp` uses)
- `crates/http/http-mcp.md` — the spec that implements the gateway
- `/workspace/rust-sdk/crates/rmcp/src/model/tool.rs` — the MCP
  `Tool` struct (name, description, input_schema, output_schema)
- `/workspace/rust-sdk/crates/rmcp/src/handler/server.rs` —
  `list_tools` / `call_tool` server trait (the interface `to_mcp`
  implements)