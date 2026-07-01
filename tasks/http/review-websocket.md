---
id: http/review-websocket
name: Review WebSocket path for ADR-044/048 conformance (native session, no length prefix, browsers-not-peers)
status: completed
depends_on: [http/websocket/connection-overlay]
scope: moderate
risk: low
impact: phase
level: review
---

## Description

Review the WebSocket path for spec conformance, pattern consistency,
and correctness. This is the quality checkpoint for the browser
bidirectional path — the most architecturally subtle part of the crate
(browsers are not peers, the WS path carries the native session not the
gateway shape, no length prefix).

### Review Checklist

1. **Dispatcher transport abstraction conformance** (ADR-012):
   - `Dispatcher::dispatch_requested` is `pub` (was `pub(crate)`)
   - Abort-handling method is `pub` (extracted from `handle_stream`)
   - `CallConnection` constructible from non-QUIC source
   - Non-QUIC `CallConnection` holds overlay + pending + identity
   - Existing QUIC path (`CallAdapter`, `CallClient`) unchanged — no regressions
   - Full `alknet-call` test suite passes (no regressions in the cross-crate change)

2. **WS upgrade handler conformance** (websocket.md, ADR-044/048):
   - Upgrade route at `/alknet/call` (default, ADR-046 collision rule)
   - Bearer auth on upgrade request (shared `bearer_auth_middleware`)
   - No token → 401 (upgrade rejected)
   - Insufficient scopes → 403 at call time (not upgrade time)
   - Resolved identity stored on `CallConnection`
   - Upgrade works over HTTP/1.1 (RFC 6455) and HTTP/2 (RFC 8441)
   - Handler does not branch on HTTP version (WS frame stream same post-upgrade)

