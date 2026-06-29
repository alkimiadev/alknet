# ADR-039: HTTP Server and Client Host Colocated in alknet-http

## Status

Proposed

## Context

`alknet-http` has two roles: an HTTP server (the `HttpAdapter`
`ProtocolHandler` for `h2`/`http/1.1`/`h3`, built on `axum`/`hyper`)
and an HTTP client host (the `from_openapi`/`from_mcp` forwarding
handlers, built on `reqwest`). The question is whether these two
directions live in one crate (`alknet-http`) or are split into two
crates (`alknet-http-server` + `alknet-http-client`).

ADR-003 lists `alknet-http` as a single crate with dependency
`alknet-core, axum` and justifies the per-handler-crate decomposition
with "each handler is self-contained — it receives a byte stream and
manages its own protocol." That rationale covers the server side (the
`HttpAdapter` is self-contained), but it does not address the
within-crate dual-role question: should the inbound HTTP server and
the outbound HTTP client (the adapter forwarding handlers) be
colocated, or split?

This is a load-bearing choice. Once published, downstream consumers
build import paths against the crate boundary; the shared `reqwest::Client`
and the no-env-vars invariant boundary (ADR-014) are scoped by it; the
`to_openapi`/`to_mcp` projections are pure-registry-consumers that
*describe* the server surface but live where the adapter types do.
Splitting later would be a rewrite of every consumer's import paths,
not a cheap revert. It needs an ADR.

## Decision

**One crate — `alknet-http` houses both the HTTP server and the HTTP
client host (the adapter forwarding handlers and the `to_*` projections).**

The two directions share the HTTP dependencies and HTTP-specific
concerns that make splitting them counterproductive:

- **Shared HTTP dependencies.** Both `axum` (server) and `reqwest`
  (client) pull in `hyper`, `http`, `http-body`, `rustls`/TLS stack
  types, and the HTTP header/status code types. A split into two crates
  would either duplicate these dependencies across both crates or
  force a third shared-types crate, neither of which is an improvement.
- **Shared HTTP-specific concerns.** Both directions care about HTTP
  headers, status codes, content types, SSE framing, streaming vs
  non-streaming bodies, and TLS trust stores. The `from_openapi`
  forwarding handler's error mapping (HTTP status → `HTTP_<status>`
  error codes, ADR-023) and the `to_openapi` projection's error mapping
  (`ErrorDefinition.http_status` → HTTP response status) are *the same
  mapping* read in two directions — splitting them would put the two
  halves in different crates.
- **The `to_*` projections describe the server surface.** `to_openapi`
  generates an OpenAPI doc whose paths mirror the `/{service}/{op}`
  HTTP routes the `HttpAdapter` serves (ADR-036). `to_mcp` exposes the
  same operations as MCP tools. These projections consume the
  `OperationRegistry` and produce specs; they live with the adapter
  types (in `alknet-http`, per the adapter location map — see
  [client-and-adapters.md](../crates/call/client-and-adapters.md))
  because they share the operation-spec→HTTP mapping logic with the
  server's request dispatch.
- **The no-env-vars invariant boundary is crate-scoped.** The
  `from_openapi`/`from_mcp` forwarding handlers are the credential
  injection point (ADR-014). The invariant — "no handler reads outbound
  credentials from any source other than `OperationContext.capabilities`"
  — is verified against the handler implementations in this crate. A
  split would put the invariant verification boundary across two crates.

### What this does NOT change

- ADR-003's rule "no handler crate depends on another handler crate"
  applies to peer handler crates (`alknet-http` does not depend on
  `alknet-ssh`). The `alknet-http` → `alknet-call` edge is the
  protocol-foundation exception (ADR-003 Amendment 1). This ADR is
  about the *internal* structure of `alknet-http`, not its dependency
  edges.
