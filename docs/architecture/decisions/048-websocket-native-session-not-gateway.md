# ADR-048: WebSocket Carries the Native Call-Protocol Session, Not the Gateway Shape

## Status

Accepted

## Context

ADR-044 (Accepted) deferred `h3`/WebTransport and committed WebSocket as the v1
browser bidirectional path: a browser upgrades an HTTP/1.1 or HTTP/2 request
to WebSocket and the resulting full-duplex WS connection carries call-protocol
`EventEnvelope` frames as binary WS messages. ADR-044 §1 established that the
call protocol's `call.requested`/`call.responded`/`call.completed`/`call.aborted`
exchange "works over WebSocket with no protocol change — the same `Dispatcher`,
the same `PendingRequestMap`, the same correlation by request ID."

ADR-044 §1 also established *what shape the WS session carries* — the native
`EventEnvelope` call-protocol session — but it does so as part of a larger
argument about why WebTransport isn't required, not as a crisp rule an
implementer is told not to violate. Two facts make the distinction worth its
own explicit decision record:

1. **The HTTP surface has a deliberate, well-documented invoke contract: the
   `to_openapi` gateway pattern** (ADR-042, ADR-047). The gateway is 5 fixed
   endpoints (`/search`, `/schema`, `/call`, `/batch`, `/subscribe`), where
   `/call` takes `{ "operation": "/fs/readFile", "input": {...} }` and invokes
   through `OperationRegistry::invoke()`. It is a well-shaped, simple contract.
   An implementer writing the WS handler could plausibly ask: "should the WS
   path expose the same 5-endpoint gateway shape, so a WS client looks like an
   HTTP client?" That is a reasonable question, and the answer is no — but the
   answer is currently implicit in ADR-044's framing, not stated as a rule.

2. **The two surfaces serve different architectural roles, and that difference
   is load-bearing.** The HTTP gateway is, by HTTP's nature, a *one-directional
   projection* — client initiates, server responds (see
   [http-server.md](../crates/http/http-server.md) §"One-directional
   projection"). The whole reason WebSocket exists in this architecture is to
   *restore the call protocol's native bidirectionality for browsers* (ADR-044
   §4): a WS connection is full-duplex, so both sides can initiate
   `call.requested` frames. Putting the gateway's one-directional shape on WS
   re-introduces the one-directional limitation that WS exists to fix. The two
   surfaces are not interchangeable; they are deliberately different tools for
   deliberately different jobs.

### The two invoke contracts, contrasted

