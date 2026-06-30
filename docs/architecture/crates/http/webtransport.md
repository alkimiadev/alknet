---
status: deferred
last_updated: 2026-06-30
---

# WebTransport — the h3 ALPN handler

> **DEFERRED per [ADR-044](../../decisions/044-defer-webtransport-browsers-use-websocket.md).**
> This spec is kept intact for revival. `h3`/WebTransport is not
> implemented in the initial `alknet-http` release; the browser
> bidirectional path uses WebSocket (see
> [http-server.md](http-server.md) §"WebSocket browser path"). ADR-038
> is superseded; ADR-040 and ADR-043 are parked (their decisions revive
> unchanged when WebTransport revives). The reversal trigger is a
> concrete deployment needing the ALPN-stream-proxy (a browser running
> a WASM SSH/SFTP/git client to reach a non-call ALPN). Two transfers
> apply during the deferment: ADR-043 §2 (call-protocol bidirectionality)
> and §3 (the no-`PeerId` connection-local overlay) apply over WebSocket
> unchanged; ADR-040 (the ALPN-stream-proxy) and ADR-043 §4 (the
> non-call-ALPN substrate) do not — they require WebTransport's stream
> model and revive with it.
>
> **Research note (for revival):** `wtransport` (v0.7.1, the reference
> implementation read during initial research) is *probably not* the
> right dependency choice at revival time, despite being a complete and
> readable implementation. The load-bearing integration concern is that
> the `h3` handler must route HTTP/3 requests through the same axum
> `Router` as `h2`/`http/1.1` (ADR-036), and `wtransport` owns its own
> HTTP serving path — bridging its request type into the `http::Request`
> axum consumes is cross-ecosystem adapter work. The hyperium stack
> (`h3` + `h3-quinn` + `h3-webtransport` + `h3-datagram`) operates at
> the stream level and produces `http::Request` types natively, which is
> a better fit for the axum integration — but its server-side
> WebTransport API needs verification before commitment (the axum-bridge
> feasibility is the load-bearing claim and is not yet confirmed against
> actual crate APIs, only against READMEs and design philosophy). This
> research is **not** run now (WebTransport is deferred); it is recorded
> here so the revival does not re-derive the question from scratch. See
> ADR-044 §"Research note (for revival)" for the cross-reference.

The `HttpAdapter` registration for the `h3` ALPN: HTTP/3 and
WebTransport. WebTransport is a **bidirectional ALPN transport
substrate** (ADR-043) — it carries ALPN protocols as bidirectional
streams, with the call protocol as the first/canonical target (needs no
WASM parser) and the ALPN-stream-proxy (ADR-040) as the mechanism for
non-call ALPNs (SSH, git, SFTP) that need a client-side parser. This
document covers the WebTransport session/stream handling, the
substrate's three stream destinations, the no-`PeerId` connection-local
overlay for non-peer clients, and the relationship to the `h2`/
`http/1.1` server (the one-directional projection WebTransport restores
bidirectionality for). The `h3` support is a first-class transport
(ADR-038).

## What

The `h3` ALPN handler is the same `HttpAdapter` instance that serves
`h2`/`http/1.1`, registered for the `h3` ALPN when the `h3` feature is
enabled. It serves two things on a single `h3` connection:

1. **HTTP/3 requests** — the standard HTTP/3 over QUIC framing. An
   HTTP/3 request is dispatched through the same axum `Router` as `h2`/
   `http/1.1` requests (ADR-042 + ADR-047 — the gateway endpoints are
   the sole invoke path; the direct-call `POST /{service}/{op}` surface
   was removed). From the axum router's perspective, an HTTP/3 request
   is just another HTTP request; the framing difference is handled
   below the router. The HTTP/3 request path is the **one-directional
   projection** (client→server calls only — HTTP is request/response;
   see [http-server.md](http-server.md) §"One-directional projection").
