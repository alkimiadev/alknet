# ADR-038: HTTP/3 and WebTransport as First-Class HTTP Transports

## Status

Proposed

## Context

The alknet-http Phase 0 research findings
(`docs/research/alknet-http/phase-0-findings.md`, decision point DH-2)
framed HTTP/3 + WebTransport as "deferred past v1" — a two-way-door
addition to land "when the agent service needs browser streaming." That
framing was a residual of the "two-way door as deferral" anti-pattern
that ADR-009 §"What this framework is NOT" was later written to prevent.
The deferral framing is rejected here; this ADR records the decision as
made.

WebTransport is not a "later" feature. It is the browser-streaming
transport — QUIC streams are cheap (better than WebSocket/SSE by far for
multiplexed bidirectional streaming), and WebTransport is supported in
major browsers. The `alknet-http` crate's purpose is to be the HTTP
interface library for downstream crates that need to expose HTTP, and
browser streaming is a first-class requirement of that purpose, not a
fast-follow.

### The ALPN registry already reserves h3

The overview ALPN Registry maps `h3` to `HttpAdapter (HTTP/3 +
WebTransport)`. The `h3` ALPN is reserved; the implementation lands as
part of `alknet-http`, not as a future crate. This ADR confirms that
reservation and records the architectural decision: HTTP/3 + WebTransport
is in scope, in this crate, as a first-class transport alongside `h2` and
`http/1.1`.

### Why WebTransport matters

- **QUIC streams are cheap.** A browser opens a WebTransport connection
  once and multiplexes many bidirectional streams over it. This is
  fundamentally better than WebSocket (one stream, one connection) or
  SSE (one-directional, one stream per subscription) for the
  subscription-heavy, streaming-heavy patterns the call protocol
  supports natively.
- **Browser support.** WebTransport is supported in modern Chromium-based
  browsers (Chrome, Edge). The browser path for alknet is WebTransport,
  not WebSocket.
- **The call protocol maps cleanly onto WebTransport streams.** A
  `call.requested` over a WebTransport bidirectional stream is the same
  EventEnvelope framing over a different QUIC stream source. The
  `CallConnection`/`Dispatcher` dispatch loop is stream-agnostic
  (ADR-012) — the `h3` handler hands a bidirectional stream to the call
  protocol the same way the `h2`/`http/1.1` handler hands a hyper
  connection to axum.

### The TLS constraint (browsers require X.509)

Browsers do not support RFC 7250 raw public keys (ADR-027, OQ-12). A
WebTransport session from a browser requires an X.509 cert — meaning the
`h3` handler is a domain-hosted-service concern, not a P2P concern. A
node serving WebTransport must have an X.509 identity (`TlsIdentity::X509`
or `TlsIdentity::Acme`). This is a property of the browser, not a
decision this ADR makes — it's recorded so the spec doesn't pretend a
raw-key node can serve browsers.

### WebTransport relay-as-proxy

A distinct WebTransport feature — a proxy that terminates the browser's
WebTransport connection and forwards encrypted traffic to a P2P hub's
Ed25519 endpoint (so the hub need not expose its own public X.509 cert)
— was recorded in ADR-034 §5. That feature does not change the auth model
(bearer token + `PeerEntry.auth_token_hash`; the proxy is transport-only)
and was explicitly placed in the same bucket as the rest of h3/
WebTransport. With this ADR, h3/WebTransport is in scope; the
relay-as-proxy is a genuine scope question (does the proxy belong in
alknet-http or in a separate relay crate?), tracked as OQ-38 — not
deferred, just scoped to a separate decision when the proxy use case
becomes concrete.

## Decision

**HTTP/3 + WebTransport is a first-class HTTP transport in `alknet-http`,
implemented alongside `h2` and `http/1.1`. It is not deferred.**

The `HttpAdapter` implements `ProtocolHandler` for three ALPNs:

| ALPN | Transport | Use case | Browser? |
|------|-----------|----------|----------|
| `http/1.1` | HTTP/1.1 over QUIC stream | Legacy clients, curl | No (browsers use h2/h3) |
| `h2` | HTTP/2 over QUIC stream | Modern HTTP clients, curl | No (browsers use h3) |
| `h3` | HTTP/3 / WebTransport | Browser streaming | Yes (X.509 required) |

All three are served by `alknet-http`. The `h3` ALPN handler upgrades to
WebTransport sessions and serves both HTTP/3 requests (the standard
HTTP/3 over QUIC framing) and WebTransport streams (the bidirectional/
unidirectional stream API). The handler dispatches HTTP/3 requests
through the same axum `Router` as `h2`/`http/1.1`; WebTransport streams
that target the call protocol are handed to the call protocol's dispatch
loop directly (a WebTransport stream is a QUIC bidirectional stream, the
same stream type the call protocol already speaks).

### WebTransport as the browser streaming path

The `h3` handler drives two distinct stream types, distinguished at the
HTTP/3 framing layer (not by peeking application bytes):

1. **HTTP/3 request streams** — standard HTTP/3 GET/POST carrying
   `:method`/`:path`. Dispatched through the axum `Router`, same as
   `h2`/`http/1.1` (ADR-036). These are not WebTransport streams.