| Aspect | HTTP gateway (`/call`, ADR-042/047) | WS native session (this ADR) |
|---------|--------------------------------------|------------------------------|
| Direction | One-directional (client→server calls only) | Bidirectional (either side can `call.requested`) |
| Wire unit | HTTP request/response | One `EventEnvelope` = one binary WS message |
| Invoke shape | `POST /call` with `{ "operation": "/fs/readFile", "input": {...} }` | `call.requested` event with `{ operation, input }` payload (the call protocol's native shape) |
| Discovery | `GET /search` (gateway endpoint) | `services/list` as an ordinary call-protocol op |
| Schema | `GET /schema` (gateway endpoint) | `services/schema` as an ordinary call-protocol op |
| Streaming | `GET /subscribe` (SSE frames) | `call.responded` events as binary WS messages (no SSE) |
| Dispatcher | axum route handler → `OperationRegistry::invoke()` | shared `Dispatcher` (ADR-012, stream-agnostic) |
| Multiplexing | HTTP/2 native; HTTP/1.1 sequential | By request ID (ADR-012), not by stream |

The WS row is the call protocol's own native session, with WebSocket as the
transport instead of QUIC. The HTTP row is a projection of that session into
HTTP's one-directional shape, with the gateway as the deliberate interface for
clients that only speak HTTP.

### Why the gateway shape is wrong on WS

Three concrete reasons:

1. **It duplicates the native invoke path with a lossier one.** The call
   protocol's `call.requested` event *is* the invoke primitive; the gateway's
   `/call` is that primitive wrapped in an HTTP envelope. On WS, the envelope
   is unnecessary — there is no HTTP request/response cycle to fit into. A
   gateway-on-WS would be a translation layer translating the call protocol to
   itself, losing bidirectionality in the process.

2. **It loses the per-caller filtering property the native session already
   has.** The gateway's `/search` exists to give HTTP clients the
   `AccessControl::check(identity)`-filtered discovery that the call protocol
   provides natively via `services/list`. On WS, `services/list` is already a
   call-protocol op the browser can call directly — the filtering is already
   there. A gateway-on-WS re-implements a filtering property the native session
   already provides.

3. **It breaks the symmetry with the QUIC path.** The `alknet/call` QUIC path
   (ADR-012, ADR-017) is the native `EventEnvelope` session over QUIC
   bidirectional streams. WS is the same session over WS messages. Making the
   WS path different from the QUIC path (by putting the gateway on WS) creates
   two browser-reachable invoke contracts for no architectural reason — the
   QUIC path is the reference, and WS should mirror it, not diverge from it.

### The prior art is already native-session-shaped

The `@alkdev/pubsub` WebSocket client/server (`event-target-websocket-client.ts`,
`event-target-websocket-server.ts`) — the working prior art ADR-044 cites as
the reason the WS path is cheap — already carries the `EventEnvelope { type, id,
payload }` shape over WS binary messages, with no gateway-style wrapping. The
alknet call protocol's `EventEnvelope` was derived from the pubsub envelope
(refined with typed event names and structured payloads); the delta is small
and well-defined (see [call-protocol.md](../crates/call/call-protocol.md)
§"Transport agnosticism"). A browser/Node WS client derived from the pubsub
prior art speaks the native session shape, not a gateway shape. The
gateway-on-WS variant would require *un-translating* the pubsub client's
native-session shape into the gateway's `{ operation, input }` shape — work
that has no payoff because the native shape is what both the QUIC path and the
pubsub prior art use.

## Decision

### 1. A WebSocket connection is a native `EventEnvelope` call-protocol session, not the HTTP gateway shape

The WS handler on `HttpAdapter` hands the WS message stream to the call
protocol's shared `Dispatcher` — the same dispatch loop the `CallAdapter` uses
for `alknet/call` QUIC connections (ADR-012, stream-agnostic correlation; a
WS message stream is another `BiStream`-satisfying transport). The browser
writes `EventEnvelope` frames as binary WS messages; the handler reads them
and dispatches via `OperationRegistry::invoke()`. Responses
(`call.responded`, `call.error`, `call.completed`, `call.aborted`) are written
back as binary WS messages.

The `to_openapi` gateway endpoints (`/search`, `/schema`, `/call`, `/batch`,
`/subscribe` — ADR-042, ADR-047) **do not appear on the WebSocket path**. They
are the HTTP one-directional projection's invoke contract; WS carries the
call protocol's own native session, which is a different (and richer) thing.

### 2. Discovery and schema are call-protocol ops, not WS-specific endpoints

The browser calls `services/list` and `services/schema` as ordinary
`call.requested` events over the WS connection. They are call-protocol
operations, not WS endpoints. There is no `/search` or `/schema` on WS — those
are the HTTP gateway's names for the same discovery primitives. The
filtering the gateway provides via `AccessControl::check(identity)`-filtered
`/search` (ADR-042 §3) is provided on the WS path by the same mechanism the
call protocol uses everywhere: `services/list` is `AccessControl`-filtered
natively (see [client-and-adapters.md](../crates/call/client-and-adapters.md)
§"services/list"). No WS-specific discovery surface exists or is needed.

### 3. Subscriptions project as native `call.responded` events, not SSE

A `Subscription` operation invoked over WS streams `call.responded` events as
binary WS messages directly — no SSE `data:` framing (that is the
`h2`/`http/1.1` projection for `/subscribe`, ADR-036 §Streaming; on WS it is
unnecessary because WS is already a framed full-duplex channel). `call.completed`
closes the stream; `call.aborted` closes it with an error frame. This is the
native streaming projection for the WS path, mirroring how subscriptions work
on the QUIC path.

### 4. Bidirectionality is native and unchanged from the QUIC path

The WS call-protocol session inherits the call protocol's native
bidirectionality (ADR-043 §2, transferred to WebSocket per ADR-044 §3): both
sides can send `call.requested` frames. The browser calls operations on the
hub; the hub can call operations registered on the browser's side, over the
same session, using the same `PendingRequestMap` and `EventEnvelope` framing
as `alknet/call`. The browser case where the client registers no operations of
its own is the common case — the server→client call direction is unused
because the browser has nothing to call. That is a use-case scoping, not an
architectural limitation.

### 5. This is a clarifying decision, not a new one

ADR-044 §1 commits the native-session shape ("the call protocol's
`EventEnvelope` framing fits a WebSocket binary message boundary cleanly ...
the same `Dispatcher`, the same `PendingRequestMap`, the same correlation by
request ID"). This ADR does not change that decision; it makes the
implication an explicit, implementer-visible rule: **the WS path is the
native session, and the gateway shape is deliberately not applied to it.** An
implementer reading ADR-044 alone could plausibly ask "should the WS path
expose the gateway endpoints too?" — this ADR's job is to make the answer
discoverable as a decision record, not implicit in framing.

## Consequences

**Positive:**
- One invoke model for the call protocol, regardless of transport. The QUIC
  path and the WS path run the same `EventEnvelope` session through the same
  `Dispatcher`; the HTTP gateway is the one-directional projection for clients
  that only speak HTTP. An implementer building the WS handler reuses the
  `Dispatcher` and `OperationRegistry::invoke()` dispatch path verbatim — no
  WS-specific routing, no WS-specific discovery surface, no second invoke
  contract to design or maintain.
- The `@alkdev/pubsub`/`@alkdev/operations` TypeScript clients sync to the
  alknet call protocol with no translation layer: their `EventEnvelope` shape
  is already the native session shape, and the call protocol's envelope is a
  refined superset of the pubsub envelope (ADR-044 §"Concrete prior art").
  The gateway-on-WS variant would have required un-translating the pubsub
  client's native-session shape; this decision avoids that un-translation
  entirely.
- Per-caller `AccessControl`-filtered discovery is already a property of the
  native session (`services/list`). No WS-specific filtering surface to build
  or document; the call protocol's authorization model applies unchanged.
- The browser is a bidirectional call target during a live session, not a
  peer-graph member (ADR-044 §5, ADR-034 §4). The native session shape is what
  makes this clean: the browser gets bidirectional call capability through the
  connection-local Layer 2 overlay (ADR-024, ADR-043 §3) without peer-graph
  membership, and the gateway shape would not have changed this — but it would
  have made the WS path diverge from the QUIC path for no benefit.

**Negative:**
- A WS client cannot use the gateway's `{ "operation": ..., "input": ... }`
  body shape — it must speak the call protocol's native `call.requested`
  event. This is honest (the WS path *is* the call protocol), but a developer
  who learned the gateway shape from the HTTP surface must learn the
  `EventEnvelope` shape for WS. The pubsub prior art and the
  `@alkdev/operations` TypeScript client already speak this shape, so the
  delta is small for the primary consumer — but it is a real difference from
  the HTTP gateway's simpler `{ operation, input }` invoke body.
- The 5 gateway endpoint names (`/search`, `/schema`, `/call`, `/batch`,
  `/subscribe`) are HTTP-specific and do not carry over to WS. A deployment
  documenting its surface for both HTTP and WS clients documents two invoke
  shapes (the gateway for HTTP; the native session for WS). This is the cost
  of using the right tool for each transport instead of forcing one shape onto
  both.

## Reversal

This ADR clarifies a decision ADR-044 already committed (§1 describes the
native `EventEnvelope` session; this ADR makes that the explicit, implementer-
visible rule). The reversal posture is therefore ADR-044's, not a separate
one: the WS path itself is not deferred (it is the v1 browser path), and
the native-session-not-gateway choice is a clarification of what that path
carries — reversing it would mean adopting the gateway shape on WS, which
would re-introduce the one-directional limitation WS exists to fix (§Context
reason 1). The realistic reversal path is not "switch the WS path to the
gateway shape" but "WebTransport revives and adds a second browser
bidirectional path" — which is ADR-044's reversal trigger (a concrete
ALPN-stream-proxy deployment), not this ADR's. When WebTransport revives,
the two coexist: WS stays as the native-session call-protocol path;
WebTransport adds the ALPN-stream-proxy path (ADR-040). This ADR's rule (WS
= native session, not gateway) is unaffected by WebTransport's revival —
the gateway shape stays HTTP-only regardless of how many browser
bidirectional transports exist.