3. **Framing conformance** (websocket.md §"Framing", ADR-044 Assumption 1):
   - Binary WS message = one `EventEnvelope` (JSON serde)
   - **No length prefix** (WS message boundary is delimiter, unlike QUIC's 4-byte prefix)
   - No `FrameFramedReader`/`FrameFramedWriter` on the WS path
   - Text WS messages rejected (protocol-level close)
   - Binary payloads follow base64-as-JSON-string convention (same as QUIC)

4. **Dispatch conformance** (websocket.md §"Dispatch", ADR-012):
   - `call.requested` → `Dispatcher::dispatch_requested` (the pub API)
   - `AccessControl::check(identity)` gates every `call.requested`
   - `FORBIDDEN` → `call.error` (before handler runs)
   - `call.responded`/`call.completed`/`call.aborted` correlated by `id` via `PendingRequestMap`
   - Response `EventEnvelope` frames written as binary WS messages
   - `call.aborted` → the pub abort-handling method
   - No `RemoteFilter`/`remote_safe` (retired by ADR-029 §3)

5. **Bidirectionality conformance** (websocket.md §"Bidirectionality", ADR-043 §2):
   - Both sides can send `call.requested` (native call-protocol bidirectionality)
   - Hub can call browser-registered ops via overlay
   - Browser with no registered ops → server→client unused (use-case scoping)

6. **Connection-local overlay conformance** (websocket.md §"Connection-local overlay", ADR-024/034/044):
   - Browser-registered ops land in `CallConnection`'s Layer 2 overlay (not `PeerCompositeEnv`)
   - No `PeerId` for browser (no `PeerEntry`, no peer-graph membership)
   - Hub's outgoing `call.requested` routes through `overlay_env()` (not `PeerRef::Specific`)
   - `PeerRef::Specific("browser-X")` → routes to nothing (no `PeerEntry`)
   - Overlay dropped on WS close (no explicit deregistration)
   - `AccessControl` on browser ops gates hub's calls

7. **Browsers-are-not-peers rationale** (websocket.md §"Browsers are not alknet peers", ADR-044 §5):
   - No stable cryptographic identity (bearer token, not fingerprints)
   - Ephemeral (close tab → overlay dies)
   - Not addressable from other nodes (no `PeerEntry`)
   - "Peer" = addressable peer-graph node, not "any endpoint that exchanges calls"

8. **Streaming conformance** (websocket.md §"Streaming"):
   - `Subscription` over WS → `call.responded` as binary WS messages (no SSE)
   - `call.completed` closes subscription; `call.aborted` closes with error
   - WS disconnect mid-subscription → `call.aborted` cascade (ADR-016)

9. **ADR conformance**:
   - ADR-012: stream-agnostic correlation (Dispatcher runs unchanged)
   - ADR-016: abort cascade on disconnect
   - ADR-024: Layer 2 connection-local overlay
   - ADR-029 §3: AccessControl::check is sole gate (no remote_safe)
   - ADR-034 §4: browsers are not peers (amended by ADR-044 §5)
   - ADR-044: WS is v1 browser path, no length prefix, no h3
   - ADR-048: WS carries native session, not gateway shape

10. **Security constraints**:
    - AccessControl::check(identity) is the sole authorization gate
    - No secret material on the WS path (ADR-014)
    - Internal ops → NOT_FOUND (don't leak existence)
    - Abort cascade on disconnect (ADR-016)

11. **Test coverage**:
    - Integration test: WS upgrade → call.requested → call.responded round-trip
    - Integration test: no Bearer token → 401
    - Integration test: AccessControl denied → call.error FORBIDDEN
    - Integration test: Subscription over WS → call.responded + call.completed
    - Integration test: WS disconnect mid-subscription → call.aborted cascade
    - Integration test: text WS message → protocol close
    - Integration test: bidirectional (hub calls browser-registered op)
    - Integration test: PeerRef::Specific("browser-X") → NOT_FOUND
    - alknet-call tests pass (no regressions from the transport abstraction change)

## Acceptance Criteria

- [ ] Dispatcher transport abstraction: pub API, non-QUIC CallConnection, no regressions
- [ ] WS upgrade: /alknet/call, Bearer auth, 401 on no token, HTTP/1.1 + HTTP/2
- [ ] Framing: binary WS = EventEnvelope, no length prefix, text rejected
- [ ] Dispatch: call.requested → dispatch_requested, AccessControl gates, correlation by id
- [ ] Bidirectionality: both sides can call.requested, hub calls browser ops via overlay
- [ ] Connection-local overlay: no PeerId, no PeerEntry, overlay dies on close
- [ ] Browsers-not-peers: no stable identity, ephemeral, not addressable
- [ ] Streaming: native call.responded (no SSE), abort cascade on disconnect
- [ ] All ADRs conformed to (012, 016, 024, 029, 034, 044, 048)
- [ ] AccessControl is the sole authorization gate
- [ ] No secret material on WS path
- [ ] Internal ops → NOT_FOUND
- [ ] Test coverage adequate for all WS functionality
- [ ] `cargo fmt --check -p alknet-http` passes
- [ ] `cargo clippy -p alknet-http` passes with no warnings
- [ ] `cargo test -p alknet-call` passes (no regressions)
- [ ] All tests pass

## References

- docs/architecture/crates/http/websocket.md — full WS spec
- docs/architecture/decisions/044-defer-webtransport-browsers-use-websocket.md — ADR-044
- docs/architecture/decisions/048-websocket-native-session-not-gateway.md — ADR-048
- docs/architecture/decisions/012-call-protocol-stream-model.md — ADR-012
- docs/architecture/decisions/016-abort-cascade-for-nested-calls.md — ADR-016
- docs/architecture/decisions/024-operation-registry-layering.md — ADR-024
- docs/architecture/decisions/029-peer-graph-routing-model.md — ADR-029 §3
- docs/architecture/decisions/034-outgoing-only-x509-and-three-peer-roles.md — ADR-034 §4
- /workspace/@alkdev/pubsub/src/event-target-websocket-client.ts — prior art

## Notes

> The WebSocket path is the most architecturally subtle part of the
> crate. The review should verify: (1) the Dispatcher transport
> abstraction didn't regress the QUIC path (run alknet-call tests), (2)
> the WS path carries the native EventEnvelope session not the gateway
> shape (ADR-048), (3) no length prefix (ADR-044 Assumption 1), (4)
> browsers are not peers (no PeerId, connection-local overlay, ADR-034 §4
> + ADR-044 §5), (5) AccessControl is the sole gate (ADR-029 §3), (6)
> abort cascade on disconnect (ADR-016). The "browsers are not peers"
> rationale is load-bearing — verify the three grounds (no stable
> identity, ephemeral, not addressable) are reflected in the
> implementation. If deviations are found, document and fix before
> considering the WS path complete.

## Summary

> WebSocket path reviewed against all 11 checklist items. All conformance criteria
> pass: dispatcher transport abstraction (pub API, non-QUIC CallConnection, no
> regressions — 277+2 alknet-call tests), WS upgrade (/alknet/call, Bearer auth, 401,
> HTTP/1.1+HTTP/2), framing (binary WS = EventEnvelope, no length prefix, text → close
> 1002), dispatch (dispatch_requested, AccessControl gates, FORBIDDEN → call.error,
> correlation by id, handle_abort), bidirectionality (hub calls browser ops via overlay),
> connection-local overlay (no PeerId, no PeerEntry, overlay_env() routing,
> PeerRef::Specific → NOT_FOUND, overlay dies on close), browsers-not-peers rationale,
> streaming (native call.responded, no SSE, abort cascade on disconnect), all ADRs
> (012/016/024/029/034/044/048), security constraints. 230 tests pass, clippy clean,
> fmt clean.