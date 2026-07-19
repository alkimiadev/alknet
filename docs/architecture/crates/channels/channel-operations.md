---
status: draft
last_updated: 2026-07-18
---

# channel-operations.md — Channel Lifecycle on the Call Protocol

Channel lifecycle is orchestrated by the call protocol on channel 0
(ADR-072). Four operations on channel 0's `OperationRegistry` (ADR-073)
handle open, close, control, and resource discovery. All four go through
the existing `OperationContext` / `AccessControl::check` path — no new auth
machinery, no new framing.

## The four operations

### `channel/open` — open a data channel

Request (on channel 0):

```json
{
  "operation": "channel/open",
  "input": {
    "alpn": "alknet/tty",
    "params": { "backend": "docker", "cmd": ["bash"], "container": "abc123" },
    "direction": "initiator-to-responder"
  }
}
```

| field | type | meaning |
|-------|------|---------|
| `alpn` | string | The ALPN the channel will carry. Responder looks this up in its `HandlerRegistry`. |
| `params` | object | ALPN-specific parameters. For `alknet/tty` this is `NegotiateRequest`. For `alknet/tunnel` this is the target resource. The channels layer does not interpret `params`. |
| `direction` | string | `initiator-to-responder` or `responder-to-initiator`. See "Direction semantics" below. |

Response:

```json
{
  "output": {
    "channel_id": 7
  }
}
```

| field | type | meaning |
|-------|------|---------|
| `channel_id` | u32 | Server-assigned (DP-1). The responder allocates via monotonic `AtomicU32`. |

**Channel ID allocation: server-assigned (DP-1).** One round-trip before
data flows — the same round-trip the call protocol makes for every
operation. All current channel types (TTY, tunnel, SSH) already require a
negotiation round-trip, so the open round-trip is not additive latency.

**Error codes** (new `CallError.code` strings, not new framing):

| code | meaning | retryable |
|------|---------|-----------|
| `channel:unknown_alpn` | ALPN not in responder's `HandlerRegistry` | false |
| `channel:forbidden` | `AccessControl::check` denied the open | false |
| `channel:allocation_failed` | Handler allocate failed | true (often transient) |
| `channel:invalid_params` | `params` JSON didn't satisfy the ALPN's expectations | false |
| `channel:too_many_channels` | Per-connection channel limit hit (ADR-076) | false |

### `channel/close` — tear down a channel

```json
{
  "operation": "channel/close",
  "input": { "channel_id": 7, "reason": "exit" }
}
```

The responder (the side that didn't send the close) drains its reassembled
stream for `channel_id`, signals EOF to the handler, and returns
`{ "closed": true }`. The `channel_id` is eligible for reuse after the drain
completes (ADR-076 — monotonic IDs with wrap-around, not a free-list).
`reason` is free-form for observability — not semantically required.

**REQ-CH-06: exit-chunk-before-close ordering.** The channel's data chunks
MUST be written and flushed before the `channel/close` operation is sent on
channel 0. The side closing must observe the data-channel pump complete
before issuing the call operation. For TTY this is the exit-chunk-is-last
invariant (ADR-055) carried forward — the exit control message rides on
TTY's `STREAM_CTRL_OUT` (stream_type 4, inside TTY's 5-byte payload
format); for tunnels it is the last data byte before close. This invariant
crosses two channels (the data channel and channel 0), so the channels
layer owns the ordering guarantee.

### `channel/control` — out-of-band control on channel 0

For control that doesn't need ordering relative to data (resize, signal,
keepalive):

```json
{
  "operation": "channel/control",
  "input": {
    "channel_id": 7,
    "message": { "type": "resize", "cols": 80, "rows": 24 }
  }
}
```

The channels layer routes `message` to the handler's control handle for
`channel_id`. The `message` JSON is ALPN-specific; the channels layer does
not interpret it.

### `channel/resources/subscribe` — live resource discovery

