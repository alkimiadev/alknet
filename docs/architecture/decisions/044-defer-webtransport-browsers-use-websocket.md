# ADR-044: Defer h3/WebTransport; Browsers Use WebSocket

## Status

Accepted (supersedes ADR-038; parks ADR-040, ADR-043)

## Context

ADR-038 brought `h3`/WebTransport into scope as a first-class HTTP transport,
framed against the "two-way door as deferral" anti-pattern (ADR-009 §"What
this framework is NOT"). ADR-040 (the ALPN-stream-proxy) and ADR-043 (the
bidirectional-substrate reframing) extended it. Three ADRs, one crate-spanning
spec (`webtransport.md`), and a body of design work.

Working through the implementation path surfaced a different concern than the
one ADR-038 was written to correct. ADR-038 correctly rejected *deferral-
as-hedging*; the present decision is *deferral-as-scoping*, which ADR-009
explicitly permits (a decision that "genuinely doesn't need to be made yet
because the use case isn't concrete" — scope management, not door-type
classification). The two must not be
confused. Three concrete findings drove the scope re-evaluation:

### Finding 1 — the browser bidirectional path doesn't require WebTransport

The load-bearing use case for `h3`/WebTransport in v1 is **a browser reaching
the call protocol bidirectionally**. ADR-043 §2 establishes that the call
protocol's bidirectionality applies unchanged over any bidirectional stream —
the `Dispatcher` is stream-agnostic (ADR-012). That property is not unique to
WebTransport streams. **WebSocket is a full-duplex, long-lived connection over
which either side can send framed messages**, and the call protocol's
`EventEnvelope` framing fits a WebSocket binary message boundary cleanly (an
`EventEnvelope` is a self-delimited JSON object; one frame = one WS binary
message). The `call.requested`/`call.responded`/`call.completed`/`call.aborted`
exchange works over WebSocket with no protocol change — the same `Dispatcher`,
the same `PendingRequestMap`, the same correlation by request ID.

What WebTransport gives *over* WebSocket — native multiplexed bidirectional
streams, datagrams, the "carry any ALPN as a stream" substrate framing
(ADR-043) — is genuinely better engineering, but none of it is *required* for
the call protocol from a browser. The call protocol multiplexes multiple calls
over a single connection by request ID (ADR-012); it does not need
WebTransport's per-stream multiplexing. The substrate/proxy framing (ADR-040,
ADR-043) is the thing that *does* benefit from WebTransport's stream model —
and that use case is the speculative one (see Finding 3).

### Finding 2 — WebTransport is a draft standard on an experimental dependency stack

WebTransport over HTTP/3 is still an IETF draft (`draft-ietf-webtrans-http3`,
at `-07` at time of writing), not an RFC. The Rust implementation landscape is
correspondingly immature:

- `wtransport` (the reference read during research) is a complete
  pure-Rust implementation, but its own README states it "is not considered
  completely production-ready" and "may undergo changes as the WebTransport
  specification evolves."
- The hyperium stack (`h3` + `h3-quinn` + `h3-webtransport` + `h3-datagram`)
  fits the axum/hyper ecosystem more naturally (h3 produces `http::Request`
  types that axum consumes directly, which is load-bearing for the spec's
  "HTTP/3 requests go through the same axum `Router`" commitment), but h3's
  own README says it is "still very experimental... API could change."
- A research spike would be needed to verify the hyperium stack's
  server-side WebTransport API before committing to it — the axum-bridge
  feasibility is the load-bearing claim and is not yet confirmed against
  actual crate APIs, only against READMEs and design philosophy.

Either choice puts a draft-standard protocol and an experimental Rust
dependency on the security surface of `alknet-http`'s first release. The `h3`
feature gate (ADR-038) isolates the risk for non-browser-facing deployments,
but a browser-facing hub must enable it — so the risk is borne precisely by
the deployment shape that motivates having a browser path at all.