## Assumptions

1. **The call protocol's `EventEnvelope` framing fits a WebSocket binary
   message boundary cleanly.** An `EventEnvelope` is a self-delimited JSON
   object; one envelope per WS binary message. No streaming deserializer
   across message boundaries is needed. This is ADR-044 Assumption 1, already
   verified by the `@alkdev/pubsub` WebSocket client/server prior art.

2. **The shared `Dispatcher` runs over a WS message stream unchanged.**
   ADR-012 commits stream-agnostic correlation; a WS message stream is another
   `BiStream`-satisfying transport. The `Dispatcher` and `PendingRequestMap`
   are transport-agnostic; only the connection-establishment half differs
   (WS upgrade handler vs QUIC accept/dial).

3. **The primary WS consumer is a browser or Node client derived from the
   `@alkdev/pubsub`/`@alkdev/operations` prior art.** That client already
   speaks the native `EventEnvelope` shape. The gateway's simpler
   `{ operation, input }` body shape is the HTTP path's affordance for clients
   that only speak HTTP; a client that has chosen WS has already opted into
   the call protocol's native framing.

4. **`services/list` and `services/schema` are sufficient discovery for the
   WS path.** They are `AccessControl`-filtered (per-caller) and return the
   full `OperationSpec` respectively. The gateway's `/search` and `/schema`
   are HTTP-shaped names for these same primitives; on WS the primitives
   apply directly. No WS-specific discovery surface is needed.

