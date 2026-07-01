---
status: draft
last_updated: 2026-07-01
---

# WebSocket — the Browser Bidirectional Path

WebSocket is the v1 browser bidirectional path to the call protocol.
`h3`/WebTransport is deferred per
[ADR-044](../../decisions/044-defer-webtransport-browsers-use-websocket.md);
the deferred handler design is at
[webtransport.md](webtransport.md). This document specifies the WebSocket
upgrade handler on `HttpAdapter`, the framing, the dispatch path, the
identity model, the streaming projection, and the explicit relationship to
the HTTP gateway surface.

A WebSocket connection is a **native `EventEnvelope` call-protocol session**,
not the HTTP gateway shape — the gateway endpoints (`/search`, `/schema`,
`/call`, `/batch`, `/subscribe`, ADR-042/047) are the HTTP one-directional
projection and **do not appear on the WebSocket path**. See
[ADR-048](../../decisions/048-websocket-native-session-not-gateway.md) for
the decision and its rationale.

## What

The WebSocket path is an axum WS upgrade handler on the same `HttpAdapter`
that serves `h2`/`http/1.1` (see [http-server.md](http-server.md)). A browser
(or any WS client — Node, a native app with a WS library) opens an HTTP/1.1
or HTTP/2 request to the upgrade path, authenticates by bearer token on the
upgrade request, and the resulting full-duplex WS connection carries
call-protocol `EventEnvelope` frames as binary WebSocket messages — one
envelope per message. The WS message stream is handed to the call protocol's
shared `Dispatcher`, which runs the same dispatch loop the `CallAdapter` uses
for `alknet/call` QUIC connections (ADR-012, stream-agnostic correlation).

## Why

WebSocket is the HTTP-family transport that restores the call protocol's
native bidirectionality for browsers — HTTP/1.1 + HTTP/2 are
request/response (a one-directional projection of the call protocol),
while WS is a full-duplex, long-lived, framed-message channel over which
the call protocol's native `EventEnvelope` session runs unchanged. The
decision to use WebSocket for the browser bidirectional path (deferring
`h3`/WebTransport) is [ADR-044](../../decisions/044-defer-webtransport-browsers-use-websocket.md);
the decision that the WS path carries the native session rather than the
HTTP gateway shape is [ADR-048](../../decisions/048-websocket-native-session-not-gateway.md).
Both decisions' rationale is in those ADRs; this spec covers what the WS
path *is* and how an implementer builds it.

### Prior art: `@alkdev/pubsub`

The browser/Node WS client is mostly already written. The
`@alkdev/pubsub` package (`/workspace/@alkdev/pubsub/`) has a working
WebSocket client (`src/event-target-websocket-client.ts`) and server
(`src/event-target-websocket-server.ts`) built on an
`EventEnvelope { type, id, payload }` shape — the envelope the alknet call
protocol's `EventEnvelope` was derived from (refined with typed event
names `call.requested`/`call.responded`/etc. and structured payloads).
The sibling `@alkdev/operations` package (`/workspace/@alkdev/operations/`)
shares the lineage, with one mechanical delta: `path.do.op` (dot-separated)
vs alknet's `path/to/op` (slash-separated). Syncing the pubsub/operations
WS client to the alknet envelope is a small adjustment (envelope shape,
event-name typing, path separator), not a from-scratch build. See
[call-protocol.md](../call/call-protocol.md) §"Transport agnosticism" and
ADR-044 §"Concrete prior art".

### Illustrative deployment: `api.alk.dev` hub-spokes-browser

A concrete topology this path serves (the early-stage deployment motivating
the spec promotion): `api.alk.dev` runs as a hub. The vast majority of
spokes are Rust processes using `CallClient` to connect to the hub over QUIC
on ALPN `alknet/call` — e.g., a git runner, container services — and are
**peers** (stable `PeerId`, fingerprint-pinned, in `PeerCompositeEnv`,
addressable via `PeerRef::Specific` per ADR-029). A browser UI for the
early stages (while a desktop app is being fleshed out) connects via
WebSocket and is a **bidirectional call target during a live session, not
a peer-graph member** (bearer token, no `PeerId`, connection-local Layer 2
overlay, dies when the tab closes). The browser UI calls `services/list`
to populate its view with only the ops its bearer-token identity is
authorized to call, calls `services/schema` for shapes, and invokes via
`call.requested` — per-privilege filtering comes free from the call
protocol's `AccessControl::check(identity)`-filtered `services/list`. The
browser operator UI sees only what its privs allow; the Rust spokes are
full-addressable peers. This is the canonical ADR-044 §5 scenario.

