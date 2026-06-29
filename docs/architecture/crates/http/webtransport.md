---
status: draft
last_updated: 2026-06-29
---

# WebTransport — the h3 ALPN handler

The `HttpAdapter` registration for the `h3` ALPN: HTTP/3 and
WebTransport. This document covers the WebTransport session/stream
handling, the browser streaming path, and the relationship to the `h2`/
`http/1.1` server. The `h3` support is a first-class transport, not a
deferral (ADR-038).

## What

The `h3` ALPN handler is the same `HttpAdapter` instance that serves
`h2`/`http/1.1`, registered for the `h3` ALPN when the `h3` feature is
enabled. It serves two things on a single `h3` connection:

1. **HTTP/3 requests** — the standard HTTP/3 over QUIC framing. An
   HTTP/3 request is dispatched through the same axum `Router` as `h2`/
   `http/1.1` requests (ADR-036 — the HTTP path IS the operation path).
   From the axum router's perspective, an HTTP/3 request is just
   another HTTP request; the framing difference is handled below the
   router.
2. **WebTransport sessions** — the browser streaming path. A WebTransport
   session is a long-lived connection over which the browser opens
   bidirectional and unidirectional streams. A WebTransport stream that
   targets the call protocol is handed to the call protocol's dispatch
   loop directly — a WebTransport bidirectional stream is a QUIC
   bidirectional stream, the same stream type the call protocol already
   speaks (ADR-012).

### Why h3 is a first-class transport

WebTransport is the browser streaming transport. QUIC streams are cheap
(multiplexed over one connection, no head-of-line blocking), and
WebTransport is supported in major browsers. The call protocol's
subscription/streaming model maps onto WebTransport streams with no
translation loss — a `call.responded` stream over a WebTransport
bidirectional stream is the native representation, not an SSE
translation (which is the projection for `h2`/`http/1.1` clients per
ADR-036).

