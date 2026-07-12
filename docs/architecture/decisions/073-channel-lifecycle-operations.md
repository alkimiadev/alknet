# ADR-073: Channel Lifecycle Operations on the Call Protocol

## Status

Accepted

## Context

Channel lifecycle — open, close, control, resource discovery — must be
orchestrated somehow. The phase-0 research (`docs/research/alknet-channels/
phase-0-findings.md` §Channel Open Negotiation, §DP-4) established that
channel lifecycle is orchestrated by the call protocol on channel 0
(ADR-072). This ADR pins the exact operation shapes, the `direction` field
semantics, the control-message division, and the resource-discovery model.

Three things from the research needed real decisions, not hedges:

1. **Resource discovery: poll vs subscribe (OQ-CH-08).** The research
   recommended "poll for v1, add subscription if staleness bites." This is a
   hedge: the call protocol already has `StreamingHandler` /
   `invoke_streaming` (ADR-049, implemented and tested), and the first
   consumer (the hub aggregating worker resources) needs live updates. Polling
   would be built, immediately found insufficient, and reworked. This ADR
   commits to subscribe from day one.

2. **The `direction` field and who writes first (OQ-CH-09).** The research
   said "ALPN-specific and probably doesn't need a channels-layer rule…
   needs to be pinned down." That IS the rule: the channels layer declares
   write-order is ALPN-specific (determined by who is the ALPN-server), not
   channels-enforced. This ADR pins which side is the ALPN-server for each
   `direction` value.

3. **Control messages: call ops vs stream_type 3 (DP-4).** The research
   recommended "both, with clear division." This ADR pins the division.

## Decision

### Four operations on channel 0's `OperationRegistry`

Registered at assembly time by the channels crate (via `ChannelOperations::
register_on(&mut call_registry)`). All four go through the existing
`OperationContext` / `AccessControl::check` path — no new auth machinery.

#### `channel/open` — open a data channel

Request (`call.requested` on channel 0):

```json
{
  "operation": "channel/open",
  "input": {
    "alpn": "alknet/tty",
    "stream_types": [0, 1, 2, 3],
    "params": { "backend": "docker", "cmd": ["bash"], "container": "abc123" },
    "direction": "initiator-to-responder"
  }
}
```

| field | type | meaning |
|-------|------|---------|
| `alpn` | string | The ALPN the channel will carry. The responder looks this up in its `HandlerRegistry`. |
| `stream_types` | `[u8]` | Which sub-stream types this channel will use. Declared at open time so both sides size reassembly. E.g. `[0,1,2,3]` for TTY, `[0,1]` for a tunnel. |
| `params` | object | ALPN-specific parameters. For `alknet/tty` this is the `NegotiateRequest`. For `alknet/tunnel` this is the target resource. The channels layer does not interpret `params` — it hands the JSON to the handler. |
| `direction` | string | `initiator-to-responder` or `responder-to-initiator`. See "Direction semantics" below. |

Response (`call.responded`):

```json
{
  "output": {
    "channel_id": 7,
    "stream_types": [0, 1, 2, 3]
  }
}
```

| field | type | meaning |
|-------|------|---------|
| `channel_id` | u32 | The server-assigned channel ID (DP-1: server-assigned). Both sides route chunks with this ID to the new channel. |
| `stream_types` | `[u8]` | The *negotiated* set — the responder may narrow the initiator's requested set (e.g., refuse stderr). The intersection of requested and supported. |

**Channel ID allocation (DP-1): server-assigned.** The responder allocates
the `channel_id` via a monotonic `AtomicU32` (`next_id.fetch_add(1, Relaxed)`)
and returns it in the response. One round-trip before data flows — the same
round-trip the call protocol makes for every operation. All current channel
types (TTY, tunnel, SSH) already require a negotiation round-trip, so the
open round-trip is not additive latency.

**Error codes** (new `CallError.code` strings, not new framing):

