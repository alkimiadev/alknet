---
status: draft
last_updated: 2026-06-25
---

# alknet-http — Phase 0 Research Findings

This document captures Phase 0 (Exploration) findings for the `alknet-http`
crate. Unlike alknet-call (which has existing architecture being completed),
alknet-http has **zero architecture docs and zero implementation** — this is a
true greenfield exploration. It will need iteration; the goal here is to surface
the design space, the settled decisions, and the open questions so Phase 1 can
proceed with direction.

## Vision

`alknet-http` is the HTTP protocol handler for the ALPN-as-service architecture.
It serves two roles:

1. **HTTP server** — a `ProtocolHandler` that accepts HTTP/2, HTTP/1.1, and
   optionally HTTP/3 (WebTransport) connections on standard ALPNs (`h2`,
   `http/1.1`, `h3`), serving REST APIs, dashboards, MCP endpoints, and the
   `to_openapi`/`to_mcp` projections of local call-protocol operations.
2. **HTTP client host** — the home of the HTTP-transport-backed adapters
   (`from_openapi`, `from_mcp`, `to_openapi`, `to_mcp`) that use reqwest (client)
   and axum (server) to bridge between the call protocol and external HTTP/MCP
   systems.

The key architectural insight: **HTTP is both a server surface and a client
transport for adapters.** Both directions share the same HTTP dependencies
(axum for serving, reqwest for calling out), which is why they live in one
crate rather than being split.

## Sources Investigated

| Source | Path | Note |
|--------|------|------|
| alknet-core types | `docs/architecture/crates/core/*` | ProtocolHandler, Connection, AuthContext, IdentityProvider |
| alknet-call specs | `docs/architecture/crates/call/*`, ADR-017/022/024 | OperationAdapter trait (where from_openapi/from_mcp implement), HandlerRegistration, Capabilities |
| alknet-call gap analysis | `docs/research/alknet-call-completion/gap-analysis.md` | Adapter location map, the no-env-vars invariant |
| TS @alkdev/operations | `/workspace/@alkdev/operations/src/` | `from_openapi.ts`, `from_mcp.ts`, `from_schema.ts` — prior art for adapter patterns |
| MCP Rust SDK (rmcp) | `/workspace/rust-sdk/` | Streamable HTTP transport (client + server), the `simple_auth_streamhttp.rs` example |
| aisdk | `/workspace/aisdk/` | Rust port of Vercel AI SDK; 75 providers; the no-env-vars problem this crate's credential injection solves |
| overview.md ALPN Registry | `docs/architecture/overview.md` | `h2`/`http/1.1` → HttpAdapter, `h3` → HttpAdapter (WebTransport) |
| endpoint.md stealth section | `docs/architecture/crates/core/endpoint.md` | HTTP handler on standard ALPNs serves decoy — the "stealth mode" mapping |

## What's Settled

These are confirmed by existing ADRs or our prior conversation and should not
be re-litigated in Phase 1.

### 1. alknet-http implements `ProtocolHandler` for standard HTTP ALPNs

From overview.md's ALPN Registry:

| ALPN | Handler | Use case |
|------|---------|----------|
| `h2` | HttpAdapter | HTTP/2 for browsers, curl, standard clients |
| `http/1.1` | HttpAdapter | HTTP/1.1 fallback for legacy clients |
| `h3` | HttpAdapter (WebTransport upgrade) | HTTP/3 / WebTransport for browser streaming |

Unlike custom alknet ALPNs (`alknet/ssh`, `alknet/call`), HTTP uses the
**standard IANA ALPN strings**. This means any HTTP client (browser, curl,
axios) can connect without knowing about alknet — the TLS handshake negotiates
`h2` or `http/1.1` normally, and the HttpAdapter serves HTTP.

### 2. The adapter implementations live here, the trait lives in alknet-call

From the gap analysis adapter location map:

```
alknet-call owns:           alknet-http owns:
  OperationAdapter trait      from_openapi (parse + reqwest forwarding)
  from_call (QUIC)            to_openapi (generate OpenAPI doc)
  from_jsonschema (pure)      from_mcp (streamable HTTP client — reqwest)
                              to_mcp (streamable HTTP server — axum)
```

