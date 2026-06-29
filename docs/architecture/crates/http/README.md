---
status: draft
last_updated: 2026-06-29
---

# alknet-http

HTTP interface for alknet: serves HTTP/1.1, HTTP/2, and HTTP/3 (WebTransport)
on standard ALPNs, and hosts the HTTP-backed call-protocol adapters
(`from_openapi`, `to_openapi`, `from_mcp`, `to_mcp`).

## Documents

| Document | Status | Description |
|----------|--------|-------------|
| [overview.md](overview.md) | draft | Crate purpose, two roles (server + client host), dependencies, adapter location map |
| [http-server.md](http-server.md) | draft | `HttpAdapter` (`ProtocolHandler` for `h2`/`http/1.1`), axum over QUIC, Bearer auth, stealth, `/healthz` |
| [http-adapters.md](http-adapters.md) | draft | `from_openapi` (reqwest client) and `to_openapi` (OpenAPI projection); no-env-vars invariant point |
| [http-mcp.md](http-mcp.md) | draft | `from_mcp` / `to_mcp` (feature-gated), streamable-HTTP-only, stdio exclusion |
| [webtransport.md](webtransport.md) | draft | `h3`/WebTransport handler — the browser streaming path |

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
| [027](../../decisions/027-tls-identity-redesign-acme-rawkey-decoupling.md) | TLS Identity Redesign | Browsers require X.509; WebTransport requires X.509 |
| [034](../../decisions/034-outgoing-only-x509-and-three-peer-roles.md) | Outgoing-Only X.509 and Three Peer Roles | Browsers are not alknet peers; WebTransport relay-as-proxy recorded |
| [036](../../decisions/036-http-to-call-operation-mapping.md) | HTTP-to-Call Operation Mapping | Direct path mapping; `to_openapi` is projection, not router |
| [037](../../decisions/037-mcp-stdio-transport-exclusion.md) | MCP Stdio Transport Exclusion | Streamable HTTP only; stdio not built |
| [038](../../decisions/038-http3-and-webtransport-as-first-class.md) | HTTP/3 and WebTransport as First-Class HTTP Transports | `h3` in scope, not deferred |
| [039](../../decisions/039-http-server-and-client-host-colocated.md) | HTTP Server and Client Host Colocated in alknet-http | One crate for server + client host (shared HTTP deps, shared mapping) |

## Relevant Open Questions

| OQ | Title | Status | Relevance |
|----|-------|--------|-----------|
| OQ-11 | Handler-level auth resolution observability | resolved | HTTP handler stores resolved identity on `Connection` via `set_identity` |
| OQ-12 | TLS identity provisioning | resolved | Browsers require X.509 (gates the entire `h3` feature) |
| OQ-13 | Operation path format | resolved | `/{service}/{op}` is the HTTP path (ADR-036) |
| OQ-17 | Call protocol client and adapter contract | resolved | `OperationAdapter` trait; `to_*` projections |
| OQ-24 | Operation error schemas | resolved | `from_openapi`/`to_openapi` error fidelity |
| OQ-26 | OperationAdapter error type | resolved | `AdapterError` variants reused by HTTP adapters |
| OQ-37 | X.509 outgoing-only / three peer roles | resolved | Browsers are not peers; hub with mixed fingerprints |
| OQ-38 | WebTransport relay-as-proxy scope | open (scope, not deferral) | Does the proxy live in `alknet-http` or a separate relay crate? |
| OQ-39 | `to_openapi` published-spec versioning | open | Versioning strategy for generated OpenAPI specs |
| OQ-40 | reqwest client config and connection pooling | open | Two-way-door: pooling/retry config shape |

## Key Design Principles

1. **HTTP is both a server surface and a client transport for adapters.**
   Inbound HTTP (`h2`/`http/1.1`/`h3`) is served by `axum` over a QUIC
   stream; outbound HTTP (`from_openapi`/`from_mcp` forwarding) uses
   `reqwest`. Both directions share the same HTTP dependencies, which is
   why they live in one crate rather than being split. See
   [overview.md](overview.md).
2. **The HTTP surface is a projection of the call protocol.** An HTTP
   request at `POST /fs/readFile` becomes a `call.requested` for
   `/fs/readFile`. The HTTP path IS the operation path; `to_openapi`
   describes this surface, it does not define a second one. See
   [ADR-036](../../decisions/036-http-to-call-operation-mapping.md).
3. **Standard ALPNs, not alknet ALPNs.** `h2`, `http/1.1`, `h3` are
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
6. **HTTP/3 + WebTransport is a first-class transport, not a deferral.**
   The browser streaming path uses QUIC streams directly. See
   [ADR-038](../../decisions/038-http3-and-webtransport-as-first-class.md).
7. **`h3` requires X.509.** Browsers don't support RFC 7250 raw keys
   (ADR-027). A node serving WebTransport must have an X.509 identity.
   This is a browser limitation, not an alknet decision.

## References

- `docs/research/alknet-http/phase-0-findings.md` — Phase 0 research
  (directionally close; DH-2's deferral framing is corrected by ADR-038)
- `docs/research/alknet-call-completion/gap-analysis.md` — adapter
  location map, no-env-vars invariant
- `/workspace/@alkdev/operations/src/from_openapi.ts`,
  `/workspace/@alkdev/operations/src/from_mcp.ts` — TypeScript prior art
- `/workspace/rust-sdk/` — MCP Rust SDK (rmcp v1.8.0); streamable HTTP
  transport examples
- `/workspace/wtransport/` — pure-Rust WebTransport reference
  implementation (the `h3` feature's candidate dependency)