# ADR-043: WebTransport as a Bidirectional ALPN Transport Substrate

## Status

**Proposed — implementation deferred per [ADR-044](044-defer-webtransport-browsers-use-websocket.md).**

This ADR's decision is correct and is not superseded. It revives unchanged
when WebTransport revives, **with two transfers to WebSocket that apply
during the deferment**:

- **§2 (call-protocol bidirectionality) transfers to WebSocket unchanged.**
  WebSocket is full-duplex; the call protocol's bidirectionality applies over
  a WS connection exactly as §2 describes for WebTransport. The browser case
  where the client registers no ops remains a use-case scoping, not an
  architectural limitation.
- **§3 (the no-`PeerId` connection-local overlay) transfers to WebSocket
  unchanged.** A browser over WSS has no `PeerId` on the hub's side for the
  same reasons it has none over WebTransport (ADR-044 §5); the
  connection-local Layer 2 overlay applies. The pattern is transport-agnostic.

What does **not** transfer to WebSocket is §4 (the non-call-ALPN substrate
mechanism / the ALPN-stream-proxy, ADR-040) and §5's WebTransport-specific
framing. Those require WebTransport's stream model and revive with it.
ADR-044 §3 states the transfer explicitly; ADR-044 §5 states the
"browser is not a peer" rationale (addressability vs. bidirectionality)
that this ADR's §3 relies on but does not argue.

## Context

`alknet-http`'s `h3`/WebTransport specs
([webtransport.md](../crates/http/webtransport.md),
[ADR-040](040-webtransport-alpn-stream-proxy.md)) describe the
WebTransport session as a browser-reached path: a browser opens a
WebTransport session to a hub, the hub's `h3` handler serves it. The
two stream destinations described (call-protocol `EventEnvelope`, and
the ALPN-handler proxy) are both framed browser→server: the browser
initiates, the hub responds.

That framing is correct for the browser case (ADR-034 §4 — browsers are
not alknet peers; they connect to a hub and authenticate by bearer
token), but it is **not the general case**, and writing the spec as if
it were leaks an assumption that is only true for the OpenAPI/MCP
direction model into the WebTransport architecture. Three concrete
problems result:

### Problem 1 — the call protocol is bidirectional; the WebTransport spec is not

