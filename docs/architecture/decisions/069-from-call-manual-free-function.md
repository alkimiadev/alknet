# ADR-069: from_call Is a Manual Free Function, Not Auto-Wired

## Status

Proposed

## Context

OQ-27 resolved (2026-06-27): "The decision is **auto-re-import on connection
establishment**. The overlay is per-connection (Layer 2, ADR-024), so a stale
overlay dies with the connection; re-import on reconnect is naturally scoped to
the new connection."

The spec in `client-and-adapters.md` §"from_call" (line 358) states: "This is
the v1 default; explicit re-import via a future `CallConnection::refresh()` is
additive."

The implementation does not match. `from_call` is a standalone free function
(`client/from_call.rs:80`). `CallClient::connect()` does not call it. The
assembly layer must call `from_call()` + `register_imported_all()` explicitly
after every `connect()`. There is no `CallConnection::refresh()`.

The "v1 default" language is hedging — it makes a committed-but-not-implemented
feature sound like a deliberate phase. The spec says "auto-re-import on
connection establishment" but the code says "the assembly layer calls
`from_call` immediately after `connect()`" (the doc comment on `from_call`,
line 76). These are different things: auto-wiring means `connect()` calls
`from_call()` internally; manual means the caller does it.

The alkapi project identified this as gap G.4: the hedging language in the
spec, and the question of whether `from_call` should be auto-wired into
`connect()`.

## Decision

**`from_call` is a manual free function. The assembly layer calls it after
`connect()`. It is not auto-wired into `CallClient::connect()`.**

### Why manual is correct

1. **The hub controls discovery timing.** A hub may want to verify the
   connection, resolve the peer's identity, check authorization, and *then*
   discover operations. Auto-wiring `from_call` into `connect()` would run
   discovery before the assembly layer has a chance to inspect the connection.

2. **Discovery is not always wanted.** A pure-client connection to a public
   X.509 endpoint (ADR-034) has no `PeerEntry` and no `PeerId` — the remote
   is not in the peer graph. Auto-discovering ops on such a connection would
   register them in a connection overlay that has no peer key, making them
   unreachable via `PeerRef`. The assembly layer decides whether to run
   `from_call` based on whether the remote is a known peer.

3. **The `from_call` function is already the right API.** It takes a
   `&CallConnection` and a `FromCallConfig`, returns
   `Result<Vec<HandlerRegistration>, AdapterError>`, and the caller registers
   the bundles. This is a clean separation: connect, discover, register. Each
   step is independently testable and independently controllable.

4. **Auto-wiring would require `from_call` to know about the registry.**
   `CallClient` holds an `Arc<OperationRegistry>`, but `from_call` produces
   `HandlerRegistration` bundles that the caller registers — the caller
   decides *where* to register them (the connection's overlay, a session
   overlay, or not at all). Auto-wiring would hardcode the registration
   target.

### What changes in the spec

The "v1 default" language in `client-and-adapters.md` and ADR-017 is replaced
with an honest statement: `from_call` is a free function; the assembly layer
calls it after `connect()`; there is no `CallConnection::refresh()` for
mid-connection re-discovery. A `CallConnection::refresh()` method is a
genuine feature addition — non-breaking, additive — if a deployment needs
manual re-discovery without drop-and-reconnect.

OQ-27 is updated: the resolution changes from "auto-re-import on connection
establishment" to "manual — the assembly layer calls `from_call` after
`connect()`." The door type remains two-way (auto-wiring is additive).

### What does NOT change

- The `from_call` function signature, behavior, and tests are unchanged.
- `CallClient::connect()` is unchanged.
- The re-import-on-reconnect pattern is unchanged: the assembly layer's
  supervision loop calls `from_call` after each `connect()`. The overlay is
  per-connection, so a stale overlay dies with the connection; re-import on
  reconnect is naturally scoped. This is the correct behavior — it just
  isn't automatic.

## Consequences

**Positive:**
- The spec matches the implementation. No hedging language.
- The assembly layer has full control over discovery timing and registration
  target.
- The separation of concerns (connect / discover / register) is clean and
  testable.
- No code changes needed — this is a spec correction, not an implementation
  change.

**Negative:**
- The assembly layer must remember to call `from_call` after `connect()`.
  This is a documentation concern, not a correctness concern — forgetting to
  call `from_call` means the peer's ops are not imported, which is immediately
  visible (calls to those ops return `NOT_FOUND`).
- The "auto-re-import on connection establishment" resolution of OQ-27 was
  aspirational and is now corrected. The resolution was written before the
  implementation existed; the implementation made the right call (manual),
  and the spec is catching up.

## References

- ADR-017 §3: `from_call` adapter specification
- ADR-017 Amendments (DC-2): the amended `from_call` re-import resolution
  (manual free function)
- ADR-067: Aggregated Peer-Environment Wiring (sibling hub-wiring decision)
- ADR-068: PeerCompositeEnv::peer_operations Override (sibling hub-wiring decision)
- OQ-27: from_call re-import trigger (amended 2026-07-09)
- `client-and-adapters.md` §"from_call" (updated)
- `crates/alknet-call/src/client/from_call.rs:80` — `from_call` free function
- `crates/alknet-call/src/client/call_client.rs:142-168` — `connect()` does
  not call `from_call`
- alkapi gap G.4: `from_call` wiring + "v1 default" hedging cleanup
