---
status: draft
last_updated: 2026-06-29
---

# alknet-http — Overview

The HTTP interface crate: serves inbound HTTP on standard ALPNs and hosts
the HTTP-backed call-protocol adapters. This document covers the crate's
two roles, its dependency edges, and the adapter location map. Component
details are in the sibling documents.

## What

`alknet-http` is the HTTP protocol handler for the ALPN-as-service
architecture. It serves two roles in one crate:

1. **HTTP server** — a `ProtocolHandler` (`HttpAdapter`) that accepts
   HTTP/2, HTTP/1.1, and HTTP/3 (WebTransport) connections on the
   standard IANA ALPNs (`h2`, `http/1.1`, `h3`). It serves REST APIs, the
   `to_openapi`/`to_mcp` projections of local call-protocol operations,
   the `/healthz` operational endpoint, and the decoy surface for
   stealth mode.
2. **HTTP client host** — the home of the HTTP-transport-backed call
   adapters: `from_openapi` (import external HTTP APIs as call
   operations, using `reqwest` for outbound calls) and `from_mcp` (import
   remote MCP tools over streamable HTTP, using `reqwest`). The reverse
   projections `to_openapi` (generate an OpenAPI doc from the local
   registry's `External` operations) and `to_mcp` (expose local ops as
   MCP tools over streamable HTTP, using `axum`) also live here.

Both directions share the same HTTP dependencies (`axum` for serving,
`reqwest` for calling out), which is why they live in one crate rather
than being split into a server crate and a client crate. See
[ADR-039](../../decisions/039-http-server-and-client-host-colocated.md)
for the full rationale.

## Why

The crate's purpose is to be the HTTP interface library for downstream
crates that need to expose an HTTP interface. A downstream consumer (the
CLI binary, a hub deployment, a browser-facing service) wires
`HttpAdapter` into the `HandlerRegistry` for the standard HTTP ALPNs and
gets a full HTTP surface: REST projection of the call protocol, OpenAPI
discovery, MCP tool exposure, and WebTransport for browsers.

The key architectural insight that shapes the crate: **HTTP is both a
server surface and a client transport for adapters.** The server side
serves HTTP to external clients (browsers, curl, axios); the client side
makes outbound HTTP calls to external APIs (OpenAI, Anthropic, vast.ai)
through the `from_openapi`/`from_mcp` forwarding handlers. Both
directions share HTTP dependencies and HTTP-specific concerns (TLS,
headers, streaming, SSE), so they belong in one crate. See
[ADR-039](../../decisions/039-http-server-and-client-host-colocated.md)
for the colocation decision.

