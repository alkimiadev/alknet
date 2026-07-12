---
status: draft
last_updated: 2026-07-12
---

# channels-wire.md — The 9-Byte Chunk Format

The wire format for `alknet/channels`: a 9-byte chunk header that
multiplexes N logical channels, each with up to 256 sub-stream types, over
a single ordered, reliable bidirectional transport stream. ADR-071 is the
decision; this doc specifies the format and the wire-level invariants.

## Chunk header

```
[channel_id: u32 be][stream_type: u8][length: u32 be][payload bytes]
```

9 bytes of header, followed by `length` bytes of payload.

| field | offset | width | meaning |
|-------|--------|-------|---------|
| `channel_id` | 0 | 4 (BE) | The logical channel this chunk belongs to. Channel 0 is pre-negotiated as `alknet/call` (ADR-072). Channels 1..N are opened dynamically via `channel/open` (ADR-073). |
| `stream_type` | 4 | 1 | The sub-stream within the channel. See "Stream types" below. |
| `length` | 5 | 4 (BE) | The payload length in bytes. 0 = EOF sentinel. Max `MAX_CHUNK_LEN`. |

This is a 4-byte extension of alknet-tty's 5-byte format (ADR-052): the
`channel_id` prefix is added; `stream_type` and `length` are identical. The
`ChunkReader` / `ChunkWriter` pattern, the framing-disambiguation trick,
and the zero-length sentinel convention all carry forward from TTY.

## `MAX_CHUNK_LEN`

`16 * 1024 * 1024` (16 MiB), matching TTY's cap (ADR-052 §5). A chunk with
`length > MAX_CHUNK_LEN` returns `ChunkTooLarge` and does not corrupt the
stream — the demux drops the chunk and continues. The header is always
exactly 9 bytes, so the demux can always resync by reading the next 9-byte
header.

## Stream types — unidirectional, grouped in threes

**Every stream_type is unidirectional.** Bidirectionality is two
stream_types (write + read), not one "bidirectional" stream_type. The
stream_types are grouped in threes:

| Group | stream_type | direction | purpose |
|-------|-------------|-----------|---------|
| Data | 0 | write (client→server) | data in (stdin equivalent) |
| | 1 | read (server→client) | data out (stdout equivalent) |
| | 2 | read (server→client) | data err (stderr equivalent, optional) |
| Control | 3 | write (client→server) | control in (ALPN-specific format) |
| | 4 | read (server→client) | control out (ALPN-specific format) |
| | 5 | read (server→client) | control err (optional) |
| Future | 6/7/8 | write/read/read | next group, same pattern |
| | ... | | |

**Formula:** `stream_type % 3 == 0` → write half (in), `stream_type % 3 ==
1` → read half (out), `stream_type % 3 == 2` → diagnostic read half (err).

256 values / 3 = 85 groups. The `u32` channel_id space combined with 85
stream_type groups is effectively unlimited for the intended use cases.

**Why unidirectional:** each stream_type gets its own reassembly buffer, its
own flow control, its own EOF. Control is bidirectional via two halves
(3 in, 4 out), not one shared stream both sides write to. This resolves the
TTY control channel's "not actually bidirectional" flaw (ADR-077).

**Control payload format is ALPN-specific.** The channels layer is blind to
what stream_types 3/4/5 carry — it reassembles bytes and delivers them to
the handler. TTY happens to use JSON for its control channel; another ALPN
might use a binary format. The channels layer does not mandate JSON on
control stream_types, the same way it doesn't mandate a format for data
stream_types.

Not all channels use all sub-streams. The active set is declared at
`channel/open` time (ADR-073 `stream_types` field) and fixed for the
channel's lifetime.

| Channel ALPN | Active stream_types | Why |
|--------------|---------------------|-----|
| `alknet/call` (channel 0) | [0, 1] | call frames bidirectional via 0=in, 1=out |
| `alknet/tty` | [0, 1, 2, 3, 4] | data in/out/err + control in/out |
| `alknet/tunnel` | [0, 1] | data in/out only (no channels-layer control needed) |
| `alknet/ssh` | [0, 1] | SSH multiplexes internally, including its own control |

## Substrate modes — same wire format, different stream counts

The 9-byte header is used in all substrates, on every bidi stream. The
difference between substrates is only **how many bidi streams the transport
yields**:

