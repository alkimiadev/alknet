# ADR-072: Channel 0 Is Pre-Negotiated `alknet/call`

## Status

Accepted

## Context

A channels connection carries N logical channels. One of them must carry the
call protocol — the JSON-RPC layer that orchestrates channel lifecycle
(`channel/open`, `channel/close`, `channel/control`, `channel/resources`).
The question is how channel 0 relates to the call protocol: is it a special
"control plane" with its own framing, or is it just `alknet/call` pre-
negotiated?

The phase-0 research (`docs/research/alknet-channels/phase-0-findings.md`
§DP-2) recommends channel 0 is `alknet/call` pre-negotiated — no special
framing, no separate control-plane wire format. The call protocol runs on
channel 0 exactly as it runs on a top-level `alknet/call` QUIC connection.

This matters because the alternative (a special control plane) would mean
the channels layer has its own JSON protocol for channel lifecycle, parallel
to and duplicating the call protocol's `OperationRegistry`, `AccessControl`,
`OperationContext`, and `forwarded_for` machinery. That duplication is the
"re-implement every protocol's framing per transport" problem the hub
motivation (§Hub Motivation) identifies as the thing channels exists to
collapse.

## Decision

**Channel 0 is `alknet/call`, pre-negotiated.** Both sides of a channels
connection know that `channel_id = 0` is routed to the `CallAdapter` without
an explicit `channel/open` exchange. The `CallAdapter` receives a
`Connection` backed by channel-0 chunk reassembly and dispatches operations
exactly as it does on a top-level `alknet/call` connection.

### What this means concretely

1. **Channel 0 uses the same 9-byte chunk format as every other channel**
   (ADR-071). Its chunks have `channel_id = 0` in the header. No special
   first-byte trick, no separate framing.

2. **The `CallAdapter` is unchanged.** It receives a `Connection`, calls
   `accept_bi()`, gets one bidi stream (the channel-0 reassembled stream),
   and runs its dispatch loop. `EventEnvelope` frames ride on `stream_type =
   0` of channel 0. The `CallAdapter` does not know it is inside a channels
   connection.

3. **Channel lifecycle operations are call operations.** `channel/open`,
   `channel/close`, `channel/control`, `channel/resources` are registered on
   the call protocol's `OperationRegistry` at assembly time (ADR-073). They
   are dispatched through the existing `OperationContext` (identity, scopes,
   capabilities, ownership, `forwarded_for`), gated by the existing
   `AccessControl::check`. No new auth machinery, no new framing, no
   protocol version bump.

4. **Channel 0 is allocated at `ChannelsAdapter::handle` entry.** The
   `ChannelsAdapter` constructs channel 0's reassembly buffers, wraps them
   as a `Connection` (via `Connection::from_source` with a
   `ChannelBidiStreamSource` — ADR-070/074), and hands that `Connection` to
   the `CallAdapter` — exactly as if `alknet/call` had been the top-level
   ALPN. The `CallAdapter` is looked up in the same `HandlerRegistry` as
   every other ALPN.

### Channel 0's stream_type usage

| stream_type | direction | purpose |
|-------------|-----------|---------|
| 0 | write (client→server) | `EventEnvelope` frames from the client (call.requested, call.aborted) |
| 1 | read (server→client) | `EventEnvelope` frames from the server (call.responded, call.completed, call.error) |

Channel 0 uses stream_types [0, 1] — the call protocol is bidirectional via
two unidirectional halves, the same way every channel type works
(ADR-071 §stream_type decomposition). The call protocol's `(SendStream,
RecvStream)` pair maps directly: `SendStream` backed by stream_type 0,
`RecvStream` backed by stream_type 1. Both sides write to their write half
and read from their read half — no shared stream_type both sides write to.

The call protocol is JSON-only and single-stream by design (ADR-064).
stream_types 2-255 on channel 0 are reserved for future call-protocol
sub-streams.

## Consequences

**Positive:**
- No control-plane duplication. The channels layer reuses the call protocol's
  `OperationRegistry`, `AccessControl`, `OperationContext`, `forwarded_for`,
  and `StreamingHandler` (ADR-049) machinery verbatim. Channel lifecycle is
  just another class of call operations.
- The `CallAdapter` is transport-agnostic by construction — it works
  identically whether the `Connection` is a top-level QUIC stream or a
  channels-reassembled channel-0 stream. This is the "streams are streams"
  insight made concrete.
- `channel/resources/subscribe` (ADR-073) is a `Subscription` operation on
  channel 0, using the already-implemented `StreamingHandler` /
  `invoke_streaming` path (ADR-049). The resource registry is a live view,
  not a polled snapshot.
- Auth is inherited: `channel/open` goes through `AccessControl::check`
  exactly like any other call operation. The channels layer does not re-
  implement auth.

**Negative:**
- Channel 0 is a single point of orchestration. If channel 0's `CallAdapter`
  hangs, no new channels can be opened. This is the same property as the call
  protocol today (one dispatch loop per connection) and is not a new
  vulnerability.
- The call protocol's JSON-only nature means channel lifecycle operations
  are JSON. For high-frequency control (e.g., per-keystroke resize), this is
  more overhead than a binary control frame. The division (ADR-073 §DP-4)
  handles this: `stream_type 3` on the data channel for data-ordered control,
  call operations for lifecycle and infrequent control.

## Door type

**One-way.** Channel 0's role as `alknet/call` pre-negotiated is a wire-
format and protocol-structure commitment. Changing it after deployments
exist (e.g., to a special control plane) requires a version migration and
re-architecting the channel lifecycle operations. The reservation of
`stream_type` 1-255 on channel 0 is a two-way-door detail (they're currently
unused; assigning them is additive).

## References

- ADR-071: channels wire format (the 9-byte chunk header channel 0 uses)
- ADR-073: channel lifecycle operations (registered on channel 0's
  `OperationRegistry`)
- ADR-064: irpc never integrated — hand-rolled EventEnvelope framing (the
  call protocol channel 0 carries)
- ADR-049: StreamingHandler for subscriptions (the machinery
  `channel/resources/subscribe` uses)
- ADR-070: BidiStreamSource trait (the `Connection` extension point)
- `docs/research/alknet-channels/phase-0-findings.md` §DP-2, §Channel 0