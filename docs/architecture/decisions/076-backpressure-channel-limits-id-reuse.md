# ADR-076: Backpressure, Channel Limits, and ID Reuse

## Status

Accepted (amended 2026-07-18 by ADR-093 — backpressure is per-`channel_id`,
not per-`(channel_id, stream_type)`; the channels layer has one reassembly
buffer per channel, yielding a `BiStream` — see "Amendment (ADR-093,
2026-07-18)" below; **amended 2026-07-19 by ADR-094 — the per-connection
`max_channels = 256` is reframed as a per-connection memory bound, not a
DoS defense; the per-identity DoS defense lives in `channels-call` via
`ChannelLifecyclePolicy` — see "Amendment (ADR-094, 2026-07-19)" below**)

## Amendment (ADR-094, 2026-07-19)

The per-connection `max_channels = 256` cap is **reframed as a
per-connection memory bound**, not a DoS defense. A single peer can
open an unbounded number of transport connections, so a per-connection
cap is not a per-peer DoS defense — it is a bound on one connection's
reassembly-buffer cost. The per-identity DoS defense (256 per
`PeerId`, enforced in `channels-call` via `ChannelLifecyclePolicy`)
is documented in [ADR-094](094-per-identity-channel-cap.md).

What changes in this ADR:

1. **§"Maximum channels per connection: 256 default"** — the cap stays
   at 256, but its role is reframed. It is a per-connection memory
   bound (limits one connection's reassembly-buffer cost regardless of
   policy), not the DoS defense against an authenticated peer. The
   per-identity DoS defense is the `ChannelLifecyclePolicy`
   consultation in the `channel/open` handler (ADR-094).
2. **§"DoS defense summary"** — the table is **removed**. It framed
   the per-connection cap as the DoS defense, which it is not. ADR-094
   §2 contains the corrected per-identity DoS defense summary.
3. **The "per-connection, not per-peer — a peer can open more channels
   on a second connection" line** — this was the channels layer
   confessing a hole and hoping the layer above it would fill it. The
   line is **corrected** to state that the per-connection cap is a
   memory bound, and that the per-identity cap is the DoS defense
   (ADR-094). A peer that opens a second connection gets a second
   per-connection memory bound; it does **not** get a second
   per-identity quota — the `ChannelLifecyclePolicy` is shared across
   connections.

What stays:

- The 256 default and the `max_channels` field on `ChannelManager`
  (still returns `channel:too_many_channels` when hit — the
  per-identity policy returns the same error code, so an over-cap
  peer sees the same error either way).
- The bounded-buffer backpressure decision (DP-5) — unchanged.
- The channel-ID reuse decision (monotonic `next_id` with
  wrap-around) — unchanged.
- The drain-before-reuse invariant — unchanged, and the
  `channel/close` handler now also calls
  `ChannelLifecyclePolicy::on_close` at this point (ADR-094 §3).

## Amendment (ADR-093, 2026-07-18)

The bounded-buffer backpressure is per-`channel_id` (not per
`(channel_id, stream_type)`). The channels layer has one reassembly
buffer per channel (yielding a `BiStream`), not one per
`(channel_id, stream_type)`. The 1 MiB default and the 256-channel cap are
unchanged; the per-channel memory ceiling is 1 MiB (was up to 5 MiB for a
TTY channel with 5 active stream_types under the per-stream_type model).
This is a net improvement (lower memory ceiling per channel), not a
regression. The bounded-buffer *approach* is unchanged; only the
buffer granularity changes (per-channel, not per-stream_type).

The body below describes the **original** (per-stream_type) shape; the
amendment above is the operative decision. See ADR-093 for the resolution
rationale.

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

### Maximum channels per connection: 256 default (OQ-CH-05/06 — memory bound)

The `channel_id` is `u32` — the wire format supports ~4 billion channels.
The practical limit is memory (reassembly buffers per channel) and the
transport's flow control.