**This is a `Subscription` operation (ADR-049), not a polled Query.** The
call protocol has `StreamingHandler` / `invoke_streaming` (implemented and
tested). The first consumer (the hub aggregating worker resources) needs
live updates when workers connect/disconnect or containers start/stop.

```json
{
  "operation": "channel/resources/subscribe",
  "input": {}
}
```

The responder registers a `StreamingHandler` that emits a `ResponseEnvelope`
whenever the resource set changes. Each event:

```json
{
  "output": {
    "resources": [
      {
        "alpn": "alknet/tty",
        "backends": ["docker", "local"],
        "access": { "required_scopes": ["tty:open"] }
      },
      {
        "alpn": "alknet/tunnel",
        "targets": ["container:*", "service:postgres"],
        "access": { "required_scopes_any": ["tunnel:open", "admin"] }
      }
    ]
  }
}
```

| field | type | meaning |
|-------|------|---------|
| `alpn` | string | The ALPN this side accepts `channel/open` for. |
| `backends` / `targets` | `[string]` | ALPN-specific enumeration of what's available. The channels layer doesn't interpret these. |
| `access` | object | A preview of the `AccessControl` that `channel/open` will check. Advisory — lets the initiator fail fast. The real check happens on `channel/open`. |

The stream emits an initial snapshot immediately, then subsequent events on
any change. The stream is long-lived; the subscriber cancels by dropping the
subscription (ADR-016 abort cascade applies).

A `channel/resources` (non-subscribe, `Query`) operation is NOT provided.
The subscription's initial snapshot serves the poll use case (subscribe,
read the first event, cancel). Providing both would be redundant and would
pressure consumers toward the stale-poll path.

## Direction semantics (OQ-CH-09 — pinned)

Channel open is **bidirectional** — either side can initiate. The
`direction` field determines who is the ALPN-server (allocates the handler,
writes the negotiation response) vs the ALPN-client (writes the first
request).

| `direction` | Initiator role | Responder role | Who writes first |
|-------------|----------------|----------------|-------------------|
| `initiator-to-responder` | ALPN-client | ALPN-server | Initiator writes first (the request data); responder's handler is the server side. The common case: "open me a TTY on your docker container." |
| `responder-to-initiator` | ALPN-server | ALPN-client | Responder writes first (the negotiation response); initiator's handler is the client side. The "worker exposes, hub consumes" case: the worker initiates the open to make itself available; the hub is the client. |

**The channels layer does not enforce write order.** Write order is
ALPN-specific, determined by which side is the ALPN-server. The channels
layer routes chunks; the handlers negotiate who writes first via their
ALPN's `params` contract.

**`channel_id` allocation is always by the responder** (DP-1), regardless of
`direction`. The responder is the side that receives the `channel/open` call
operation; it allocates the ID and returns it. In the `responder-to-
initiator` case, the initiator (worker) sends the `channel/open`, so the
responder (hub) allocates the ID — even though the worker is the ALPN-server
for the channel's data. This keeps ID allocation in one place and avoids the
collision-prone client-assigned alternative.

## Control-message division (DP-4 — pinned)

| Control path | When | Examples |
|--------------|------|----------|
| Call operations on channel 0 (`channel/control`, `channel/close`) | Control that doesn't need ordering relative to data, or lifecycle events | resize, signal, keepalive, close |
| Data-ordered bytes on the data channel's `BiStream` (handler-internal framing) | Control that MUST be ordered relative to data | EOF before exit, flush before close |

