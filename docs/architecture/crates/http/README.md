---
status: draft
last_updated: 2026-06-30
---

# alknet-http

HTTP interface for alknet: serves HTTP/1.1 and HTTP/2 on standard ALPNs
(with WebSocket upgrade for browser bidirectional access to the call
protocol), and hosts the HTTP-backed call-protocol adapters
(`from_openapi`, `to_openapi`, `from_mcp`, `to_mcp`). HTTP/3 + WebTransport
(`h3`) is deferred per
[ADR-044](../../decisions/044-defer-webtransport-browsers-use-websocket.md).

## Documents

| Document | Status | Description |
|----------|--------|-------------|
| [overview.md](overview.md) | draft | Crate purpose, two roles (server + client host), dependencies, adapter location map |
| [http-server.md](http-server.md) | draft | `HttpAdapter` (`ProtocolHandler` for `h2`/`http/1.1` + WS upgrade), axum over QUIC, Bearer auth, stealth, `/healthz`, WebSocket browser path |
| [http-adapters.md](http-adapters.md) | draft | `from_openapi` (reqwest client) and `to_openapi` (OpenAPI projection); no-env-vars invariant point |
| [http-mcp.md](http-mcp.md) | draft | `from_mcp` / `to_mcp` (feature-gated), streamable-HTTP-only, stdio exclusion |
| [webtransport.md](webtransport.md) | deferred | `h3`/WebTransport handler — **deferred per ADR-044**; spec kept intact for revival |

## Applicable ADRs

| ADR | Title | Relevance |
|-----|-------|-----------|
| [001](../../decisions/001-alpn-protocol-dispatch.md) | ALPN-Based Protocol Dispatch | `HttpAdapter` registers on standard HTTP ALPNs |
| [002](../../decisions/002-protocol-handler-trait.md) | ProtocolHandler Trait | `HttpAdapter` implements `ProtocolHandler` |
| [003](../../decisions/003-crate-decomposition.md) | Crate Decomposition | `alknet-http` depends on `alknet-core` + `alknet-call` (protocol-foundation exception, Amendment 1) |
| [004](../../decisions/004-auth-as-shared-core.md) | Auth as Shared Core | Bearer → `resolve_from_token` |
| [007](../../decisions/007-bistream-type-definition.md) | BiStream Type Definition | `HttpAdapter` receives `Connection`, accepts a stream for hyper |
| [010](../../decisions/010-alpn-router-and-endpoint.md) | ALPN Router and Endpoint | Stealth mode = HTTP handler on standard ALPNs |
| [014](../../decisions/014-secret-material-flow-and-capability-injection.md) | Secret Material Flow | `from_openapi`/`from_mcp` are the credential injection point |
| [015](../../decisions/015-privilege-model-and-authority-context.md) | Privilege Model | Adapter-registered ops are `Internal` by default |
| [017](../../decisions/017-call-protocol-client-and-adapter-contract.md) | Call Protocol Client and Adapter Contract | `OperationAdapter` trait; `to_*` are projections; published-spec contract |
| [022](../../decisions/022-handler-registration-provenance-and-composition-authority.md) | Handler Registration, Provenance, Composition Authority | `from_openapi`/`from_mcp` produce leaf bundles |
| [023](../../decisions/023-operation-error-schemas.md) | Operation Error Schemas | `from_openapi`/`to_openapi` error fidelity; `HTTP_<status>` error codes |
| [027](../../decisions/027-tls-identity-redesign-acme-rawkey-decoupling.md) | TLS Identity Redesign | Browsers require X.509; applies to WebTransport (deferred) and any browser-facing TLS |
| [034](../../decisions/034-outgoing-only-x509-and-three-peer-roles.md) | Outgoing-Only X.509 and Three Peer Roles | Browsers are not alknet peers (§4 amended by ADR-044 §5 with the addressability rationale) |
| [036](../../decisions/036-http-to-call-operation-mapping.md) | HTTP-to-Call Operation Mapping | ~~Direct path mapping~~ — **routing superseded by ADR-047**; non-routing clauses survive (SSE projection, Bearer auth, `/healthz`, stealth, error mapping) |
| [037](../../decisions/037-mcp-stdio-transport-exclusion.md) | MCP Stdio Transport Exclusion | Streamable HTTP only; stdio not built |
| [038](../../decisions/038-http3-and-webtransport-as-first-class.md) | HTTP/3 and WebTransport as First-Class HTTP Transports | **Superseded by ADR-044** (anti-pattern correction stands; specific decision reversed) |
| [039](../../decisions/039-http-server-and-client-host-colocated.md) | HTTP Server and Client Host Colocated in alknet-http | One crate for server + client host (shared HTTP deps, shared mapping) |
| [040](../../decisions/040-webtransport-alpn-stream-proxy.md) | WebTransport ALPN-Stream-Proxy | **Parked** per ADR-044; revives unchanged when WebTransport revives |
| [041](../../decisions/041-mcp-tool-gateway-pattern.md) | MCP Tool-Gateway Pattern for to_mcp | 4 fixed gateway tools (search/schema/call/batch), not one tool per operation; Subscription excluded |
| [042](../../decisions/042-openapi-gateway-pattern.md) | OpenAPI Gateway Pattern for to_openapi | 5 fixed gateway endpoints (search/schema/call/batch/subscribe), not one path per operation; per-caller AccessControl-filtered |
| [043](../../decisions/043-webtransport-bidirectional-alpn-substrate.md) | WebTransport as a Bidirectional ALPN Transport Substrate | **Parked** per ADR-044; §2/§3 transfer to WebSocket for v1 |
| [044](../../decisions/044-defer-webtransport-browsers-use-websocket.md) | Defer h3/WebTransport; Browsers Use WebSocket | `h3`/WebTransport deferred (scope); browser bidirectional path uses WebSocket; "browser is not a peer" rationale |
| [045](../../decisions/045-to-openapi-gateway-spec-versioning.md) | to_openapi Gateway-Spec Versioning | Published gateway doc carries `info.version` (semver) tracking the gateway endpoint contract, not the operation set; consumers detect breaking changes via the major version |
| [046](../../decisions/046-assembly-layer-custom-http-routes.md) | Assembly-Layer Custom HTTP Routes on HttpAdapter | `extra_routes: Option<Router>` at construction; deployments add raw HTTP endpoints (e.g., OAI-compatible proxy, or a REST-like per-operation projection) that coexist with the default surface; default surface takes precedence on collision |
| [047](../../decisions/047-remove-direct-call-http-surface.md) | Remove the Direct-Call HTTP Surface; Gateway Is the Sole Invoke Path | `POST /{service}/{op}` direct-call surface removed; the 5 gateway endpoints are the sole invoke path; per-caller `AccessControl`-filtered `/search` is the discovery; ADR-036's non-routing clauses survive |