2. **WebTransport sessions** — the **bidirectional** path. WebTransport
   is a transport substrate that carries ALPN protocols as
   bidirectional streams (ADR-043), not a browser→hub one-way path. A
   WebTransport session is a long-lived connection over which either
   side can open bidirectional and unidirectional streams. Streams
   within a session target one of three destinations (see
   [ADR-040](../../decisions/040-webtransport-alpn-stream-proxy.md)):
   - The call protocol (`EventEnvelope` → the call `Dispatcher`) — the
     canonical target; needs no WASM parser because the EventEnvelope
     framing is platform/language/runtime agnostic (JSON-RPC over QUIC
     streams). Both sides can initiate calls — the call protocol's
     bidirectionality applies unchanged (ADR-043 §2,
     [../call/call-protocol.md](../call/call-protocol.md) §
     "Bidirectional Calls").
   - An ALPN handler proxy (the stream is handed to another ALPN
     handler like `SshAdapter` — the client runs a WASM parser for the
     target protocol). This is the substrate's mechanism for non-call
     ALPNs (SSH, git, SFTP) that need a parser on the client side
     (ADR-043 §4).
   - Another sub-protocol (declared at CONNECT time).

The ALPN-stream-proxy is what makes the browser a universal alknet
client: with a WASM parser for SSH (or SFTP, git), a browser can reach
any ALPN handler via WebTransport, no install, no native client, no
VPN. This is the "VPN-like without being a VPN" use case the project
was originally built for, now on a clean foundation. See
[ADR-040](../../decisions/040-webtransport-alpn-stream-proxy.md) and
the substrate framing in [ADR-043](../../decisions/043-webtransport-bidirectional-alpn-substrate.md).

### Why h3 is a first-class transport

WebTransport is the bidirectional streaming transport for the call
protocol and a transport substrate for any ALPN. QUIC streams are
cheap (multiplexed over one connection, no head-of-line blocking), and
WebTransport is supported in major browsers and beyond (Deno, Node,
native Rust). The call protocol's subscription/streaming model maps
onto WebTransport streams with no translation loss — a `call.responded`
stream over a WebTransport bidirectional stream is the native
representation, not an SSE translation (which is the projection for
`h2`/`http/1.1` clients per ADR-036).

More importantly, **WebTransport restores the call protocol's
bidirectionality** that the HTTP/1.1 + HTTP/2 surface structurally
cannot carry. HTTP is request/response — the client initiates, the
server responds; the server→client *call* direction has no HTTP
expression (see [http-server.md](http-server.md) §"One-directional
projection"). WebTransport is a long-lived connection over which either
side can open bidirectional streams and send `call.requested` in either
direction — the call protocol's native bidirectionality applies
unchanged (ADR-043 §2). WebTransport is also supported beyond browsers
(Deno, Node, native Rust via `wtransport`), and the call protocol —
JSON-RPC over QUIC streams — is platform/language/runtime agnostic, so
call-protocol-over-WebTransport is a general bidirectional RPC
substrate, not a browser-only path (ADR-043 §1).

WebTransport is in scope, in this crate, as a first-class transport
(ADR-038). See [ADR-043](../../decisions/043-webtransport-bidirectional-alpn-substrate.md)
for the substrate framing.

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
client creates bidirectional streams within it. The handler dispatches
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
  stream-agnostic correlation). The client speaks the EventEnvelope
  wire format directly over the WebTransport stream.

  **Bidirectionality (ADR-043 §2):** the call-protocol session inherits
  the call protocol's native bidirectionality — both sides can initiate
  calls. The client calls operations on the hub; the hub can call
  operations registered on the client's side, over the same session,
  using the same `PendingRequestMap` and `EventEnvelope` framing as
  `alknet/call` (see [../call/call-protocol.md](../call/call-protocol.md)
  §"Bidirectional Calls"). The browser case (ADR-034 §4) is the common
  case where the client registers no operations of its own, so the
  server→client call direction is unused — that is a use-case scoping,
  not an architectural limitation. A non-browser WebTransport client
  (Deno, Node, a peer preferring WebTransport) that registers
  operations receives calls from the hub over the same session.