## References

- [ADR-012](012-call-protocol-stream-model.md) — stream-agnostic correlation;
  a WS message stream is another `BiStream`-satisfying transport
- [ADR-017](017-call-protocol-client-and-adapter-contract.md) §5 — `to_*`
  adapters are projections that consume the registry; WS is not a `to_*`
  adapter (it carries the native session, it doesn't project it)
- [ADR-024](024-operation-registry-layering.md) — Layer 2 per-connection
  overlay where browser-registered ops (if any) land
- [ADR-034](034-outgoing-only-x509-and-three-peer-roles.md) §4 (amended by
  ADR-044 §5) — browsers are not alknet peers; the connection-local overlay
  gives the browser bidirectional-call capability without peer-graph
  membership
- [ADR-036](036-http-to-call-operation-mapping.md) — the SSE projection for
  `/subscribe` (the HTTP one-directional streaming path; on WS, subscriptions
  project as native `call.responded` events, no SSE)
- [ADR-042](042-openapi-gateway-pattern.md) — the gateway pattern this ADR
  clarifies is HTTP-only
- [ADR-043](043-webtransport-bidirectional-alpn-substrate.md) §2/§3 —
  bidirectionality and the no-`PeerId` connection-local overlay, transferred
  to WebSocket per ADR-044 §3
- [ADR-044](044-defer-webtransport-browsers-use-websocket.md) — the ADR that
  committed WS as the v1 browser path; this ADR clarifies the shape of what
  ADR-044 committed (§1 implies the native session; this ADR makes it an
  explicit rule)
- [ADR-047](047-remove-direct-call-http-surface.md) — the gateway as the
  sole HTTP invoke path (the HTTP-only contract this ADR clarifies does not
  extend to WS)
- `crates/http/websocket.md` — the spec that implements this decision
- `crates/http/http-server.md` §"WebSocket browser path" — the original
  section this decision promotes into `websocket.md`