The TTY crate's exit-chunk-is-last invariant (ADR-055) is the canonical
example of data-ordered control — it rides on TTY's `STREAM_CTRL_OUT`
(stream_type 4, inside TTY's 5-byte payload format) because it must arrive
after the last data on TTY's stdout stream_type, guaranteed by TTY's
per-stream_type chunk ordering within its own 5-byte format, not by a
call-protocol round-trip. The `channel/close` operation that follows is
on channel 0 and is ordered after the data pump completes (REQ-CH-06).

**The control-message division is handler-internal.** Under ADR-093, the
channels layer has no `stream_type` concept — it carries the handler's
framing transparently in the payload. TTY's `STREAM_CTRL_IN` (stream_type
3) and `STREAM_CTRL_OUT` (stream_type 4) are stream_types in TTY's 5-byte
format (ADR-052, amended by Phase 7), not channels-layer concepts. The
channels layer routes by `channel_id` only; the handler owns its
sub-stream multiplexing on the `BiStream` it receives. The
"bidirectional control channel" property is a TTY-layer concern, fixed
at the TTY layer by Phase 7's split — the channels layer doesn't know
about it.

## ACL flow (end-to-end)

A browser opening a TTY channel to a spoke through a hub (ADR-079):

1. Browser's channel 0 → hub's channel 0: `channel/open`
   `{ alpn: "alknet/tty", params: { backend: "docker", cmd: ["bash"], container: "abc123" } }`.
   The browser's identity is a bearer token (ADR-034).
2. Hub's `CallAdapter` runs `AccessControl::check` on `channel/open` with
   the browser's identity. If denied → `channel:forbidden`.
3. Hub forwards to spoke via `from_call`: the hub's `forwarded_for` handler
   constructs a `call.requested` with the hub as caller and the browser as
   `forwarded_for` (ADR-032 §3). The spoke receives `channel/open` with
   `caller = hub`, `forwarded_for = browser`.
4. Spoke's `CallAdapter` runs `AccessControl::check` with the hub as caller
   (the spoke authorizes the hub — ADR-050). The spoke's ownership store
   verifies the hub (or the `forwarded_for` browser, per policy) owns
   `container:abc123`.
5. Spoke allocates the channel via `TtyAdapter` / `DockerTtyBackend`,
   returns `channel_id`.
6. Hub opens a matching channel on the browser's side and bridges them
   (byte-forward with `channel_id` rewrite — ADR-079).

The hub ran **zero** protocol-specific auth. It ran `channel/open`'s
`AccessControl::check` (call-protocol machinery) and forwarded. The channels
layer inherited the auth model by being a call-protocol operation.

## Per-identity channel cap (ADR-094)

A channel slot is a resource. The cap on how many channels an identity
may hold open is a quota check on that resource — parallel to
`OwnershipProvider::owns` (ADR-050) for spawned resources. Same
primitive, different resource. The cap is a **peer concern**, not a
hub-specific concern: any accepting peer (worker or hub) enforces the
cap on its inbound channels, just as it enforces `AccessControl::check`
on `channel/open`. The cap is also **symmetric** — both sides of a
channels connection enforce their cap on the other's channels.

### Why the cap is not in the channels layer

`ChannelManager` (ADR-075) is auth-blind by design — no auth state, no
identity, no scopes. That decision is load-bearing (it is what makes
the channels layer WASM-compatible, transport-agnostic, and
ALPN-blind). So the per-identity cap lives in `channels-call`, where
the identity is already on `OperationContext` (the same place
`AccessControl::check` runs). The channels layer (`channels-core`) is
unchanged. See ADR-094 §"Why the channels layer cannot hold the cap".

The channels-layer per-connection `max_channels = 256` (ADR-076) is
a **per-connection memory bound** (limits one connection's
reassembly-buffer cost), not a DoS defense. A peer can open an
unbounded number of transport connections, so a per-connection cap is
not a per-peer DoS defense. The per-identity DoS defense is the cap
documented here; see ADR-094 for the corrected DoS-defense framing.

### The `ChannelLifecyclePolicy` trait

