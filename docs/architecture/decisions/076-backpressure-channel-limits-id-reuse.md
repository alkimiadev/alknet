# ADR-076: Backpressure, Channel Limits, and ID Reuse

## Status

Accepted

## Context

The phase-0 research (`docs/research/alknet-channels/phase-0-findings.md`
§DP-5, §OQ-CH-03/04/05/06) raised four operational questions about the
channels layer:

1. **Flow control (DP-5, OQ-CH-03):** if one data channel's consumer is
   slow, could it block all other channels on the same transport
   (head-of-line blocking)? The research recommended "bounded-buffer
   backpressure (option c)… if head-of-line blocking becomes a real problem,
   full windowing can be added." The "if it becomes a problem" is a hedge —
   the POC validated bounded-buffer with a 1 MiB test and no deadlock. The
   decision is bounded-buffer.
2. **Channel ID reuse (OQ-CH-04):** after a channel is closed, can its ID be
   reused?
3. **Maximum channels per connection (OQ-CH-05):** is there a limit?
4. **Channel open DoS (OQ-CH-06):** an authenticated peer could open many
   channels and never read from them, exhausting memory.

The de-risk POC (`docs/research/alknet-channels/poc-summary.md` §POC Target
1, §POC Target 3) validated the bounded-buffer backpressure path: the 1 MiB
`tunnel_large_payload` test exercises a channel writer faster than the TCP
echo server consumer, with no deadlock and no cross-channel blocking.

## Decision

### Backpressure: bounded-buffer, 1 MiB default (DP-5)

Each `(channel_id, stream_type)` pair has an independent bounded `mpsc`
buffer. When a channel's buffer is full, the demux stops reading chunks for
that `channel_id` until the consumer drains it. Other channels keep flowing
— the demux's per-chunk route awaits the matching sender without holding a
global lock.

**Default buffer cap: 1 MiB per `(channel_id, stream_type)`.** Configurable
per `ChannelManager` (`buffer_cap` field). This prevents memory exhaustion
without the complexity of SSH's sliding-window protocol.

Full channel-level windowing (SSH-style sliding-window per channel) is a
deferred extension, tracked as [OQ-56](../questions/056-full-channel-level-flow-control-windowing.md)
(deferred(scope)). It is blocked on a real deployment observing head-of-
line blocking where the bounded-buffer mitigation is insufficient. The
bounded-buffer decision is made; the extension is not.

### Channel ID reuse: yes, after drain (OQ-CH-04)

After a channel is closed (`channel/close` acknowledged), its `channel_id`
is eligible for reuse. The reassembly buffers must be fully drained before
reuse to prevent data from the old channel leaking into the new one.

**Drain-before-reuse invariant:** the `ChannelManager` marks a closed
channel's ID as "draining" (not in the `channels` map, but not yet returned
to the free pool). The ID returns to the free pool only after:
1. The `channel/close` response is sent (the close is acknowledged).
2. All reassembly buffers for that `channel_id` are empty (the handler has
   consumed all data).

The `next_id: AtomicU32` is monotonic (not a free-list) — IDs are not
immediately reused; the monotonic counter wraps at `u32::MAX`. This is
simpler than a free-list and avoids the drain-tracking complexity. With a
default `max_channels` of 256, the `u32` space is effectively unlimited
(~16.7 million channels before wrap). Reuse happens naturally on wrap, by
which time old channels are long drained. **The "reuse" in OQ-CH-04 is
satisfied by the wrap-around, not by a free-list.**

### Maximum channels per connection: 256 default (OQ-CH-05/06)

The `channel_id` is `u32` — the wire format supports ~4 billion channels.
The practical limit is memory (reassembly buffers per channel) and the
transport's flow control.

**Default per-connection channel limit: 256** (`max_channels` field on
`ChannelManager`, configurable). This is the DoS defense (OQ-CH-06): an
authenticated peer that opens many channels and never reads from them is
bounded by `max_channels × buffer_cap` = 256 × 1 MiB = 256 MiB worst case.
Bounded buffers (DP-5) limit the damage per channel; the connection cap
limits the number of channels. Defense in depth.

Exceeding the limit returns `channel:too_many_channels` (ADR-073 error
codes). The limit is per-connection, not per-peer — a peer can open more
channels on a second connection.

### DoS defense summary (OQ-CH-06)

| Layer | Mechanism | Default |
|-------|-----------|---------|
| Per-channel | Bounded reassembly buffer (stop reading when full) | 1 MiB per `(channel_id, stream_type)` |
| Per-connection | Channel count cap | 256 channels |
| Per-peer | Auth (`AccessControl::check` on `channel/open`) | Assembly-layer policy |

An authenticated peer that opens 256 channels and never reads from them
consumes at most 256 MiB of reassembly buffers — bounded, not unbounded.
The assembly layer's `AccessControl` policy can further restrict
`channel/open` (e.g., `required_scopes: ["channel:open:alknet/tty"]`) to
limit who can open channels at all.

## Consequences

**Positive:**
- Bounded-buffer backpressure is validated by the POC (1 MiB test, no
  deadlock, no cross-channel blocking). The decision is made, not hedged.
- The 256-channel default cap with 1 MiB buffers gives a bounded 256 MiB
  worst-case memory per connection — a clear DoS ceiling, not an open-ended
  one.
- Monotonic `next_id` with wrap-around avoids free-list drain-tracking
  complexity while still satisfying ID reuse (on wrap, after ~16.7M
  channels).

**Negative:**
- The 256-channel default may be too low for a hub with many concurrent
  browser sessions each opening multiple channels. The cap is configurable
  per `ChannelManager`; the hub assembly layer may set it higher for
  deployments with many concurrent sessions. This is a deployment-time
  decision, not an architecture decision.
- Bounded-buffer backpressure does not eliminate head-of-line blocking — it
  bounds the memory cost. A slow consumer still stalls its own channel's
  demux reads. For the intended use cases (TTY, SSH, tunnels) this is
  acceptable; full windowing is tracked as OQ-56 (deferred(scope)).

## Door type

**Two-way.** The buffer cap (1 MiB), the channel limit (256), and the
monotonic-ID-with-wrap strategy are all configurable / changeable without a
wire-format change. The bounded-buffer *approach* (vs full windowing) is
one-way in the sense that the demux/mux code is written around it — but
full windowing is an additive extension (per-channel window tracking) that
doesn't change the wire format, so even that reversal is feasible.

## References

- ADR-071: channels wire format (the chunks the buffers hold)
- ADR-073: channel lifecycle operations (`channel:too_many_channels` error)
- ADR-075: ChannelManager (`buffer_cap`, `max_channels`, `next_id` fields)
- `docs/research/alknet-channels/poc-summary.md` §POC Target 1 (backpressure
  validation), §POC Target 3 (1 MiB tunnel test)
- `docs/research/alknet-channels/phase-0-findings.md` §DP-5, §OQ-CH-03/04/
  05/06