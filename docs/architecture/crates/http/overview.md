---
status: draft
last_updated: 2026-07-06
---

# alknet-http — Overview

The HTTP interface crate: serves inbound HTTP on standard ALPNs (with
WebSocket upgrade for browser bidirectional access) and hosts the
HTTP-backed call-protocol adapters. This document covers the crate's two
roles, its dependency edges, and the adapter location map. Component
details are in the sibling documents.

## What

`alknet-http` is the HTTP protocol handler for the ALPN-as-service
architecture. It serves two roles in one crate:

1. **HTTP server** — a `ProtocolHandler` (`HttpAdapter`) that accepts
   HTTP/2 and HTTP/1.1 connections on the standard IANA ALPNs (`h2`,
   `http/1.1`), plus WebSocket upgrade (for browser bidirectional
   access to the call protocol). It serves REST APIs, the
   `to_openapi`/`to_mcp` projections of local call-protocol operations,
   the `/healthz` operational endpoint, and the decoy surface for
   stealth mode. HTTP/3 + WebTransport (`h3` ALPN) is deferred per
   [ADR-044](../../decisions/044-defer-webtransport-browsers-use-websocket.md);
   the deferred handler design is at [webtransport.md](webtransport.md).
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
discovery, MCP tool exposure, and WebSocket for browser bidirectional
access to the call protocol. (WebTransport is deferred per ADR-044; the
deferred browser-streaming path is at [webtransport.md](webtransport.md).)

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
**WebSocket is the HTTP-family transport that restores the call
protocol's native bidirectionality for browsers** (ADR-044) — a WS
connection is a long-lived full-duplex channel over which either side
can send `call.requested` frames in either direction. WebTransport
(`h3`, deferred) would restore it via native multi-stream multiplexing;
WebSocket restores it via framed messages over one connection. The
"from/to" naming of the OpenAPI/MCP adapters should not be read as a
statement about the call protocol's directionality; it is a statement
about OpenAPI's and MCP's directionality.

## Dependencies

```
alknet-http
├── alknet-core     (ProtocolHandler, Connection, AuthContext, IdentityProvider, Capabilities)
├── alknet-call     (OperationAdapter, OperationSpec, Handler, HandlerRegistration,
│                    OperationRegistry, AdapterError, OperationProvenance)
├── axum            (HTTP server — Router, extractors, middleware, WebSocket upgrade)
├── reqwest         (HTTP client — from_openapi/from_mcp forwarding)
├── hyper           (HTTP/1.1 + HTTP/2 framing; axum is built on hyper)
├── yaml_serde      (YAML parse for from_openapi YAML input — ADR-051; maintained fork of serde_yaml)
└── rmcp            (MCP streamable HTTP — feature-gated behind `mcp`)
```

> **Note:** the `h3`/WebTransport dependency (`wtransport` or the
> hyperium `h3` stack) is **not** in the v1 dependency tree —
> `h3`/WebTransport is deferred per
> [ADR-044](../../decisions/044-defer-webtransport-browsers-use-websocket.md).
> The `h3` feature gate and its dependency are absent from the initial
> release; the browser bidirectional path uses WebSocket (native axum
> support, no new dependency). The deferred dependency analysis is
> recorded in ADR-044 §"Research note (for revival)" and
> [webtransport.md](webtransport.md) §"Research note".

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
| `http/1.1` | `HttpAdapter` | HTTP/1.1 over QUIC stream (+ WS upgrade) | Yes (WS upgrade for bidirectional) |
| `h2` | `HttpAdapter` | HTTP/2 over QUIC stream (+ WS upgrade) | Yes (WS upgrade for bidirectional) |
| `h3` | — (deferred) | HTTP/3 / WebTransport | Deferred per ADR-044 |

These are standard IANA ALPN strings, not `alknet/`-prefixed. Any HTTP
client connects without knowing about alknet — the TLS handshake
negotiates `h2` or `http/1.1` normally, and the `HttpAdapter` serves
HTTP. This is the stealth mapping (ADR-010): clients that don't offer
alknet ALPNs get the HTTP handler, just like port scanners in stealth
mode. A browser negotiates `h2` or `http/1.1` and upgrades to WebSocket
for the bidirectional call-protocol path (ADR-044).

The `HttpAdapter` registers for `http/1.1` and `h2`. The endpoint's
`HandlerRegistry` maps each ALPN to the same `HttpAdapter` instance;
the handler branches on `connection.remote_alpn()` to pick the right
framing. The `h3` ALPN is not registered in v1 (deferred per ADR-044).

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
├── HttpAdapter                  (axum server — inbound HTTP on h2/http1.1 + WS upgrade route)
├── [WS upgrade → native session] (hands the WS message stream to the shared Dispatcher —
│                                not an adapter; see websocket.md, ADR-048)
├── from_openapi                 (parse OpenAPI doc + reqwest forwarding handler)
├── to_openapi                   (generate OpenAPI doc from local registry)
├── from_mcp   (feature-gated)   (import remote MCP tools over streamable HTTP — reqwest)
├── to_mcp     (feature-gated)   (expose local ops as MCP tools over streamable HTTP — axum)
└── from_wss   (out of scope)    (future: import a remote alknet node's ops over WS —
                                 from_call-aligned, same-protocol; see websocket.md §"Future")
