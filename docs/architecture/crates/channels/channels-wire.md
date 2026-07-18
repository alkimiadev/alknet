---
status: draft
last_updated: 2026-07-18
---

# channels-wire.md — The 8-Byte Chunk Format

The wire format for `alknet/channels`: an 8-byte chunk header that
multiplexes N logical channels over a single ordered, reliable
bidirectional transport stream. ADR-071 (amended by ADR-093) is the
decision; this doc specifies the format and the wire-level invariants.
The channels layer has no `stream_type` concept — not in its header, not
in its code, not in its mental model. The handler owns its sub-stream
multiplexing on the `BiStream` the channels layer gives it.

## Chunk header

```
[channel_id: u32 BE][length: u32 BE][payload bytes]
```

8 bytes of header, followed by `length` bytes of opaque payload.

| field | offset | width | meaning |
|-------|--------|-------|---------|
| `channel_id` | 0 | 4 (BE) | The logical channel this chunk belongs to. Channel 0 is pre-negotiated as `alknet/call` (ADR-072). Channels 1..N are opened dynamically via `channel/open` (ADR-073). |
| `length` | 4 | 4 (BE) | The payload length in bytes. 0 = EOF sentinel. Max `MAX_CHUNK_LEN`. |

The payload is opaque to the channels layer. The handler parses its own
framing from the payload — TTY's `[stream_type:u8][length:u32][payload]`
(5-byte format, ADR-052), call's length-prefixed JSON (`EventEnvelope`
framing, ADR-064), tunnel's raw bytes, SSH's channel protocol. The
channels layer carries the bytes transparently.

### How the wire formats compose

The channels 8-byte header and the handler's framing compose by layering:

```
channels:  [channel_id:u32 BE][length:u32 BE][payload]
                        = 8-byte header + opaque payload
                            8 bytes

TTY inside channels:
  [channel_id:u32][ch_len:u32][stream_type:u8][tty_len:u32][payload]
      4 bytes      4 bytes       1 byte        4 bytes     N bytes
      \_________  __________/    \_________  _____________/
                 |                           |
          channels header              TTY chunk (5+N bytes)
            (8 bytes)              carried as channels payload
```

The channels layer reads its 8-byte header (`channel_id` + `length`),
reads `length` bytes of payload, and hands the payload to the handler.
The handler parses its own framing from the payload — TTY reads its
5-byte header (`stream_type` + `length`) from the payload bytes.