- **`/alknet/<name>` → ALPN-handler proxy session.** Each bidirectional
  stream is handed to the target ALPN handler (e.g., `SshAdapter` for
  `/alknet/ssh`, `GitAdapter` for `/alknet/git`) as a `Connection`
  wrapping the WebTransport stream. The client runs a WASM parser for
  the target protocol and speaks it directly over the stream. This is
  the substrate's mechanism for non-call ALPNs (ADR-043 §4) — the
  ALPN-stream-proxy — see
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

The browser's `WebTransport` JS API is one client side of this:
`new WebTransport('https://hub.example.com/alknet/ssh')` →
`transport.createBidirectionalStream()` → the browser's WASM SSH client
reads/writes the stream as a `BiStream` (ADR-007). No SSE translation,
no HTTP framing — the target protocol speaks directly over the
WebTransport stream. For the call-protocol session, the browser writes
`EventEnvelope` frames; for an SSH session, the browser runs the WASM
SSH parser. A non-browser client (Deno, Node, native Rust) speaks the
same wire formats over the same substrate without a WASM parser — the
call protocol needs no parser, and native ALPN clients (SSH, git) use
native parsers rather than WASM.

### Subscription projection (native, not SSE)

A `Subscription` operation served over WebTransport projects its
`call.responded` stream directly onto the WebTransport bidirectional
stream — each `call.responded` event is a frame on the stream, no SSE
`data:` framing. `call.completed` closes the stream; `call.aborted`
closes the stream with an error frame. This is the native streaming
projection; SSE (ADR-036) is the projection for `h2`/`http/1.1` clients
that don't speak WebTransport.

### ALPN-stream-proxy (ADR-040, repositioned by ADR-043 §4)

The ALPN-stream-proxy is the `h3` handler's third stream destination and
the substrate's mechanism for non-call ALPNs — the protocols (SSH, git,
SFTP) that need a client-side parser, unlike the call protocol which
speaks EventEnvelope directly. ADR-040 framed it as "the browser's
gateway to every ALPN handler"; ADR-043 §4 repositions it as the
substrate's non-call-ALPN mechanism, of which the browser use case is
the primary (but not the only) instance. The decision in ADR-040 (the
`HandlerRegistry` reference, path-based routing) stands unchanged; the
framing is what ADR-043 refines.

The browser use case: a browser opens a WebTransport session to
`/alknet/ssh` (or `/alknet/git`, `/alknet/sftp`), and the `h3` handler
hands each bidirectional stream within that session to the target ALPN
handler as a `Connection`. The browser runs a WASM parser for the
target protocol and speaks it directly over the stream.

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

### The no-`PeerId` connection-local overlay (ADR-043 §3)

A non-peer WebTransport client (a browser, or any WebTransport client
that is not a `PeerEntry`-bearing alknet peer) has **no `PeerId` on the
hub's side**. The connection is served by the `h3` handler; the
bearer-token-resolved `Identity` authorizes calls via
`AccessControl::check`, but the connection does not enter
`PeerCompositeEnv` and has no peer-keyed overlay entry. This is the
**inbound mirror of ADR-034 §2** (the outgoing pure-client X.509 case:
ops discovered land in "that connection's Layer 2 overlay" —
connection-local, no `PeerId`). On the inbound WebTransport path, ops
the client registers (if any) land in a connection-local Layer 2
overlay on the hub side — same pattern, opposite direction.

The `CallAdapter`'s `compose_root_env` builds the root
`OperationContext.env` from:

- the curated base (Layer 0),
- **this connection's** local overlay (Layer 2 — connection-scoped, not
  peer-keyed), and
- the active session overlay (if any, ADR-024).

There is no `PeerCompositeEnv` entry because there is no `PeerId` to key
it. An implementer building `compose_root_env` for a WebTransport
session applies the ADR-034 §2 connection-local-overlay pattern (mirror
direction) and does not hunt for a `PeerId` that isn't there.