| Substrate | Transport | Streams | Header role |
|-----------|-----------|---------|--------------|
| In-line | TCP+TLS, WebTransport session, SSH `direct-tcpip` | 1 | Header demuxes N channels from that 1 stream |
| Native | QUIC (quinn/iroh) | N | Each stream carries 1 logical channel; header provides `stream_type` + `channel_id` correlation |
| Multi-connection | Any, N connections | N × M | Each connection is self-contained (own channel 0, own demux); header is per-connection |

The `ChannelsAdapter::handle` loop: `accept_bi()` → for each stream, read
the 9-byte header → route by `(channel_id, stream_type)` → reassemble. On
an in-line transport, `accept_bi()` yields once then `ConnectionClosed` —
the header does all the demux. On QUIC, `accept_bi()` yields repeatedly —
each stream is a channel, and the header provides `stream_type` and
`channel_id` correlation. Same code path, same wire format, same handler
experience. See ADR-071 §substrate modes, ADR-075.

## Channel 0 — pre-negotiated `alknet/call`

Channel 0 is not a special "control plane" with its own framing. It is
`alknet/call` pre-negotiated (ADR-072): both sides know `channel_id = 0` is
routed to the `CallAdapter` without an explicit `channel/open` exchange.

Channel 0 uses stream_types [0, 1] — call frames bidirectional via 0=in
(client→server), 1=out (server→client). The call protocol's `(SendStream,
RecvStream)` pair maps directly: `SendStream` backed by stream_type 0,
`RecvStream` backed by stream_type 1. stream_types 2-255 on channel 0 are
reserved for future call-protocol sub-streams.

Channel 0's chunks have `channel_id = 0` in the header — same format as
every other channel. Disambiguation between channel 0 and data channels is
by `channel_id`, not by a special first-byte trick.

## Framing disambiguation (from ADR-052 §5)

The 9-byte header is always exactly 9 bytes. `length` is bounded by
`MAX_CHUNK_LEN`. The demux reads 9 bytes, parses the header, reads
`length` bytes of payload, and routes. If a chunk is dropped (e.g.,
`ChunkTooLarge`), the demux resyncs by reading the next 9-byte header —
the format is self-synchronizing.

Within a channel, `stream_type` 0 (write half) from the server is invalid,
so `0x00` as the first byte of a chunk payload from the server is
unambiguous (carried from ADR-052 §5).

## Zero-length sentinel = EOF

A zero-length chunk (`length = 0`) is delivered as an empty `Bytes`, which
the reassembled stream interprets as EOF. This is the clean-shutdown signal
for a `(channel_id, stream_type)` pair — same convention as TTY (ADR-052
§Sentinels).

The sentinel is emitted by the write side's `AsyncWrite::shutdown` (see
REQ-CH-01 below) and consumed by the read side's `AsyncRead::poll_read` as
EOF.

## Wire-level invariants (REQ-CH-01, 02, 04, 05)

The de-risk POC (`docs/research/alknet-channels/poc-summary.md` §Issues
Surfaced) surfaced invariants that hang channels silently if underspecified.
These are **contracts**, not implementation details — both sides must agree.

### REQ-CH-01: `AsyncWrite::shutdown` emits a zero-length sentinel

The reassembled stream's write half (`MpscSendStream` or equivalent) MUST
send an empty `Bytes` (the EOF sentinel) before dropping the sender on
`AsyncWrite::shutdown`. Without this, the demux never sees EOF on the
channel's `stream_type`, and `tokio::io::copy` in the handler never
completes — the session hangs.

The TTY crate's `pump_session` emits the zero-length stdout sentinel
explicitly via `Chunk::stdout(Bytes::new())`; the channels layer's
per-channel write pump does NOT forward a sentinel on sender-drop, so the
send adapter must. Both sides must agree on this convention, or channels
hang on clean shutdown.

### REQ-CH-02: transport close → all channel senders drop → all handlers see EOF

The demux loop MUST clear its `channels` map on transport EOF, dropping all
`ReassemblyBuffer` senders. Every handler's reassembled `RecvStream` sees
EOF even without an explicit zero-length sentinel arriving on the wire.

Without this, `read_to_end` / `tokio::io::copy` in handlers hangs forever
waiting for a sender that never drops because the demux task is holding the
map. This is a teardown invariant of the `ChannelsAdapter::handle` contract.

### REQ-CH-04: lenient unknown-`channel_id` handling with error counter

A chunk with an unallocated `channel_id` (or `stream_type` on an allocated
channel) is dropped with a debug log and an error counter (exposed via
`Demux::stats()`), and the demux continues. This matches SSH's behavior and
survives transient mis-ordering during teardown (a chunk for a channel that
was just closed may arrive after the close is processed).

The alternative (strict — close the transport on unknown `channel_id`) is
fragile during teardown and catches bugs at the cost of reliability. The
lenient approach with an error counter provides observability without
fragility.

### REQ-CH-05: bounded-buffer backpressure does not deadlock

Each `(channel_id, stream_type)` has an independent bounded `mpsc` buffer
(default 1 MiB — ADR-076). A slow reader on one channel does not block
another channel's reads — the demux's per-chunk route awaits the matching
sender without holding a global lock.

The 1 MiB `tunnel_large_payload` POC test exercised this end-to-end: a
channel writer faster than the TCP echo server consumer, with no deadlock
and no cross-channel blocking. This invariant must hold for all transport
shapes — the bounded-buffer approach is the decision (ADR-076).

## Sync core / async shell split

The wire format's core is pure byte manipulation:

```rust
// wire.rs — sync core, no async, no platform deps, WASM-clean

