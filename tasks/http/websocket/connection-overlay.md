---
id: http/websocket/connection-overlay
name: Implement connection-local Layer 2 overlay for browser-registered ops (no PeerId, ADR-024/034/044)
status: pending
depends_on: [http/websocket/upgrade-handler]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Implement the connection-local Layer 2 overlay for browser-registered
ops in `src/websocket/overlay.rs`. This is the mechanism that gives a
browser bidirectional-call capability *without* peer-graph membership
(ADR-024, ADR-034 §4, ADR-044 §5). A browser over WebSocket has no
`PeerId`, does not enter `PeerCompositeEnv`, and any ops it registers
land in a per-`CallConnection` overlay that dies when the connection
drops.

### Browsers are not alknet peers (websocket.md §"Browsers are not alknet peers")

A browser over WebSocket authenticates by bearer token, gets no
`PeerId`, does not enter `PeerCompositeEnv`, and its registered ops (if
any) land in the connection-local Layer 2 overlay. The rationale, stated
in ADR-044 §5 and amending ADR-034 §4 by reference, is a load-bearing
distinction:

**"Peer" in alknet means an addressable node in the call-protocol peer
graph** — a stable `PeerId`, reachable via `PeerRef::Specific`, whose ops
land in `PeerCompositeEnv`, whose identity is stable across reconnects.
It does *not* mean "any endpoint that exchanges calls during a live
session." A browser is the second thing but not the first, on three
concrete grounds:

1. **No stable cryptographic identity of its own.** A `PeerEntry` is
   anchored to fingerprints (Ed25519, X.509) that *the peer* presents
   and the local node pins. A browser presents a bearer token the *hub*
   issued; the "identity" is the hub's bookkeeping for that token, not
   something the browser owns or that could be pinned by another node.
   There is nothing to put in `PeerEntry.fingerprints`.

2. **Ephemeral.** Close the tab → connection dies → the connection-local
   Layer 2 overlay dies with it. A `PeerEntry` keyed to a browser would
   be a permanently-dead entry within seconds. `PeerRef::Specific("browser-X")`
   from another node would route to nothing.

3. **Not addressable from other nodes.** `PeerRef::Specific` resolves
   through `PeerEntry` → `PeerId`. Another alknet node has no way to
   reach "the browser currently connected to hub-A"; the hub holds that
   connection as a live `CallConnection` handle, not as a peer-graph
   entry. The connection-local overlay is precisely the mechanism that
   gives the browser bidirectional-call capability *without* peer-graph
   membership.

### The overlay (websocket.md §"Connection-local overlay")

A browser over WebSocket has no `PeerId` on the hub's side. Any ops the
browser registers land in a **connection-local Layer 2 overlay**
(ADR-024) — a per-`CallConnection` overlay that dies when the connection
drops. This is the same mechanism ADR-034 §2 describes for the inbound
browser case: the browser is a bidirectional call target during a live
session, not a peer-graph member, and the connection-local overlay is
what gives it bidirectional-call capability *without* peer-graph
membership.

When the WS connection closes (browser closes the tab, network drops),
the overlay and all its registered ops are dropped — no explicit
deregistration needed. A `PeerRef::Specific("browser-X")` from another
node would route to nothing, because there is no `PeerEntry` for the
browser.

### Bidirectionality (websocket.md §"Bidirectionality")

The WS call-protocol session inherits the call protocol's native
bidirectionality: both sides can send `call.requested` frames. The
browser calls operations on the hub; the hub can call operations
registered on the browser's side, over the same session, using the same
`PendingRequestMap` and `EventEnvelope` framing as `alknet/call`.

The browser case where the client registers no operations of its own
is the common case — the server→client call direction is unused because
the browser has nothing to call. That is a use-case scoping, not an
architectural limitation. A browser that *does* expose ops (e.g., a UI
that registers a `ui/dragged` op the hub can call to push live updates)
registers them in the connection-local Layer 2 overlay, and the hub
reaches them through the live `CallConnection` handle — not through
`PeerRef::Specific` (the browser is not a peer).

### Implementation