| code | meaning | retryable |
|------|---------|-----------|
| `channel:unknown_alpn` | ALPN not in responder's `HandlerRegistry` | false |
| `channel:forbidden` | `AccessControl::check` denied the open | false |
| `channel:allocation_failed` | Handler allocate failed (e.g., backend couldn't start) | true (often transient) |
| `channel:invalid_params` | `params` JSON didn't satisfy the ALPN's expectations | false |
| `channel:too_many_channels` | Per-connection channel limit hit (ADR-076) | false |
| `channel:stream_type_unavailable` | Responder can't provide a requested `stream_type` | false |

#### `channel/close` — tear down a channel

```json
{
  "operation": "channel/close",
  "input": { "channel_id": 7, "reason": "exit" }
}
```

The responder (the side that didn't send the close) drains its reassembled
streams for `channel_id`, signals EOF to the handler, and returns
`{ "closed": true }`. The `channel_id` is now eligible for reuse after the
drain completes (ADR-076 §channel-id-reuse). `reason` is free-form for
observability — not semantically required.

**Exit-chunk-before-close ordering (generalizes ADR-055):** the channel's
data chunks must be written and flushed before the `channel/close` operation
is sent on channel 0. This is a wire-level invariant: the side closing must
observe the data-channel pump complete before issuing the call operation.
For TTY this is the exit-chunk-is-last invariant (ADR-055) carried forward;
for tunnels it is the last data byte before close. The channels layer's
close handler observes the pump completion; the call operation is issued
after. This is REQ-CH-06.

#### `channel/control` — out-of-band control on channel 0

For control that doesn't need ordering relative to data (resize, signal,
keepalive):

```json
{
  "operation": "channel/control",
  "input": {
    "channel_id": 7,
    "stream_type": 3,
    "message": { "type": "resize", "cols": 80, "rows": 24 }
  }
}
```

The channels layer routes `message` to the handler's control handle for
`channel_id`. The `message` JSON is ALPN-specific; the channels layer does
not interpret it.

#### `channel/resources/subscribe` — live resource discovery

**This is a `Subscription` operation (ADR-049), not a polled Query.** The
research's "poll for v1, add subscription if staleness bites" is a hedge that
would cause rework — the `StreamingHandler` / `invoke_streaming` machinery
exists and is tested, and the hub consumer needs live updates when workers
connect/disconnect or containers start/stop.

```json
{
  "operation": "channel/resources/subscribe",
  "input": {}
}
```

The responder registers a `StreamingHandler` that emits a
`ResponseEnvelope` whenever the resource set changes. Each event:

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
| `backends` / `targets` | `[string]` | ALPN-specific enumeration of what's available. The channels layer doesn't interpret these; they're for the initiator to know what `params` to send. |
| `access` | object | A preview of the `AccessControl` that `channel/open` will check. Advisory — lets the initiator fail fast. The real check happens on `channel/open`. |

The stream emits an initial snapshot immediately, then subsequent events on
any change (worker connects/disconnects, container starts/stops, resource
exposed/withdrawn). The stream is long-lived; the subscriber cancels by
dropping the subscription (ADR-016 abort cascade applies). This is the
resource-discovery analogue of `services/list`, but live — matching the
bidirectional symmetry of the operation overlay.

A `channel/resources` (non-subscribe, Query) operation is NOT provided. The
subscription's initial snapshot serves the poll use case (subscribe, read
the first event, cancel). Providing both would be redundant and would
pressure consumers toward the stale-poll path.

### Direction semantics (OQ-CH-09 — pinned)

Channel open is **bidirectional** — either side can initiate. The
`direction` field determines who is the ALPN-server (allocates the handler,
writes the negotiation response) vs the ALPN-client (writes the first
request).

| `direction` | Initiator role | Responder role | Who writes first |
|-------------|----------------|----------------|-------------------|
| `initiator-to-responder` | ALPN-client | ALPN-server | Initiator writes first (the request data); responder's handler is the server side of the ALPN. The common case: "open me a TTY on your docker container." |
| `responder-to-initiator` | ALPN-server | ALPN-client | Responder writes first (the negotiation response / server greeting); initiator's handler is the client side. The "worker exposes, hub consumes" case: the worker initiates the open to make itself available; the hub is the client that connects to the exposed resource. |