The Phase 0 research framing ("defer h3/WebTransport past v1") was a
residual of the "two-way door as deferral" anti-pattern (ADR-009 §"What
this framework is NOT"). WebTransport is in scope, in this crate, as a
first-class transport. See ADR-038.

## Architecture

### The h3 handler entry

The `HttpAdapter::handle()` method for the `h3` ALPN drives two
distinct stream types, distinguished at the HTTP/3 framing layer (not by
peeking application bytes):

1. **HTTP/3 request streams** — standard HTTP/3 GET/POST carrying
   `:method`/`:path`. These are the same request model as `h2`/
   `http/1.1`, just over HTTP/3 framing. Dispatched through the axum
   `Router` (same router as `h2`/`http/1.1`, ADR-036). An HTTP/3 request
   is never a WebTransport stream — the stream type is set by the
   HTTP/3 frame that opens it.
2. **WebTransport sessions** — opened by a browser's
   `new WebTransport(url)` call, which triggers an HTTP/3 extended
   CONNECT request. The handler accepts the session (the `wtransport`
   crate's `Endpoint::server(config)?.accept().await.await?.accept()
   .await?` pattern, or the quinn HTTP/3 endpoint's WebTransport
   extension — the exact library is a two-way-door implementation
   detail, ADR-038). Within an established session, the browser opens
   bidirectional streams via `transport.createBidirectionalStream()`;
   the handler accepts each via `session.accept_bi()`.

The two stream types are not disambiguated by "reading the first frame"
— they are distinguished by the HTTP/3 frame type that opens them
(regular request headers vs. extended CONNECT). The "first frame"
routing below applies *within* a WebTransport session, not between an
HTTP/3 request and a WebTransport stream.

### WebTransport session and stream handling

Once a WebTransport session is established (via extended CONNECT), the
browser creates bidirectional streams within it. The handler accepts
each stream (`session.accept_bi()`) and reads the first frame to
determine the sub-protocol:

- **Call-protocol `EventEnvelope`** — the stream is a call-protocol
  stream. The handler hands the `(SendStream, RecvStream)` pair to the
  call protocol's `Dispatcher` (see [../call/call-protocol.md](../call/call-protocol.md)
  for `EventEnvelope` and [../call/client-and-adapters.md](../call/client-and-adapters.md)
  §"Shared Dispatcher" for the `Dispatcher` — the same dispatch loop
  the `CallAdapter` uses for `alknet/call` connections, ADR-012,
  stream-agnostic correlation). The browser speaks the EventEnvelope
  wire format directly over the WebTransport stream.
- **Other sub-protocols** — a session may carry other framing
  conventions (e.g., a future WT-native RPC framing). The session's
  purpose is declared at CONNECT time (by path/origin), so the handler
  knows which sub-protocol to expect; the first-frame tag is a
  belt-and-suspenders disambiguator for sessions that multiplex
  sub-protocols. For the call-protocol session, the first frame is an
  `EventEnvelope` JSON object; the handler dispatches accordingly.

The browser's `WebTransport` JS API is the client side of this:
`new WebTransport('https://hub.example.com')` →
`transport.createBidirectionalStream()` → write an `EventEnvelope` frame
→ read `call.responded` frames. No SSE translation, no HTTP framing —
the call protocol speaks directly over the WebTransport stream.

### Subscription projection (native, not SSE)

A `Subscription` operation served over WebTransport projects its
`call.responded` stream directly onto the WebTransport bidirectional
stream — each `call.responded` event is a frame on the stream, no SSE
`data:` framing. `call.completed` closes the stream; `call.aborted`
closes the stream with an error frame. This is the native streaming
projection; SSE (ADR-036) is the projection for `h2`/`http/1.1` clients
that don't speak WebTransport.

### The TLS constraint (browsers require X.509)

Browsers do not support RFC 7250 raw public keys (ADR-027, OQ-12). A
WebTransport session from a browser requires an X.509 cert — the `h3`
handler is a domain-hosted-service concern, not a P2P concern. A node
serving WebTransport must have an X.509 identity
(`TlsIdentity::X509` or `TlsIdentity::Acme`).

This is a property of the browser, not a decision this spec makes. It's
recorded so the spec doesn't pretend a raw-key node can serve browsers.
A raw-key node serves `h2`/`http/1.1` (for curl, axios, alknet-native
clients) but not `h3`/WebTransport (for browsers). A browser-facing hub
has a `PeerEntry` with mixed fingerprints (Ed25519 for P2P, X.509 for
browsers — ADR-030, ADR-034 §3).

### Browsers are not alknet peers

A browser connecting to a hub over WebTransport is served by the `h3`
handler. The browser authenticates by bearer token (HTTP `Authorization`
header on the WebTransport session request), resolved by the hub's
`IdentityProvider::resolve_from_token` against the hub's
`PeerEntry.auth_token_hash` or `ApiKeyEntry`. The browser is **not** an
alknet peer (ADR-034 §4): it gets no `PeerId`, does not enter
`PeerCompositeEnv`, and its "ops" are WebTransport streams served by
the `h3` handler, not entries in the call-protocol peer-keyed overlay.

### Stealth on h3

The `h3` handler participates in the same stealth model as `h2`/
`http/1.1` (ADR-010, ADR-036): a client that offers `h3` gets the HTTP
handler. Unknown WebTransport paths and unknown HTTP/3 paths get the
decoy (the same configurable `DecoyConfig` — fake 404, static site,
redirect). Real services use `alknet/ssh`, `alknet/call`, etc.

### Implementation reference: wtransport

The `wtransport` crate (`/workspace/wtransport/`, v0.7.1) is a pure-Rust
WebTransport implementation built on `quinn` + `h3`/`qpack`. Its API:

```rust
// Server (from the wtransport README):
let config = ServerConfig::builder()
    .with_bind_default(4433)
    .with_identity(&identity)   // X.509 identity
    .build();
let connection = Endpoint::server(config)?
    .accept().await            // await connection
    .await?                    // await session request
    .accept().await?;          // await ready session
let stream = connection.accept_bi().await?;
```

`wtransport` is a candidate dependency for the `h3` feature gate. The
exact WebTransport library choice (wtransport vs a quinn-native HTTP/3
+ WebTransport extension) is a two-way-door implementation detail
(ADR-038); the one-way constraint is that `h3` is served by this crate
as a first-class transport.

## Constraints

- **`h3` requires X.509.** Browsers don't support RFC 7250 (ADR-027).
  A node serving `h3` must have an X.509 identity. Raw-key-only nodes
  serve `h2`/`http/1.1` but not `h3`.
- **`h3` is behind the `h3` feature gate.** The `wtransport` (or
  quinn HTTP/3 extension) dependency is heavier than `h2`/`http/1.1`;
  non-browser-facing deployments don't compile it.
- **Browsers are not alknet peers.** A browser over WebTransport
  authenticates by bearer token, gets no `PeerId` (ADR-034 §4).
- **WebTransport streams target the call protocol directly.** A
  WebTransport bidirectional stream carrying an `EventEnvelope` is
  handed to the call protocol's `Dispatcher` — no SSE translation, no
  HTTP framing. The browser speaks the call protocol wire format
  directly.
- **The HTTP/3 request path uses the same axum `Router` as `h2`/
  `http/1.1`.** An HTTP/3 request is just another HTTP request from
  the router's perspective (ADR-036).
- **WebTransport is a draft standard.** The `wtransport` README notes
  the protocol is not yet standardized; the API may change. The `h3`
  feature gate isolates the risk.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| `h3`/WebTransport is first-class | [ADR-038](../../decisions/038-http3-and-webtransport-as-first-class.md) | In scope, not deferred; browser streaming uses QUIC streams |
| Browsers require X.509 | [ADR-027](../../decisions/027-tls-identity-redesign-acme-rawkey-decoupling.md) | `h3` needs X.509 (browser limitation) |
| Browsers are not alknet peers | [ADR-034](../../decisions/034-outgoing-only-x509-and-three-peer-roles.md) | Bearer token, no `PeerId` |
| WebTransport streams → call protocol directly | [ADR-012](../../decisions/012-call-protocol-stream-model.md) | Stream-agnostic; WebTransport stream = QUIC bidirectional stream |
| Stealth on h3 | [ADR-010](../../decisions/010-alpn-router-and-endpoint.md) | Unknown paths get the decoy |
| HTTP path = operation path (for HTTP/3 requests) | [ADR-036](../../decisions/036-http-to-call-operation-mapping.md) | Same axum `Router` as h2/http1.1 |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-38** (open, scope): WebTransport relay-as-proxy — a proxy that
  terminates the browser's WebTransport connection and forwards to a
  P2P hub's Ed25519 endpoint (so the hub need not expose a public
  X.509 cert). Recorded in ADR-034 §5. Does the proxy live in
  `alknet-http` or a separate relay crate? This is a genuine scope
  question (the proxy use case is not yet concrete enough to decide the
  crate boundary), not a deferral.

## References

- [ADR-038](../../decisions/038-http3-and-webtransport-as-first-class.md)
  — the decision that `h3` is in scope
- [ADR-036](../../decisions/036-http-to-call-operation-mapping.md) —
  the HTTP-to-call mapping (the HTTP/3 request path uses the same
  axum `Router`)
- [overview.md](overview.md) — crate overview, feature gates
- [http-server.md](http-server.md) — the `h2`/`http/1.1` companion
- `/workspace/wtransport/` — pure-Rust WebTransport reference
  implementation (the `h3` feature's candidate dependency)