**Default per-connection channel limit: 256** (`max_channels` field on
`ChannelManager`, configurable). This is a **per-connection memory
bound**: it limits one connection's reassembly-buffer cost (256 × 1 MiB
= 256 MiB worst case per connection) regardless of policy. It composes
with the per-identity DoS defense (ADR-094) but is not itself a DoS
defense — a peer can open an unbounded number of transport connections,
so a per-connection cap cannot bound a peer's total channels. The
per-identity DoS defense (256 per `PeerId`, enforced in `channels-call`
via `ChannelLifecyclePolicy`) is documented in
[ADR-094](094-per-identity-channel-cap.md).

Exceeding the per-connection limit returns `channel:too_many_channels`
(ADR-073 error codes) — the same error code the per-identity policy
returns when the per-identity cap is hit. An over-cap peer sees the
same error either way; which cap fired first is an implementation
detail. The limit is per-connection as a memory bound; the per-identity
cap (ADR-094) is what bounds a peer's total channels across all its
connections.

### DoS defense summary (OQ-CH-06)

The DoS defense against an authenticated peer opening many channels is
the **per-identity cap** enforced in `channels-call` via
`ChannelLifecyclePolicy` — documented in
[ADR-094](094-per-identity-channel-cap.md). A per-connection cap
cannot be the DoS defense because a peer can open an unbounded number
of transport connections; the unit that must be bounded is the
identity, not the connection.

The per-connection `max_channels = 256` (this ADR) is a **memory
bound** that limits one connection's reassembly-buffer cost. It
composes with the per-identity cap as defense-in-depth (the
`NoCap` policy path still has the per-connection memory bound), but
it is not the security boundary. See ADR-094 §2 for the corrected
DoS defense summary.

## Consequences

**Positive:**
- Bounded-buffer backpressure is validated by the POC (1 MiB test, no
  deadlock, no cross-channel blocking). The decision is made, not hedged.
- The 256-channel default cap with 1 MiB buffers gives a bounded 256 MiB
  worst-case memory per connection — a clear per-connection memory
  ceiling, not an open-ended one. The per-identity DoS ceiling (256 per
  `PeerId` across all the peer's connections) is documented in ADR-094.
- Monotonic `next_id` with wrap-around avoids free-list drain-tracking
  complexity while still satisfying ID reuse (on wrap, after ~16.7M
  channels).

**Negative:**
- The 256-channel per-connection cap may be too low for a hub with many
  concurrent browser sessions each opening multiple channels. The cap
  is configurable per `ChannelManager`; the hub deployment may set it
  higher for deployments with many concurrent sessions. This is a
  deployment-time decision, not an architecture decision. (The
  per-identity cap in ADR-094 is the DoS-relevant bound; the
  per-connection cap is a memory backstop.)
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

- ADR-071: channels wire format (the chunks the buffers hold, as amended
  by ADR-093)
- ADR-093: channels pure channel multiplexing (amends this ADR —
  per-channel reassembly buffer, not per-`(channel_id, stream_type)`)
- ADR-094: per-identity channel cap as DoS defense (amends this ADR —
  the per-connection `max_channels = 256` is reframed as a per-connection
  memory bound, not a DoS defense; the per-identity DoS defense lives in
  `channels-call` via `ChannelLifecyclePolicy`)
- ADR-073: channel lifecycle operations (`channel:too_many_channels`
  error; the `channel/open` and `channel/close` handlers that gain the
  `ChannelLifecyclePolicy` consultation)
- ADR-075: ChannelManager (`buffer_cap`, `max_channels`, `next_id`
  fields; the auth-blindness that forces the per-identity cap into
  `channels-call`, not `channels-core`)
- ADR-032: forwarded-for identity (why the spoke caps the hub, not the
  browser — `forwarded_for` is metadata, not authority, for the cap as
  for `AccessControl::check`)
- `docs/research/alknet-channels/poc-summary.md` §POC Target 1 (backpressure
  validation), §POC Target 3 (1 MiB tunnel test)
- `docs/research/alknet-channels/phase-0-findings.md` §DP-5, §OQ-CH-03/04/
  05/06