**The channels layer does not enforce write order.** Write order is
ALPN-specific, determined by which side is the ALPN-server (per the table
above). The channels layer's job is to route chunks; the handlers negotiate
who writes first via their ALPN's `params` contract. This is the rule the
research asked for: "the channels layer declares write-order is ALPN-
specific, not channels-enforced."

**`channel_id` allocation is always by the responder** (DP-1), regardless of
`direction`. The responder is the side that receives the `channel/open` call
operation; it allocates the ID and returns it. In the `responder-to-
initiator` case, the initiator (worker) sends the `channel/open`, so the
responder (hub) allocates the ID — even though the worker is the ALPN-
server for the channel's data. This keeps ID allocation in one place (the
`channel/open` responder) and avoids the collision-prone client-assigned
alternative.

### Control-message division (DP-4 — pinned)

| Control path | When | Examples |
|--------------|------|----------|
| Call operations on channel 0 (`channel/control`, `channel/close`) | Control that doesn't need ordering relative to data, or lifecycle events | resize, signal, keepalive, close |
| `stream_type 3` chunks on the data channel | Control that MUST be ordered relative to data | EOF before exit, flush before close |

The TTY crate's exit-chunk-is-last invariant (ADR-055) is the canonical
example of data-ordered control — it rides on `stream_type 3` because it
must arrive after the last stdin chunk, guaranteed by chunk ordering within
`(channel_id, stream_type)`, not by a call-protocol round-trip. The
`channel/close` operation that follows is on channel 0 and is ordered after
the data pump completes (REQ-CH-06).

## Consequences

**Positive:**
- Channel lifecycle reuses the call protocol's `OperationRegistry`,
  `AccessControl`, `OperationContext`, `forwarded_for`, and
  `StreamingHandler` verbatim. Zero new auth, zero new framing.
- `channel/resources/subscribe` gives the hub a live view of worker
  resources — no polling, no staleness, no rework when the first consumer
  needs subscriptions.
- The `direction` field makes bidirectional open explicit and pins who is
  the ALPN-server, resolving the "who writes first" ambiguity without
  channels-layer write-order enforcement.
- The control-message division (call ops vs stream_type 3) handles both
  lifecycle control (infrequent, benefits from auth/observability) and
  data-ordered control (frequent, needs ordering) without duplicating
  machinery.

**Negative:**
- Four new operation names in the `OperationRegistry`. The registry already
  handles namespaced operations (`docker/container/list`, etc.); these are
  in the `channel/` namespace. No registry changes needed.
- `channel/resources/subscribe` is a long-lived `Subscription` stream per
  interested peer. This is the same cost as any other subscription (ADR-049);
  the hub holds one per connected peer. Acceptable.
- The `direction` field adds one field to the `channel/open` input. It is
  required (no default) — the initiator must state its intent. This is a
  one-way-door wire-format field (removing it would break the bidirectional
  open contract).

## Door type

**One-way.** The four operation names (`channel/open`, `channel/close`,
`channel/control`, `channel/resources/subscribe`), their input/output
schemas, and the `direction` field's semantics are wire-format commitments.
Changing them after deployments exist requires a protocol version migration.
The `reason` field on `channel/close` (free-form, observability-only) is a
two-way-door detail.

The decision to use `Subscription` for resource discovery (not `Query`) is
one-way: consumers will depend on the live stream, and the Decision section
committed to Subscribe-only (a `Query` variant is NOT provided — the
subscription's initial snapshot serves the poll use case). The
`Handler` / `StreamingHandler` / `HandlerKind` API surface (ADR-049) is
the underlying one-way commitment.

## References

- ADR-071: channels wire format
- ADR-072: channel 0 is pre-negotiated `alknet/call`
- ADR-049: StreamingHandler for subscriptions (the machinery
  `channel/resources/subscribe` uses — implemented and tested)
- ADR-016: abort cascade (subscription cancellation)
- ADR-032: forwarded-for identity (the auth chain for hub-relayed opens)
- ADR-055: exit-chunk-is-last (the TTY invariant generalized by REQ-CH-06)
- `docs/research/alknet-channels/phase-0-findings.md` §Channel Open
  Negotiation, §DP-4, §OQ-CH-08, §OQ-CH-09