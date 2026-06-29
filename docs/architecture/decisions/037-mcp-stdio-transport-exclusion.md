# ADR-037: MCP Stdio Transport Exclusion

## Status

Proposed

## Context

The Model Context Protocol (MCP) defines multiple transports for
communicating between an MCP client and an MCP server. The MCP Rust SDK
(`rmcp` at `/workspace/rust-sdk/`) implements two:

1. **Streamable HTTP** (`transport-streamable-http-client-reqwest` for
   clients, `transport-streamable-http-server` for servers). The client
   connects to an HTTP endpoint; the server serves an HTTP endpoint.
   Network-isolated, auth-gatable (Bearer token middleware, per the rmcp
   `simple_auth_streamhttp.rs` example), and runs under whatever auth/
   identity/capabilities machinery the host applies to HTTP.

2. **stdio** (`transport-child-process`). The client spawns the MCP server
   as a child process and pipes JSON-RPC over its stdin/stdout. This is
   the model the MCP spec promotes for "just download an MCP server and
   run it locally."

The alknet-http crate implements `from_mcp` (import remote MCP tools as
call-protocol operations) and `to_mcp` (expose local operations as MCP
tools). Both are feature-gated behind an `mcp` feature (the rmcp
dependency is optional). The question this ADR resolves is which MCP
transports alknet-http supports.

### The stdio security problem

MCP stdio transport is `transport-child-process` — the rmcp client calls
`StdioClientTransport { command, args, env, cwd }`, which spawns an
arbitrary executable and pipes JSON-RPC over its stdin/stdout. An MCP
server is an arbitrary program that the MCP client executes with whatever
privileges the client process has.

The "download untrusted MCP servers and run them via stdio" model is
indistinguishable from `curl | sh` with extra steps:

- **Arbitrary code execution.** The MCP server is an executable. Running
  it is RCE. There is no sandbox — the child process has the full
  privileges of the client process (filesystem, network, environment
  variables, ability to spawn further processes).
- **No auth boundary.** The MCP protocol messages flow over stdin/stdout;
  there is no TLS, no auth token, no identity resolution. The server is
  trusted by construction (you spawned it).
- **The "download untrusted MCP server" UX.** The MCP ecosystem's
  promoted workflow is: find an MCP server on a registry, install it,
  point your client at it. This is the npm-without-the-checksums model,
  but the "package" is a process with full local privileges, not a
  library that runs in-process.

alknet's security posture is the opposite of this. alknet-vault is
local-only by construction (ADR-025); the no-env-vars invariant
(ADR-014, client-and-adapters.md) exists specifically to avoid the
"download untrusted code that reads your secrets" pattern; capabilities
are injected by the assembly layer, not read from the environment a
spawned process can inspect. Building stdio support into alknet-http
would import the exact RCE vector the rest of the architecture is
designed to avoid.

## Decision

**alknet-http supports only streamable HTTP for MCP. Stdio is not built.**

The `mcp` feature gate pulls in rmcp with the streamable HTTP transport
features only:

```toml
[features]
mcp = [
    "dep:rmcp",
    # rmcp client transport (for from_mcp) — streamable HTTP only
    # rmcp server transport (for to_mcp) — streamable HTTP only
]
```

The stdio transport (`transport-child-process`) is explicitly **not** a
dependency and **not** feature-gated. It is not built, not optional, not
"behind a separate feature." alknet-http's `from_mcp` uses rmcp's
`StreamableHttpClientTransport` (reqwest-based); `to_mcp` uses rmcp's
`StreamableHttpService` (axum-based, a tower service that nests into an
axum `Router` — see the rmcp `simple_auth_streamhttp.rs:134-159`
example). No stdio code path exists in the crate.

### If someone wants stdio MCP

They run it themselves, outside alknet. An operator who wants to use a
stdio-only MCP server can spawn it as a subprocess, run a small
streamable-HTTP-to-stdio bridge, and point `from_mcp` at the bridge's
HTTP endpoint. That bridge is the operator's responsibility — alknet
does not ship it, does not endorse it, and the bridge is where the RCE
risk lives, explicitly in the operator's hands, not hidden behind an
alknet feature flag.