const CHUNK_HEADER_LEN: usize = 9;
const MAX_CHUNK_LEN: u32 = 16 * 1024 * 1024;

pub struct ChunkHeader {
    pub channel_id: u32,
    pub stream_type: u8,
    pub length: u32,
}

pub fn parse_header(buf: &[u8; 9]) -> Result<ChunkHeader, ChunkError> { ... }
pub fn write_header(channel_id: u32, stream_type: u8, length: u32, out: &mut [u8; 9]) { ... }
```

The async shell (demux/mux — see [channels-adapter.md](channels-adapter.md))
wraps this core with `read_exact` / `write_all` on the transport and `mpsc`
routing. The split keeps the WASM-compatible core separate from the
tokio-dependent shell. The POC validated the sync core compiles under
`wasm32-unknown-unknown`.

## Channel lifecycle (summary)

| Phase | Mechanism | Reference |
|-------|-----------|-----------|
| Open | `channel/open` call operation on channel 0; responder allocates `channel_id`, returns it | ADR-073 |
| Data | chunks with `channel_id` routed to reassembly buffers; handler sees `AsyncRead + AsyncWrite` | this doc, [channels-connection.md](channels-connection.md) |
| Control (data-ordered) | `stream_type 3` (write) and `stream_type 4` (read) chunks on the data channel (JSON, in-order with data) | ADR-073 §DP-4 |
| Control (out-of-band) | `channel/control` call operation on channel 0 | ADR-073 |
| Close | `channel/close` call operation on channel 0; data chunks flushed before close | ADR-073, REQ-CH-06 |

### REQ-CH-06: exit-chunk-before-close ordering (generalizes ADR-055)

The channel's data chunks MUST be written and flushed before the
`channel/close` operation is sent on channel 0. This is a wire-level
invariant: the side closing must observe the data-channel pump complete
before issuing the call operation.

For TTY this is the exit-chunk-is-last invariant (ADR-055) carried forward:
the exit control message on `stream_type 4` (read, server→client) is the
last data before `channel/close`. For tunnels it is the last data byte
before close. The channels layer's close handler observes the pump
completion; the call operation is issued after.

This invariant crosses two channels (the data channel and channel 0), so
the channels layer owns the ordering guarantee — it is not a handler
concern.

## References

- ADR-071: channels wire format (the decision)
- ADR-052: alknet-tty wire format (the 5-byte format this generalizes;
  amended by ADR-077 — scoped to direct TTY)
- ADR-072: channel 0 pre-negotiated
- ADR-073: channel lifecycle operations
- ADR-076: backpressure, channel limits, ID reuse
- `docs/research/alknet-channels/poc-summary.md` §POC Target 1, §Issues
  Surfaced #4-#6 (REQ-CH-01, 02, 04)
- `crates/alknet-tty/src/wire.rs` — the 5-byte format implementation