A note on the "from/to" direction model: the `from_openapi`/`to_openapi`
and `from_mcp`/`to_mcp` adapters are *inherently directional* because
OpenAPI and MCP are client/server protocols — one side serves, the
other calls. That directionality is a property of those protocols, not
of the call protocol itself. The call protocol is bidirectional (see
[../call/call-protocol.md](../call/call-protocol.md) §"Bidirectional
Calls": both sides can initiate calls). The HTTP/1.1 + HTTP/2 surface
inherits HTTP's request/response constraint and projects the call
protocol one-directionally (client→server calls only — see
[http-server.md](http-server.md) §"One-directional projection").
WebTransport (`h3`) is the HTTP-family transport that restores the
call protocol's native bidirectionality — it is a transport substrate
for the call protocol (and, via the ALPN-stream-proxy, for any ALPN),
not a browser→hub one-way path. See [webtransport.md](webtransport.md)
and ADR-043. The "from/to" naming of the OpenAPI/MCP adapters should not
be read as a statement about the call protocol's directionality; it is
a statement about OpenAPI's and MCP's directionality.

## Dependencies

```
alknet-http
├── alknet-core     (ProtocolHandler, Connection, AuthContext, IdentityProvider, Capabilities)
├── alknet-call     (OperationAdapter, OperationSpec, Handler, HandlerRegistration,
│                    OperationRegistry, AdapterError, OperationProvenance)
├── axum            (HTTP server — Router, extractors, middleware)
├── reqwest         (HTTP client — from_openapi/from_mcp forwarding)
├── hyper           (HTTP/1.1 + HTTP/2 framing; axum is built on hyper)
├── wtransport      (HTTP/3 + WebTransport — feature-gated behind `h3`)
└── rmcp            (MCP streamable HTTP — feature-gated behind `mcp`)
```

### The `alknet-call` dependency (ADR-003 Amendment 1)

`alknet-http` depends on `alknet-call`. ADR-003's rule is "no handler
crate depends on another handler crate," but `alknet-call` is both a
handler (it implements `ProtocolHandler` on `alknet/call`) *and* the
protocol-foundation crate that `alknet-agent`, `alknet-napi`, and now
`alknet-http` consume. `alknet-http` depending on `alknet-call` is
"HTTP uses the call protocol types" (`OperationSpec`, `Handler`,
`HandlerRegistration`, `OperationAdapter`), not "HTTP depends on SSH."
See [ADR-003 Amendment 1](../../decisions/003-crate-decomposition.md).

`alknet-call` stays lean — it has no `reqwest`, no `axum`, no HTTP
dependencies. The `from_openapi`/`from_mcp` forwarding handlers are
opaque `Arc<dyn Handler>` from the registry's perspective: constructed by
`alknet_http::from_openapi()` at registration time, stored in
`HandlerRegistration`, dispatched by the `CallAdapter` which doesn't
know `reqwest` is involved.

## ALPNs

| ALPN | Handler | Transport | Browser? |
|------|---------|-----------|----------|
| `http/1.1` | `HttpAdapter` | HTTP/1.1 over QUIC stream | No |
| `h2` | `HttpAdapter` | HTTP/2 over QUIC stream | No |
| `h3` | `HttpAdapter` | HTTP/3 / WebTransport | Yes (X.509 required) |

These are standard IANA ALPN strings, not `alknet/`-prefixed. Any HTTP
client connects without knowing about alknet — the TLS handshake
negotiates `h2` or `http/1.1` normally, and the `HttpAdapter` serves
HTTP. This is the stealth mapping (ADR-010): clients that don't offer
alknet ALPNs get the HTTP handler, just like port scanners in stealth
mode.

The `HttpAdapter` registers for all three ALPNs (when the corresponding
features are enabled). The endpoint's `HandlerRegistry` maps each ALPN to
the same `HttpAdapter` instance; the handler branches on
`connection.remote_alpn()` to pick the right framing.

## Adapter Location Map

The decomposition principle (settled in
[client-and-adapters.md](../call/client-and-adapters.md)): the adapter
trait lives where the types live (`alknet-call`); the adapter
implementations live where their transport dependencies live.

```
alknet-call (lean — no HTTP client, no HTTP server)
├── OperationAdapter trait       (the contract — async, ADR-017 §5)
├── from_call                    (QUIC — discovers remote ops via call protocol)
├── from_jsonschema              (pure parse — caller fetches the doc, passes it in)
└── CallClient                   (outbound connection opener)

alknet-http (owns HTTP server + HTTP client)
├── HttpAdapter                  (axum server — inbound HTTP on h2/http1.1/h3)
├── from_openapi                 (parse OpenAPI doc + reqwest forwarding handler)
├── to_openapi                   (generate OpenAPI doc from local registry)
├── from_mcp   (feature-gated)   (import remote MCP tools over streamable HTTP — reqwest)
└── to_mcp     (feature-gated)   (expose local ops as MCP tools over streamable HTTP — axum)
```

`alknet-call` never sees the HTTP client. The `from_openapi`/`from_mcp`
forwarding handlers are opaque `Arc<dyn Handler>` from the registry's
perspective. `alknet-call` stays lean; `alknet-http` owns both HTTP
directions.

## Feature Gates

```toml
[features]
default = ["h2", "http1"]   # the non-browser HTTP surface
h3 = ["dep:wtransport"]      # HTTP/3 + WebTransport (browser path; X.509 required)
mcp = ["dep:rmcp"]           # from_mcp / to_mcp (streamable HTTP only — ADR-037)
```

- `h2` + `http1` (default): the `axum` + `hyper` HTTP/1.1 + HTTP/2
  server. This is the surface non-browser clients use.
- `h3`: the `wtransport` (or quinn HTTP/3 extension) dependency. Adds
  the `h3` ALPN handler and the WebTransport streaming path. See
  [webtransport.md](webtransport.md) and
  [ADR-038](../../decisions/038-http3-and-webtransport-as-first-class.md).
- `mcp`: the `rmcp` dependency with streamable HTTP transport features
  only. Adds `from_mcp`/`to_mcp`. See [http-mcp.md](http-mcp.md) and
  [ADR-037](../../decisions/037-mcp-stdio-transport-exclusion.md).

A deployment that only needs the REST surface (no browsers, no MCP) uses
the default features. A browser-facing hub enables `h3`. A deployment
that wants MCP tool import/export enables `mcp`.

## The No-Env-Vars Invariant

The `from_openapi`/`from_mcp` forwarding handlers are the **credential
injection point** for the no-env-vars architecture. The path (from the
gap analysis):

```
vault → assembly layer → Capabilities → HandlerRegistration.capabilities
  → OperationContext.capabilities → from_openapi handler reads
    context.capabilities.get("openai") → injects into HTTP Authorization
    header → reqwest request goes out with vault-derived credential
```

This makes aisdk's `std::env::var("OPENAI_API_KEY")` reads unreachable —
the assembly layer never calls `Default::default()` on a provider; it
constructs them with vault-derived credentials, or routes HTTP calls
through `from_openapi` operations that carry the credential in
`Capabilities`.

**This is a spec-level invariant**: no handler reads outbound
credentials from any source other than `OperationContext.capabilities`.
The `from_openapi`/`from_mcp` implementations in `alknet-http` are
verified against this invariant. See ADR-014 and
[client-and-adapters.md](../call/client-and-adapters.md).

## Architecture (component pointers)

- **[http-server.md](http-server.md)** — the `HttpAdapter` for `h2`/
  `http/1.1`: how axum is run over a QUIC bidirectional stream, Bearer
  auth resolution, the `/healthz` raw route, stealth decoy, and the
  HTTP-to-call dispatch (ADR-036).
- **[http-adapters.md](http-adapters.md)** — `from_openapi` (parse
  OpenAPI, build forwarding handlers with `reqwest`) and `to_openapi`
  (generate an OpenAPI doc from the registry's `External` operations).
  Error fidelity per ADR-023.
- **[http-mcp.md](http-mcp.md)** — `from_mcp`/`to_mcp` (feature-gated),
  streamable HTTP only (ADR-037), the rmcp integration.
- **[webtransport.md](webtransport.md)** — the `h3` ALPN handler,
  WebTransport session/stream handling, the browser streaming path
  (ADR-038).

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| HTTP-to-call operation mapping | [ADR-036](../../decisions/036-http-to-call-operation-mapping.md) | Direct path mapping; `to_openapi` is projection, not router |
| MCP stdio transport exclusion | [ADR-037](../../decisions/037-mcp-stdio-transport-exclusion.md) | Streamable HTTP only; stdio is not built (RCE vector) |
| HTTP/3 + WebTransport first-class | [ADR-038](../../decisions/038-http3-and-webtransport-as-first-class.md) | `h3` in scope, not deferred; browser streaming uses QUIC streams |
| HTTP server + client host colocated | [ADR-039](../../decisions/039-http-server-and-client-host-colocated.md) | One crate for server + adapters (shared HTTP deps, shared mapping) |
| WebTransport ALPN-stream-proxy | [ADR-040](../../decisions/040-webtransport-alpn-stream-proxy.md) | The substrate's mechanism for non-call ALPNs (SSH, git, SFTP) — browser → WebTransport stream → target ALPN handler via WASM parser |
| `to_mcp` tool-gateway pattern | [ADR-041](../../decisions/041-mcp-tool-gateway-pattern.md) | 4 fixed gateway tools (search/schema/call/batch), not one tool per operation |
| `to_openapi` gateway pattern | [ADR-042](../../decisions/042-openapi-gateway-pattern.md) | 5 fixed gateway endpoints (search/schema/call/batch/subscribe); per-caller AccessControl-filtered |
| WebTransport bidirectional ALPN substrate | [ADR-043](../../decisions/043-webtransport-bidirectional-alpn-substrate.md) | WebTransport carries ALPNs as bidirectional streams; call protocol is the first target; both sides can initiate calls; non-peer clients use a connection-local overlay |
| `alknet-call` is protocol-foundation | [ADR-003](../../decisions/003-crate-decomposition.md) Am. 1 | `alknet-http` depends on `alknet-call` (types, not peer handler) |
| Bearer auth via `resolve_from_token` | [ADR-004](../../decisions/004-auth-as-shared-core.md) | HTTP handler credential source + resolution (settled) |
| Stealth mode = HTTP handler on standard ALPNs | [ADR-010](../../decisions/010-alpn-router-and-endpoint.md) | Decoy for unknown paths (settled) |
| Adapter-registered ops are `Internal` | [ADR-015](../../decisions/015-privilege-model-and-authority-context.md) | `from_openapi`/`from_mcp` produce `Internal` leaves (settled) |
| `OperationAdapter` trait is async | [ADR-017](../../decisions/017-call-protocol-client-and-adapter-contract.md) | HTTP adapters implement the async trait (settled) |
| `to_*` adapters are projections | [ADR-017](../../decisions/017-call-protocol-client-and-adapter-contract.md) | `to_openapi`/`to_mcp` consume the registry, don't produce entries (settled) |
| Error schema fidelity | [ADR-023](../../decisions/023-operation-error-schemas.md) | `from_openapi` maps HTTP status → `HTTP_<status>` codes; `to_openapi` projects back (settled) |
| Browsers require X.509 | [ADR-027](../../decisions/027-tls-identity-redesign-acme-rawkey-decoupling.md) | `h3`/WebTransport needs X.509 (settled) |
| Browsers are not alknet peers | [ADR-034](../../decisions/034-outgoing-only-x509-and-three-peer-roles.md) | Browser over WebTransport/HTTPS = bearer token, no `PeerId` (settled) |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-13** (resolved): Operation path format `/{service}/{op}` — the
  HTTP path.
- **OQ-26** (resolved): `AdapterError` variants — reused by HTTP
  adapters; `#[non_exhaustive]` allows extension.
- **OQ-37** (resolved): Browsers are not peers; `h3` hub is a
  mixed-fingerprint `PeerEntry`.
- **OQ-38** (open, scope): WebTransport relay-as-proxy — does the proxy
  live in `alknet-http` or a separate relay crate?
- **OQ-39** (open): `to_openapi` published-spec versioning — versioning
  strategy for generated OpenAPI specs.
- **OQ-40** (open): reqwest client config and connection pooling —
  two-way-door config shape.

## References

- `docs/research/alknet-http/phase-0-findings.md` — Phase 0 research
- `docs/research/alknet-call-completion/gap-analysis.md` — adapter
  location map, no-env-vars invariant
- `/workspace/@alkdev/operations/src/from_openapi.ts`,
  `/workspace/@alkdev/operations/src/from_mcp.ts` — TypeScript prior art
- `/workspace/rust-sdk/` — MCP Rust SDK (rmcp); streamable HTTP examples
- `/workspace/wtransport/` — pure-Rust WebTransport reference