```

`alknet-call` never sees the HTTP client. The `from_openapi`/`from_mcp`
forwarding handlers are opaque `Arc<dyn Handler>` from the registry's
perspective. `alknet-call` stays lean; `alknet-http` owns both HTTP
directions.

## Feature Gates

```toml
[features]
default = ["h2", "http1"]   # the HTTP surface (incl. WebSocket upgrade for browsers)
mcp = ["dep:rmcp"]           # from_mcp / to_mcp (streamable HTTP only — ADR-037)
# h3 (HTTP/3 + WebTransport) is deferred per ADR-044 — not in the v1
# feature set. The browser bidirectional path uses WebSocket (native to
# axum, no feature gate). When WebTransport revives, the `h3` feature
# gate returns; see ADR-044 and webtransport.md.
```

- `h2` + `http1` (default): the `axum` + `hyper` HTTP/1.1 + HTTP/2
  server, including WebSocket upgrade for browser bidirectional access
  (ADR-044). This is the surface all clients — including browsers, via
  WS upgrade — use in v1.
- `mcp`: the `rmcp` dependency with streamable HTTP transport features
  only. Adds `from_mcp`/`to_mcp`. See [http-mcp.md](http-mcp.md) and
  [ADR-037](../../decisions/037-mcp-stdio-transport-exclusion.md).

A deployment that only needs the REST surface (no MCP) uses the default
features. A browser-facing hub also uses the default features — the
browser bidirectional path is WebSocket, native to axum, no `h3` feature
gate needed. A deployment that wants MCP tool import/export enables
`mcp`.

**`yaml_serde` is not feature-gated** — YAML OpenAPI is a first-class
input format (vast.ai publishes a YAML schema), not an edge case. A
deployment that imports a YAML-publishing provider must not have to
remember a feature flag (ADR-051 §3). The dependency is small (pure
Rust, no native code), consistent with the default-features philosophy.

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
  `http/1.1` (+ the WS upgrade route): how axum is run over a QUIC
  bidirectional stream, Bearer auth resolution, the `/healthz` raw route,
  stealth decoy, the HTTP-to-call dispatch (ADR-036/042/047), and the
  WS upgrade route (which hands off to the native call-protocol session).
- **[websocket.md](websocket.md)** — the WebSocket browser bidirectional
  path: native `EventEnvelope` call-protocol session (not the gateway
  shape, ADR-048), framing, dispatch via the shared `Dispatcher`,
  bidirectionality, connection-local Layer 2 overlay, the
  browsers-are-not-peers rationale, streaming (native `call.responded`,
  no SSE), and the deferred `from_wss` adapter.
- **[http-adapters.md](http-adapters.md)** — `from_openapi` (parse
  OpenAPI, build forwarding handlers with `reqwest`) and `to_openapi`
  (generate an OpenAPI doc from the registry's `External` operations).
  Error fidelity per ADR-023.
- **[http-mcp.md](http-mcp.md)** — `from_mcp`/`to_mcp` (feature-gated),
  streamable HTTP only (ADR-037), the rmcp integration.
- **[webtransport.md](webtransport.md)** — the deferred `h3` ALPN
  handler (HTTP/3 + WebTransport). **Deferred per ADR-044**; kept intact
  for revival when a concrete ALPN-stream-proxy use case arrives.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| HTTP-to-call operation mapping | [ADR-036](../../decisions/036-http-to-call-operation-mapping.md) | ~~Direct path mapping~~ — **routing superseded by ADR-047**; gateway `/call` is the sole invoke path; ADR-036's non-routing clauses survive (SSE, auth, `/healthz`, stealth, error mapping) |
| MCP stdio transport exclusion | [ADR-037](../../decisions/037-mcp-stdio-transport-exclusion.md) | Streamable HTTP only; stdio is not built (RCE vector) |
| Defer h3/WebTransport; browsers use WebSocket | [ADR-044](../../decisions/044-defer-webtransport-browsers-use-websocket.md) | `h3`/WebTransport deferred (scope, not hedging); browser bidirectional path uses WebSocket; ADR-038 superseded, ADR-040/043 parked |
| WebSocket carries the native session, not the gateway shape | [ADR-048](../../decisions/048-websocket-native-session-not-gateway.md) | WS is the native `EventEnvelope` session; the gateway endpoints are HTTP-only; discovery via `services/list`/`services/schema` as call-protocol ops; subscriptions as native `call.responded` events (no SSE) |
| HTTP server + client host colocated | [ADR-039](../../decisions/039-http-server-and-client-host-colocated.md) | One crate for server + adapters (shared HTTP deps, shared mapping) |
| ~~HTTP/3 + WebTransport first-class~~ | [ADR-038](../../decisions/038-http3-and-webtransport-as-first-class.md) | **Superseded by ADR-044** (anti-pattern correction stands; specific decision reversed) |
| ~~WebTransport ALPN-stream-proxy~~ | [ADR-040](../../decisions/040-webtransport-alpn-stream-proxy.md) | **Parked** per ADR-044; revives unchanged when WebTransport revives |
| `to_mcp` tool-gateway pattern | [ADR-041](../../decisions/041-mcp-tool-gateway-pattern.md) | 4 fixed gateway tools (search/schema/call/batch), not one tool per operation |
| `to_openapi` gateway pattern | [ADR-042](../../decisions/042-openapi-gateway-pattern.md), [ADR-047](../../decisions/047-remove-direct-call-http-surface.md) | 5 fixed gateway endpoints are the sole HTTP invoke path (no per-operation `POST /{service}/{op}`); per-caller AccessControl-filtered `/search` is the discovery |
| Assembly-layer custom HTTP routes | [ADR-046](../../decisions/046-assembly-layer-custom-http-routes.md) | `extra_routes: Option<Router>` at construction; deployments add raw HTTP endpoints (e.g., OAI-compatible proxy, or a REST-like per-operation projection) that coexist with the default surface; default surface takes precedence on collision |
| ~~WebTransport bidirectional ALPN substrate~~ | [ADR-043](../../decisions/043-webtransport-bidirectional-alpn-substrate.md) | **Parked** per ADR-044; §2/§3 transfer to WebSocket for v1; §4/§5 revive with WebTransport |
| `alknet-call` is protocol-foundation | [ADR-003](../../decisions/003-crate-decomposition.md) Am. 1 | `alknet-http` depends on `alknet-call` (types, not peer handler) |
| Bearer auth via `resolve_from_token` | [ADR-004](../../decisions/004-auth-as-shared-core.md) | HTTP handler credential source + resolution (settled) |
| Stealth mode = HTTP handler on standard ALPNs | [ADR-010](../../decisions/010-alpn-router-and-endpoint.md) | Decoy for unknown paths (settled) |
| Adapter-registered ops are `Internal` | [ADR-015](../../decisions/015-privilege-model-and-authority-context.md) | `from_openapi`/`from_mcp` produce `Internal` leaves (settled) |
| `OperationAdapter` trait is async | [ADR-017](../../decisions/017-call-protocol-client-and-adapter-contract.md) | HTTP adapters implement the async trait (settled) |
| `to_*` adapters are projections | [ADR-017](../../decisions/017-call-protocol-client-and-adapter-contract.md) | `to_openapi`/`to_mcp` consume the registry, don't produce entries (settled) |
| Error schema fidelity | [ADR-023](../../decisions/023-operation-error-schemas.md) | `from_openapi` maps HTTP status → `HTTP_<status>` codes; `to_openapi` projects back (settled) |
| Browsers require X.509 | [ADR-027](../../decisions/027-tls-identity-redesign-acme-rawkey-decoupling.md) | `h3`/WebTransport needs X.509 (settled; applies when WebTransport revives) |
| Browsers are not alknet peers | [ADR-034](../../decisions/034-outgoing-only-x509-and-three-peer-roles.md) §4 (amended by ADR-044 §5) | Browser over WS/WebTransport = bearer token, no `PeerId` (settled; rationale in ADR-044 §5) |

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
- **OQ-39** (resolved): `to_openapi` published-spec versioning —
  `info.version` semver tracks the gateway endpoint contract, not the
  operation set (ADR-045); per-caller operations discovered via
  `/search`.
- **OQ-40** (resolved): reqwest client config and connection pooling —
  `ClientWithMiddleware` + middleware stack (retry + Retry-After).

## References

- `docs/research/alknet-http/phase-0-findings.md` — Phase 0 research
- `docs/research/alknet-call-completion/gap-analysis.md` — adapter
  location map, no-env-vars invariant
- `/workspace/@alkdev/operations/src/from_openapi.ts`,
  `/workspace/@alkdev/operations/src/from_mcp.ts` — TypeScript prior art
  for the HTTP adapters (the SSE normalization, auth-header, and
  `createHTTPOperation` patterns)
- `/workspace/@alkdev/pubsub/src/event-target-websocket-client.ts`,
  `/workspace/@alkdev/pubsub/src/event-target-websocket-server.ts` —
  TypeScript prior art for the WebSocket browser path (the
  `EventEnvelope { type, id, payload }` over WS binary messages; the
  call protocol's envelope is a refined superset — see ADR-044
  §"Concrete prior art")
- `/workspace/rust-sdk/` — MCP Rust SDK (rmcp); streamable HTTP examples
- `/workspace/wtransport/` — pure-Rust WebTransport reference
  (read during research; not a dependency — see ADR-044 §"Research note"
  for why `wtransport` is probably not the right revival choice)