### Finding 3 — the ALPN-stream-proxy is speculative; the call protocol is not

ADR-040 (the ALPN-stream-proxy — a browser with a WASM parser for SSH/SFTP/git
reaching any ALPN handler via WebTransport) is the genuinely compelling
WebTransport use case. It is also the one that is *not* required for v1:

- The call protocol from a browser works over WebSocket (Finding 1).
- The downstream crates unlocked by completing `alknet-http` (the SSH, git,
  SFTP crates) do not require WebTransport or the proxy. They expose their
  ALPNs natively over QUIC; the proxy is a *browser reachability* feature
  for those ALPNs, not a prerequisite for the ALPNs to exist.
- The WASM parsers (the browser-side SSH/SFTP/git clients) are themselves
  downstream artifacts not yet built. The proxy is only useful once a parser
  exists to consume it.

The proxy is "useful, and cheap-on-top *if* WebTransport already exists" —
but WebTransport does not yet exist, and building it speculatively to enable
a proxy whose consumers do not yet exist is the scope inversion.

### The iroh precedent

iroh's own relay (`iroh-relay`, the DERP-equivalent that provides NAT traversal
fallback) chose **WebSocket (WSS)**, not WebTransport, for its fallback path.
This is a strong signal from a project whose entire design center is QUIC and
P2P connectivity: when the question was "what does a browser need to reach our
protocol bidirectionally," their answer was WSS, not WebTransport. Aligning
with that precedent is not cutting against competent practice — it is
matching it.

## Decision

### 1. Defer `h3`/WebTransport. Browsers reach the call protocol over WebSocket.

The `h3` ALPN, the `h3` feature gate, and the WebTransport dependency stack
are **deferred** — not implemented in the initial `alknet-http` release. A
browser connecting to a hub authenticates by bearer token and upgrades an
HTTP/1.1 or HTTP/2 request to WebSocket. The resulting full-duplex WS
connection carries call-protocol `EventEnvelope` frames as binary WebSocket
messages. The browser is a bidirectional call-protocol client over this
connection, using the same `Dispatcher` and `PendingRequestMap` as the
`alknet/call` QUIC path (ADR-012 — stream-agnostic correlation; a WS message
stream is just another `BiStream`-satisfying transport, extending ADR-012's
stream-agnostic claim from QUIC bidirectional streams to any framed
full-duplex byte channel).