- The adapter location map (the `OperationAdapter` trait in
  `alknet-call`; the HTTP-backed adapter implementations in
  `alknet-http`) is unchanged. This ADR records *why* the HTTP-backed
  adapters live in the same crate as the HTTP server, not whether they
  live in `alknet-http` vs `alknet-call`.

## Consequences

**Positive:**
- One crate, one set of HTTP dependencies, one HTTP-specific concern
  surface. No duplicated `hyper`/`http` types across two crates, no
  shared-types crate needed.
- The `to_*` projections live with the server whose surface they
  describe, and with the adapter types they consume. The operation-spec
  → HTTP mapping logic is in one place.
- The no-env-vars invariant verification boundary is one crate. The
  `from_openapi`/`from_mcp` handlers and the credential injection
  logic they share are co-located.
- A downstream consumer wires one crate (`alknet-http`) into the
  `HandlerRegistry` and gets the full HTTP surface — server + adapters +
  projections. No two-crate wiring.

**Negative:**
- A deployment that only needs the HTTP server (no `from_openapi`/`from_
  mcp` forwarding) still compiles the `reqwest` dependency. Mitigated:
  the `mcp` feature is already gated (ADR-037); the `from_openapi`
  forwarding is always available but the `reqwest` client is only
  constructed if a `from_openapi`/`from_mcp` adapter is registered at
  assembly time. The dependency is compiled, the client is lazy.
- A deployment that only needs the HTTP client (e.g., an agent crate
  that only uses `from_openapi` forwarding, no inbound HTTP) still
  compiles `axum`/`hyper`. This is the rarer case — the agent crate
  (`alknet-agent`) consumes `alknet-call` directly for tool dispatch
  and uses `from_openapi` via `alknet-http`'s adapter, but doesn't
  serve inbound HTTP itself. In practice, the agent deployment wires
  `alknet-http` for the adapters and the CLI wires it for the server;
  the compile cost is paid once per workspace, not once per deployment.
- The crate is larger than a single-direction crate would be. This is
  the cost of colocating shared concerns; the alternative (two crates
  + a shared types crate) is more crates, not less code.

## Assumptions

1. **The shared-HTTP-dependencies argument holds.** `axum` and
   `reqwest` both pull in `hyper` and the `http` crate's types; the
   shared types (headers, status codes, method, URI) are the same. If
   a future version of `axum` or `reqwest` diverges its HTTP types
   (e.g., `axum` moves to a different HTTP implementation), this
   argument weakens. As of `axum` 0.7+ and `reqwest` 0.12+, both are
   built on `hyper` 1.x and share `http` types.

2. **The `to_*` projections share enough mapping logic with the server
   to justify colocation.** The operation-spec → HTTP path/method/
   error-status mapping is the same in both directions. If the
   projections turn out to be pure registry-consumers with no
   HTTP-mapping logic (just spec serialization), the colocation
   argument is weaker — but the current design (ADR-036, ADR-023)
   has them sharing the mapping.

## References

- [ADR-003](003-crate-decomposition.md) — crate decomposition (this
  ADR addresses the within-`alknet-http` dual-role question, not the
  dependency edge; Amendment 1 covers the `alknet-call` edge)
- [ADR-014](014-secret-material-flow-and-capability-injection.md) —
  the no-env-vars invariant whose verification boundary is crate-scoped
- [ADR-017](017-call-protocol-client-and-adapter-contract.md) — the
  adapter contract; `to_*` are projections
- [ADR-023](023-operation-error-schemas.md) — the error mapping shared
  between `from_openapi` (status → code) and `to_openapi` (code →
  status)
- [ADR-036](036-http-to-call-operation-mapping.md) — the HTTP path =
  operation path mapping shared between server dispatch and `to_openapi`
- [ADR-037](037-mcp-stdio-transport-exclusion.md) — the `mcp` feature
  gate
- `crates/http/overview.md` — the crate overview (the inline rationale
  for this decision is replaced by a pointer to this ADR)