---
id: http/websocket/upgrade-handler
name: Implement WebSocket upgrade handler (native EventEnvelope session, no length prefix, bearer auth)
status: completed
depends_on: [http/server/http-adapter, http/websocket/dispatcher-transport-abstraction, http/server/bearer-auth-middleware]
scope: broad
risk: high
impact: component
level: implementation
---

## Description

Implement the WebSocket upgrade handler in `src/websocket/upgrade.rs`.
This is the v1 browser bidirectional path (ADR-044): a browser (or any
WS client) upgrades an HTTP/1.1 or HTTP/2 request to WebSocket and
speaks the call protocol over binary WS messages — full-duplex, both
sides can initiate calls (the call protocol's native bidirectionality,
ADR-012). The WS path carries the **native `EventEnvelope` session, not
the HTTP gateway shape** (ADR-048): the gateway endpoints
(`/search`/`/schema`/`/call`/`/batch`/`/subscribe`) are HTTP-only and do
not appear on WS; discovery is via `services/list`/`services/schema` as
ordinary call-protocol ops.

### The upgrade handler (websocket.md §"The WS upgrade handler")

The WS upgrade is an HTTP/1.1 or HTTP/2 request handled by an axum route
on `HttpAdapter`'s router. The handler:

1. Receives the HTTP upgrade request (axum's `WebSocketUpgrade` extractor).
2. Resolves the caller's identity from the `Authorization: Bearer` header
   via `identity_provider.resolve_from_token(&AuthToken { raw:
   token_bytes })` (the shared `bearer_auth_middleware` — same auth path
   as any HTTP request). The upgrade is rejected (`401`) if no token is
   present; insufficient scopes for any op the browser later calls
   surface as `403`/`FORBIDDEN` at call time, not at upgrade time (the
   upgrade doesn't know which ops the browser will call).
3. Upgrades to WebSocket (axum's `WebSocketUpgrade::on_upgrade`),
   producing a full-duplex `WebSocket` stream.
4. Wraps the `WebSocket` stream as a `BiStream`-satisfying transport — a
   WS binary message in either direction is one `EventEnvelope` frame.
5. Constructs a `Dispatcher` (the shared dispatch loop) with the
   `Arc<OperationRegistry>` and `Arc<dyn IdentityProvider>` the
   `HttpAdapter` holds, plus a connection-local Layer 2 overlay for any
   ops the browser registers (the `connection-overlay` task).
6. Spawns the dispatch task on a tokio task; the WS connection is live
   until either side closes it or the browser drops the handle (closes
   the tab).

### The upgrade path

The **default upgrade path is `/alknet/call`** (the deployment may
override it via the `extra_routes` mechanism of ADR-046, but a
deployment that passes no custom routes gets `/alknet/call`). The path
must not collide with the reserved gateway/`/healthz`/`/openapi.json`/
MCP/custom-route paths per ADR-046's collision rule; `/alknet/call`
namespaces away from the reserved set naturally. A deployment that
builds a custom REST projection with `POST /{service}/{op}` routes
(ADR-047 §4) coexists with the WS upgrade at `/alknet/call` — axum's
`Router::merge` prioritizes specific routes over wildcards, so the WS
upgrade's exact `/alknet/call` path wins over any `/{service}/{op}`
wildcard.

The upgrade runs over HTTP/1.1 (the standard `Upgrade: websocket` header,
RFC 6455) or HTTP/2 (the extended CONNECT protocol, RFC 8441);
axum/hyper supports both, and the handler does not branch on which —
the WS frame stream is the same once the upgrade completes.

### Framing: `EventEnvelope` over binary WS messages (websocket.md §"Framing")

Every message on the WS connection is a binary WebSocket message
containing one `EventEnvelope`:

```rust
pub struct EventEnvelope {
    pub r#type: String,    // "call.requested" | "call.responded" | "call.completed" | "call.aborted" | "call.error"
    pub id: String,        // Correlation key (request ID, subscription ID)
    pub payload: Value,    // serde_json::Value — schema depends on event type
}
```

This is the call protocol's wire format verbatim. **The WS path carries
no length prefix**: one `EventEnvelope` JSON object = one binary WS
message, and the WS message boundary is the delimiter. The
implementation must not prepend the QUIC length prefix on outbound WS
messages or expect it on inbound ones — the two framings are
deliberately different, matching each transport's native boundary
semantics. (The `FrameFramedReader`/`FrameFramedWriter` types the QUIC
dispatch loop uses are replaced on the WS path by direct JSON serde
over the WS message type; the `Dispatcher` itself is transport-agnostic
and consumes `EventEnvelope` values, not raw bytes.)

Binary payloads within `EventEnvelope.payload` follow the same
base64-as-JSON-string convention the QUIC path uses — the envelope
carries `serde_json::Value` and does not interpret binary fields; that's
a handler-level concern, transport-agnostic.

Text WS messages are not used; all call-protocol frames are binary. A
client that sends a text message gets a protocol-level close (the WS
handler validates message type).

### Dispatch: the shared `Dispatcher` (websocket.md §"Dispatch")

The WS message stream is handed to the `Dispatcher` — the same dispatch
loop the `CallAdapter` uses for `alknet/call` QUIC connections. The
dispatch half is one implementation; the connection-establishment half
differs (WS upgrade handler vs QUIC accept/dial), but after
establishment the `Dispatcher` runs identically:

- Reads `EventEnvelope` frames from the WS message stream (deserialized
  from binary WS messages — no `FrameFramedReader`).
- For `call.requested`: resolves the peer's identity (the bearer-token
  identity resolved at upgrade time, stored on the connection), runs
  `AccessControl::check(identity)` against the op's `AccessControl`,
  dispatches via `OperationRegistry::invoke()` if allowed, returns
  `FORBIDDEN` (→ `call.error`) before the handler runs if not.
- For `call.responded`/`call.completed`/`call.aborted`: correlates by
  `id` via `PendingRequestMap` (keyed by request ID, not by transport —
  ADR-012).
- Writes response `EventEnvelope` frames back as binary WS messages.

Peer authorization flows through the existing `AccessControl::check`
against the resolved identity — no `RemoteFilter`, no `remote_safe`
gate (retired by ADR-029 §3).

### Using the exposed dispatch API

This task uses the `pub` dispatch API exposed by the
`dispatcher-transport-abstraction` task:

- `Dispatcher::dispatch_requested(connection, request_id, payload)` —
  for `call.requested` events.
- The `pub` abort-handling method — for `call.aborted` events.
- `CallConnection` constructed from the non-QUIC source (holding the
  resolved bearer identity, a fresh Layer 2 overlay, a fresh
  `PendingRequestMap`).

### Bidirectionality (websocket.md §"Bidirectionality")

The WS call-protocol session inherits the call protocol's native
bidirectionality: both sides can send `call.requested` frames. The
browser calls operations on the hub; the hub can call operations
registered on the browser's side, over the same session, using the same
`PendingRequestMap` and `EventEnvelope` framing as `alknet/call`.

The browser case where the client registers no operations of its own
is the common case — the server→client call direction is unused
because the browser has nothing to call. That is a use-case scoping,
not an architectural limitation. A browser that *does* expose ops
registers them in the connection-local Layer 2 overlay (the
`connection-overlay` task).

### Streaming: native `call.responded` events, no SSE (websocket.md §"Streaming")

A `Subscription` operation invoked over WS streams `call.responded`
events as binary WS messages directly — **no SSE `data:` framing**. SSE
is the `h2`/`http/1.1` streaming projection; on WS it is unnecessary
because WS is already a framed full-duplex channel. The browser receives
`call.responded` events one per WS binary message, with the same `id`
correlating them to the original `call.requested`; `call.completed`
closes the subscription; `call.aborted` closes it with an error frame.

On WS client disconnect (the browser closes the tab mid-subscription),
the WS handler detects the stream close and sends `call.aborted` for
the in-flight subscription, which cascades to descendants per ADR-016.

## Acceptance Criteria

- [ ] WS upgrade route at `/alknet/call` (default, ADR-046 collision rule)
- [ ] Upgrade handler uses axum's `WebSocketUpgrade` extractor
- [ ] Bearer auth on upgrade request via shared `bearer_auth_middleware`
- [ ] No token → `401` (upgrade rejected)
- [ ] Token present but insufficient scopes → `403` at call time (not upgrade time)
- [ ] Resolved identity stored on the `CallConnection` (for observability + AccessControl)
- [ ] WS binary message = one `EventEnvelope` (JSON serde, no length prefix)
- [ ] No `FrameFramedReader`/`FrameFramedWriter` on the WS path (WS message boundary is delimiter)
- [ ] Text WS messages rejected (protocol-level close)
- [ ] `call.requested` → `Dispatcher::dispatch_requested` (the pub API)
- [ ] `AccessControl::check(identity)` gates every `call.requested`
- [ ] `FORBIDDEN` → `call.error` event (before handler runs)
- [ ] `call.responded`/`call.completed`/`call.aborted` correlated by `id` via `PendingRequestMap`
- [ ] Response `EventEnvelope` frames written as binary WS messages
- [ ] `call.aborted` → the pub abort-handling method
- [ ] Bidirectionality: hub can `call.requested` to browser-registered ops
- [ ] `Subscription` streams `call.responded` as binary WS messages (no SSE)
- [ ] `call.completed` closes subscription; `call.aborted` closes with error
- [ ] WS client disconnect mid-subscription → `call.aborted` (ADR-016 cascade)
- [ ] WS close → fail all pending, drop overlay (connection-local)
- [ ] Upgrade works over HTTP/1.1 (RFC 6455) and HTTP/2 (RFC 8441)
- [ ] Handler does not branch on HTTP version (WS frame stream is same post-upgrade)
- [ ] Integration test: WS upgrade → `call.requested` → `call.responded` round-trip
- [ ] Integration test: no Bearer token → 401
- [ ] Integration test: `AccessControl` denied → `call.error` FORBIDDEN
- [ ] Integration test: `Subscription` over WS → multiple `call.responded` + `call.completed`
- [ ] Integration test: WS disconnect mid-subscription → `call.aborted` cascade
- [ ] Integration test: text WS message → protocol close
- [ ] Integration test: bidirectional (hub calls browser-registered op)
- [ ] `cargo test -p alknet-http` succeeds
- [ ] `cargo clippy -p alknet-http --all-targets` succeeds with no warnings

## References

- docs/architecture/crates/http/websocket.md — full WS spec (upgrade handler, framing, dispatch, bidirectionality, streaming)
- docs/architecture/decisions/044-defer-webtransport-browsers-use-websocket.md — ADR-044 (WS is v1 browser path, no length prefix)
- docs/architecture/decisions/048-websocket-native-session-not-gateway.md — ADR-048 (native session, not gateway shape)
- docs/architecture/decisions/012-call-protocol-stream-model.md — ADR-012 (stream-agnostic correlation)
- docs/architecture/decisions/016-abort-cascade-for-nested-calls.md — ADR-016 (disconnect → abort cascade)
- docs/architecture/decisions/029-peer-graph-routing-model.md — ADR-029 §3 (AccessControl::check is sole gate)
- docs/architecture/decisions/046-assembly-layer-custom-http-routes.md — ADR-046 (collision rule for /alknet/call)
- /workspace/@alkdev/pubsub/src/event-target-websocket-client.ts — TypeScript prior art (EventEnvelope over WS binary messages)

## Notes

> The WS path is the native EventEnvelope session, not the gateway shape
> (ADR-048). The gateway endpoints are HTTP-only; discovery is via
> services/list/services/schema as call-protocol ops. The WS path carries
> no length prefix (ADR-044 Assumption 1 — the WS message boundary is the
> delimiter, unlike QUIC's 4-byte prefix). Text messages are rejected. The
> dispatch uses the pub API exposed by the dispatcher-transport-abstraction
> task (dispatch_requested + abort-handling + non-QUIC CallConnection).
> Bidirectionality: both sides can call.requested (ADR-043 §2 transferred
> per ADR-044 §3). Streaming is native call.responded events, no SSE. The
> default upgrade path is /alknet/call (namespaces away from reserved paths
> per ADR-046). This is the second-highest-risk task (after the transport
> abstraction) — the WS dispatch loop must be identical to the QUIC dispatch
> loop on the security axis (AccessControl, identity, abort cascade).

## Summary

> Implemented src/websocket/upgrade.rs: WS upgrade handler at /alknet/call using axum
> WebSocketUpgrade, bearer auth via shared bearer_auth_middleware (no token → 401),
> resolved identity stored on CallConnection::new_overlay_only, native EventEnvelope
> over binary WS messages (no length prefix, text → protocol close 1002), shared
> Dispatcher::dispatch_requested for call.requested (AccessControl::check gates →
> FORBIDDEN call.error), Dispatcher::handle_abort for call.aborted, responded/completed/
> aborted correlated via PendingRequestMap, fail_all_pending on disconnect (ADR-016
> cascade), bidirectionality via connection-local overlay. Wired /alknet/call route
> into adapter.rs router. 168 tests pass (incl. round-trip, 401, FORBIDDEN, subscription,
> disconnect abort, text-close, bidirectional overlay, no-length-prefix). Clippy clean.