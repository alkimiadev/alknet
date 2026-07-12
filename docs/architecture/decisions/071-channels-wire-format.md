# ADR-071: alknet-channels Wire Format — 9-Byte Chunk Header

## Status

Accepted

## Context

`alknet-channels` is a multiplexing proxy: a `ProtocolHandler` on
`alknet/channels` that decomposes a single bidirectional transport stream into
N logical channels, each carrying a different ALPN. The wire format is the
substrate that makes one transport stream carry many channels.

Two prior formats inform this design:

1. **SSH's channel multiplexer (RFC 4254)** — `ChannelId(u32)` with
   string-named types negotiated per channel, all traffic interleaved on one
   encrypted transport stream.
2. **alknet-tty's chunk format (ADR-052)** — `[stream_type: u8][length: u32 be]
   [payload]`, a fixed set of four sub-streams (stdin/stdout/stderr/control)
   within one bidi stream. Validated by two POCs (alknet-docker-poc,
   alknet-tty-poc) and in production code (`crates/alknet-tty/src/wire.rs`).

The channels format is the generalization: add a `channel_id: u32` prefix to
TTY's 5-byte header, turning a fixed 4-channel multiplexer into an arbitrary
N-channel multiplexer. The de-risk POC (`docs/research/alknet-channels/poc-
summary.md`, 28 tests) validated this is a clean generalization: the sync core
(`parse_header`/`write_header`) is pure and WASM-compatible by construction;
the mpsc-bridged async shell scales to N concurrent channels with per-channel
order preservation and cross-channel isolation.

## Decision

### Chunk header

```
[channel_id: u32 be][stream_type: u8][length: u32 be][payload bytes]
```

9 bytes of header. The `channel_id` is the addition over TTY's 5-byte format;
`stream_type` and `length` are identical to TTY's fields (ADR-052), preserving
the framing-disambiguation soundness property (§5 below).

| field | width | meaning |
|-------|-------|---------|
| `channel_id` | u32 BE | The logical channel this chunk belongs to. Channel 0 is pre-negotiated as `alknet/call` (ADR-072). Channels 1..N are opened dynamically via `channel/open`. |
| `stream_type` | u8 | The sub-stream within the channel. 0=stdin/data-in, 1=stdout/data-out, 2=stderr/diagnostic (optional), 3=control (JSON), 4-255 reserved. |
| `length` | u32 BE | The payload length in bytes. 0 = EOF sentinel (same convention as TTY — ADR-052 §Sentinels). |

### `MAX_CHUNK_LEN`

`16 * 1024 * 1024` (16 MiB), matching TTY's cap (ADR-052 §5). A chunk with
`length > MAX_CHUNK_LEN` returns `ChunkTooLarge` and does not corrupt the
stream — the demux drops the chunk and continues. This preserves the framing-
disambiguation soundness property: the header is always exactly 9 bytes, so
the demux can always resync after a dropped chunk by reading the next 9-byte
header.

### Stream types (per channel)

| stream_type | direction | purpose |
|-------------|-----------|---------|
| 0 | write half | data flowing in (stdin equivalent) |
| 1 | read half | data flowing out (stdout equivalent) |
| 2 | read half (optional) | error/diagnostic output (stderr equivalent) |
| 3 | bidirectional | control messages (ALPN-specific JSON) |
| 4-255 | reserved | future sub-stream types |

Not all channels use all sub-streams. A TTY session uses 0-3. A raw tunnel
uses 0 and 1. An SSH connection uses 0 and 1 (SSH multiplexes internally).
The active `stream_type` set is declared at `channel/open` time
(ADR-073) and fixed for the channel's lifetime.

### Framing disambiguation (carried from ADR-052 §5)

Channel 0 is just another channel — its chunks have `channel_id=0` in the
header. Disambiguation between channel 0 (call protocol) and data channels is
by `channel_id`, not by a special first-byte trick. Within a channel,
`stream_type` 0 (stdin) from the server is invalid, so `0x00` as the first
byte of a chunk payload from the server is unambiguous.

### Zero-length sentinel = EOF

A zero-length chunk is delivered as an empty `Bytes`, which the reassembled
stream interprets as EOF (same convention as TTY — ADR-052 §Sentinels). This
is the clean-shutdown signal for a `(channel_id, stream_type)` pair.

### Sync core / async shell split

The wire format's core is pure byte manipulation — `parse_header(&[u8; 9]) ->
ChunkHeader` and `write_header(channel_id, stream_type, length, &mut [u8; 9])`.
No async, no platform dependencies. Compiles under `wasm32-unknown-unknown`
(validated by the POC). The async shell (demux/mux) wraps this core with
`read_exact`/`write_all` on the transport and `mpsc` routing. This split is
the same pattern as TTY's (REQ-TTY-01 generalization) and keeps the
WASM-compatible core separate from the tokio-dependent shell.

## Consequences

**Positive:**
- One multiplexing model replaces three (connection-level ALPN, stream-level
  QUIC native, sub-stream-level TTY chunks). The hub's relay logic becomes
  channel-by-channel byte forwarding, not per-protocol framing parsers.
- The 9-byte overhead is negligible for the intended use cases (TTY sessions,
  SSH, tunnels, call operations). The format is a 4-byte extension to a
  proven 5-byte format.
- WASM-compatible by construction — the pure core has no platform deps.
- The framing-disambiguation property from ADR-052 carries forward unchanged.

**Negative:**
- All channels on one `alknet/channels` connection share one transport
  stream's flow-control window. A slow consumer on one channel can
  backpressure others. This is mitigated by bounded-buffer backpressure
  (ADR-076) but not eliminated. For high-throughput bulk transfer, the
  application uses N independent channels connections — same trade-off as
  HTTP/2-over-TLS vs HTTP/3-over-QUIC. This is a transport property, not a
  channels-format property.
- 9 bytes per chunk is 4 bytes more than TTY's 5-byte format. For
  high-frequency small-chunk workloads (e.g., typing in a terminal), this is
  a 80% header overhead increase. In practice the chunk size is driven by
  the write pattern (a terminal sends a few bytes per keystroke regardless),
  and the 4-byte delta is noise next to the TLS/QUIC overhead.

## Door type

**One-way.** The chunk header layout (`channel_id:u32 + stream_type:u8 +
length:u32`) is a wire-format commitment. Changing field widths, order, or
semantics after deployments exist requires a version migration. The
`stream_type` assignments (0=stdin, 1=stdout, etc.) are one-way for the same
reason — they are inherited from ADR-052 and preserved.

The `MAX_CHUNK_LEN` value (16 MiB) is a two-way-door implementation detail
within the one-way format — it can be changed without a wire-format version
bump as long as both ends agree (it's a validation threshold, not a field
width).

## References

- ADR-052: alknet-tty wire format (the 5-byte format this generalizes)
- ADR-065: `Connection::from_stream` (the transport-agnostic Connection this
  format rides on)
- ADR-070: `BidiStreamSource` trait (the extension point the channels
  connection implements)
- `docs/research/alknet-channels/poc-summary.md` — the POC that validated the
  format (28 tests, WASM compile check)
- `docs/research/alknet-channels/phase-0-findings.md` §The Wire Format
- `crates/alknet-tty/src/wire.rs` — the 5-byte format implementation