The call protocol is explicitly bidirectional
([call-protocol.md](../crates/call/call-protocol.md) §"Bidirectional
Calls"): *"Both sides of the connection can initiate calls. The server
can call operations on the client just as the client calls operations
on the server."* The `CallConnection`/`Dispatcher` dispatch loop is
stream-agnostic (ADR-012) — a WebTransport bidirectional stream is a
QUIC bidirectional stream, and the call protocol's bidirectionality
applies unchanged over it.

The current `webtransport.md` describes only the browser-initiates-a-
call direction. A reader would reasonably conclude WebTransport is a
one-directional session (browser calls hub, hub responds), when in
fact a WebTransport call-protocol session inherits the call protocol's
bidirectionality: the hub can call operations registered on the
browser/WebTransport-client side, exactly as it can over `alknet/call`.
The spec doesn't say this, doesn't scope it down, and doesn't say *why*
it's scoped down. It's just silent.

### Problem 2 — the ALPN-stream-proxy is framed as "browser reaches hub ALPNs via WASM," not as "WebTransport carries ALPNs as streams"

ADR-040 frames the ALPN-stream-proxy as the browser's gateway to every
ALPN handler: a browser with a WASM parser for SSH (or SFTP, git) can
reach any ALPN handler via WebTransport. That framing is correct and
important (the anti-censorship property — SSH-over-WebTransport is
HTTPS-shaped — is real). But it bakes the browser-initiated direction
into the architecture.

WebTransport is more general than that: a WebTransport stream is a
QUIC bidirectional stream (ADR-012), and the `BiStream` trait
(`AsyncRead + AsyncWrite + Send + Unpin`, ADR-007) is source-agnostic.
WebTransport can carry **any** ALPN protocol as streams, in either
direction, between any two endpoints that can terminate WebTransport —
not only browser→hub. The call protocol is the **first/canonical**
target because it is already JSON-RPC over QUIC streams and needs no
WASM parser (the EventEnvelope framing is platform/language/runtime
agnostic), but it is one target among possible many. SSH, git, SFTP
are additional targets that require a WASM parser on the client side.

The current framing — "browser runs a WASM parser that reaches the
hub's ALPN handler" — is a *use case* of the proxy, not the *nature* of
it. The nature is: **WebTransport is a transport substrate that carries
ALPN protocols as bidirectional streams; the call protocol is the
straightforward first target, and any other ALPN can be proxied the same
way.**

### Problem 3 — "browsers are not peers" reconciles awkwardly with the WebTransport call session, and the reconciliation isn't stated

ADR-034 §4 establishes that a browser over WebTransport authenticates by
bearer token, gets no `PeerId`, and doesn't enter `PeerCompositeEnv`
(the peer-keyed overlay). ADR-034 §2 establishes the analogous
**outgoing** case: a pure-client X.509 dial has no client-side `PeerId`,
and ops discovered via `from_call`/`from_openapi`/`from_mcp` land in
"that connection's Layer 2 overlay" — connection-local, not in the
peer-keyed overlay.

The **inbound** WebTransport case is the mirror of ADR-034 §2: a
browser (or any non-peer WebTransport client) connects to a hub, the
hub's `h3` handler hands its streams to the call protocol's
`Dispatcher`, and the connection has no `PeerId` on the hub's side
either. Ops the browser registers (if it registers any — e.g., a
browser-based agent exposing local ops) land in a connection-local
Layer 2 overlay, exactly like the outgoing pure-client X.509 case.
`compose_root_env` builds the root `OperationContext.env` from the
curated base + that connection's local overlay + (if active) the
session overlay — *without* a peer-keyed entry, because there is no
`PeerId` to key it.

The current `webtransport.md` doesn't say this. A reader would
reasonably ask: *if this is the same `Dispatcher` as `alknet/call`,
where's the `PeerId`? how does `compose_root_env` build the root env for
a no-`PeerId` WebTransport call session?* The answer exists — it's the
ADR-034 §2 connection-local-overlay pattern applied inbound — it's
just not written down in the http crate.

## Decision

### 1. WebTransport is a bidirectional ALPN transport substrate; the call protocol is the first target

The `h3`/WebTransport handler is reframed: WebTransport is a
**transport substrate** that carries ALPN protocols as bidirectional
streams, not a browser→hub one-way path. The call protocol is the
**first/canonical target** — it is already JSON-RPC over QUIC streams
(ADR-012), needs no WASM parser (the EventEnvelope framing is
platform/language/runtime agnostic), and is supported in runtimes that
speak WebTransport (Deno, Node, browsers, native Rust via `wtransport`).
Other ALPN protocols (SSH, git, SFTP) are additional targets that
require a WASM parser on the browser/client side; the ALPN-stream-proxy
(ADR-040) is the mechanism for those targets. The call-protocol-over-
WebTransport path needs no proxy — it speaks the EventEnvelope wire
format directly.

This is a **framing** change to ADR-040 and `webtransport.md`, not a
structural change. The three stream destinations (call protocol,
ALPN-handler proxy, other sub-protocols) are unchanged; what changes is
how they are described. The call-protocol destination is the substrate's
canonical use; the ALPN-handler proxy is the substrate carrying other
ALPNs. The browser→hub direction is one use case of the substrate, not
its definition.

### 2. The WebTransport call-protocol session inherits the call protocol's bidirectionality

A WebTransport session opened to `/` or `/alknet/call` is a
call-protocol session. Within it, **both sides can initiate calls** —
the WebTransport client can call operations on the hub, and the hub can
call operations registered on the WebTransport client's side. This is
the call protocol's native bidirectionality (call-protocol.md §
"Bidirectional Calls"), applying unchanged over the WebTransport stream.
The `Dispatcher` is the same dispatch loop the `CallAdapter` uses for
`alknet/call` connections (ADR-012 — stream-agnostic correlation).

The browser case (ADR-034 §4) is the common case: a browser connects
to a hub, calls the hub's operations, and registers no operations of
its own — the server→client call direction is unused because the browser
has nothing to call. That is a use-case scoping, not an architectural
limitation. A non-browser WebTransport client (a Deno process, a Node
process, another alknet node that prefers WebTransport over raw
`alknet/call` QUIC) that registers operations on its side receives
calls from the hub over the same session. The spec must state this,
not leave it implicit.

### 3. The no-`PeerId` connection-local overlay (inbound mirror of ADR-034 §2)

A WebTransport call-protocol session from a non-peer client (a browser,
or any WebTransport client that is not a `PeerEntry`-bearing alknet
peer) has **no `PeerId` on the hub's side**. The connection is served by
the `h3` handler; the browser/client authenticates by bearer token
(ADR-034 §4); the resolved `Identity` authorizes calls via
`AccessControl::check`, but the connection does not enter
`PeerCompositeEnv` and has no peer-keyed overlay entry.