2. **WebTransport sessions** — opened by a browser's
   `new WebTransport(url)` call (an HTTP/3 extended CONNECT request).
   The handler accepts the session (the `wtransport` crate's
   `Endpoint::server` + `accept` + `accept_bi` pattern, or the quinn
   HTTP/3 endpoint's WebTransport extension). Within an established
   session, the browser creates bidirectional streams via
   `transport.createBidirectionalStream()`; the handler accepts each
   and dispatches by sub-protocol. For a call-protocol session, the
   first frame is an `EventEnvelope` and the handler hands the stream
   to the call protocol's `Dispatcher` — no SSE translation, no HTTP
   framing. The browser's `WebTransport` JS API speaks to this handler
   directly.

The stream-type distinction (HTTP/3 request vs. WebTransport session)
is made at the HTTP/3 frame layer (regular request headers vs. extended
CONNECT), not by reading the first application byte. The first-frame
routing applies *within* a WebTransport session (determining the
sub-protocol), not between an HTTP/3 request and a WebTransport stream.

This means the browser's subscription/streaming path uses WebTransport
streams directly, not the SSE projection (ADR-036) that HTTP/1.1 + HTTP/2
clients use. The same `Subscription` operation is served as SSE over
`h2` and as a native WebTransport stream over `h3` — the projection is
transport-dependent, the operation is the same.

### h3 and the stealth mapping

The `h3` handler participates in the same stealth model as `h2`/
`http/1.1` (ADR-010, ADR-036): a client that offers `h3` gets the HTTP
handler. Unknown WebTransport paths and unknown HTTP/3 paths get the
decoy (configurable). Real services use `alknet/ssh`, `alknet/call`, etc.

### Implementation reference: wtransport

The `wtransport` crate (`/workspace/wtransport/`, v0.7.1) is a pure-Rust
WebTransport implementation built on `quinn` + `h3`/`qpack`. It provides
the `Endpoint::server` + `accept` + `accept_bi` API that the `h3`
handler uses. `wtransport` is a candidate dependency for the `h3`
feature gate; the exact WebTransport library choice (wtransport vs a
quinn-native HTTP/3 + WebTransport extension) is a two-way-door
implementation detail. The one-way constraint is: `h3` is served, by this
crate, as a first-class transport.

### Feature gating

The `h3`/WebTransport support is behind an `h3` feature gate (the
WebTransport/HTTP/3 dependencies are heavier than `h2`/`http/1.1`):
```toml
[features]
default = ["h2", "http1"]
h3 = ["dep:wtransport"]      # or the quinn h3 extension
mcp = ["dep:rmcp"]           # MCP feature gate (ADR-037)
```
A deployment that only needs `h2`/`http/1.1` (a non-browser-facing
node) does not compile the WebTransport dependencies. A browser-facing
node enables `h3`.

## Consequences

**Positive:**
- Browser streaming uses QUIC streams directly, not SSE-over-HTTP/2.
  The call protocol's subscription model maps onto WebTransport streams
  with no translation loss — a `call.responded` stream over a
  WebTransport bidirectional stream is the native representation.
- The `h3` ALPN reservation in the overview is honored — the
  implementation lands in this crate, not a future one.
- A browser-facing node (a hub with an X.509 cert) serves the same
  operations over `h3` as a non-browser-facing node serves over `h2` —
  the operation registry is transport-agnostic, the projection is
  transport-dependent.
- The WebTransport relay-as-proxy (ADR-034 §5) has a clear home: it's a
  feature that lives in or near `alknet-http`'s `h3` handler, scoped by
  OQ-38.

**Negative:**
- `alknet-http` gains the `wtransport` (or equivalent HTTP/3 +
  WebTransport) dependency behind the `h3` feature. This is a heavier
  dependency than `h2`/`http/1.1`. The feature gate keeps it out of
  non-browser-facing builds.
- WebTransport is still a draft standard (the `wtransport` README notes
  it). The API may change. This is inherent to being an early adopter
  of WebTransport; the `h3` feature gate isolates the risk.
- Browsers require X.509 (ADR-027). A raw-key-only node cannot serve
  WebTransport. This is a browser limitation, not an alknet decision,
  but it means the `h3` feature is useful only on domain-hosted nodes
  with X.509 certs.

## Assumptions

1. **WebTransport is the browser streaming transport for alknet.** The
   browser path is WebTransport, not WebSocket and not SSE-over-HTTP/2.
   SSE remains the streaming projection for non-WebTransport HTTP
   clients (curl, axios over h2); WebTransport is the native path for
   browsers.

2. **The `wtransport` crate (or an equivalent quinn-native HTTP/3 +
   WebTransport implementation) is the dependency for the `h3` feature.**
   The exact library is a two-way-door implementation detail; the
   one-way constraint is that `h3` is served by this crate.

3. **The WebTransport relay-as-proxy (ADR-034 §5) is a separate scope
   decision (OQ-38).** It is not deferred — it's a feature with a clear
   home (the `h3` handler or a sibling relay crate) that gets designed
   when the browser-to-P2P-peer proxy use case becomes concrete.

## References

- [ADR-009](009-one-way-door-decision-framework.md) §"What this framework
  is NOT" — the anti-pattern this ADR corrects (two-way door as deferral)
- [ADR-010](010-alpn-router-and-endpoint.md) — the ALPN router; `h3` is
  one of the ALPNs the `HttpAdapter` registers for
- [ADR-012](012-call-protocol-stream-model.md) — stream-agnostic
  correlation; a WebTransport stream is a QUIC bidirectional stream
- [ADR-027](027-tls-identity-redesign-acme-rawkey-decoupling.md) — the
  browser limitation (no RFC 7250); WebTransport requires X.509
- [ADR-034](034-outgoing-only-x509-and-three-peer-roles.md) §4 (browsers
  are not alknet peers) and §5 (WebTransport relay-as-proxy, recorded
  for this bucket)
- [ADR-036](036-http-to-call-operation-mapping.md) — the SSE projection
  for `h2`/`http/1.1` that WebTransport replaces for the browser path
- `docs/research/alknet-http/phase-0-findings.md` DH-2 — the deferral
  framing this ADR rejects
- `/workspace/wtransport/` — pure-Rust WebTransport (the `h3` feature's
  reference implementation)
- `crates/http/webtransport.md` — the spec that implements the `h3`
  handler