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
| [093](../../decisions/093-channels-pure-channel-multiplexing.md) | channels Pure Channel Multiplexing | No `stream_types` on `channel/open`; no `stream_type` on `channel/control`; handler owns sub-stream multiplexing |
| [049](../../decisions/049-streaming-handler-for-subscriptions.md) | StreamingHandler | The machinery `channel/resources/subscribe` uses |
| [032](../../decisions/032-forwarded-for-identity.md) | Forwarded-For Identity | The auth chain for hub-relayed opens |
| [050](../../decisions/050-dynamic-resource-ownership-for-runtime-spawned-resources.md) | Dynamic Resource Ownership | The ownership store the spoke queries |

## References

- ADR-073: channel lifecycle operations (the decision)
- ADR-079: hub relay (the translate contract)
- `docs/research/alknet-channels/phase-0-findings.md` §Channel Open
  Negotiation, §ACL and Security Model