```rust
/// Per-identity channel lifecycle policy. Consulted by the
/// `channel/open` handler (after `AccessControl::check`, before
/// allocation) and the `channel/close` handler (after deallocation).
/// Both handlers have the identity via `OperationContext`.
pub trait ChannelLifecyclePolicy: Send + Sync + 'static {
    /// Before channel allocation. Deny with `channel:too_many_channels`
    /// (ADR-073) when the identity is over its cap. The identity is
    /// the direct caller (the peer that opened this channels
    /// connection); `forwarded_for` is metadata and is NOT consulted
    /// (ADR-032).
    fn check_open(&self, identity: &Identity) -> Result<(), ChannelError>;

    /// After channel deallocation. Decrement the per-identity count.
    /// Called by the `channel/close` handler after the drain completes
    /// (ADR-076 §channel-id-reuse).
    fn on_close(&self, identity: &Identity);
}
```

### Default: `PerIdentityChannelPolicy::new(256)`

The default constructor enforces 256 per identity out of the box — no
"NoOp default + wire it later." A channels-accepting peer that
constructs `ChannelOperations::new(manager)` with no policy argument
gets `PerIdentityChannelPolicy::new(256)`. The default is secure;
opt-outs are explicit:

- `PerIdentityChannelPolicy::new(cap)` — shared per-identity state
  (`HashMap<PeerId, usize>` + cap), constructed **once per accepting
  peer** and shared (via `Arc`) across every channels connection that
  peer accepts. The sharing is what makes the cap per-identity, not
  per-connection.
- `PerIdentityChannelPolicy::with_per_identity_caps(mapping)` —
  per-peer-role variant: `HashMap<PeerId, usize>` overrides the
  default cap for specific peers. Used by a spoke that serves a
  high-fan-out hub (the hub peer's cap is set higher than a worker
  peer's cap — see "Relay consequence" below).
- `NoCap` — no cap. Explicit opt-out for tests, POCs, and trusted
  single-peer deployments. Not the default.

The policy is constructed once and passed to `ChannelOperations` at
registration time:

```rust
let policy = Arc::new(PerIdentityChannelPolicy::new(256));
let channel_ops = ChannelOperations::new(manager, policy);
channel_ops.register_on(&mut call_registry)?;
```

### Enforcement point: between `AccessControl::check` and allocation

The `channel/open` handler (above) gains the policy check after ACL
and before `next_id.fetch_add`:

1. ACL is already checked by `OperationRegistry::invoke` (the existing
   `AccessControl::check` path — unchanged).
2. **NEW:** `policy.check_open(&op_ctx.identity)?` — deny with
   `channel:too_many_channels` if over cap.
3. Allocate the `channel_id` via `next_id.fetch_add(1, Relaxed)`
   (DP-1: server-assigned — unchanged).
4. Construct the `ChannelBidiStreamSource`, spawn the handler, record
   the `ChannelState` (unchanged).
5. Return the `channel_id`.

The `channel/close` handler gains the decrement after the drain
completes (the same point ADR-076 marks the `channel_id` as eligible
for reuse):

1. Drain the reassembly buffer for `channel_id` (existing — ADR-076
   §channel-id-reuse).
2. **NEW:** `policy.on_close(&op_ctx.identity)` — decrement the
   per-identity count.
3. Return `{ "closed": true }` (unchanged).

### Relay consequence: the spoke caps the hub, not the browser

When the hub relays a browser's channel to a spoke (ADR-079), the
spoke sees the hub as the direct caller. `forwarded_for` carries the
browser's identity as metadata (ADR-032 — `forwarded_for` is not
authority; `AccessControl::check` never reads it). The channel cap
follows the same shape: the spoke's `ChannelLifecyclePolicy` is
consulted with the **hub's** identity, not the browser's. The spoke
asks "does the hub have access to open another channel?" and the
hub's quota on the spoke reflects the aggregate of all relayed
channels. The hub's per-browser caps are the hub's own concern
(enforced on the browser leg by the hub's own policy), not the
spoke's.

This is correct and consistent — the spoke authorizes the hub for
container access the same way it authorizes any peer, and the hub's
browser-relay ACL is the hub's own layer. The channel cap follows the
same pattern as any other resource ACL.

**Deployment consequence:** a spoke that serves a hub relaying for
many browsers must set the hub peer's cap higher than a worker peer's
cap, or the spoke denies legitimate relayed channels when the hub's
aggregate count exceeds a worker-sized cap. This is a per-peer-role
policy, set by the spoke via `with_per_identity_caps`. The
architecture provides the mechanism; the deployment sets the numbers.
This is not a flaw — it is the same shape as any per-peer ACL (a
spoke may authorize one peer for 1000 containers and another for 10;
the channel cap is the same kind of per-peer policy).

### Recursive channels do not bypass the cap

A recursive `alknet/channels`-inside-`alknet/channels` channel runs a
new `ChannelsAdapter` with a new `ChannelManager`. If the same
`ChannelLifecyclePolicy` is wired into the inner `ChannelOperations`,
the inner channels are counted against the same identity. Recursion
is not a bypass; the 13-byte-per-chunk overhead is the documented
cost (ADR-093), and the cap behavior is unchanged. Recursive channels
are an edge case for edge cases and not specced further.

## Hub relay contract (ADR-079 — summary)

The hub **translates**, not transparently forwards:

1. **Call-protocol layer (channel 0): translate.** The hub terminates
   channel 0 on both legs. `channel/open` from the browser → hub's
   `AccessControl::check` → hub re-issues `channel/open` on the spoke leg
   with `forwarded_for` → spoke returns its `channel_id` → hub maps
   browser-id ↔ spoke-id.
2. **Data-channel layer: byte-forward with `channel_id` rewrite.** The
   relay reads chunks for `browser_id`, rewrites the `channel_id` field to
   `spoke_id`, writes onto the spoke's channels connection — and vice versa.
   The relay does not parse the payload.

`channel/control` operations on channel 0 carry `channel_id` in their JSON
payload; the hub's `CallAdapter` translates these too (rewrites
`channel_id` in the payload). The relay does not touch `channel/control` —
it's a call operation, translated, not byte-forwarded.