alknet-http depends on alknet-call for the types (`OperationSpec`, `Handler`,
`HandlerRegistration`, `OperationAdapter`). This is a protocol-foundation
dependency (alknet-call is not a peer handler), within the spirit of ADR-003.
Phase 1 should note this explicitly and possibly amend ADR-003 to clarify
alknet-call's dual role.

### 3. The no-env-vars invariant — credential injection point

The `from_openapi`/`from_mcp` forwarding handlers are the **credential injection
point** for the no-env-vars architecture. The path (from the gap analysis):

```
vault → assembly layer → Capabilities → HandlerRegistration.capabilities
  → OperationContext.capabilities → from_openapi handler reads
    context.capabilities.get("openai") → injects into HTTP Authorization
    header → reqwest request goes out with vault-derived credential
```

This makes aisdk's `std::env::var("OPENAI_API_KEY")` reads **unreachable** —
the assembly layer never calls `Default::default()` on a provider; it
constructs them with vault-derived credentials, or routes HTTP calls through
`from_openapi` operations that carry the credential in `Capabilities`.

**This is a spec-level invariant**: no handler reads outbound credentials from
any source other than `OperationContext.capabilities`. The `from_openapi`/
`from_mcp` implementations in alknet-http are verified against this invariant.

### 4. MCP stdio is explicitly excluded (security position)

From our conversation: MCP stdio transport (`transport-child-process` in rmcp)
= spawn an arbitrary executable, pipe JSON-RPC over stdin/stdout. Downloading
untrusted MCP servers and running them via stdio is indistinguishable from
`curl | sh` with extra steps. The "download untrusted and probably poorly
written mcp servers" model is an RCE vector.

**alknet-http supports only streamable HTTP for MCP.** Stdio is not built. If
someone wants stdio MCP, they run it themselves outside alknet. This is an
explicit security position, recorded as an ADR-level constraint in the
alknet-http spec. The streamable HTTP transport is the supported path; it's
network-isolated, auth-gatable (Bearer token middleware, per the rmcp
`simple_auth_streamhttp.rs` example), and runs under alknet's auth/identity/
capabilities machinery.

### 5. Stealth mode = HTTP handler on standard ALPNs

From endpoint.md: the reference implementation's "stealth mode" (SSH-over-TLS
on port 443 with a fake nginx 404 for non-SSH traffic) maps to the HTTP handler
serving a decoy website or fake 404 on standard HTTP ALPNs. Clients that don't
offer alknet ALPNs get the HTTP handler — just like port scanners in stealth
mode. No byte-peeking; ALPN does the routing.

## Design Space (Decision Points)

These are genuine architectural choices for a greenfield crate. Each is tagged
with door type per ADR-009. Phase 1 resolves these (some via ADRs, some as
two-way-door defaults).

### DH-1: HTTP framework — axum
*(Recommended: two-way door — axum is the obvious choice but not locked)*

The overview.md and the rmcp examples all use axum. axum is built on hyper,
integrates with tower (middleware), and is the de facto Rust web framework.
Alternatives (actix-web, warp, raw hyper) exist but axum is the path of least
resistance and aligns with the rmcp streamable HTTP server (which uses axum's
`Router` + `StreamableHttpService`).

**Recommendation**: **axum**. The rmcp `StreamableHttpService` is a tower
service that nests into an axum `Router` directly (see
`simple_auth_streamhttp.rs:134-159`), so using axum makes the MCP server
integration trivial. Two-way door — switching frameworks later is painful but
not architecturally locked.

### DH-2: HTTP/3 + WebTransport — in v1 or deferred?
*(Recommended: two-way door — defer h3/WebTransport past v1)*

HTTP/3 (QUIC-based) and WebTransport are the browser-streaming path. Browsers
don't support RFC 7250 raw keys (ADR-027), so WebTransport requires X.509
certs — meaning it's a domain-hosted-service concern, not a P2P concern.
quinn already speaks QUIC; HTTP/3 is a framing layer on top.

The question is whether v1 needs WebTransport or whether HTTP/2 + HTTP/1.1
suffices. For the runner pattern and the call-protocol HTTP projection,
HTTP/2 is sufficient. WebTransport is the browser path for the agent service
(alknet-agent), which is downstream of alknet-http anyway.