## Relevant Open Questions

| OQ | Title | Status | Relevance |
|----|-------|--------|-----------|
| OQ-11 | Handler-level auth resolution observability | resolved | HTTP handler stores resolved identity on `Connection` via `set_identity` |
| OQ-12 | TLS identity provisioning | resolved | Browsers require X.509 (applies to WebTransport when it revives; WebSocket uses the same TLS as h2/http1.1) |
| OQ-13 | Operation path format | resolved | `/{service}/{op}` is the HTTP path (ADR-036) |
| OQ-17 | Call protocol client and adapter contract | resolved | `OperationAdapter` trait; `to_*` projections |
| OQ-24 | Operation error schemas | resolved | `from_openapi`/`to_openapi` error fidelity |
| OQ-26 | OperationAdapter error type | resolved | `AdapterError` variants reused by HTTP adapters |
| OQ-37 | X.509 outgoing-only / three peer roles | resolved | Browsers are not peers; hub with mixed fingerprints |
| OQ-38 | WebTransport standalone relay service scope | open (scope, not deferral) | The standalone relay (future `alknet-relay`, fork of iroh-relay) — distinct from the in-process ALPN-stream-proxy (ADR-040) |
| OQ-39 | `to_openapi` published-spec versioning | resolved | `info.version` semver tracks the gateway endpoint contract (ADR-045); per-caller operation set discovered via `/search`, not in the doc |
| OQ-40 | reqwest client config and connection pooling | resolved | `ClientWithMiddleware` + `RetryTransientMiddleware` + inlined `RetryAfterMiddleware`; rebuild-and-swap hot-reload |

## Key Design Principles

1. **HTTP is both a server surface and a client transport for adapters.**
   Inbound HTTP (`h2`/`http/1.1` + WebSocket upgrade) is served by `axum`
   over a QUIC stream; outbound HTTP (`from_openapi`/`from_mcp`
   forwarding) uses `reqwest`. Both directions share the same HTTP
   dependencies, which is why they live in one crate rather than being
   split. See [overview.md](overview.md).
