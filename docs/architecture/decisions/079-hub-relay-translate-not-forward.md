# ADR-079: Hub Relay — Translate, Not Transparently Forward

## Status

Accepted

## Context

The hub is the architectural role (ADR-029, ADR-034) that bridges peers and
browsers. With channels, the hub holds one channels connection per leg
(browser↔hub, hub↔spoke) and relays channels between them. The phase-0
research (`docs/research/alknet-channels/phase-0-findings.md` §OQ-CH-11,
§The hub relay) identified the key question: does the hub *translate*
`channel/open` (terminate channel 0 on both legs, re-issue the open on the
spoke leg) or *transparently forward* (pass the call operation through
unchanged)?

This is the most under-specified part of the research for something that is
the *primary motivation* for the channels crate (§Hub Motivation: the
multi-transport collapse). The research said "Phase 1 must specify whether
the hub translates or transparently forwards, and how the `channel_id`
mapping is maintained."

The answer is derivable from the existing machinery:
- The hub terminates channel 0 on both legs (it runs its own `CallAdapter`
  per leg — ADR-072).
- The hub's `CallAdapter` receives the browser's `channel/open` as a call
  operation, runs `AccessControl::check` with the browser's identity, then
  forwards via `from_call` to the spoke (the hub as caller, the browser as
  `forwarded_for` — ADR-032 §3).
- The spoke allocates its `channel_id` and returns it; the hub maps
  browser-id ↔ spoke-id.

Transparent forwarding (passing the `channel/open` call operation through
without the hub's `CallAdapter` terminating it) would bypass the hub's
`AccessControl::check` and the `forwarded_for` auth chain — the hub would
not authenticate the open, and the spoke would see the browser as the direct
caller (not the hub), breaking the ADR-032/ADR-050 auth model. Translation
is the only option that preserves the auth model.

## Decision

### The hub translates, not transparently forwards

The hub's relay has two layers:

1. **Call-protocol layer (channel 0): translate.** The hub terminates
   channel 0 on both legs. A `channel/open` from the browser is received by
   the hub's `CallAdapter`, which:
   1. Runs `AccessControl::check` on `channel/open` with the browser's
      identity (bearer token resolved per ADR-034). If denied →
      `channel:forbidden` to the browser.
   2. Issues a *new* `channel/open` on the spoke's channel 0 via `from_call`,
      with the hub as caller and the browser as `forwarded_for` (ADR-032
      §3). The spoke's `AccessControl::check` sees the hub as the direct
      peer (authorized per ADR-050) and the browser as `forwarded_for`.
   3. The spoke allocates its `channel_id` and returns it.
   4. The hub opens a matching channel on the browser's side (the hub is now
      the *responder* for the browser leg, *initiator* for the spoke leg)
      and records the `channel_id` mapping: `browser_id ↔ spoke_id`.

2. **Data-channel layer: byte-forward with `channel_id` rewrite.** Once the
   mapping is established, the relay reads chunks for `browser_id` off the
   browser's channels connection, rewrites the `channel_id` field to
   `spoke_id`, and writes them onto the spoke's channels connection — and
   vice versa. The relay does not parse the payload; it does not know if the
   bytes are TTY chunks, SSH frames, or tunnel data. The channels layer on
   each end does the chunk↔stream conversion; the relay just moves bytes
   between two `AsyncRead + AsyncWrite` pairs with a 4-byte header rewrite.

### `channel_id` mapping

The hub maintains a `HashMap<channel_id, channel_id>` per (browser, spoke)
pair — the relay map. On `channel/open` (translated), the mapping is
inserted. On `channel/close` (translated the same way), the mapping is
removed. The relay task per channel reads the map to determine the rewrite
target.

`channel/control` operations on channel 0 carry `channel_id` in their JSON
payload (not in the chunk header). The hub's `CallAdapter` translates these
too: the browser's `channel/control` for `browser_id` is re-issued on the
spoke leg with `spoke_id` in the payload. The relay does not touch
`channel/control` — it's a call operation, translated by the hub's
`CallAdapter`, not byte-forwarded.

### What the hub runs

