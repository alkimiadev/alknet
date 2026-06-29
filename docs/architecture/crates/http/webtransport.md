---
status: draft
last_updated: 2026-06-29
---

# WebTransport — the h3 ALPN handler

The `HttpAdapter` registration for the `h3` ALPN: HTTP/3 and
WebTransport. This document covers the WebTransport session/stream
handling, the browser streaming path, the ALPN-stream-proxy (browser
access to any ALPN handler via WebTransport), and the relationship to
the `h2`/`http/1.1` server. The `h3` support is a first-class transport,
not a deferral (ADR-038).

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
   bidirectional and unidirectional streams. Streams within a session
   target one of three destinations (see [ADR-040](../../decisions/040-webtransport-alpn-stream-proxy.md)):
   - The call protocol (`EventEnvelope` → the call `Dispatcher`),
   - An ALPN handler proxy (the stream is handed to another ALPN
     handler like `SshAdapter` — the browser runs a WASM parser for the
     target protocol), or
   - Another sub-protocol (declared at CONNECT time).

The ALPN-stream-proxy is what makes the browser a universal alknet
client: with a WASM parser for SSH (or SFTP, git), a browser can reach
any ALPN handler via WebTransport, no install, no native client, no
VPN. This is the "VPN-like without being a VPN" use case the project
was originally built for, now on a clean foundation. See
[ADR-040](../../decisions/040-webtransport-alpn-stream-proxy.md).

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
browser creates bidirectional streams within it. The handler dispatches
each stream to one of three destinations, determined by the session's
CONNECT path (the routing key, declared at CONNECT time — not by peeking
the first application frame):

- **`/` or `/alknet/call` → call-protocol session.** Each bidirectional
  stream carries call-protocol `EventEnvelope` frames. The handler
  hands the `(SendStream, RecvStream)` pair to the call protocol's
  `Dispatcher` (see [../call/call-protocol.md](../call/call-protocol.md)
  for `EventEnvelope` and [../call/client-and-adapters.md](../call/client-and-adapters.md)
  §"Shared Dispatcher" for the `Dispatcher` — the same dispatch loop
  the `CallAdapter` uses for `alknet/call` connections, ADR-012,
  stream-agnostic correlation). The browser speaks the EventEnvelope
  wire format directly over the WebTransport stream.
- **`/alknet/<name>` → ALPN-handler proxy session.** Each bidirectional
  stream is handed to the target ALPN handler (e.g., `SshAdapter` for
  `/alknet/ssh`, `GitAdapter` for `/alknet/git`) as a `Connection`
  wrapping the WebTransport stream. The browser runs a WASM parser for
  the target protocol and speaks it directly over the stream. This is
  the ALPN-stream-proxy — see
  [ADR-040](../../decisions/040-webtransport-alpn-stream-proxy.md).
  The `h3` handler looks up the target ALPN handler in the
  `HandlerRegistry` (`HttpAdapter` holds `Arc<HandlerRegistry>` for
  this purpose), wraps the WebTransport stream as a `Connection`, and
  calls `handler.handle(connection, &auth)`. The target handler runs
  its normal protocol over the stream — SSH key exchange, git smart
  protocol, SFTP — exactly as if the stream had arrived on that ALPN
  via a native QUIC connection.
- **Other paths → other sub-protocols.** Sessions may carry other
  framing conventions; the session's purpose is declared at CONNECT
  time by path/origin. The first-frame tag is a belt-and-suspenders
  confirmation for sessions that multiplex sub-protocols, not the
  routing mechanism.

The browser's `WebTransport` JS API is the client side of this:
`new WebTransport('https://hub.example.com/alknet/ssh')` →
`transport.createBidirectionalStream()` → the browser's WASM SSH client
reads/writes the stream as a `BiStream` (ADR-007). No SSE translation,
no HTTP framing — the target protocol speaks directly over the
WebTransport stream. For the call-protocol session, the browser writes
`EventEnvelope` frames; for an SSH session, the browser runs the WASM
SSH parser.

### Subscription projection (native, not SSE)

A `Subscription` operation served over WebTransport projects its
`call.responded` stream directly onto the WebTransport bidirectional
stream — each `call.responded` event is a frame on the stream, no SSE
`data:` framing. `call.completed` closes the stream; `call.aborted`
closes the stream with an error frame. This is the native streaming
projection; SSE (ADR-036) is the projection for `h2`/`http/1.1` clients
that don't speak WebTransport.

### ALPN-stream-proxy (ADR-040)

The ALPN-stream-proxy is the `h3` handler's third stream destination and
the browser's gateway to every ALPN handler. A browser opens a
WebTransport session to `/alknet/ssh` (or `/alknet/git`, `/alknet/sftp`),
and the `h3` handler hands each bidirectional stream within that session
to the target ALPN handler as a `Connection`. The browser runs a WASM
parser for the target protocol and speaks it directly over the stream.

