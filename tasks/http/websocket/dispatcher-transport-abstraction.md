---
id: http/websocket/dispatcher-transport-abstraction
name: Expose EventEnvelope-level dispatch API in alknet-call for non-QUIC transports (WebSocket)
status: completed
depends_on: [http/crate-init]
scope: moderate
risk: high
impact: project
level: implementation
---

## Description

Expose an `EventEnvelope`-level dispatch API in `alknet-call` so the
WebSocket handler can feed deserialized envelopes directly to the shared
`Dispatcher`, without requiring a QUIC `Connection`. This is a
**cross-crate task** (modifies `alknet-call`) and the **highest-risk
task** in the http phase: the spec says "the `Dispatcher` runs unchanged"
over WS (ADR-012, ADR-048), but the current implementation is
QUIC-specific in two places that need loosening.

### The problem

The current `Dispatcher` (in `crates/alknet-call/src/protocol/dispatch.rs`)
is transport-agnostic in *intent* (ADR-012 — stream-agnostic
correlation) but QUIC-specific in *two* integration points:

1. **`Dispatcher::handle_stream`** takes raw `SendStream` / `RecvStream`
   (QUIC-backed `alknet_core::types::SendStream` / `RecvStream`) and uses
   `FrameFramedReader` (4-byte length-prefixed framing). The WebSocket
   path does NOT use length-prefix framing — a WS binary message is
   already length-delimited by the WS frame boundary (ADR-044 Assumption
   1). The WS handler deserializes `EventEnvelope` from each binary WS
   message directly (no `FrameFramedReader`), and needs to feed the
   envelope to the dispatch logic.

2. **`CallConnection`** wraps an `alknet_core::types::Connection` (which
   wraps a QUIC `quinn::Connection` or `iroh::endpoint::Connection`).
   The WS path has no QUIC connection — it has a WS message stream. The
   `CallConnection` is needed for: the Layer 2 overlay
   (`imported_operations`), the `PendingRequestMap` (correlation), and
   the `connection.identity()` (the resolved bearer identity). The WS
   path needs a `CallConnection`-equivalent that holds these without a
   QUIC `Connection`.

### The fix: expose `dispatch_requested` as `pub`

The core dispatch logic — `Dispatcher::dispatch_requested` — is already
transport-agnostic: it takes a `request_id: String`, a `payload: Value`
(the `EventEnvelope` payload), and a `&Arc<CallConnection>`, and returns
a `ResponseEnvelope`. It is currently `pub(crate)`. **Expose it as
`pub`** so the WS handler can call it directly with a deserialized
`EventEnvelope` payload.

Similarly, the abort-cascade handling (`call.aborted` events) is in
`Dispatcher::handle_stream` — extract the abort-handling logic into a
`pub` method so the WS handler can call it for `call.aborted` events.

### The fix: `CallConnection` from a non-QUIC transport

The `CallConnection` needs to be constructible from a non-QUIC source.
Two options (pick the cleaner one during implementation):

**Option A: A `CallConnection::new_overlay_only(identity)` constructor.**
Construct a `CallConnection` that holds the Layer 2 overlay +
`PendingRequestMap` + the resolved bearer `Identity`, but no QUIC
`Connection`. The `connection()` accessor returns a stub or the
`identity()` is stored directly. This is the minimal change —
`CallConnection` gains a constructor that doesn't require a QUIC
`Connection`, and the `identity()` is read from a stored field rather
than `connection.identity()`.

**Option B: Extract a `CallSession` trait.** Define a trait that
`CallConnection` and a new `WsCallSession` both implement, with
`identity()`, `overlay_env()`, `pending()`, `register_imported()`. The
`Dispatcher` takes `&Arc<dyn CallSession>`. This is more invasive but
cleaner; it's the right choice if the QUIC/WS divergence is large.

**Recommendation: Option A** unless the divergence is larger than it
appears. The `CallConnection` already holds the overlay + pending as
`Arc<RwLock<...>>` / `Arc<Mutex<...>>` (independent of the QUIC
`Connection`); the only QUIC-coupled piece is the `connection: Arc<Connection>`
field and the `connection.identity()` call. A constructor that stores
the `Identity` directly (and returns `None` from `connection()` or
provides a `identity()` accessor that reads the stored field) is the
minimal change.

### The WS dispatch loop (how the WS handler uses this)

The WS upgrade handler (the `websocket/upgrade-handler` task) will:

1. Resolve the bearer identity at upgrade time.
2. Construct a `CallConnection` (via the new constructor — Option A) or
   equivalent (Option B) holding the identity, a fresh Layer 2 overlay,
   and a fresh `PendingRequestMap`.
3. Construct a `Dispatcher` (already `pub`).
4. For each binary WS message: deserialize `EventEnvelope`, match on
   `envelope.r#type`:
   - `call.requested` → call `Dispatcher::dispatch_requested(connection,
     request_id, payload)` (now `pub`), get `ResponseEnvelope`, convert
     to `EventEnvelope`, write back as binary WS message.
   - `call.aborted` → call the extracted `pub` abort-handling method.
   - `call.responded` / `call.completed` → correlate via
     `PendingRequestMap` (the WS handler's outgoing calls —
     bidirectionality, ADR-043 §2).