The hub never runs a handler for `alknet/tty`, `alknet/ssh`, or
`alknet/tunnel`. It runs `alknet/channels` (the relay) and `alknet/call`
(for its own hub-level operations + translation).

## Design Decisions

All design decisions are documented as ADRs in [decisions/](../../decisions/).

| ADR | Decision | Summary |
|-----|----------|---------|
| [073](../../decisions/073-channel-lifecycle-operations.md) | Channel Lifecycle Operations | The four ops; `direction` pinned; subscribe not poll |
| [072](../../decisions/072-channel-0-pre-negotiated-call.md) | Channel 0 Pre-Negotiated | Channel 0 = `alknet/call` |
| [079](../../decisions/079-hub-relay-translate-not-forward.md) | Hub Relay | Translate channel 0, byte-forward data channels |
| [094](../../decisions/094-per-identity-channel-cap.md) | Per-Identity Channel Cap | 256 per `PeerId`, enforced via `ChannelLifecyclePolicy` in `channels-call`; per-connection `max_channels` reframed as a memory bound |
| [093](../../decisions/093-channels-pure-channel-multiplexing.md) | channels Pure Channel Multiplexing | No `stream_types` on `channel/open`; no `stream_type` on `channel/control`; handler owns sub-stream multiplexing |
| [049](../../decisions/049-streaming-handler-for-subscriptions.md) | StreamingHandler | The machinery `channel/resources/subscribe` uses |
| [032](../../decisions/032-forwarded-for-identity.md) | Forwarded-For Identity | The auth chain for hub-relayed opens (and why the cap is per direct-caller, not per `forwarded_for`) |
| [050](../../decisions/050-dynamic-resource-ownership-for-runtime-spawned-resources.md) | Dynamic Resource Ownership | The parallel — a channel slot is a resource, the cap is a quota check |

## References

- ADR-073: channel lifecycle operations (the decision)
- ADR-094: per-identity channel cap (the cap, the trait, the relay
  consequence)
- ADR-079: hub relay (the translate contract)
- `docs/research/alknet-channels/phase-0-findings.md` §Channel Open
  Negotiation, §ACL and Security Model