2. **The HTTP surface is the 5-endpoint gateway — a few fixed
   endpoints, not a per-operation REST tree.** An HTTP client invokes an
   operation via `POST /call` with `{ "operation": "/fs/readFile",
   "input": {...} }`, discovers what it can call via
   `AccessControl`-filtered `GET /search`, and learns an operation's
   shape via `GET /schema`. There is no per-operation `POST /{service}/{op}`
   direct-call surface (removed by ADR-047; the per-caller API surface is
   the default — the "dump the full API regardless of privs" failure mode
   is structurally impossible). `to_openapi` *describes* this gateway
   surface (5 fixed endpoints; per-caller operations discovered via
   `/search`, not preloaded into the doc). A deployment that wants a
   REST-like per-operation HTTP surface builds it as a custom route
   projection (ADR-046). See
   [ADR-042](../../decisions/042-openapi-gateway-pattern.md) (gateway
   pattern), [ADR-047](../../decisions/047-remove-direct-call-http-surface.md)
   (direct-call surface removed; ADR-036's routing superseded, non-routing
   clauses survive), and
   [ADR-046](../../decisions/046-assembly-layer-custom-http-routes.md)
   (custom routes extension point).
3. **Standard ALPNs, not alknet ALPNs.** `h2`, `http/1.1` are
   IANA-registered ALPN strings. Any HTTP client (browser, curl, axios)
   connects without knowing about alknet — the TLS handshake negotiates
   `h2` or `http/1.1` normally. This is the stealth mapping (ADR-010).
4. **`from_openapi`/`from_mcp` are the no-env-vars injection point.** The
   forwarding handlers read `context.capabilities`, not `std::env::var`.
   This is the architectural mechanism that makes aisdk's env-var reads
   unreachable. See ADR-014,
   [client-and-adapters.md](../call/client-and-adapters.md).
5. **MCP streamable HTTP only; stdio is not built.** stdio = spawn
   arbitrary executable = RCE. Streamable HTTP is network-isolated,
   auth-gatable, and runs under alknet's auth model. See
   [ADR-037](../../decisions/037-mcp-stdio-transport-exclusion.md).
6. **WebSocket is the browser bidirectional path.** A browser upgrades
   an HTTP/1.1 or HTTP/2 request to WebSocket and speaks the call
   protocol over binary WS messages — full-duplex, both sides can
   initiate calls (the call protocol's native bidirectionality, ADR-012).
   HTTP/3 + WebTransport (`h3`) is deferred per
   [ADR-044](../../decisions/044-defer-webtransport-browsers-use-websocket.md)
   — a scope decision (the browser bidirectional path doesn't require
   WebTransport's stream model; WebSocket suffices). The reversal
   trigger is a concrete ALPN-stream-proxy use case (a browser running
   a WASM SSH/SFTP/git client).
7. **Browsers are not alknet peers.** A browser over WebSocket (or, when
   it revives, WebTransport) authenticates by bearer token, gets no
   `PeerId`, and its registered ops land in a connection-local Layer 2
   overlay. "Peer" means an addressable node in the call-protocol peer
   graph (stable `PeerId`, `PeerRef::Specific`-reachable, identity
   stable across reconnects) — not "any endpoint that exchanges calls
   during a live session." A browser is the second but not the first: no
   stable cryptographic identity of its own, ephemeral, not addressable
   from other nodes. See
   [ADR-034](../../decisions/034-outgoing-only-x509-and-three-peer-roles.md)
   §4 (amended by ADR-044 §5 with the addressability rationale).

## References

- `docs/research/alknet-http/phase-0-findings.md` — Phase 0 research
  (directionally close; DH-2's deferral framing was corrected by
  ADR-038, then ADR-038 was superseded by ADR-044 which re-defers
  `h3`/WebTransport as a genuine scope decision)
- `docs/research/alknet-call-completion/gap-analysis.md` — adapter
  location map, no-env-vars invariant
- `/workspace/@alkdev/operations/src/from_openapi.ts`,
  `/workspace/@alkdev/operations/src/from_mcp.ts` — TypeScript prior art
- `/workspace/rust-sdk/` — MCP Rust SDK (rmcp v1.8.0); streamable HTTP
  transport examples
- `/workspace/wtransport/` — pure-Rust WebTransport reference
  implementation (read during research; not a dependency. See ADR-044
  §"Research note" for why `wtransport` is probably not the right
  revival choice — the hyperium stack fits the axum integration better.)