5. On WS close: fail all pending, drop the overlay (connection-local,
   dies with the WS connection).

### What this task does NOT do

- **No WS upgrade handler.** The upgrade handler is the
  `websocket/upgrade-handler` task. This task exposes the API it calls.
- **No WS framing.** The WS message → `EventEnvelope` deserialization is
  the `websocket/upgrade-handler` task. This task takes deserialized
  envelopes.
- **No `from_wss` adapter.** Out of scope (websocket.md §"Future" —
  scope decision, not a two-way-door deferral).

### Why this is the highest-risk task

This task modifies `alknet-call`'s security-relevant dispatch code. The
`dispatch_requested` method runs `AccessControl::check(identity)` — the
sole authorization gate (ADR-029 §3). Exposing it as `pub` is safe (the
WS handler is in `alknet-http`, a trusted crate), but the change must
not alter the dispatch logic itself. The `CallConnection` change must
not break the existing QUIC path (the `CallAdapter` and `CallClient`
construct `CallConnection` from a QUIC `Connection` — that path must
continue to work unchanged). Run the full `alknet-call` test suite after
the change.

## Acceptance Criteria

- [ ] `Dispatcher::dispatch_requested` is `pub` (was `pub(crate)`)
- [ ] Abort-cascade handling extracted to a `pub` method (was inline in `handle_stream`)
- [ ] `CallConnection` constructible from a non-QUIC source (Option A or B)
- [ ] New `CallConnection` constructor stores `Identity` directly (or equivalent)
- [ ] `CallConnection::identity()` works for the non-QUIC case
- [ ] `CallConnection::overlay_env()`, `pending()`, `register_imported()` work for non-QUIC
- [ ] Existing QUIC path (`CallAdapter`, `CallClient`) unchanged — no regressions
- [ ] `Dispatcher::handle_stream` (QUIC path) still works unchanged
- [ ] `Dispatcher::run_loop` (QUIC path) still works unchanged
- [ ] `cargo test -p alknet-call` — all existing tests pass (no regressions)
- [ ] `cargo clippy -p alknet-call --all-targets` — no warnings
- [ ] Unit test: `dispatch_requested` callable with a non-QUIC `CallConnection`
- [ ] Unit test: abort-handling method callable with a non-QUIC `CallConnection`
- [ ] Unit test: `CallConnection` from non-QUIC source holds overlay + pending + identity
- [ ] Integration test: dispatch a `call.requested` via the `pub` API → `ResponseEnvelope`
- [ ] Integration test: abort cascade via the `pub` API
- [ ] `cargo test -p alknet-http` succeeds (the WS handler can use the API)
- [ ] `cargo clippy -p alknet-http --all-targets` succeeds with no warnings

## References

- docs/architecture/crates/http/websocket.md — Dispatch (§"Dispatch: the shared Dispatcher, unchanged"), Framing (§"Framing")
- docs/architecture/decisions/012-call-protocol-stream-model.md — ADR-012 (stream-agnostic correlation)
- docs/architecture/decisions/048-websocket-native-session-not-gateway.md — ADR-048 (WS carries native session)
- docs/architecture/decisions/044-defer-webtransport-browsers-use-websocket.md — ADR-044 (WS message boundary is delimiter, no length prefix)
- docs/architecture/crates/call/call-protocol.md — Dispatcher, EventEnvelope wire format
- docs/architecture/crates/call/client-and-adapters.md — Shared Dispatcher (§"Shared Dispatcher")
- crates/alknet-call/src/protocol/dispatch.rs — current Dispatcher implementation
- crates/alknet-call/src/protocol/connection.rs — current CallConnection implementation

## Notes

> This is the highest-risk task in the http phase. It modifies
> alknet-call's security-relevant dispatch code to expose an
> EventEnvelope-level API for non-QUIC transports. The spec says "the
> Dispatcher runs unchanged" (ADR-012), but the current implementation is
> QUIC-specific in two places: handle_stream takes raw SendStream/RecvStream
> (length-prefixed framing), and CallConnection wraps a QUIC Connection.
> The fix is to expose dispatch_requested as pub and make CallConnection
> constructible from a non-QUIC source. The existing QUIC path (CallAdapter,
> CallClient) must not regress — run the full alknet-call test suite. The
> WS handler (websocket/upgrade-handler task) is the consumer of this API.
> This task is tracked in tasks/http/ because it unblocks the WS path, but
> it modifies alknet-call — coordinate with the call crate's conventions.

## Summary

> Cross-crate change (alknet-call): exposed Dispatcher::dispatch_requested as pub
> and extracted abort-cascade handling into pub handle_abort method in
> protocol/dispatch.rs. Added CallConnection::new_overlay_only(identity) constructor
> (Option A) + identity() accessor in protocol/connection.rs for non-QUIC transports.
> compose_root_env uses identity() accessor for both QUIC and non-QUIC paths. Existing
> QUIC path (CallAdapter, CallClient, run_loop, handle_stream) unchanged. Zero
> regressions: alknet-call 277+2 tests pass, alknet-http 41 tests pass, clippy clean
> on both crates. 13 unit tests in alknet-call + 6 integration tests in alknet-http.