| Leg | What the hub runs |
|-----|-------------------|
| Browser leg | `ChannelsAdapter` (the relay's read/demux) + `CallAdapter` (channel 0, for the hub's own ops + translating the browser's ops) |
| Spoke leg | `ChannelsAdapter` + `CallAdapter` (same) |
| Relay | Per-channel byte-forward tasks with `channel_id` rewrite |

The hub never runs a handler for `alknet/tty`, `alknet/ssh`, or
`alknet/tunnel`. It runs `alknet/channels` (the relay) and `alknet/call`
(for its own hub-level operations + translation). The endpoints at each end
do the protocol work.

### What the hub still owns (unchanged from phase-0 §What the hub does still own)

- **Routing:** which spoke serves `container:abc123`? The hub's resource
  registry / ownership store (ADR-050), queried via call operations on
  channel 0. Channels doesn't touch this.
- **ACL at the hub:** does this browser's identity have `channel:open` scope
  for `alknet/ssh` to `spoke-X`? `AccessControl::check` on `channel/open`,
  run by the hub's `CallAdapter` before it forwards. Channels doesn't touch
  this.
- **Relay lifecycle:** when a browser disconnects, the hub tears down the
  spoke-side channels (and vice versa). `channel/close` on each channel, or
  a transport-level close the channels layer observes (REQ-CH-02).

### Scope note: this is a hub-crate concern, not a channels-crate concern

This ADR defines the relay *contract* (translate channel 0, byte-forward
data channels with ID rewrite) so the channels crate's `ChannelManager`
exposes the interface the relay needs (`open_channel_stream(channel_id,
stream_type) -> (SendStream, RecvStream)` for the byte-forward pumps). The
relay *implementation* lives in `alknet-hub` (or a downstream hub like
alkapi), not in `alknet-channels`. The channels crate is ALPN-blind and
does not know it is being relayed.

## Consequences

**Positive:**
- The auth model reuses cleanly: the hub's `AccessControl::check` +
  `forwarded_for` (ADR-032) is the existing machinery, not a new one. The
  spoke sees the hub as caller, the browser as `forwarded_for` — the
  kernel/user-land + forwarded-for model from ADR-050.
- The relay is one pump function per channel, not per (protocol × transport)
  cell. The hub's complexity is O(channels), not O(protocols × transports ×
  spokes).
- The hub never runs protocol-specific handlers — it doesn't parse TTY
  chunks, SSH frames, or tunnel data. It moves bytes and translates call
  operations.
- `channel/resources/subscribe` (ADR-073) gives the hub a live view of each
  spoke's resources, which the hub aggregates and exposes to the browser.

**Negative:**
- The hub maintains a `channel_id` mapping per (browser, spoke) pair. This
  is per-channel state, not per-connection — a hub with many concurrent
  browser sessions each with multiple channels has a non-trivial map. The
  map is `HashMap<u32, u32>` per pair — cheap per entry, but the entry count
  is (browsers × channels-per-browser). Bounded by `max_channels` (ADR-076)
  per connection.
- The translate path adds one `channel/open` round-trip per relayed channel
  (browser→hub, hub→spoke). This is the same cost as any hub-relayed call
  operation and is not avoidable without transparent forwarding, which
  breaks the auth model.
- `channel/control` translation requires the hub's `CallAdapter` to rewrite
  `channel_id` in the JSON payload. This is a small but real translation
  step — the hub is not a pure byte relay for channel 0.

## Door type

**One-way.** The translate-vs-forward decision is structural: transparent
forwarding would bypass the hub's `AccessControl::check` and the
`forwarded_for` chain, breaking the auth model. Reversing to transparent
forwarding after deployments exist would require re-architecting the hub's
auth path. The `channel_id` mapping strategy (`HashMap` per pair) is two-way
— an implementation detail that can change without breaking the contract.

## References

- ADR-029: peer-graph routing model (the hub's role)
- ADR-032: forwarded-for identity (the auth chain the translate path uses)
- ADR-034: outgoing-only X.509 and the three peer roles (browser identity
  resolution)
- ADR-050: dynamic resource ownership (the ownership store the hub queries)
- ADR-072: channel 0 is pre-negotiated `alknet/call` (what the hub
  terminates on each leg)
- ADR-073: channel lifecycle operations (what the hub translates)
- ADR-075: ChannelsAdapter and ChannelManager (the interface the relay uses)
- `docs/research/alknet-channels/phase-0-findings.md` §Hub Motivation,
  §The hub relay, §OQ-CH-11
- `docs/architecture/crates/hub/README.md` — the hub crate (the relay
  implementation's home)