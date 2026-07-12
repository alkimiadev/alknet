# ADR-071: alknet-channels Wire Format — 9-Byte Chunk Header

## Status

Accepted (revised 2026-07-12: substrate simplification + stream_type
decomposition)

## Context

`alknet-channels` is a multiplexing proxy: a `ProtocolHandler` on
`alknet/channels` that carries N logical channels, each with a different
ALPN, over transport stream(s). The wire format is the substrate that makes
this work.

Two prior formats inform this design:

1. **SSH's channel multiplexer (RFC 4254)** — `ChannelId(u32)` with
   string-named types negotiated per channel, all traffic interleaved on one
   encrypted transport stream.
2. **alknet-tty's chunk format (ADR-052)** — `[stream_type: u8][length: u32 be]
   [payload]`, a fixed set of four sub-streams (stdin/stdout/stderr/control)
   within one bidi stream. Validated by two POCs and in production code.

The channels format is the generalization: add a `channel_id: u32` prefix to
TTY's 5-byte header, turning a fixed 4-channel multiplexer into an arbitrary
N-channel multiplexer. The de-risk POC (28 tests) validated the core
mechanics.

### The substrate simplification

Different transports have different native multiplexing capabilities. QUIC
has native bidi streams; TCP+TLS does not. The initial framing of this ADR
treated the 9-byte header as "the in-line substrate format" — used when the
transport has no native multiplexing, with a separate substrate for QUIC
native streams. That framing adds complexity (two substrates, a
`channel_id`↔stream-ID mapping) for no benefit.

The simplification: **the 9-byte header is used in all substrates, on every
bidi stream that carries channels.** The `channel_id` in the header is the
logical correlation key. The transport's native multiplexing (when present)
is a performance optimization (independent flow-control windows per
stream), not a protocol change. The `ChannelsAdapter` reads the 9-byte
header off every bidi stream it accepts — on a transport with one stream
(in-line), the header demuxes N channels from that stream; on a transport
with N streams (QUIC native), each stream carries one logical channel and
the header provides `stream_type` and `channel_id` correlation. Same code
path, same wire format, same handler experience.

### The stream_type decomposition

The initial framing had `stream_type` 3 as "bidirectional" (control
messages). This is a design flaw: one stream_type both sides write to is not
properly multiplexed — it loses independent flow control, independent EOF,
and clean separation of concerns. The TTY crate's control channel already
exhibits this problem ("the control channel isn't actually bidirectional…
the adapter ignores Exit from the client" — phase-0 findings §Less
Straightforward Parts).

The fix: **every stream_type is unidirectional.** Bidirectionality is
achieved by having two stream_types (one write, one read), the same way
QUIC bidi streams are two unidirectional halves. Control becomes 3 (write,
client→server) and 4 (read, server→client), not one "bidirectional" 3.

## Decision

### Chunk header

```
[channel_id: u32 be][stream_type: u8][length: u32 be][payload bytes]
```

9 bytes of header. The `channel_id` is the addition over TTY's 5-byte
format; `stream_type` and `length` are identical to TTY's fields (ADR-052),
preserving the framing-disambiguation soundness property.

| field | width | meaning |
|-------|-------|---------|
| `channel_id` | u32 BE | The logical channel this chunk belongs to. Channel 0 is pre-negotiated as `alknet/call` (ADR-072). Channels 1..N are opened dynamically via `channel/open`. |
| `stream_type` | u8 | The unidirectional sub-stream within the channel. See "Stream types" below. |
| `length` | u32 BE | The payload length in bytes. 0 = EOF sentinel (same convention as TTY — ADR-052 §Sentinels). |

### `MAX_CHUNK_LEN`

`16 * 1024 * 1024` (16 MiB), matching TTY's cap (ADR-052 §5). A chunk with
`length > MAX_CHUNK_LEN` returns `ChunkTooLarge` and does not corrupt the
stream — the demux drops the chunk and continues. The header is always
exactly 9 bytes, so the demux can always resync by reading the next 9-byte
header.

### Stream types — unidirectional, grouped in threes

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
| | 9/10/11 | write/read/read | next group |
| | ... | | |

**Formula:** `stream_type % 3 == 0` → write half (in), `stream_type % 3 ==
1` → read half (out), `stream_type % 3 == 2` → diagnostic read half (err).

256 values / 3 = 85 groups. The `u32` channel_id space (~4 billion channels
before wrap, ADR-076) combined with 85 stream_type groups is effectively
unlimited for the intended use cases.

**Why unidirectional:** each stream_type gets its own reassembly buffer, its
own flow control, its own EOF. The TTY control channel becomes *actually*
bidirectional because there are two unidirectional streams (3 in, 4 out),
not one stream both sides write to. This resolves the "control channel
isn't actually bidirectional" problem the TTY crate has today. The same
principle applies to any future channel type — control is two halves, not
one shared stream.

**Control payload format is ALPN-specific, not channels-enforced.** The
channels layer is blind to what stream_types 3/4/5 carry — it reassembles
bytes and delivers them to the handler. The TTY crate happens to use JSON
for its control channel (resize, signal, eof, exit) because its control
messages map cleanly to JSON; another ALPN might use a binary control
format. The channels layer does not mandate JSON on control stream_types.
This is the same ALPN-blindness principle that applies to the data
stream_types: the channels layer routes bytes, the handler interprets them.

### Per-ALPN stream_type sets

| ALPN | Active stream_types | Why |
|------|---------------------|-----|
| `alknet/call` (channel 0) | [0, 1] | call frames bidirectional via 0=in, 1=out |
| `alknet/tty` | [0, 1, 2, 3, 4] | data in/out/err + control in/out |
| `alknet/tunnel` | [0, 1] | data in/out only (no channels-layer control needed) |
| `alknet/ssh` | [0, 1] | SSH multiplexes internally, including its own control |

The active set is declared at `channel/open` time (ADR-073 `stream_types`
field) and fixed for the channel's lifetime. A tunnel that wants keepalive
could declare [0, 1, 3, 4].

### Substrate modes — same wire format, different stream counts

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
experience.

**Why keep the header on QUIC when the stream already separates channels:**
- **`stream_type` decomposition.** TTY needs 5 sub-streams. On QUIC, one
  stream per channel + the header carrying `stream_type` is simpler than 5
  streams per channel and matches the in-line case's shape. Per-stream-type
  flow control is handled at the reassembly buffer level (ADR-076)
  regardless of substrate.
- **`channel_id` correlation.** The hub relay forwards a channel from the
  browser leg to the spoke leg, mapping `browser_id ↔ spoke_id`. If
  `channel_id` is in the header on every stream, the relay correlates by
  reading the header — regardless of substrate. If `channel_id` lived only
  in the `channel/open` response, the relay would need a stream-ID-to-
  `channel_id` mapping per leg.
- **Uniform handler experience.** The handler receives a `Connection` and
  calls `accept_bi()` or `into_sub_streams()`. It doesn't know or care
  whether the substrate is in-line or native — the
  `ChannelBidiStreamSource` wraps the reassembled stream either way.

The "bloat" (9 bytes/chunk on transports that have native multiplexing) is
the cost of uniformity. For the intended use cases (TTY, SSH, tunnels, call
operations), this is noise. The high-throughput escape hatch is
multi-connection, not stripping the header.

### Framing disambiguation (carried from ADR-052 §5)

Channel 0 is just another channel — its chunks have `channel_id=0` in the
header. Disambiguation between channel 0 (call protocol) and data channels
is by `channel_id`, not by a special first-byte trick. Within a channel,
`stream_type` 0 (write half) from the server is invalid, so `0x00` as the
first byte of a chunk payload from the server is unambiguous.

### Zero-length sentinel = EOF

A zero-length chunk is delivered as an empty `Bytes`, which the reassembled
stream interprets as EOF (same convention as TTY — ADR-052 §Sentinels). This
is the clean-shutdown signal for a `(channel_id, stream_type)` pair.

### Sync core / async shell split

The wire format's core is pure byte manipulation — `parse_header(&[u8; 9])
-> ChunkHeader` and `write_header(channel_id, stream_type, length, &mut
[u8; 9])`. No async, no platform dependencies. Compiles under
`wasm32-unknown-unknown` (validated by the POC). The async shell (demux/mux)
wraps this core with `read_exact`/`write_all` on the transport and `mpsc`
routing. This split keeps the WASM-compatible core separate from the
tokio-dependent shell.

## Consequences

**Positive:**
- One wire format across all substrates. The `ChannelsAdapter` code path is
  the same regardless of transport; the transport's native multiplexing is a
  performance optimization, not a protocol change.
- Every stream_type is unidirectional — proper multiplexing with independent
  flow control and EOF per half. The TTY control channel becomes actually
  bidirectional (3 in, 4 out), resolving the "not actually bidirectional"
  flaw.
- The hub relay correlates by `channel_id` in the header, uniformly across
  substrates — no stream-ID-to-`channel_id` mapping per leg.
- WASM-compatible by construction — the pure core has no platform deps.
- The framing-disambiguation property from ADR-052 carries forward unchanged.

**Negative:**
- 9 bytes per chunk on all substrates, including QUIC where the transport
  already separates streams. This is 4 bytes more than using the QUIC stream
  ID directly as the `channel_id`. For the intended use cases this is noise;
  for high-throughput bulk transfer, the answer is multi-connection, not
  stripping the header.
- All channels on one in-line connection share one transport stream's
  flow-control window. A slow consumer on one channel can backpressure
  others. Mitigated by bounded-buffer backpressure (ADR-076), not
  eliminated. The native substrate (QUIC streams) avoids this — each
  channel gets its own flow-control window. For high-throughput, use native
  or multi-connection.

## Door type

**One-way.** The chunk header layout (`channel_id:u32 + stream_type:u8 +
length:u32`) and the stream_type group assignments (0/1/2 = data, 3/4/5 =
control, `% 3` formula) are wire-format commitments. Changing them after
deployments exist requires a version migration.

The `MAX_CHUNK_LEN` value (16 MiB) is a two-way-door implementation detail
within the one-way format.

## References

- ADR-052: alknet-tty wire format (the 5-byte format this generalizes;
  amended by ADR-077 — scoped to direct TTY)
- ADR-065: `Connection::from_stream` (the transport-agnostic Connection)
- ADR-070: `BidiStreamSource` trait (the extension point the channels
  connection implements; its docstring already anticipated per-channel
  streams)
- ADR-072: channel 0 pre-negotiated (now uses stream_types [0, 1])
- ADR-073: channel lifecycle operations (stream_types field examples
  updated)
- ADR-074: ChannelBidiStreamSource (into_sub_streams returns unidirectional
  handles)
- ADR-076: backpressure (bounded-buffer applies at reassembly regardless of
  substrate)
- ADR-077: TTY inside channels (5 sub-streams; control properly
  bidirectional via 3/4)
- `docs/research/alknet-channels/poc-summary.md` — the POC that validated
  the format (28 tests, WASM compile check)
- `docs/research/alknet-channels/phase-0-findings.md` §The Wire Format, §Less
  Straightforward Parts (the control-channel bidirectionality problem)