This is the **inbound mirror of ADR-034 §2** (the outgoing pure-client
X.509 case). Outbound: a `CallClient` dials a public X.509 endpoint,
ops discovered land in "that connection's Layer 2 overlay" —
connection-local, no `PeerId`. Inbound: a WebTransport client connects
to a hub, ops the client registers (if any) land in a connection-local
Layer 2 overlay on the hub side — same pattern, opposite direction. The
`CallAdapter`'s `compose_root_env` builds the root
`OperationContext.env` from:

- the curated base (Layer 0),
- **this connection's** local overlay (Layer 2 — connection-scoped, not
  peer-keyed), and
- the active session overlay (if any, ADR-024).

There is no `PeerCompositeEnv` entry because there is no `PeerId` to key
it. This is the explicit closure of the "browser as peer" path
(ADR-034 §4) on the inbound side — the same closure ADR-034 §2 makes on
the outbound side. `webtransport.md` must state it so an implementer
building `compose_root_env` for a WebTransport session knows the
connection-local-overlay pattern applies and does not hunt for a
`PeerId` that isn't there.

The case where the WebTransport client *is* a `PeerEntry`-bearing
alknet peer (a hub or spoke node that prefers WebTransport as its
transport) is the symmetric case: the connection has a `PeerId`
(resolved from the bearer token via `IdentityProvider::resolve_from_token`
→ `Identity.id` = `PeerEntry.peer_id`, ADR-030), and ops the peer
registers land in the peer-keyed overlay, exactly as they would over
`alknet/call`. The no-`PeerId` pattern above is the *non-peer* case; the
peer case is unchanged from the `alknet/call` model.

### 4. ADR-040's ALPN-stream-proxy is the substrate's mechanism for non-call ALPNs

ADR-040 (the ALPN-stream-proxy) is not superseded by this ADR; it is
**repositioned**. The proxy is the substrate's mechanism for carrying
ALPN protocols *other than the call protocol* — SSH, git, SFTP — that
require a WASM parser on the client side. The call protocol needs no
proxy (it speaks EventEnvelope directly); the ALPN-stream-proxy is for
the protocols that do. The browser→hub direction is the primary use
case (a browser with a WASM SSH client reaching the hub's SSH handler),
but it is not the only one — any WebTransport-capable endpoint can
proxy any ALPN via the same mechanism.

This reframing does not change ADR-040's decision (the `h3` handler
gains `Arc<HandlerRegistry>`, streams route by CONNECT path); it
changes how the decision is described. The "three stream destinations"
in `webtransport.md` remain; what changes is the framing of the
ALPN-stream-proxy as the substrate's non-call-ALPN mechanism, not as
the browser's gateway.

### 5. HTTP/1.1 + HTTP/2 is the one-directional projection; WebTransport is the bidirectional one

The HTTP/1.1 + HTTP/2 surface projects the call protocol
one-directionally (client→server calls only — HTTP is request/response;
the server→client call direction has no HTTP expression). This is
named as a lossy consequence of HTTP in `http-server.md` §
"One-directional projection." WebTransport is the HTTP-family transport
that **restores** the call protocol's bidirectionality: a WebTransport
session is a long-lived connection over which either side can open
streams and send `call.requested` in either direction. The two surfaces
coexist on the `h3` ALPN (HTTP/3 requests use the axum `Router` — the
one-directional projection; WebTransport sessions use the call
protocol `Dispatcher` — the bidirectional one). An HTTP/3 request is
never a WebTransport stream, and vice versa (the HTTP/3 frame type
distinguishes them — see `webtransport.md`).

## Consequences

**Positive:**
- The WebTransport spec stops silently inheriting the OpenAPI/MCP
  direction assumption. The call protocol's bidirectionality is named
  as a property of WebTransport call sessions, not left implicit.
- The ALPN-stream-proxy is framed as the substrate's non-call-ALPN
  mechanism, not as a browser-only gateway. The call protocol is named
  as the first/canonical target — the easy case that needs no WASM
  parser and runs in Deno, Node, and browsers.
- The inbound no-`PeerId` connection-local overlay is stated, so an
  implementer building `compose_root_env` for a WebTransport session
  applies the ADR-034 §2 pattern (mirror direction) and does not hunt
  for a `PeerId`.
- The HTTP/1.1 + HTTP/2 one-directional projection is named as a lossy
  consequence, and WebTransport is named as the surface that restores
  bidirectionality. The two surfaces' relationship is clear.
- A non-browser WebTransport client (Deno, Node, a peer preferring
  WebTransport) is a first-class case, not an accident of the spec's
  browser framing.

**Negative:**
- The WebTransport spec gains complexity: the browser-only framing was
  simpler to describe. The bidirectional framing requires stating both
  the browser case (no registered ops, server→client call direction
  unused) and the non-browser case (registered ops, bidirectional
  calls). This is honest complexity — the substrate is more general
  than the browser-only framing suggested.