This is the same posture as ADR-025 (remote vault access requires a
separate crate with its own ADR and threat model): the dangerous thing
is not built by default; if someone wants it, they build it themselves
and own the security model.

## Consequences

**Positive:**
- alknet-http does not import the MCP stdio RCE vector. There is no code
  path in alknet-http that spawns an arbitrary executable.
- The streamable HTTP path is network-isolated, auth-gatable (Bearer
  middleware), and runs under alknet's auth/identity/capabilities
  machinery — the same machinery that gates every other HTTP request.
- `from_mcp` operations (imported MCP tools) are `Internal` by default
  (ADR-015, ADR-022) — composition material, not directly callable from
  the wire. The MCP server is reached over HTTP with a Bearer token from
  `Capabilities` (the no-env-vars invariant), not by spawning a process
  that could read the environment.
- `to_mcp` (expose local ops as MCP tools) serves an axum route with
  Bearer auth middleware, matching the rmcp
  `simple_auth_streamhttp.rs` pattern. An external MCP client (an
  editor, an AI tool) discovers and calls alknet operations through
  streamable HTTP, with alknet's auth/identity model applied at the
  HTTP boundary.

**Negative:**
- MCP servers that only support stdio (a significant fraction of the
  current MCP ecosystem) cannot be consumed by `from_mcp` directly. The
  operator runs a bridge (above). This is a deliberate exclusion, not a
  feature gap.
- The "just download an MCP server and run it" UX that the MCP
  ecosystem promotes is not supported. An alknet user who wants that UX
  has to build the bridge and own the RCE risk. This is the correct
  tradeoff for alknet's threat model, but it means alknet-http is not a
  drop-in client for the stdio MCP ecosystem.

## Assumptions

1. **Streamable HTTP is the supported MCP transport in alknet.** This
   is a one-way door: removing stdio support later (if it were ever
   added) would break deployments that depend on it; not adding it is
   the stable position. The streamable HTTP transport is the MCP
   spec's network-isolated path and is what the rmcp examples use for
   auth-gated servers.

2. **The MCP ecosystem's stdio UX is not a target.** alknet is not trying
   to be a drop-in client for "download untrusted MCP servers." If a
   user wants that, the bridge approach puts the RCE risk explicitly
   in their hands.

3. **rmcp's streamable HTTP features are the right subset.** The
   `mcp` feature gate pulls in `transport-streamable-http-client-reqwest`
   (for `from_mcp`) and `transport-streamable-http-server` (for
   `to_mcp`). The exact rmcp feature names are a two-way-door
   implementation detail (rmcp may rename features across versions); the
   one-way constraint is "streamable HTTP only, no stdio."

## References

- [ADR-014](014-secret-material-flow-and-capability-injection.md) — the
  no-env-vars invariant; spawned processes reading env vars is the
  pattern this ADR's exclusion prevents
- [ADR-015](015-privilege-model-and-authority-context.md) —
  adapter-registered ops (`from_mcp`) are `Internal` by default
- [ADR-022](022-handler-registration-provenance-and-composition-authority.md)
  — `from_mcp` provenance is a leaf
- [ADR-025](025-vault-local-only-dispatch.md) — the analogous "dangerous
  thing is not built by default; a separate crate with its own ADR" pattern
- `docs/research/alknet-http/phase-0-findings.md` §4 (MCP stdio exclusion)
- `/workspace/rust-sdk/` — MCP Rust SDK (rmcp v1.8.0); streamable HTTP
  transport
- `/workspace/rust-sdk/examples/servers/src/simple_auth_streamhttp.rs` —
  streamable HTTP MCP server with Bearer auth (the `to_mcp` pattern)
- `/workspace/rust-sdk/examples/clients/src/streamable_http.rs` —
  streamable HTTP MCP client (the `from_mcp` pattern)
- `crates/http/http-mcp.md` — the spec that implements `from_mcp`/`to_mcp`