**Recommendation**: **defer h3/WebTransport past v1**. Start with `h2` +
`http/1.1`. The `h3` ALPN is reserved in the registry but the implementation
lands as a fast-follow when the agent service needs browser streaming. This
keeps v1 focused on the adapter + REST surface. Two-way door.

### DH-3: How does HTTP map to call-protocol operations?
*(One-way door — needs an ADR)*

The core question: when an HTTP request arrives at `alknet/http://api.example.com/container/exec`,
how does it become a call-protocol operation? Options:

- **(a) Direct path mapping**: `POST /{service}/{op}` → `call.requested` for
  `/{service}/{op}`. Matches the call protocol's `/{service}/{op}` path format
  (OQ-13). The HTTP handler is a thin bridge: parse the HTTP request body as
  the operation input, send `call.requested`, return the response as JSON.
- **(b) OpenAPI-defined routes**: the HTTP surface is defined by the
  `to_openapi` projection — routes, methods, schemas are generated from the
  registry's `External` operations. The HTTP handler dispatches based on the
  generated OpenAPI spec's path mapping.
- **(c) Explicit route registration**: the assembly layer registers HTTP routes
  explicitly, mapping URL paths to operations. Most flexible, most boilerplate.

**Recommendation**: **(a) direct path mapping as the default, with (b) as the
discovery/projection layer**. The HTTP handler receives `POST /container/exec`,
constructs `{operationId: "container/exec", input: <parsed body>}`, and
dispatches it through the call protocol. `to_openapi` generates the spec that
*describes* this surface for external consumers; it doesn't define separate
routes. This keeps the HTTP surface a thin projection of the call protocol,
not a parallel routing layer. Needs an ADR.

### DH-4: Auth — how does HTTP auth map to AuthContext?
*(One-way door — needs an ADR, but largely settled by existing ADRs)*

The `HttpAdapter` extracts credentials and resolves identity through
`IdentityProvider` (ADR-004). The credential source for HTTP is the
`Authorization: Bearer <token>` header. The handler calls
`resolve_from_token(&AuthToken { raw: token_bytes })`. This is already in
auth.md's table:

| Handler | Credential source | Resolution method |
|---------|------------------|-----------------|
| HttpAdapter | `Authorization: Bearer` header | `resolve_from_token()` |