**Why this matters:** SSH-over-WebTransport is HTTPS-shaped at the
network layer (WebTransport is HTTP/3 over QUIC over UDP, the same as
HTTP/3). Blocking it requires blocking HTTP/3, which breaks the web.
This is the anti-censorship property — the protocol that governments
most want to block (VPN-like connectivity) rides on the protocol they
can't block without breaking the web. This is the "VPN-like without
being a VPN" use case on a clean foundation.

**The WASM client side:** the browser's WASM parser for the target
protocol (SSH, SFTP, git) reads/writes the WebTransport stream as a
`BiStream` (ADR-007). The `BiStream` trait (`AsyncRead + AsyncWrite +
Send + Unpin`) was designed for this — a browser implements it over a
WebTransport stream, and the WASM parser speaks the protocol over it.
The WASM parsers are downstream artifacts (the SSH WASM client, the
SFTP WASM client), not part of `alknet-http`; `russh-sftp`'s WASM
targeting demonstrates feasibility, SSH is the next target.

**Auth for proxied ALPN sessions:** the browser authenticates by bearer
token on the WebTransport session request (the HTTP `Authorization`
header on the CONNECT request), resolved by the hub's
`IdentityProvider::resolve_from_token` — same as any other browser
connection (ADR-034 §4). The browser is not an alknet peer (no
`PeerId`). The target ALPN handler receives the `Connection` and
`AuthContext` from the `h3` handler; the `AuthContext` carries the
bearer-token-resolved `Identity`. The target protocol then runs its
own auth (the browser's WASM SSH client does SSH key exchange over the
WebTransport stream, same as a native SSH client over a QUIC stream).
Two layers: the bearer token gates the WebTransport session (does the
browser have access to this hub?); the protocol's own auth gates the
protocol session (does this SSH identity have access to this shell?).

**The `HandlerRegistry` reference:** the `HttpAdapter` holds
`Arc<HandlerRegistry>` so the `h3` handler can look up the target ALPN
handler. The assembly layer constructs the `HttpAdapter` with the
`HandlerRegistry` it already builds for the endpoint — no new
registry, no new construction path. The `HandlerRegistry` is static at
startup (ADR-010), so the lookup is against an immutable registry. See
[ADR-040](../../decisions/040-webtransport-alpn-stream-proxy.md).

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
- **WebTransport streams target one of three destinations** (the
  session's CONNECT path is the routing key): the call protocol
  (`EventEnvelope` → `Dispatcher`), an ALPN handler proxy (→
  `HandlerRegistry` lookup → target handler's `handle()`), or another
  sub-protocol. See ADR-040.
- **The ALPN-stream-proxy requires `Arc<HandlerRegistry>` on
  `HttpAdapter`.** The `h3` handler looks up ALPN handlers in the
  registry; the `h2`/`http/1.1` path does not use it. The registry is
  static at startup (ADR-010).
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
| WebTransport ALPN-stream-proxy | [ADR-040](../../decisions/040-webtransport-alpn-stream-proxy.md) | Browser → WebTransport stream → any ALPN handler (SSH, git, SFTP) via WASM parser |
| Browsers require X.509 | [ADR-027](../../decisions/027-tls-identity-redesign-acme-rawkey-decoupling.md) | `h3` needs X.509 (browser limitation) |
| Browsers are not alknet peers | [ADR-034](../../decisions/034-outgoing-only-x509-and-three-peer-roles.md) | Bearer token, no `PeerId` |
| WebTransport streams → call protocol directly | [ADR-012](../../decisions/012-call-protocol-stream-model.md) | Stream-agnostic; WebTransport stream = QUIC bidirectional stream |
| `BiStream` is a trait (WASM door) | [ADR-007](../../decisions/007-bistream-type-definition.md) | Browser implements `BiStream` over WebTransport stream; WASM parser speaks the protocol |
| Stealth on h3 | [ADR-010](../../decisions/010-alpn-router-and-endpoint.md) | Unknown paths get the decoy |
| HTTP path = operation path (for HTTP/3 requests) | [ADR-036](../../decisions/036-http-to-call-operation-mapping.md) | Same axum `Router` as h2/http1.1 |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-38** (open, scope): WebTransport relay-as-proxy — the
  *standalone relay service* (a future `alknet-relay` crate, fork of
  iroh-relay with WebTransport-based proxy fallback). This is distinct
  from the in-process ALPN-stream-proxy (ADR-040, in `alknet-http`).
  See OQ-38 for the relay crate boundary question.

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