The two length fields are close but not identical: `ch_len = tty_len + 5`.
This is a small amount of waste per chunk (the channels `length` is always
5 bytes more than TTY's `length`), but the trade-off is clean separation
of concerns: the channels layer has no `stream_type` concept — not in
its header, not in its code, not in its mental model. The handler owns
its framing entirely. See ADR-093 for the full cost/benefit analysis.

## `MAX_CHUNK_LEN`

`16 * 1024 * 1024` (16 MiB), matching TTY's cap (ADR-052 §5). A chunk with
`length > MAX_CHUNK_LEN` returns `ChunkTooLarge` and does not corrupt the
stream — the demux drops the chunk and continues. The header is always
exactly 8 bytes, so the demux can always resync by reading the next
8-byte header.

## Channel 0 — pre-negotiated `alknet/call`

Channel 0 is not a special "control plane" with its own framing. It is
`alknet/call` pre-negotiated (ADR-072): both sides know `channel_id = 0`
is routed to the `CallAdapter` without an explicit `channel/open`
exchange.

Channel 0's chunks have `channel_id = 0` in the 8-byte header — same
format as every other channel. The call protocol's `EventEnvelope` JSON
framing (ADR-064) is the payload; the channels layer carries it
transparently. Disambiguation between channel 0 and data channels is by
`channel_id`, not by a special first-byte trick.

## Framing disambiguation

The 8-byte header is always exactly 8 bytes. `length` is bounded by
`MAX_CHUNK_LEN`. The demux reads 8 bytes, parses the header, reads
`length` bytes of payload, and routes. If a chunk is dropped (e.g.,
`ChunkTooLarge`), the demux resyncs by reading the next 8-byte header —
the format is self-synchronizing.

There is no channels-layer framing-disambiguation trick beyond the fixed
8-byte header. The channels layer does not interpret the payload — it
doesn't know if the payload is TTY chunks, call frames, or tunnel bytes.
Any framing disambiguation within the payload is the handler's concern
(see `tty-wire.md` §"Framing disambiguation" for TTY's first-byte trick,
which is internal to TTY's 5-byte format).

## Zero-length sentinel = EOF

A zero-length chunk (`length = 0`) is delivered as an empty payload,
which the reassembled stream interprets as EOF. This is the clean-shutdown
signal for a `channel_id` — the same convention as TTY (ADR-052
§Sentinels), now at the channels layer (one sentinel per channel, not
per `(channel_id, stream_type)`).

The sentinel is emitted by the write side's `AsyncWrite::shutdown` (see
REQ-CH-01 below) and consumed by the read side's `AsyncRead::poll_read` as
EOF.

## Substrate modes — same wire format, different stream counts

The 8-byte header is used in all substrates, on every bidi stream. The
difference between substrates is only **how many bidi streams the
transport yields**:

| Substrate | Transport | Streams | Header role |
|-----------|-----------|---------|--------------|
| In-line | TCP+TLS, WebTransport session, SSH `direct-tcpip` | 1 | Header demuxes N channels from that 1 stream |
| Native | QUIC (quinn/iroh) | N | Each stream carries 1 logical channel; header provides `channel_id` correlation |
| Multi-connection | Any, N connections | N × M | Each connection is self-contained (own channel 0, own demux); header is per-connection |

The `ChannelsAdapter::handle` loop: `accept_bi()` → for each stream, read
the 8-byte header → route by `channel_id` → reassemble into a `BiStream`.
On an in-line transport, `accept_bi()` yields once then
`ConnectionClosed` — the header does all the demux. On QUIC, `accept_bi()`
yields repeatedly — each stream is a channel, and the header provides
`channel_id` correlation. Same code path, same wire format, same handler
experience. See ADR-071 §substrate modes (as amended by ADR-093), ADR-075.

## Wire-level invariants (REQ-CH-01, 02, 04, 05)

The de-risk POC (`docs/research/alknet-channels/poc-summary.md` §Issues
Surfaced) surfaced invariants that hang channels silently if underspecified.
These are **contracts**, not implementation details — both sides must agree.

### REQ-CH-01: `AsyncWrite::shutdown` emits a zero-length sentinel

The reassembled stream's write half (`MpscSendStream` or equivalent) MUST
send an empty payload (the EOF sentinel) before dropping the sender on
`AsyncWrite::shutdown`. Without this, the demux never sees EOF on the
channel, and `tokio::io::copy` in the handler never
completes — the session hangs.

The TTY crate's `pump_session` emits the zero-length stdout sentinel
explicitly via its own 5-byte format's zero-length chunk; the channels
layer's per-channel write pump does NOT forward a sentinel on
sender-drop, so the send adapter must. Both sides must agree on this
convention, or channels hang on clean shutdown.

### REQ-CH-02: transport close → all channel senders drop → all handlers see EOF

The demux loop MUST clear its `channels` map on transport EOF, dropping
all `ReassemblyBuffer` senders. Every handler's reassembled `BiStream`
sees EOF even without an explicit zero-length sentinel arriving on the
wire.

Without this, `read_to_end` / `tokio::io::copy` in handlers hangs forever
waiting for a sender that never drops because the demux task is holding the
map. This is a teardown invariant of the `ChannelsAdapter::handle` contract.

### REQ-CH-04: lenient unknown-`channel_id` handling with error counter

A chunk with an unallocated `channel_id` is dropped with a debug log and
an error counter (exposed via `Demux::stats()`), and the demux continues.
This matches SSH's behavior and survives transient mis-ordering during
teardown (a chunk for a channel that was just closed may arrive after
the close is processed).

The alternative (strict — close the transport on unknown `channel_id`) is
fragile during teardown and catches bugs at the cost of reliability. The
lenient approach with an error counter provides observability without
fragility.

### REQ-CH-05: bounded-buffer backpressure does not deadlock

Each `channel_id` has an independent bounded `mpsc` buffer (default 1 MiB
— ADR-076). A slow reader on one channel does not block another channel's
reads — the demux's per-chunk route awaits the matching sender without
holding a global lock.

The 1 MiB `tunnel_large_payload` POC test exercised this end-to-end: a
channel writer faster than the TCP echo server consumer, with no deadlock
and no cross-channel blocking. This invariant must hold for all transport
shapes — the bounded-buffer approach is the decision (ADR-076).

## Sync core / async shell split

The wire format's core is pure byte manipulation:

```rust
// wire.rs — sync core, no async, no platform deps, WASM-clean

const CHUNK_HEADER_LEN: usize = 8;
const MAX_CHUNK_LEN: u32 = 16 * 1024 * 1024;

pub struct ChunkHeader {
    pub channel_id: u32,
    pub length: u32,
}

pub fn parse_header(buf: &[u8; 8]) -> Result<ChunkHeader, ChunkError> { ... }
pub fn write_header(channel_id: u32, length: u32, out: &mut [u8; 8]) { ... }
```

The async shell (demux/mux — see [channels-adapter.md](channels-adapter.md))
wraps this core with `read_exact` / `write_all` on the transport and `mpsc`
routing. The split keeps the WASM-compatible core separate from the
tokio-dependent shell. The POC validated the sync core compiles under
`wasm32-unknown-unknown`.

## The add/strip composition

Each layer has its own add/strip pair. The channels layer:
`add_channel_id(channel_id, payload_bytes) -> chunk` on write (prepends
the 8-byte header); `strip_channel_id(chunk) -> (channel_id,
payload_bytes)` on read (strips the 8-byte header, returns the payload).
The handler layer (e.g. TTY) parses its own framing from the payload
bytes per its existing `wire.rs`. The handler doesn't know or care that
a `channel_id` was stripped before it saw the bytes.

The composition is uniform — the same shape at every level. This is SSH's
model (layered headers, each layer strips its own at its boundary),
applied to channels. A `alknet/channels`-inside-`alknet/channels`
recursive composition is the outer layer stripping its 8-byte header, the
inner layer parsing its own 8-byte header from the payload — same code,
same shape, each level.

The exact API shape of the add/strip pair (built into the read/write path
vs. a standalone utility) is an implementation detail for the channels
crate, tracked as OQ-68. The *contract* — the channels layer strips its
8-byte header on read and the handler parses its own framing from the
payload — is decided; the *function surface* is not.

## Channel lifecycle (summary)

| Phase | Mechanism | Reference |
|-------|-----------|-----------|
| Open | `channel/open` call operation on channel 0; responder allocates `channel_id`, returns it | ADR-073 |
| Data | chunks with `channel_id` routed to reassembly buffers; handler sees a `BiStream` | this doc, [channels-connection.md](channels-connection.md) |
| Control (out-of-band) | `channel/control` call operation on channel 0 | ADR-073 |
| Close | `channel/close` call operation on channel 0; data chunks flushed before close | ADR-073, REQ-CH-06 |

### REQ-CH-06: exit-chunk-before-close ordering (generalizes ADR-055)

The channel's data chunks MUST be written and flushed before the
`channel/close` operation is sent on channel 0. This is a wire-level
invariant: the side closing must observe the data-channel pump complete
before issuing the call operation.

For TTY this is the exit-chunk-is-last invariant (ADR-055) carried
forward: the exit control message (on TTY's `STREAM_CTRL_OUT` stream_type
4, inside TTY's 5-byte payload) is the last data before `channel/close`.
For tunnels it is the last data byte before close. The channels layer's
close handler observes the pump completion; the call operation is issued
after.

This invariant crosses two channels (the data channel and channel 0), so
the channels layer owns the ordering guarantee — it is not a handler
concern. The control-message division (data-ordered control vs
out-of-band control) is now entirely handler-internal: TTY's
`STREAM_CTRL_IN` / `STREAM_CTRL_OUT` are stream_types in TTY's 5-byte
payload format, not channels-layer concepts.

## Design Decisions

All design decisions are documented as ADRs in [decisions/](../../decisions/).

| ADR | Decision | Summary |
|-----|----------|---------|
| [071](../../decisions/071-channels-wire-format.md) | channels Wire Format | 8-byte chunk header (amended by ADR-093); channels layer has no `stream_type` concept; one-way door |
| [093](../../decisions/093-channels-pure-channel-multiplexing.md) | channels Pure Channel Multiplexing | The umbrella decision: 8-byte header, no `stream_type`, `into_sub_streams` removed, `BiStream`-only, TTY always 5-byte |

## Open Questions

Open questions are tracked in [open-questions.md](../../open-questions.md).
Key questions affecting this doc:

- **OQ-68** (open): Add/strip API shape — whether the 8-byte header
  add/strip is built into the channels read/write path or exposed as a
  standalone utility. The *contract* (channels strips, handler parses
  payload) is decided; the *function surface* is not.

## References

- ADR-071: channels wire format (the decision, amended by ADR-093 — 8-byte
  header, no `stream_type`)
- ADR-093: channels pure channel multiplexing (the umbrella decision that
  amends ADR-071/074/077)
- ADR-052: alknet-tty wire format (the 5-byte format carried
  transparently in the channels payload)
- ADR-072: channel 0 pre-negotiated
- ADR-073: channel lifecycle operations
- ADR-076: backpressure, channel limits, ID reuse
- `docs/research/alknet-channels/poc-summary.md` §POC Target 1, §Issues
  Surfaced #4-#6 (REQ-CH-01, 02, 04)
- `docs/research/stream-unification/findings.md` — the research that
  surfaced the 8-byte format decision
- `crates/alknet-tty/src/wire.rs` — the 5-byte format implementation
  (carried transparently in the channels payload)