The case where the WebTransport client *is* a `PeerEntry`-bearing
alknet peer (a hub or spoke node that prefers WebTransport as its
transport) is the symmetric case: the connection has a `PeerId`
(resolved from the bearer token via
`IdentityProvider::resolve_from_token` → `Identity.id` =
`PeerEntry.peer_id`, ADR-030), and ops the peer registers land in the
peer-keyed overlay, exactly as they would over `alknet/call`. The
no-`PeerId` pattern above is the *non-peer* case; the peer case is
unchanged from the `alknet/call` model. See ADR-043 §3.

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
  (`EventEnvelope` → `Dispatcher`, bidirectional — both sides can
  initiate calls), an ALPN handler proxy (→ `HandlerRegistry` lookup
  → target handler's `handle()`, the substrate's non-call-ALPN
  mechanism), or another sub-protocol. See ADR-040 and ADR-043.
- **The call-protocol WebTransport session is bidirectional.** Both
  sides can initiate calls, inheriting the call protocol's native
  bidirectionality (ADR-043 §2). The browser case where the client
  registers no ops is a use-case scoping, not an architectural
  limitation.
- **Non-peer WebTransport clients use a connection-local overlay.**
  A WebTransport client with no `PeerId` (browser, or any non-peer
  client) has its registered ops land in a connection-local Layer 2
  overlay, not the peer-keyed `PeerCompositeEnv`. This is the inbound
  mirror of ADR-034 §2. See ADR-043 §3.
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

> **Note:** This table reflects the design as written for revival. ADR-038
> is **superseded by [ADR-044](../../decisions/044-defer-webtransport-browsers-use-websocket.md)**;
> ADR-040 and ADR-043 are **parked** (implementation deferred per ADR-044).
> The decisions revive unchanged when WebTransport revives — see the
> header note and ADR-044 for the scope rationale and reversal trigger.

| Decision | ADR | Summary |
|----------|-----|---------|
| ~~`h3`/WebTransport is first-class~~ | [ADR-038](../../decisions/038-http3-and-webtransport-as-first-class.md) | **Superseded by ADR-044** (scope deferral); originally "in scope, not deferred; browser streaming uses QUIC streams" |
| WebTransport is a bidirectional ALPN transport substrate | [ADR-043](../../decisions/043-webtransport-bidirectional-alpn-substrate.md) | **Parked** per ADR-044. Carries ALPNs as bidirectional streams; call protocol is the first/canonical target (needs no WASM parser); both sides can initiate calls |
| WebTransport ALPN-stream-proxy | [ADR-040](../../decisions/040-webtransport-alpn-stream-proxy.md) | **Parked** per ADR-044. The substrate's mechanism for non-call ALPNs (SSH, git, SFTP) — browser → WebTransport stream → target ALPN handler via WASM parser |
| Browsers require X.509 | [ADR-027](../../decisions/027-tls-identity-redesign-acme-rawkey-decoupling.md) | `h3` needs X.509 (browser limitation; applies when WebTransport revives) |
| Browsers are not alknet peers | [ADR-034](../../decisions/034-outgoing-only-x509-and-three-peer-roles.md) §4 (amended by ADR-044 §5) | Bearer token, no `PeerId` (rationale in ADR-044 §5) |
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
- [ADR-043](../../decisions/043-webtransport-bidirectional-alpn-substrate.md)
  — the substrate framing: WebTransport carries ALPNs as bidirectional
  streams; call protocol is the first target; bidirectionality; the
  no-`PeerId` connection-local overlay
- [ADR-036](../../decisions/036-http-to-call-operation-mapping.md) —
  the HTTP-to-call mapping (the HTTP/3 request path uses the same
  axum `Router`)
- [overview.md](overview.md) — crate overview, feature gates
- [http-server.md](http-server.md) — the `h2`/`http/1.1` companion
  (§"One-directional projection" — the lossy HTTP/1.1+HTTP/2 surface
  WebTransport restores bidirectionality for)
- `/workspace/wtransport/` — pure-Rust WebTransport reference
  implementation (the `h3` feature's candidate dependency)