## Architecture

### The WS upgrade handler

The WS upgrade is an HTTP/1.1 or HTTP/2 request handled by an axum route on
`HttpAdapter`'s router. The handler:

1. Receives the HTTP upgrade request (axum's `WebSocketUpgrade` extractor).
2. Resolves the caller's identity from the `Authorization: Bearer` header
   via `identity_provider.resolve_from_token(&AuthToken { raw:
   token_bytes })` (the `AuthToken` type is from
   [../core/auth.md](../core/auth.md) — a wrapper around the raw bearer
   token bytes)
   — the same auth path as any HTTP request
   ([http-server.md](http-server.md) §"Auth"). The upgrade is rejected
   (`401`) if no token is present; insufficient scopes for any op the
   browser later calls surface as `403`/`FORBIDDEN` at call time, not at
   upgrade time (the upgrade doesn't know which ops the browser will call).
3. Upgrades to WebSocket (axum's `WebSocketUpgrade::on_upgrade`), producing
   a full-duplex `WebSocket` stream.
 4. Wraps the `WebSocket` stream as a `BiStream`-satisfying transport — a WS
    binary message in either direction is one `EventEnvelope` frame (see
    §"Framing" below for the length-prefix decision).
 5. Constructs a `Dispatcher` (the shared dispatch loop,
    [../call/client-and-adapters.md](../call/client-and-adapters.md)
    §"Shared Dispatcher") with the `Arc<OperationRegistry>` and
    `Arc<dyn IdentityProvider>` the `HttpAdapter` holds, plus a
    connection-local Layer 2 overlay for any ops the browser registers (see
    §"Bidirectionality" below).
 6. Spawns the dispatch task (`Dispatcher::run_loop`) on a tokio task; the
    WS connection is live until either side closes it or the browser drops
    the handle (closes the tab).

The upgrade path is a single axum route. The **default upgrade path is
`/alknet/call`** (the deployment may override it via the `extra_routes`
mechanism of ADR-046, but a deployment that passes no custom routes gets
`/alknet/call`). The path must not collide with the reserved
gateway/`/healthz`/`/openapi.json`/MCP/custom-route paths per ADR-046's
collision rule; `/alknet/call` namespaces away from the reserved set
naturally. A deployment that builds a custom REST projection with
`POST /{service}/{op}` routes (ADR-047 §4) coexists with the WS upgrade
at `/alknet/call` — axum's `Router::merge` prioritizes specific routes
over wildcards, so the WS upgrade's exact `/alknet/call` path wins over
any `/{service}/{op}` wildcard a custom route projection might
register, and the two do not collide. The upgrade runs over HTTP/1.1
(the standard `Upgrade: websocket` header, RFC 6455) or HTTP/2 (the
extended CONNECT protocol, RFC 8441); axum/hyper supports both, and
the handler does not branch on which — the WS frame stream is the same
once the upgrade completes.

### Framing: `EventEnvelope` over binary WS messages

Every message on the WS connection is a binary WebSocket message containing
one `EventEnvelope`:

```rust
pub struct EventEnvelope {
    pub r#type: String,    // "call.requested" | "call.responded" | "call.completed" | "call.aborted" | "call.error"
    pub id: String,        // Correlation key (request ID, subscription ID)
    pub payload: Value,    // serde_json::Value — schema depends on event type
}
```

This is the call protocol's wire format verbatim (see
[../call/call-protocol.md](../call/call-protocol.md) §"Wire Format:
EventEnvelope"). The `@alkdev/pubsub` envelope (`{ type, id, payload }`) is
the shape the alknet `EventEnvelope` was derived from; the delta is typed
event names (`call.requested` etc.) and structured payloads, both of which
are already in the alknet envelope. The five event types
(`call.requested`, `call.responded`, `call.completed`, `call.aborted`,
`call.error`) carry request/response and subscription semantics exactly as
over QUIC — see [../call/call-protocol.md](../call/call-protocol.md)
§"Event Types" for the full table.

**Length-prefix decision.** The QUIC path frames `EventEnvelope` as a
4-byte big-endian length prefix + UTF-8 JSON body (see
[../call/call-protocol.md](../call/call-protocol.md) §"Wire Format"),
because a QUIC bidirectional stream is an unbounded byte stream that needs
an explicit delimiter. A WebSocket binary message is already
length-delimited by the WS frame boundary — the receiver gets one complete
message per read, no partial reads across message boundaries (ADR-044
Assumption 1, verified by the `@alkdev/pubsub` prior art). **The WS path
therefore carries no length prefix**: one `EventEnvelope` JSON object = one
binary WS message, and the WS message boundary is the delimiter. The
implementation must not prepend the QUIC length prefix on outbound WS
messages or expect it on inbound ones — the two framings are deliberately
different, matching each transport's native boundary semantics. (The
`FrameFramedReader`/`FrameFramedWriter` types the QUIC dispatch loop uses
are replaced on the WS path by direct JSON serde over the WS message type;
the `Dispatcher` itself is transport-agnostic and consumes `EventEnvelope`
values, not raw bytes — see [../call/client-and-adapters.md](../call/client-and-adapters.md)
§"Shared Dispatcher".)

Binary payloads within `EventEnvelope.payload` follow the same base64-as-
JSON-string convention the QUIC path uses
([../call/call-protocol.md](../call/call-protocol.md) §"Wire Format") —
the envelope carries `serde_json::Value` and does not interpret binary
fields; that's a handler-level concern, transport-agnostic.

Text WS messages are not used; all call-protocol frames are binary. A client
that sends a text message gets a protocol-level close (the WS handler
validates message type).

### Dispatch: the shared `Dispatcher`, unchanged

The WS message stream is handed to the `Dispatcher` — the same dispatch loop
the `CallAdapter` uses for `alknet/call` QUIC connections (ADR-017 §1; see
[../call/client-and-adapters.md](../call/client-and-adapters.md)
§"Shared Dispatcher"). The dispatch half is one implementation; the
connection-establishment half differs (WS upgrade handler vs QUIC
accept/dial), but after establishment the `Dispatcher` runs identically:

- Reads `EventEnvelope` frames from the WS message stream.
- For `call.requested`: resolves the peer's identity (the bearer-token
  identity resolved at upgrade time, stored on the connection),
  runs `AccessControl::check(identity)` against the op's `AccessControl`,
  dispatches via `OperationRegistry::invoke()` if allowed, returns
  `FORBIDDEN` (→ `call.error`) before the handler runs if not.
- For `call.responded`/`call.completed`/`call.aborted`: correlates by `id`
  via `PendingRequestMap` (keyed by request ID, not by transport — ADR-012).
- Writes response `EventEnvelope` frames back as binary WS messages.

Peer authorization flows through the existing `AccessControl::check` against
the resolved identity — no `RemoteFilter`, no `remote_safe` gate (retired by
ADR-029 §3). An op with `AccessControl::default()` is callable by any
authenticated browser; an op with `required_scopes` is callable only by
browsers whose `Identity.scopes` satisfy them; an op with
`Visibility::Internal` is never callable from the wire (`NOT_FOUND` before
ACL). See [../call/client-and-adapters.md](../call/client-and-adapters.md)
§"CallClient" for the full mapping of the three `remote_safe` cases to
`AccessControl`/`Visibility`.

### Bidirectionality (ADR-043 §2 transferred to WebSocket per ADR-044 §3)

The WS call-protocol session inherits the call protocol's native
bidirectionality: both sides can send `call.requested` frames. The browser
calls operations on the hub; the hub can call operations registered on the
browser's side, over the same session, using the same `PendingRequestMap`
and `EventEnvelope` framing as `alknet/call`.

The browser case where the client registers no operations of its own is the
common case — the server→client call direction is unused because the
browser has nothing to call. That is a use-case scoping, not an architectural
limitation. A browser that *does* expose ops (e.g., a UI that registers a
`ui/dragged` op the hub can call to push live updates) registers them in the
connection-local Layer 2 overlay (see §"Connection-local overlay" below),
and the hub reaches them through the live `CallConnection` handle — not
through `PeerRef::Specific` (the browser is not a peer; see §"Browsers are
not alknet peers").

### Connection-local overlay (ADR-043 §3 transferred; ADR-024)

A browser over WebSocket has no `PeerId` on the hub's side. Any ops the
browser registers land in a **connection-local Layer 2 overlay**
(ADR-024) — a per-`CallConnection` overlay that dies when the connection
drops. This is the same mechanism ADR-034 §2 describes for the inbound
browser case: the browser is a bidirectional call target during a live
session, not a peer-graph member, and the connection-local overlay is what
gives it bidirectional-call capability *without* peer-graph membership.

When the WS connection closes (browser closes the tab, network drops), the
overlay and all its registered ops are dropped — no explicit deregistration
needed. A `PeerRef::Specific("browser-X")` from another node would route to
nothing, because there is no `PeerEntry` for the browser (see §"Browsers
are not alknet peers" below for why).

### Streaming: native `call.responded` events, no SSE

A `Subscription` operation invoked over WS streams `call.responded` events
as binary WS messages directly — **no SSE `data:` framing**. SSE is the
`h2`/`http/1.1` streaming projection (ADR-036 §Streaming, applied at the
gateway's `/subscribe` endpoint per ADR-042 §2); on WS it is unnecessary
because WS is already a framed full-duplex channel. The browser receives
`call.responded` events one per WS binary message, with the same `id`
correlating them to the original `call.requested`; `call.completed` closes
the subscription (no more events); `call.aborted` closes it with an error
frame. This mirrors how subscriptions work on the QUIC path — see
[../call/call-protocol.md](../call/call-protocol.md) §"Streaming subscribe
example".

On WS client disconnect (the browser closes the tab mid-subscription),
the WS handler detects the stream close and sends `call.aborted` for the
in-flight subscription, which cascades to descendants per ADR-016.

### Browsers are not alknet peers (ADR-034 §4, amended by ADR-044 §5)

A browser over WebSocket authenticates by bearer token, gets no `PeerId`,
does not enter `PeerCompositeEnv`, and its registered ops (if any) land in
the connection-local Layer 2 overlay (above). The rationale, stated in
ADR-044 §5 and amending ADR-034 §4 by reference, is a load-bearing
distinction:

**"Peer" in alknet means an addressable node in the call-protocol peer
graph** — a stable `PeerId`, reachable via `PeerRef::Specific`, whose ops
land in `PeerCompositeEnv`, whose identity is stable across reconnects. It
does *not* mean "any endpoint that exchanges calls during a live session."
A browser is the second thing but not the first, on three concrete grounds:

1. **No stable cryptographic identity of its own.** A `PeerEntry` is anchored
   to fingerprints (Ed25519, X.509) that *the peer* presents and the local
   node pins. A browser presents a bearer token the *hub* issued; the
   "identity" is the hub's bookkeeping for that token, not something the
   browser owns or that could be pinned by another node. There is nothing
   to put in `PeerEntry.fingerprints`.
2. **Ephemeral.** Close the tab → connection dies → the connection-local
   Layer 2 overlay dies with it. A `PeerEntry` keyed to a browser would be a
   permanently-dead entry within seconds. `PeerRef::Specific("browser-X")`
   from another node would route to nothing.
3. **Not addressable from other nodes.** `PeerRef::Specific` resolves
   through `PeerEntry` → `PeerId`. Another alknet node has no way to reach
   "the browser currently connected to hub-A"; the hub holds that
   connection as a live `CallConnection` handle, not as a peer-graph entry.
   The connection-local overlay is precisely the mechanism that gives the
   browser bidirectional-call capability *without* peer-graph membership.

This is the explicit closure of the "browser as peer" path, on both the
inbound (this section) and outbound (ADR-034 §2) sides. The browser is a
**bidirectional call target during a live session**, not a **peer-graph
member**. The connection-local Layer 2 overlay (ADR-024, ADR-043 §3) is what
makes the former possible without requiring the latter. This rationale
applies transport-agnostically — to WebSocket, to WebTransport when it
revives, and to any future browser transport.

### Auth: bearer token on the upgrade request

Inbound WS auth is `Authorization: Bearer <token>` on the HTTP upgrade
request, resolved via `IdentityProvider::resolve_from_token()` — the same
path as any HTTP request ([http-server.md](http-server.md) §"Auth").
Bearer-only is the auth mechanism; other HTTP auth schemes are not
implemented for the WS upgrade (a deployment needing a different scheme
adds it as axum middleware on the upgrade route, two-way door). The
resolved identity is stored on the `Connection` for observability
(`connection.set_identity(identity)`), same as the HTTP handler.

The bearer token's `Identity` is what `AccessControl::check` runs against
when the browser calls an op via `call.requested` — the browser's
privileges are the token's privileges. This is the mechanism that gives the
browser per-privilege filtering for free: `services/list` is
`AccessControl::check(identity)`-filtered, so the browser's discovery
calls return only the ops its token is authorized to call. The
`api.alk.dev` operator UI sees only operator-authorized ops; a less-
privileged browser session sees a subset.

### What WebSocket does not provide (deferred to WebTransport, ADR-044)

WebSocket carries the call protocol from a browser; it does not carry the
non-call-ALPN substrate. The ALPN-stream-proxy (ADR-040) — a browser running
a WASM parser for SSH/SFTP/git to reach a non-call ALPN — requires
WebTransport's multi-stream model and is the speculative use case whose
deferral is ADR-044's reversal trigger. A browser cannot reach
SSH/SFTP/git ALPNs over WS in the v1 release; it can reach the call
protocol, and through the call protocol any ops the hub exposes. When
WebTransport revives, the two coexist: WSS stays as the simpler
call-protocol path; WebTransport adds the ALPN-stream-proxy path. Neither
replaces the other. See [webtransport.md](webtransport.md) for the deferred
design.

## Constraints

- **The WS path is the native `EventEnvelope` session, not the gateway
  shape (ADR-048).** The `to_openapi` gateway endpoints
  (`/search`/`/schema`/`/call`/`/batch`/`/subscribe`) are the HTTP
  one-directional projection and do not appear on WS. Discovery is via
  `services/list` and `services/schema` as ordinary call-protocol ops,
  not WS-specific endpoints. Subscriptions project as native
  `call.responded` events, not SSE.
- **Bearer-only auth on the upgrade request.** `Authorization: Bearer` →
  `resolve_from_token`. The resolved identity drives `AccessControl::check`
  on every `call.requested` the browser sends; per-privilege filtering is
  free via `services/list`'s existing `AccessControl` filtering.
- **Browsers are not alknet peers (ADR-034 §4, amended by ADR-044 §5).**
  Bearer token, no `PeerId`, no `PeerCompositeEnv` entry, connection-local
  Layer 2 overlay for any browser-registered ops. "Peer" means addressable
  peer-graph node, not "any endpoint that exchanges calls during a live
  session."
- **`EventEnvelope` frames are binary WS messages; one envelope per message,
  no length prefix (ADR-044 Assumption 1).** The WS message boundary is the
  delimiter — the QUIC path's 4-byte length prefix is not carried on the WS
  path (a WebSocket binary message is already length-delimited by the WS
  frame boundary). Text messages are rejected. The property is verified by
  the `@alkdev/pubsub` prior art.
- **WebSocket is the v1 browser bidirectional path; `h3`/WebTransport is
  deferred (ADR-044).** The ALPN-stream-proxy (ADR-040) is not available
  in v1. The `h3` ALPN and its feature gate are not implemented in the
  initial release; the WS path uses native axum WS support, no new
  dependency.
- **WS upgrade over HTTP/1.1 (`Upgrade: websocket`, RFC 6455) or HTTP/2
  (extended CONNECT, RFC 8441) is supported by the axum/hyper stack
  natively (ADR-044 Assumption 2).** `axum::extract::ws` provides the
  upgrade handler; the underlying connection is the same hyper HTTP
  connection the `h2`/`http/1.1` handler already drives. The handler does
  not branch on which HTTP version upgraded — the WS frame stream is the
  same once the upgrade completes. No new framing library is needed.
- **The shared `Dispatcher` runs over the WS message stream unchanged
  (ADR-012).** A WS message stream is another `BiStream`-satisfying
  transport; the `Dispatcher` and `PendingRequestMap` are transport-
  agnostic. Only the connection-establishment half differs (WS upgrade
  vs QUIC accept/dial).
- **The default WS upgrade path is `/alknet/call`; it must not collide
  with reserved paths (ADR-046).** The gateway endpoints, `/healthz`,
  `/openapi.json`, the MCP route, and any custom routes take precedence;
  `/alknet/call` namespaces away from the reserved set naturally. A
  deployment may override the path via the `extra_routes` mechanism
  (ADR-046).

## Future: `from_wss` adapter (out of scope, named for discoverability)

A `from_wss` adapter — importing a remote alknet node's operations over a
WebSocket connection, mirroring `from_call`'s pattern with WS as the
transport — is **out of scope for the current `alknet-http` work** and is
recorded here so it is not re-derived later. It is architecturally closer
to `from_call` than to `from_openapi`: `from_openapi` imports a
*foreign-protocol* surface (OpenAPI) and translates; `from_call` imports a
*same-protocol* endpoint over a different transport (QUIC) with no
translation. `from_wss` is the latter pattern with WS as the transport
instead of QUIC — open a WS connection, run `services/list` +
`services/schema` over it, register forwarding handlers that forward
`call.requested` over the WS connection. Because the `Dispatcher` is
stream-agnostic (ADR-012), it is `from_call` with a different
`BiStream`-satisfying transport.

Why it is out of scope now: it is a genuinely separate use case from the
browser-client case (it is "import a remote alknet node's ops over WSS,"
not "be a browser client"), and the consumers are not yet concrete enough
to commit the adapter's exact shape. The decision is made when a concrete
consumer arrives — a node that wants to import another node's ops but can
only reach it over WS (e.g., a browser-mediated relay, a restrictive-
network deployment). It is not needed for the v1 `api.alk.dev` topology
(Rust spokes use `from_call` over QUIC; the browser is a client, not an
importer). When revived, `from_wss` implements the `OperationAdapter` trait
(ADR-017 §5) and lives in `alknet-http` alongside `from_openapi`/`from_mcp`,
reusing the `Dispatcher` and the WS framing specified in this document.

This is a genuine scope decision (per ADR-009 §"What this framework is
NOT" — a decision that "genuinely doesn't need to be made yet because the
use case isn't concrete"), not a two-way-door deferral. The reversal
trigger is a concrete deployment needing to import a remote alknet node's
ops over WSS.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| Defer `h3`/WebTransport; browsers use WebSocket | [ADR-044](../../decisions/044-defer-webtransport-browsers-use-websocket.md) | WS is the v1 browser bidirectional path; `h3`/WebTransport deferred (scope); reversal trigger = concrete ALPN-stream-proxy use case |
| WS carries the native `EventEnvelope` session, not the gateway shape | [ADR-048](../../decisions/048-websocket-native-session-not-gateway.md) | The gateway endpoints are HTTP-only; WS is the call protocol's native session; discovery via `services/list`/`services/schema` as call-protocol ops; subscriptions as native `call.responded` events, no SSE |
| Call protocol stream model (stream-agnostic correlation) | [ADR-012](../../decisions/012-call-protocol-stream-model.md) | A WS message stream is another `BiStream`-satisfying transport; the `Dispatcher` and `PendingRequestMap` run unchanged |
| Call protocol client and adapter contract (`Dispatcher` shared) | [ADR-017](../../decisions/017-call-protocol-client-and-adapter-contract.md) §1 | The WS handler constructs a `Dispatcher` and calls `run_loop`, same as `CallAdapter`/`CallClient` — the dispatch half is one implementation |
| Operation registry layering (Layer 2 connection-local overlay) | [ADR-024](../../decisions/024-operation-registry-layering.md) | Browser-registered ops (if any) land in a connection-local overlay that dies with the WS connection |
| Bidirectionality transferred from WebTransport to WebSocket | [ADR-043](../../decisions/043-webtransport-bidirectional-alpn-substrate.md) §2 (parked; §2/§3 transfer per ADR-044 §3) | Both sides can `call.requested`; the browser case where the client registers no ops is a use-case scoping, not an architectural limitation |
| No-`PeerId` connection-local overlay transferred from WebTransport | [ADR-043](../../decisions/043-webtransport-bidirectional-alpn-substrate.md) §3 (parked; transfers per ADR-044 §3) | A browser over WS has no `PeerId`; ops land in the connection-local overlay |
| Browsers are not alknet peers (rationale: addressability vs bidirectionality) | [ADR-034](../../decisions/034-outgoing-only-x509-and-three-peer-roles.md) §4 (amended by ADR-044 §5) | Bearer token, no `PeerId`, no `PeerCompositeEnv` entry; "peer" means addressable peer-graph node, not "any endpoint that exchanges calls during a live session" |
| Peer-graph routing model (peer authorization via `AccessControl`) | [ADR-029](../../decisions/029-peer-graph-routing-model.md) §3 | `AccessControl::check(identity)` gates every `call.requested` from the browser; no `remote_safe`/`trusted_peer` (retired) |
| Abort cascade on WS disconnect | [ADR-016](../../decisions/016-abort-cascade-for-nested-calls.md) | WS close mid-subscription sends `call.aborted`, cascading to descendants |
| Bearer auth via `resolve_from_token` | [ADR-004](../../decisions/004-auth-as-shared-core.md) | WS upgrade request credential source (same as HTTP) |
| Browsers require X.509 (TLS) | [ADR-027](../../decisions/027-tls-identity-redesign-acme-rawkey-decoupling.md) | The WS upgrade runs over the same `h2`/`http/1.1` TLS as HTTP; browsers don't support RFC 7250 raw keys |
| Stealth: HTTP handler on standard ALPNs serves WS upgrade | [ADR-010](../../decisions/010-alpn-router-and-endpoint.md) | The WS upgrade route is on `HttpAdapter`'s default surface; a port scanner sees the decoy for unknown paths |
| Custom routes collision rule | [ADR-046](../../decisions/046-assembly-layer-custom-http-routes.md) | The WS upgrade route must not collide with reserved default-surface paths; it namespaces away naturally |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-38** (open, scope): WebTransport standalone relay service scope —
  the standalone relay (future `alknet-relay`, fork of iroh-relay) is
  distinct from the in-process ALPN-stream-proxy (ADR-040, parked) and
  from the WebSocket browser path (this spec). The relay is a separate
  service for NAT traversal, not a mode of the WS handler; it does not
  affect the WS path.

No new open questions. The `from_wss` deferral (§"Future") is a scope
decision stated explicitly, not an open question — the reversal trigger is
concrete (a deployment needing to import a remote node's ops over WSS),
and there is no architectural question to resolve before then.

## References

- [ADR-044](../../decisions/044-defer-webtransport-browsers-use-websocket.md)
  — the ADR that committed WS as the v1 browser bidirectional path;
  §"Concrete prior art" references the `@alkdev/pubsub` WS client/server;
  §5 states the "browser is not a peer" rationale this spec carries.
- [ADR-048](../../decisions/048-websocket-native-session-not-gateway.md)
  — the ADR that commits the WS path as the native `EventEnvelope` session,
  not the HTTP gateway shape.
- [http-server.md](http-server.md) — the `HttpAdapter` that hosts the WS
  upgrade handler; the one-directional HTTP projection (the gateway) the
  WS path is contrasted against.
- [webtransport.md](webtransport.md) — the deferred `h3`/WebTransport
  handler; kept intact for revival. When WebTransport revives, the two
  coexist (WSS for the call protocol; WebTransport for the ALPN-stream-
  proxy).
- [../call/call-protocol.md](../call/call-protocol.md) — `EventEnvelope`
  wire format, the `Dispatcher`, the `PendingRequestMap`, stream model,
  bidirectional calls, §"Transport agnosticism" (the pubsub-lineage note).
- [../call/client-and-adapters.md](../call/client-and-adapters.md) — the
  shared `Dispatcher` (§"Shared Dispatcher"), `services/list` filtering,
  the `CallClient` (the QUIC-side counterpart to the WS browser client).
- [../call/operation-registry.md](../call/operation-registry.md) —
  `OperationRegistry::invoke()`, Layer 2 connection-local overlay.
- `/workspace/@alkdev/pubsub/src/event-target-websocket-client.ts`,
  `/workspace/@alkdev/pubsub/src/event-target-websocket-server.ts` —
  TypeScript prior art: the `EventEnvelope { type, id, payload }` over WS
  binary messages. The alknet `EventEnvelope` is a refined superset (typed
  event names, structured payloads); the delta is small and well-defined.
- `/workspace/@alkdev/operations/src/` — TypeScript sibling package sharing
  the pubsub lineage; the `path.do.op` (dot-separated) convention vs
  alknet's `path/to/op` (slash-separated) is the minor mechanical delta a
  sync adjusts.