This is a **scope** decision, not a hedging deferral (ADR-009 §"What this
framework is NOT"). The reversal trigger is concrete: **a real deployment that
needs the ALPN-stream-proxy (a browser running a WASM SSH/SFTP/git client to
reach a non-call ALPN)**. When that use case arrives, ADR-038 / ADR-040 /
ADR-043 revive as the design — they are not wrong, they are not-now. No
"v1/later/when-it-arrives" hedging language attaches; the condition is stated
as a concrete trigger.

### 2. ADR-038 is superseded by this ADR.

ADR-038's core decision — that `h3` is in scope, not deferred — is reversed
by this ADR. ADR-038's *correction* of the "two-way-door-as-deferral"
anti-pattern stands as a document (the anti-pattern is real); its specific
decision (h3 in scope now) is superseded. ADR-038 is marked Superseded.

### 3. ADR-040 and ADR-043 are parked, not superseded.

ADR-040 (the ALPN-stream-proxy) and ADR-043 (the bidirectional-substrate
reframing) are **not superseded** — their decisions are correct, and they
revive unchanged when WebTransport revives. They are marked Proposed with an
amendment noting implementation is deferred per this ADR. Two specific
transfers apply during the deferment:

- **ADR-043 §2 (call-protocol bidirectionality over WebTransport) transfers
  to WebSocket unchanged.** WebSocket is full-duplex; the call protocol's
  bidirectionality applies over a WS connection exactly as ADR-043 §2
  describes for WebTransport. The browser case where the client registers
  no ops remains a use-case scoping, not an architectural limitation.
- **ADR-043 §3 (the no-`PeerId` connection-local overlay) transfers to
  WebSocket unchanged.** A browser over WSS has no `PeerId` on the hub's
  side for the same reasons it has none over WebTransport (see §5 below);
  the connection-local Layer 2 overlay applies. The pattern is
  transport-agnostic.

What does *not* transfer to WebSocket is ADR-040 (the ALPN-stream-proxy) and
ADR-043 §4 (the non-call-ALPN substrate mechanism). Those require
WebTransport's stream model and revive with it. SSH/SFTP/git-over-WSS-from-a-
browser is technically possible (multiplex logical streams over one WS frame
stream) but is not specified here — it is the same speculative use case that
motivates deferring WebTransport, and it is not needed for v1.

### 4. WebSocket is the browser bidirectional path; HTTP/1.1+HTTP/2 remain the one-directional projection.

`alknet-http`'s browser-reachable surface becomes:

| Transport | Direction | Use case |
|-----------|-----------|----------|
| `http/1.1`, `h2` | one-directional (client→server) | HTTP clients (curl, axios, `fetch` for request/response); SSE for subscription streaming (ADR-036) |
| WebSocket (over `http/1.1` or `h2` upgrade) | **bidirectional** | Browser call-protocol clients; the path that restores the call protocol's bidirectionality for browsers |

WebSocket is the surface that **restores the call protocol's bidirectionality
for browsers** (the role ADR-043 §5 assigned to WebTransport). The
one-directional projection that ADR-043 §5 names for HTTP/1.1+HTTP/2 stands
unchanged.

### 5. Browsers over WebSocket are not alknet peers — the rationale, stated.

ADR-034 §4 established that a browser over WebTransport is not an alknet peer
(no `PeerId`, no `PeerCompositeEnv` entry). The same applies to a browser over
WebSocket, and the rationale — which ADR-034 §4 states as a closure without
the supporting argument — is worth making explicit because it is the
load-bearing distinction:

**"Peer" in alknet means an addressable node in the call-protocol peer graph
— a stable `PeerId`, reachable via `PeerRef::Specific`, whose ops land in
`PeerCompositeEnv`, whose identity is stable across reconnects.** It does
*not* mean "any endpoint that exchanges calls during a live session." A
browser is the second thing but not the first, on three concrete grounds:

1. **No stable cryptographic identity of its own.** A `PeerEntry` is anchored
   to fingerprints (Ed25519, X.509) that *the peer* presents and the local
   node pins. A browser presents a bearer token the *hub* issued; the
   "identity" is the hub's bookkeeping for that token, not something the
   browser owns or that could be pinned by another node. There is nothing
   to put in `PeerEntry.fingerprints`.
2. **Ephemeral.** Close the tab → connection dies → the connection-local
   Layer 2 overlay (ADR-043 §3 / ADR-034 §2) dies with it. A `PeerEntry`
   keyed to a browser would be a permanently-dead entry within seconds.
   `PeerRef::Specific("browser-X")` from another node would route to
   nothing.
3. **Not addressable from other nodes.** `PeerRef::Specific` resolves through
   `PeerEntry` → `PeerId`. Another alknet node has no way to reach "the
   browser currently connected to hub-A"; the hub holds that connection as a
   live `CallConnection` handle, not as a peer-graph entry. The
   connection-local overlay is precisely the mechanism that gives the
   browser bidirectional-call capability *without* peer-graph membership.

This is the explicit closure of the "browser as peer" path, on both the
inbound (this section) and outbound (ADR-034 §2) sides. The browser is a
**bidirectional call target during a live session**, not a **peer-graph
member**. The connection-local Layer 2 overlay (ADR-024, ADR-043 §3) is what
makes the former possible without requiring the latter.

This rationale applies transport-agnostically — to WebSocket, to WebTransport
when it revives, and to any future browser transport. ADR-034 §4 is amended
by reference to this section.

## Consequences

**Positive:**
- `alknet-http`'s first release does not carry a draft-standard protocol or
  an experimental dependency stack on its security surface. The browser path
  uses WebSocket, a mature, well-understood, RFC 6455 protocol with first-
  class axum support (`axum::extract::ws`).
- The axum-bridge research spike for h3/WebTransport is not on the critical
  path. WebSocket upgrade over HTTP/1.1 or HTTP/2 is standard axum territory.
- The downstream crates that `alknet-http` unblocks (SSH, git, SFTP) are not
  blocked on WebTransport or the proxy. They expose their ALPNs natively over
  QUIC; browser reachability for them is a future WebTransport feature.
- Forward momentum is preserved: the `h3` handler, the feature gate, the
  `wtransport`/hyperium decision, and the ALPN-stream-proxy are all real
  design work that is already done (ADR-038, ADR-040, ADR-043,
  `webtransport.md`). Reviving them is unblocking already-written specs, not
  designing from scratch.

**Negative:**
- ADR-038, ADR-040, and ADR-043 are not implemented in the initial release.
  Their design work is preserved (the ADRs and `webtransport.md` stay in the
  record), but a reader must cross-reference this ADR to know they are
  parked. The `webtransport.md` spec is marked `deferred` with a header note.
- The ALPN-stream-proxy (ADR-040) is not available in v1. A browser cannot
  reach SSH/SFTP/git ALPNs in the initial release — it can reach the call
  protocol over WebSocket, but not the non-call ALPNs. This is the
  speculative use case whose deferral this ADR commits; the reversal trigger
  is a real deployment needing it.
- WebSocket is a single stream; it lacks WebTransport's native multi-stream
  multiplexing. For the call protocol this is fine (correlation is by request
  ID, not by stream — ADR-012), but it means a future migration to
  WebTransport would be a genuine upgrade, not a no-op. The migration path
  is the spec that already exists (`webtransport.md`).
- ADR-043's "WebTransport restores bidirectionality" framing (§5) becomes
  "WebSocket restores bidirectionality" for v1. The framing transfer is clean
  (§3 above), but the prose in `http-server.md` and the ADRs must reflect it.

## Reversal

This decision reverses when a concrete deployment needs the ALPN-stream-proxy
— i.e., a real use case of a browser running a WASM SSH/SFTP/git client to
reach a non-call ALPN over WebTransport. At that point:

1. The research spike deferred here (verify the hyperium stack's server-side
   WebTransport API and the axum-bridge feasibility — see §"Research note"
   in `webtransport.md`) is run.
2. ADR-038 / ADR-040 / ADR-043 are un-parked and implemented as written,
   with the `webtransport.md` spec as the design.
3. The WebSocket browser path (this ADR's §4) is not removed — it remains as
   the simpler browser path for deployments that don't need WebTransport's
   stream model. The two coexist.

The reversal is a one-way door at the *crate surface* (the `h3` feature gate
becomes part of the published interface) but a two-way door at the
*architecture* (the `webtransport.md` design already exists; reviving it is
implementation work, not redesign). The `webtransport.md` spec is kept intact
and marked `deferred` so the revival is unblocking, not re-deriving.

## Research note (for revival)

A note for the revival: `wtransport` (the reference implementation read during
initial research) is *probably not* the right dependency choice, despite
being a complete and readable implementation. The load-bearing integration
concern is that `alknet-http`'s `h3` handler must route HTTP/3 requests
through the same axum `Router` as `h2`/`http/1.1` (ADR-036), and `wtransport`
owns its own HTTP serving path — bridging its request type into the
`http::Request` axum consumes is cross-ecosystem adapter work. The hyperium
stack (`h3` + `h3-quinn` + `h3-webtransport`) operates at the stream level
and produces `http::Request` types natively, which is a better fit for the
axum integration — but its server-side WebTransport API needs verification
before commitment. This research is **not** run now (WebTransport is
deferred); it is recorded here so the revival does not re-derive the question
from scratch. See `webtransport.md` §"Research note" for the cross-reference.

## Assumptions

1. **The call protocol's `EventEnvelope` framing fits a WebSocket binary
   message boundary cleanly.** An `EventEnvelope` is a self-delimited JSON
   object; one envelope per WS binary message. No streaming deserializer
   across message boundaries is needed. This is verified by implementation
   when the WS browser path is built, not by a separate research spike — the
   call protocol spec (`call-protocol.md`) and the EventEnvelope shape
   already make this property clear, and WebSocket binary messages are a
   standard byte-framed transport.

2. **WebSocket upgrade over HTTP/1.1 or HTTP/2 is supported by the axum/
   hyper stack natively.** `axum::extract::ws` provides the upgrade handler;
   the underlying connection is the same hyper HTTP connection the `h2`/
   `http/1.1` handler already drives. No new framing library is needed.

3. **A browser over WebSocket has the same peer-model properties as a browser
   over WebTransport.** No `PeerId`, no `PeerCompositeEnv` entry, connection-
   local Layer 2 overlay (ADR-043 §3, ADR-034 §2). The rationale in §5 is
   transport-agnostic and applies identically to WSS.

4. **The downstream crates (SSH, git, SFTP) do not require WebTransport or
   the ALPN-stream-proxy to exist.** They expose their ALPNs natively over
   QUIC; the proxy is a browser-reachability feature, not a prerequisite for
   the ALPNs themselves. Browser reachability for non-call ALPNs is the
   speculative use case whose deferral this ADR commits.

## References

- [ADR-009](009-one-way-door-decision-framework.md) §"What this framework is
  NOT" — the anti-pattern ADR-038 was written to correct; this ADR relies on
  ADR-009's explicit distinction between deferral-as-hedging (rejected) and
  deferral-as-scoping (permitted: a decision that "genuinely doesn't need to
  be made yet because the use case isn't concrete" — scope management, not
  door-type classification)
- [ADR-038](038-http3-and-webtransport-as-first-class.md) — **superseded by
  this ADR.** Its correction of the two-way-door-as-deferral anti-pattern
  stands; its specific decision (h3 in scope now) is reversed.
- [ADR-040](040-webtransport-alpn-stream-proxy.md) — **parked, not
  superseded.** Revives unchanged when WebTransport revives. The proxy is
  the speculative use case whose deferral is this ADR's reversal trigger.
- [ADR-043](043-webtransport-bidirectional-alpn-substrate.md) — **parked, not
  superseded.** §2 (bidirectionality) and §3 (no-`PeerId` overlay) transfer
  to WebSocket unchanged; §4 (non-call-ALPN substrate) and §5's
  WebTransport-specific framing revive with WebTransport.
- [ADR-034](034-outgoing-only-x509-and-three-peer-roles.md) §4 — browsers are
  not alknet peers; this ADR's §5 states the rationale (addressability vs.
  bidirectionality) that ADR-034 §4 closes without arguing. ADR-034 §4 is
  amended by reference to this ADR's §5.
- [ADR-012](012-call-protocol-stream-model.md) — stream-agnostic correlation;
  a WebSocket message stream is another `BiStream`-satisfying transport. The
  call protocol multiplexes by request ID, not by stream.
- [ADR-036](036-http-to-call-operation-mapping.md) — the HTTP-to-call
  mapping; the WebSocket browser path layers on top of the same axum
  `Router` and `OperationRegistry::invoke()` dispatch.
- `crates/http/webtransport.md` — the deferred spec; marked `deferred` with
  a header note pointing here. Kept intact for revival.
- `crates/http/http-server.md` — gains a "WebSocket browser path" section
  (the v1 browser bidirectional path) and the "browser is not a peer"
  rationale (this ADR's §5, transported to the spec that now carries the
  browser path).