The `CallConnection` constructed by the upgrade handler (the
`upgrade-handler` task, via the `dispatcher-transport-abstraction`
task's non-QUIC constructor) already holds a Layer 2 overlay
(`imported_operations: Arc<RwLock<HashMap<String, HandlerRegistration>>>`)
and exposes `register_imported()` / `register_imported_all()` /
`overlay_env()`. The browser registers ops via these methods; the
overlay is per-connection and dies when the `CallConnection` is dropped
(WS close).

This task ensures:

1. The overlay is correctly scoped to the WS connection (not the
   `PeerCompositeEnv` — no `PeerId`, no `PeerEntry`).
2. The hub's outgoing `call.requested` to browser-registered ops routes
   through the `CallConnection`'s overlay (via `overlay_env()`), not
   through `PeerRef::Specific`.
3. The overlay is dropped on WS close (no explicit deregistration; the
   `Arc<RwLock<HashMap>>` is dropped when the `CallConnection` is
   dropped).
4. `AccessControl::check(identity)` gates the hub's calls to
   browser-registered ops (the browser's bearer-token identity is the
   caller identity for the hub's outgoing calls — wait, no: the *hub*
   is the caller when it calls a browser op; the browser's identity is
   the *handler* identity. Clarify: the hub's `call.requested` to a
   browser op runs with the hub's identity as caller, the browser's
   registration bundle's `composition_authority` as handler identity.
   The browser's `AccessControl` on its registered ops gates whether
   the hub is allowed to call them.)
5. Abort cascade on WS disconnect (ADR-016): when the WS connection
   closes, all in-flight subscriptions and calls to browser ops are
   aborted, cascading to descendants.

### What this task does NOT do

- **No `PeerEntry` for the browser.** The browser is not in the peer
  graph. This task ensures the overlay is connection-local, not
  peer-graph.
- **No `from_wss` adapter.** Out of scope (websocket.md §"Future" —
  scope decision). This task is about the browser *registering* ops on
  its connection, not about importing a remote node's ops over WS.

## Acceptance Criteria

- [ ] Browser-registered ops land in the `CallConnection`'s Layer 2 overlay (not `PeerCompositeEnv`)
- [ ] No `PeerId` created for the browser (no `PeerEntry`, no peer-graph membership)
- [ ] `register_imported()` / `register_imported_all()` work for browser ops
- [ ] Hub's outgoing `call.requested` to browser ops routes through `overlay_env()`
- [ ] Hub's outgoing calls do NOT route through `PeerRef::Specific` (browser is not a peer)
- [ ] `AccessControl` on browser-registered ops gates the hub's calls
- [ ] Overlay dropped on WS close (no explicit deregistration; `Arc<RwLock<HashMap>>` dropped)
- [ ] `PeerRef::Specific("browser-X")` from another node → routes to nothing (no `PeerEntry`)
- [ ] WS close → all in-flight subscriptions/calls to browser ops aborted (ADR-016 cascade)
- [ ] WS close → overlay and all registered ops dropped
- [ ] Bidirectionality: hub can `call.requested` to browser-registered ops
- [ ] Browser with no registered ops → server→client direction unused (use-case scoping, not a limitation)
- [ ] Integration test: browser registers op → hub calls it via overlay
- [ ] Integration test: WS close → overlay dropped (op no longer reachable)
- [ ] Integration test: `PeerRef::Specific("browser-X")` → NOT_FOUND (no PeerEntry)
- [ ] Integration test: WS close mid-call to browser op → `call.aborted` cascade
- [ ] Integration test: `AccessControl` on browser op gates hub's call
- [ ] `cargo test -p alknet-http` succeeds
- [ ] `cargo clippy -p alknet-http --all-targets` succeeds with no warnings

## References

- docs/architecture/crates/http/websocket.md — Connection-local overlay (§"Connection-local overlay"), Bidirectionality (§"Bidirectionality"), Browsers are not peers (§"Browsers are not alknet peers")
- docs/architecture/decisions/024-operation-registry-layering.md — ADR-024 (Layer 2 connection-local overlay)
- docs/architecture/decisions/034-outgoing-only-x509-and-three-peer-roles.md — ADR-034 §4 (browsers are not peers)
- docs/architecture/decisions/044-defer-webtransport-browsers-use-websocket.md — ADR-044 §5 (addressability vs bidirectionality rationale)
- docs/architecture/decisions/016-abort-cascade-for-nested-calls.md — ADR-016 (abort cascade on disconnect)
- docs/architecture/decisions/029-peer-graph-routing-model.md — ADR-029 (PeerRef::Specific routes through PeerEntry → PeerId)

## Notes

> The connection-local overlay is the mechanism that gives a browser
> bidirectional-call capability without peer-graph membership. The
> browser has no PeerId, no PeerEntry, no PeerCompositeEnv entry — it is
> a bidirectional call target during a live session, not a peer-graph
> member. The overlay dies with the WS connection (no explicit
> deregistration). The hub reaches browser ops through the live
> CallConnection handle's overlay_env(), not through PeerRef::Specific.
> The "browsers are not peers" rationale (ADR-044 §5) is load-bearing:
> "peer" means addressable peer-graph node, not "any endpoint that
> exchanges calls during a live session." A browser has no stable
> cryptographic identity, is ephemeral, and is not addressable from
> other nodes — three concrete grounds for not being a peer.

## Summary

> To be filled on completion