**Recommendation**: this is settled by ADR-004 + auth.md. The `HttpAdapter`
constructor-injects `Arc<dyn IdentityProvider>` (same pattern as `SshAdapter`).
No new ADR needed — Phase 1 documents the existing model. The one sub-question
is whether HTTP supports multiple auth mechanisms (Bearer + basic + API key in
query param) or Bearer-only. **Recommendation: Bearer-only for v1** (matches
the call protocol's AuthToken model); other mechanisms are two-way-door
additions.

### DH-5: MCP streamable HTTP — server and/or client?
*(Recommended: both, feature-gated — confirmed in conversation)*

From our conversation + rmcp survey:

- **`from_mcp`** (import remote MCP tools): uses rmcp's
  `StreamableHttpClientTransport` (reqwest-based,
  `transport-streamable-http-client-reqwest` feature). Discovers MCP tools,
  registers them as `FromMCP`-provenance operations with forwarding handlers.
- **`to_mcp`** (expose local ops as MCP tools): uses rmcp's
  `StreamableHttpService` (axum-based,
  `transport-streamable-http-server` feature). Serves local `External`
  operations as MCP tools over streamable HTTP.

Both are feature-gated (the rmcp dependency is optional). The MCP server
auth uses Bearer token middleware (the `simple_auth_streamhttp.rs` example
shows the pattern).

**Recommendation**: **both, behind an `mcp` feature gate**. The rmcp
dependency (`rmcp = { version = "1.8", features = [...] }`) is optional;
enabling the `mcp` feature pulls in rmcp with the streamable HTTP transport
features. Without the feature, alknet-http is a pure HTTP server + OpenAPI
adapter with no MCP support. Two-way door on the feature gate; one-way door
on stdio exclusion.

### DH-6: Dashboard / health / metrics surface
*(Two-way door — start minimal, extend later)*

An HTTP server typically needs operational endpoints: health check, metrics,
maybe a dashboard. The question is whether these are call-protocol operations
(`/health/check`, `/metrics/list`) or raw HTTP routes outside the call
protocol.

**Recommendation**: **health check as a raw HTTP route (`GET /healthz`)**
outside the call protocol (no auth, no operation registration — it's an
infrastructure endpoint for load balancers). Metrics and dashboard are
call-protocol operations (`/metrics/list`, `/dashboard/view`) if built at all.
Start with just `/healthz` in v1. Two-way door.

### DH-7: HTTP client — reqwest config and connection pooling
*(Two-way door — implementation detail)*

The `from_openapi`/`from_mcp` forwarding handlers use reqwest to make outbound
HTTP calls. The aisdk `core/client.rs` shows a pattern worth referencing: a
shared `reqwest::Client` with connection pooling (`OnceLock<reqwest::Client>`),
retry logic (exponential backoff, Retry-After header), and separate streaming
vs non-streaming clients.

**Recommendation**: alknet-http maintains its own shared `reqwest::Client`
(constructed once, reused across all forwarding handlers). The retry/pooling
config comes from `StaticConfig` or `DynamicConfig` (hot-reloadable). Don't
inherit aisdk's client — alknet-http owns its HTTP client. The credential
injection happens per-request (from `OperationContext.capabilities`), not at
client construction. Two-way door on the exact pooling/retry config.

### DH-8: TLS for outbound HTTP calls
*(Two-way door — implementation detail, connects to ADR-027)*

The `from_openapi`/`from_mcp` handlers make outbound HTTPS calls. The TLS
config for these calls: do they use the system trust store (default reqwest
behavior), or a custom CA bundle, or vault-derived client certs?

**Recommendation**: **system trust store by default** (standard HTTPS to
external APIs like OpenAI, Anthropic, etc.). Custom CA bundle + client certs
as an optional config for self-hosted API gateways. This is a two-way-door
implementation detail; the credential (API key/token) comes from
`Capabilities`, the TLS trust comes from the system. No ADR needed.

## Tentative Recommended Approach (Convergence)

1. **Crate**: `alknet-http`, depends on `alknet-core` (ProtocolHandler,
   AuthContext, IdentityProvider), `alknet-call` (OperationAdapter trait,
   OperationSpec, HandlerRegistration, Capabilities), `axum` (HTTP server),
   `reqwest` (HTTP client), and optionally `rmcp` (MCP, feature-gated).

2. **HTTP server** (`h2` + `http/1.1`): axum `Router` wrapped as a
   `ProtocolHandler`. Receives the QUIC `Connection`, accepts a bistream,
   hands the duplex stream to hyper/axum's connection handler. Direct path
   mapping (`POST /{service}/{op}` → `call.requested`) for the default
   surface. Bearer auth via `IdentityProvider::resolve_from_token()`.

3. **HTTP client** (`from_openapi`/`from_mcp` forwarding): shared
   `reqwest::Client` with pooling + retry. Credentials from
   `OperationContext.capabilities` per the no-env-vars invariant. The
   `from_openapi` handler reads `context.capabilities.get("openai")` and
   injects into the `Authorization` header.

4. **MCP** (feature-gated `mcp`): `from_mcp` via rmcp
   `StreamableHttpClientTransport` (reqwest); `to_mcp` via rmcp
   `StreamableHttpService` (axum). Stdio excluded (security position).

5. **`to_openapi`**: generates an OpenAPI spec from the local registry's
   `External` operations. Pure serialization — no HTTP client, no HTTP server.
   Served at `GET /openapi.json` (or similar) by the HTTP server.

6. **`h3`/WebTransport**: deferred past v1. ALPN reserved, implementation
   lands as a fast-follow for the agent service's browser-streaming path.

7. **Health**: `GET /healthz` as a raw HTTP route (no auth, no call protocol).

8. **Stealth**: the HTTP handler on `h2`/`http/1.1` serves a decoy/fake 404
   for unknown paths, matching the endpoint.md stealth-mode mapping. Real
   services use `alknet/ssh`, `alknet/call`, etc.

## Open Questions to Carry into Phase 1

- **OQ-HTTP-01 (HTTP → call operation mapping)**: direct path mapping
  (`POST /{service}/{op}` → `call.requested`) as the default — confirmed
  approach, needs an ADR (DH-3).
- **OQ-HTTP-02 (h3/WebTransport timeline)**: deferred past v1 (DH-2). Two-way
  door; lands when the agent service needs browser streaming.
- **OQ-HTTP-03 (MCP feature gate shape)**: the exact rmcp features to enable
  and the `mcp` feature gate definition (DH-5). Two-way door.
- **OQ-HTTP-04 (dashboard/metrics surface)**: minimal `/healthz` in v1,
  metrics/dashboard as call-protocol operations if built (DH-6). Two-way door.
- **OQ-HTTP-05 (ADR-003 amendment)**: clarify that alknet-http depending on
  alknet-call is permitted (alknet-call is protocol-foundation, not a peer
  handler). One-line amendment.
- **OQ-HTTP-06 (to_openapi published-spec versioning)**: ADR-017 Consequences
  notes that published `to_*` specs are compatibility contracts. The versioning
  strategy for generated OpenAPI specs (tied to the registry's External
  operation set version) needs specifying. One-way door after first
  publication.

## Next Steps (Phase 0 → Phase 1)

1. **You decide** on DH-3 (HTTP → call operation mapping) — this is the
   load-bearing architectural choice. The others (DH-1, DH-2, DH-4, DH-5,
   DH-6, DH-7, DH-8) are two-way-door defaults I recommend accepting as-is.
2. **alknet-call completion prerequisite**: the `OperationAdapter` trait must
   be defined in alknet-call before alknet-http's `from_openapi`/`from_mcp`
   can be implemented. Phase 1 for alknet-http can proceed in parallel with
   alknet-call completion (the spec can be written against the trait shape from
   ADR-017), but implementation of the HTTP-backed adapters blocks on the
   trait landing.
3. **Phase 1 (Architect)**: produce `docs/architecture/crates/http/README.md`
   + component specs (e.g., `http-server.md`, `http-client.md`,
   `http-adapters.md`, `http-mcp.md`), ADRs for DH-3 (HTTP→call mapping) and
   the MCP stdio exclusion (security position), and the OQs above. Update
   `docs/architecture/README.md` index and ADR table.

## References

- `docs/sdd_process.md` — Phase 0 process definition
- `docs/architecture/overview.md` — ALPN Registry (h2/http1.1/h3 → HttpAdapter)
- `docs/architecture/crates/core/core-types.md` — ProtocolHandler, Connection
- `docs/architecture/crates/core/auth.md` — HttpAdapter auth (Bearer → resolve_from_token)
- `docs/architecture/crates/core/endpoint.md` — stealth mode as ALPN dispatch
- `docs/architecture/decisions/017-call-protocol-client-and-adapter-contract.md` —
  OperationAdapter trait, to_openapi/to_mcp
- `docs/architecture/decisions/022-handler-registration-provenance-and-composition-authority.md` —
  Capabilities injection, FromOpenAPI/FromMCP provenance
- `docs/architecture/decisions/027-tls-identity-redesign-acme-rawkey-decoupling.md` —
  browser limitation (no RFC 7250), WebTransport needs X.509
- `docs/research/alknet-call-completion/gap-analysis.md` — adapter location map,
  no-env-vars invariant, exchange-of-operations pattern
- `/workspace/@alkdev/operations/src/` — TS prior art: `from_openapi.ts`,
  `from_mcp.ts`, `from_schema.ts`, `scanner.ts`
- `/workspace/rust-sdk/` — MCP Rust SDK (rmcp v1.8.0); streamable HTTP transport
- `/workspace/rust-sdk/examples/servers/src/simple_auth_streamhttp.rs` —
  streamable HTTP MCP server with Bearer auth (the pattern for `to_mcp`)
- `/workspace/rust-sdk/examples/clients/src/streamable_http.rs` —
  streamable HTTP MCP client (the pattern for `from_mcp`)
- `/workspace/aisdk/` — Rust port of Vercel AI SDK; 75 providers with env-var
  reads that the no-env-vars invariant makes unreachable
- `/workspace/aisdk/src/core/client.rs` — HTTP client reference (pooling,
  retry, streaming vs non-streaming)