- The "browser is not a peer" property (ADR-034 §4) now has a
  counterpart statement for the inbound overlay path. Readers must
  understand two cases: peer WebTransport clients (in the peer-keyed
  overlay) and non-peer WebTransport clients (in the connection-local
  overlay). This mirrors the outbound ADR-034 §2/§3 split and is not
  new structural complexity, but it is now stated in the http crate,
  which it wasn't before.
- The ALPN-stream-proxy's reframing (substrate mechanism for non-call
  ALPNs, not browser gateway) means ADR-040's prose reads slightly
  differently from the spec's prose. ADR-040 is not superseded; its
  *decision* (the `HandlerRegistry` reference, path-based routing)
  stands. Its *framing* is repositioned by this ADR. A future amendment
  to ADR-040 could inline the repositioning; for now this ADR records
  it and `webtransport.md` reflects it.

## Assumptions

1. **The call protocol's bidirectionality applies unchanged over
   WebTransport.** The `Dispatcher` is stream-agnostic (ADR-012); a
   WebTransport bidirectional stream is a QUIC bidirectional stream.
   No protocol change is needed to support server→client calls over
   WebTransport — the same `call.requested`/`call.responded` framing
   works in both directions, correlated by request ID, as it does over
   `alknet/call`.

2. **The browser case is the common non-peer case; non-browser
   WebTransport clients are the general case.** Most WebTransport
   clients in v1 are browsers (the anti-censorship / universal-client
   use case). Non-browser WebTransport clients (Deno, Node, native
   Rust) are supported by the same code path; they may or may not be
   peers depending on whether they present a `PeerEntry`-resolvable
   bearer token. The spec describes both cases; the implementation is
   one code path with a branch on "does this connection have a
   `PeerId`?" at `compose_root_env` time.

3. **The ALPN-stream-proxy is not the only mechanism for non-call ALPNs
   over WebTransport.** A future WebTransport session type could carry
   non-call ALPNs without the proxy's `HandlerRegistry` lookup (e.g., a
   session that negotiates a single ALPN at CONNECT time and speaks it
   directly, without per-stream registry routing). The proxy is the
   mechanism specified by ADR-040; this ADR does not foreclose others,
   but does not spec them either (scope — not needed for the current
   use cases).

4. **`PeerId` resolution for peer WebTransport clients follows the
   same path as `alknet/call`.** A peer connecting over WebTransport
   presents a bearer token; the hub resolves it via
   `IdentityProvider::resolve_from_token`; the resulting `Identity.id`
   is the `PeerId` (ADR-030). There is no WebTransport-specific peer
   resolution path — the bearer-token path is the same regardless of
   transport. This is an assumption, not a new decision: it follows from
   ADR-004, ADR-030, and ADR-034 §4.

## References

- [ADR-012](012-call-protocol-stream-model.md) — stream-agnostic
  correlation (a WebTransport stream is a QUIC bidirectional stream;
  the `Dispatcher` is the same dispatch loop)
- [ADR-007](007-bistream-type-definition.md) — `BiStream` trait
  (source-agnostic; the contract a WebTransport stream satisfies)
- [ADR-027](027-tls-identity-redesign-acme-rawkey-decoupling.md) —
  browsers require X.509 (the `h3` handler is domain-hosted)
- [ADR-034](034-outgoing-only-x509-and-three-peer-roles.md) §2 (outbound
  no-`PeerId` connection-local overlay — this ADR's §3 is the inbound
  mirror), §4 (browsers are not peers — the non-peer WebTransport case)
- [ADR-038](038-http3-and-webtransport-as-first-class.md) — `h3` is
  first-class (this ADR refines the framing, not the scope)
- [ADR-040](040-webtransport-alpn-stream-proxy.md) — the ALPN-stream-
  proxy (this ADR repositions it as the substrate's non-call-ALPN
  mechanism; the decision stands)
- [ADR-029](029-peer-graph-routing-model.md) — `PeerCompositeEnv` /
  `PeerRef` (the peer-keyed overlay that non-peer WebTransport clients
  do not enter)
- [ADR-030](030-peerentry-and-identity-id-decoupling.md) — `PeerId`
  source (`Identity.id` from bearer-token resolution)
- `crates/http/webtransport.md` — the spec this ADR refines
- `crates/http/http-server.md` §"One-directional projection" — the
  HTTP/1.1 + HTTP/2 lossy projection this ADR contrasts WebTransport
  against
- `crates/call/call-protocol.md` §"Bidirectional Calls" — the
  bidirectionality this ADR names as a WebTransport property