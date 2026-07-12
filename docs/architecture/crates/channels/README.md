---
status: draft
last_updated: 2026-07-12
---

# alknet-channels

A multiplexing proxy: a `ProtocolHandler` on `alknet/channels` that
decomposes a single bidirectional transport stream into N logical channels,
each carrying a different ALPN. Channel 0 is pre-negotiated as `alknet/call`
(ADR-072); every other channel is opened dynamically via call operations on
channel 0 and routed through the same `HandlerRegistry` as top-level
connections. The channels layer is a re-framing proxy — it converts between
"one transport stream carrying N channels" (the wire) and "N independent
`AsyncRead + AsyncWrite` handles" (what handlers see) — and it does no
protocol work itself.

## Documents

| Document | Status | Description |
|----------|--------|-------------|
| [overview.md](overview.md) | draft | Crate purpose, the multiplexing collapse, dependencies, ALPN, transport agnosticism, WASM, relationship to existing crates |
| [channels-wire.md](channels-wire.md) | draft | The 9-byte chunk format (`[channel_id:u32 be][stream_type:u8][length:u32 be][payload]`), stream types, sentinels, framing disambiguation, wire-level invariants (REQ-CH-01..05) |
| [channels-connection.md](channels-connection.md) | draft | `ChannelBidiStreamSource` (implements `BidiStreamSource` — ADR-070/074), `into_sub_streams()` typed destructure, recursive composition |
| [channels-adapter.md](channels-adapter.md) | draft | `ChannelsAdapter` (`ProtocolHandler` on `alknet/channels`), `ChannelManager`, demux/mux contracts (REQ-CH-01..04), the two-pump pattern (ADR-078) |
| [channel-operations.md](channel-operations.md) | draft | `channel/open`, `channel/close`, `channel/control`, `channel/resources/subscribe` — call-protocol operations on channel 0, ACL flow, `direction` semantics, the hub relay contract (ADR-079) |
| [channel-client.md](channel-client.md) | draft | `ChannelClient` — the client side of a channels connection, QUIC-only initially, bidirectionality preserved |

## Applicable ADRs

| ADR | Title | Relevance |
|-----|-------|-----------|
| [071](../../decisions/071-channels-wire-format.md) | channels Wire Format — 9-Byte Chunk Header | The chunk format; unidirectional stream_types in groups of 3; substrate-agnostic; one-way door |
| [072](../../decisions/072-channel-0-pre-negotiated-call.md) | Channel 0 Is Pre-Negotiated `alknet/call` | Channel 0 = call protocol, stream_types [0,1]; no special control plane |
| [073](../../decisions/073-channel-lifecycle-operations.md) | Channel Lifecycle Operations on the Call Protocol | `channel/open`/`close`/`control`/`resources/subscribe`; `direction` semantics; subscribe not poll |
| [074](../../decisions/074-channelconnection-bidistreamsource.md) | ChannelConnection — BidiStreamSource over Chunk Reassembly | Per-channel `BidiStreamSource` impl; `into_sub_streams()` with `SubStreamHandle` enum (Send/Recv) |
| [075](../../decisions/075-channelsadapter-and-channelmanager.md) | ChannelsAdapter and ChannelManager | Substrate-agnostic demux loop; REQ-CH-01..04 contracts |
| [076](../../decisions/076-backpressure-channel-limits-id-reuse.md) | Backpressure, Channel Limits, and ID Reuse | Bounded-buffer (1 MiB default), 256-channel cap, monotonic IDs with wrap |
| [077](../../decisions/077-tty-inside-channels.md) | TTY Inside Channels — Sub-Streams, Not Wire Format | TTY's two modes (direct vs channels); 5 sub-streams; control bidirectional via 3/4; amends ADR-052 scope |
| [078](../../decisions/078-two-pump-shutdown-on-completion.md) | Two-Pump Shutdown-on-Completion Pattern | The two-pump deadlock contract; handler-level, not channels-layer |
| [079](../../decisions/079-hub-relay-translate-not-forward.md) | Hub Relay — Translate, Not Transparently Forward | The hub translates channel 0, byte-forwards data channels with ID rewrite |
| [080](../../decisions/080-channelclient.md) | ChannelClient — the Client Side of a Channels Connection | `ChannelClient`, QUIC-only; `AlknetClient` deferred (OQ-55) |
| [081](../../decisions/081-channels-subcrate-decomposition.md) | channels Sub-Crate Decomposition | `channels-core` (pure multiplexer) / `channels-call` (call coupling) / hub / worker |
| [070](../../decisions/070-bidistreamsource-trait.md) | BidiStreamSource Trait | The `Connection` extension point `ChannelBidiStreamSource` implements |
| [065](../../decisions/065-connection-from-stream-generic-single-stream.md) | `Connection::from_stream` | The transport-agnostic `Connection` the channels layer rides on |
| [052](../../decisions/052-alknet-tty-wire-format-and-two-carriage.md) | alknet-tty Wire Format | The 5-byte format the 9-byte format generalizes (amended by ADR-077 — scoped to direct TTY) |
| [049](../../decisions/049-streaming-handler-for-subscriptions.md) | StreamingHandler for Subscriptions | The machinery `channel/resources/subscribe` uses |
| [032](../../decisions/032-forwarded-for-identity.md) | Forwarded-For Identity | The auth chain for hub-relayed channel opens |
| [003](../../decisions/003-crate-decomposition.md) | Crate Decomposition | alknet-channels depends on alknet-core only; no handler-depends-on-handler |

## Relevant Open Questions

| OQ | Title | Status | Relevance |
|----|-------|--------|-----------|
| OQ-55 | AlknetClient / Client Establishment Extraction | deferred(scope) | `ChannelClient` is decided (ADR-080); `AlknetClient` core extraction stays deferred — blocked on a second *transport's* client, not a second client |
| OQ-56 | Full channel-level flow-control windowing | deferred(scope) | Bounded-buffer is decided (ADR-076); full windowing is an extension blocked on "a real deployment observes HOL blocking on a saturated channel where bounded buffer is insufficient" |
| OQ-57 | Two-pump helper extraction to alknet-core | deferred(scope) | The shutdown-on-completion *contract* is decided (ADR-078); the *helper* extraction is blocked on a second two-pump handler existing (shape convergence) |

## Key Design Principles

1. **Streams are streams.** A TTY session, an SSH channel, a forwarded TCP
   connection, a QUIC bidi stream — they're all `AsyncRead + AsyncWrite`
   handles. The differences are only in how they're *opened* (negotiation
   via `channel/open` on channel 0) and what *multiplexing layer* carries
   them (the 9-byte chunk format). Once normalized, every channel is an
   ALPN routed through the same `HandlerRegistry`. See
   [overview.md](overview.md) and ADR-071.

2. **Channel 0 is `alknet/call` pre-negotiated, not a special control
   plane.** The call protocol runs on channel 0 exactly as on a top-level
   `alknet/call` connection. Channel lifecycle operations
   (`channel/open`, `channel/close`, `channel/control`,
   `channel/resources/subscribe`) are call operations on channel 0's
   `OperationRegistry`, gated by the existing `AccessControl::check`. No
   new auth machinery, no new framing. See ADR-072, ADR-073.

3. **The channels layer is a re-framing proxy, not a protocol engine.** It
   converts between "one transport stream carrying N channels" (the wire)
   and "N independent `AsyncRead + AsyncWrite` handles" (what handlers
   see). It does no ALPN-specific parsing, no auth, no transport coupling.
   This makes it WASM-compatible and transport-agnostic by construction.
   See [channels-adapter.md](channels-adapter.md) and ADR-075.

4. **`channel/resources/subscribe` is a `Subscription`, not a polled
   `Query`.** The call protocol has `StreamingHandler` / `invoke_streaming`
   (ADR-049, implemented and tested). The first consumer (the hub
   aggregating worker resources) needs live updates. Polling would be built
   and immediately reworked. See ADR-073.

5. **Bidirectional open.** Either side can open a channel to the other,
   just like the call protocol's operation overlay. The `direction` field
   on `channel/open` pins who is the ALPN-server vs ALPN-client. See
   ADR-073 §Direction semantics.

6. **Wire-level invariants are contracts, not implementation details.**
   The POC surfaced five invariants (REQ-CH-01..04, plus REQ-CH-06 for
   close ordering) that hang channels silently if underspecified: shutdown
   emits a zero-length sentinel; transport close drops all senders; the mux
   supports dynamic registration; unknown `channel_id` is lenient-dropped;
   bounded-buffer backpressure doesn't deadlock; data chunks flush before
   `channel/close`. See [channels-wire.md](channels-wire.md) and
   [channels-adapter.md](channels-adapter.md).

7. **The hub translates, not transparently forwards.** The hub terminates
   channel 0 on both legs, runs `AccessControl::check`, and re-issues
   `channel/open` on the spoke leg with `forwarded_for` (ADR-032). Data
   channels are byte-forwarded with `channel_id` rewrite. This preserves
   the auth model. See ADR-079.

## References

- `docs/research/alknet-channels/phase-0-findings.md` — Phase 0 research
  (vision, hub motivation, wire format, negotiation, internals, DPs, OQs)
- `docs/research/alknet-channels/poc-summary.md` — the de-risk POC (28
  tests, three validated targets, REQ-CH-01..06 wire-level invariants
  surfaced; REQ-CH-07 is a cosmetic clippy item, not a wire invariant)
- `docs/research/alknet-channels/poc-plan.md` — the POC plan
- `/workspace/alknet-channels-poc/` — the POC codebase
- `docs/research/alknet-tty/phase-0-findings.md` — the TTY crate's chunk
  format (the seed of the channels generalization)
- `docs/research/alknet-ssh/phase-0-findings.md` — SSH's channel
  multiplexer (the prior art for N-channel multiplexing)
- `docs/architecture/crates/hub/README.md` — the hub crate (the primary
  consumer; the relay implementation's home)