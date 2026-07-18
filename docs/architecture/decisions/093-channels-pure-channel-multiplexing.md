# ADR-093: alknet-channels — Pure Channel Multiplexing (8-Byte Header, No `stream_type`)

## Status

Accepted (amends ADR-071 — wire format is 8 bytes, not 9, and the channels
layer has no `stream_type` concept; amends ADR-074 — `into_sub_streams()`
removed, `accept_bi` is the only accessor and yields one `BiStream` per
channel; reverses ADR-077 — TTY always uses its 5-byte format, the channels
layer carries it transparently in the payload)

## Context

ADR-071 committed the channels wire format as a 9-byte chunk header
(`[channel_id:u32][stream_type:u8][length:u32]`) — a 4-byte extension of
TTY's 5-byte format, with `stream_type` carried in the channels header
and decomposed into unidirectional halves (0/1/2 = data write/read/err,
3/4/5 = control write/read/err, `% 3` formula). ADR-074 added a second
accessor (`into_sub_streams()`) alongside `accept_bi` for handlers that
need typed sub-streams (TTY's stdin/stdout/stderr/ctrl-in/ctrl-out).
ADR-077 split TTY's wire format into two modes — direct (5-byte) and
inside-channels (the channels layer de-chunks and the adapter destructures
via `into_sub_streams()`).

The stream-unification research
(`docs/research/stream-unification/findings.md`, 2026-07-18) surfaced that
these three decisions share one root: the channels layer carries a
concept (`stream_type`) it doesn't own. The 9-byte header bakes TTY's
sub-stream multiplexing into the channels wire format. The
`into_sub_streams()` accessor exists because the channels layer reassembles
per-`stream_type` and needs to expose the result. The two-mode TTY design
exists because the channels layer's `stream_type` overlaps with TTY's own
`stream_type`. The mod 2/mod 3/mod 4 numbering question (settled as mod 3
in ADR-071 revised) was a symptom of this overlap — a numbering convention
for a concept the channels layer shouldn't carry.

### The structural question

The channels layer has two objectives in tension:

1. **"Pass a stream to/from any ALPN"** — every channel is a `BiStream`;
   any handler gets `accept_bi()` and treats the channel as a duplex
   stream. Uniform, transport-agnostic, recursive-composition-friendly.
2. **"Channels carry N sub-streams"** — a TTY channel carries
   stdin/stdout/stderr/control; the handler destructures via
   `into_sub_streams()`. Carries what the source produces.

The tension is real when a sub-stream is *unidirectional* (stderr). You
can't represent stderr as a `BiStream` without wasting the write half;
you can't make it a "third half" (mod 3) without breaking pair symmetry;
you can't make the channel a single `BiStream` without losing the
stdout/stderr distinction.

ADR-074's two-accessor design resolves this by making the "pass a stream
to/from any ALPN" objective *qualified* — it applies to single-stream
channels (tunnel, SSH, call), not multi-stream channels (TTY). The mod
2/mod 3/mod 4 numbering was a symptom of that qualified design.

### The resolution: channels layer is pure channel multiplexing

The channels layer's job is "one connection carries N channels, routed
by `channel_id`." It does not know about TTY's sub-streams, SSH's channel
protocol, or how call frames its JSON. Handlers own their sub-multiplexing
on the `BiStream` the channels layer gives them.

- **Every channel is a `BiStream`.** `accept_bi()` yields one `BiStream`
  per channel (per ADR-092, already landed). No `into_sub_streams()`, no
  second-class accessor.
- **Handlers sub-multiplex their `BiStream` however they want.** TTY
  sub-demuxes `stream_type` from its `BiStream` (its 5-byte format). Tunnel
  uses the `BiStream` as raw bytes. Call length-prefixes JSON. SSH runs
  its own channel protocol. The channels layer carries the bytes
  transparently.
- **The mod 2/mod 3/mod 4 question dissolves at the channels layer.** The
  channels layer has no `stream_type` concept — not in its header, not in
  its code, not in its mental model. `stream_type` is the inner layer's
  framing byte, carried transparently.
- **The control channel is handler-internal.** TTY sub-demuxes control
  from its io `BiStream` using its 5-byte format (`STREAM_CTRL_IN = 3`,
  `STREAM_CTRL_OUT = 4` — ADR-052 amended by Phase 7). The channels layer
  doesn't carry control. The "control isn't actually bidirectional" flaw
  is fixed at the TTY layer, not the channels layer.
- **Recursive composition is literal.** A channel with ALPN
  `alknet/channels` runs another channels demux on its `BiStream`. The
  outer layer strips its 8-byte header; the inner layer parses its own
  8-byte header from the payload. Each level is the same shape —
  `BiStream → accept_bi → N BiStreams`.

### The wire format decision: 8 bytes

The channels wire format is **8 bytes**: `[channel_id:u32 BE][length:u32
BE]` followed by an opaque payload. The channels layer owns `channel_id`
and `length`; the payload is the handler's framing, carried transparently.

The 9-byte alternative (`[channel_id:u32][stream_type:u8][length:u32]`)
was considered and rejected. The 9-byte format puts `stream_type` in the
channels header, which means the channels layer carries a concept it
doesn't own. For TTY this composes cleanly (the 9-byte header is TTY's
5-byte header with `channel_id` prepended), but for non-TTY handlers
(tunnel, call, SSH) the `stream_type` byte is dead weight — the channels
layer carries a byte it doesn't understand, and the handler ignores a
byte in a header it doesn't control.

The 8-byte format is uniform across all handlers: the channels layer
carries `channel_id` + `length` + opaque payload. Every handler parses
its own framing from the payload. The cost is that TTY's `wire.rs` is
called from a payload buffer rather than directly from the wire, and the
total header for a TTY chunk is 13 bytes (8 channels + 5 TTY) instead of
9. The two length fields are close but not identical (`ch_len = tty_len +
5`); for typical TTY chunks (4 KiB+), the 5-byte overhead is ~0.1%, and
the trade is clean separation of concerns. See "Consequences" for the
full cost/benefit.

### The add/strip composition

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

### Why this can land now

Three things changed since ADR-071/074/077 were accepted:

1. **ADR-092 landed `BiStream` as the handler leaf.** `accept_bi()`
   returns a `BiStream` (a concrete `AsyncRead + AsyncWrite` newtype), not
   a split `(SendStream, RecvStream)` pair. The join moves into core's
   quinn/iroh/stream impls (once per source, invisible to handlers). This
   ADR's "every channel is a `BiStream`" is the channels-layer
   consequence of ADR-092's handler-leaf decision — the research-then-sync
   pattern applied: ADR-092 settled the transport leaf, this ADR settles
   the multiplexing layer above it.
2. **Phase 7 fixed the TTY control channel at the TTY layer.** The
   `STREAM_CONTROL = 3` "bidirectional" flaw is fixed by splitting it into
   `STREAM_CTRL_IN = 3` / `STREAM_CTRL_OUT = 4` — *inside TTY's 5-byte
   format*, not at the channels layer. This removed the load-bearing
   reason for the channels layer to carry `stream_type`: the control
   bidirectionality fix is a TTY-internal concern, not a channels-layer
   concern. ADR-077's two-mode TTY design was motivated by the channels
   layer carrying control; with control moved inside TTY, the motivation
   dissolves.
3. **No production constraint.** The develop branch is a rewrite of main
   (pre-alpha). The channels crate doesn't exist yet (per ADR-081, it's
   planned as `alknet-channels-core` + `alknet-channels-call`). The
   decision is purely "what's cleanest," not "what's least disruptive."
   The 9-byte POC validated the per-`channel_id`/`stream_type` routing
   mechanism; the 8-byte spec update changes the header before
   implementation begins.

### What this ADR does NOT decide

- **The add/strip API shape** (built into read/write vs. a separate
  utility): the stream-unification research proposed `add_channel_id` /
  `strip_channel_id` as standalone functions. Ideally the header is
  built into the read/write path so the utility isn't needed at the
  handler boundary — but there may be a generalized reason to expose it
  (recursive composition, test helpers, the hub relay's `channel_id`
  rewrite). The exact API shape is an implementation detail for the
  channels crate, tracked as OQ-68. The *contract* — the channels layer
  strips its 8-byte header on read and the handler parses its own framing
  from the payload — is decided here; the *function surface* is not.
- **TTY's `wire.rs` adaptation:** TTY's `ChunkReader` currently reads from
  an `AsyncRead`. Adapting it to read from a payload buffer (`&[u8]` or
  `Cursor<Bytes>`) is a small, well-scoped change (the framing logic —
  stream_type constants, length validation, control message parsing — is
  unchanged). This is an implementation concern for the channels + TTY
  integration, not an architecture decision.
- **Full channel-level flow-control windowing (OQ-56):** unchanged. The
  bounded-buffer backpressure (ADR-076) is the v1 mechanism; full
  windowing is an additive extension that doesn't change the wire
  format. OQ-56 stays deferred(scope).

## Decision

### 1. The channels wire format is 8 bytes

```
[channel_id: u32 BE][length: u32 BE][payload bytes]
```

8 bytes of header, followed by `length` bytes of opaque payload. The
channels layer owns `channel_id` and `length`; the payload is the
handler's framing, carried transparently.

| field | offset | width | meaning |
|-------|--------|-------|---------|
| `channel_id` | 0 | 4 (BE) | The logical channel this chunk belongs to. Channel 0 is pre-negotiated as `alknet/call` (ADR-072). Channels 1..N are opened dynamically via `channel/open` (ADR-073). |
| `length` | 4 | 4 (BE) | The payload length in bytes. 0 = EOF sentinel. Max `MAX_CHUNK_LEN` (16 MiB, matching TTY's cap — ADR-052 §5). |

The `stream_type` byte is **removed** from the channels header. The
channels layer has no `stream_type` concept — not in its header, not in
its code, not in its mental model. What was the channels header's
`stream_type` byte is now the first byte of the payload, owned by the
handler's framing (TTY's 5-byte format, call's length-prefixed JSON,
tunnel's raw bytes, SSH's channel protocol).

This amends ADR-071: the wire format is 8 bytes, not 9; the
`stream_type` decomposition (mod 3, unidirectional halves, 85 groups) is
removed from the channels layer. The stream_type concept survives in
TTY's 5-byte format (ADR-052, amended by Phase 7), which the channels
layer carries transparently.

### 2. `into_sub_streams()` is removed; `accept_bi` is the only accessor

ADR-074's `into_sub_streams()` / `ChannelSubStreams` / `SubStreamHandle`
are removed. The channels layer exposes one accessor: `accept_bi()`,
which yields one `BiStream` per channel (per ADR-092). Every handler —
TTY, tunnel, SSH, call — receives a `Connection`, calls `accept_bi()`
once, gets a `BiStream`, and sub-multiplexes it however it wants.

This amends ADR-074: the two-accessor design (`accept_bi` for generic
handlers, `into_sub_streams` for typed handlers) collapses to one
accessor. The "typed handler path" (ADR-074's motivating case for TTY) is
replaced by TTY sub-demuxing its `BiStream` via its own 5-byte format —
the same code TTY runs in direct mode. ADR-074's yield-once `accept_bi`
contract is preserved; the `into_sub_streams()` accessor is the amended
part.

### 3. TTY always uses its 5-byte format; the channels layer carries it transparently

ADR-077's two-mode TTY design (direct vs inside-channels) is reversed.
TTY's 5-byte format (`[stream_type:u8][length:u32][payload]`, ADR-052) is
TTY's internal format, used in *both* direct mode and inside-channels
mode. The two modes differ only in *where the `BiStream` comes from*
(a top-level `alknet/tty` connection vs a `channel/open` with ALPN
`alknet/tty`), not in *how TTY parses it*. The same `wire.rs` code runs
in both modes.

When TTY is inside channels, the channels layer strips its 8-byte header
and hands TTY the payload bytes. TTY parses its 5-byte header from the
payload. The channels layer carries TTY's 5-byte chunks transparently
in its payload — no shared fields, no leaked abstraction, no
double-chunking concern (the 13-byte total header is 8 channels + 5
TTY, not 8 + 9; the channels `length` is always `tty_len + 5`).

This reverses ADR-077: the 5-byte format is NOT scoped to direct — it's
TTY's internal format, carried transparently in the channels payload.
The `channels` feature on `alknet-tty` becomes "run TTY's sub-demux on a
channels-backed `BiStream`" — the same code as direct mode, different
`BiStream` source. The control channel split (`STREAM_CTRL_IN` /
`STREAM_CTRL_OUT`, Phase 7) is TTY-internal; the channels layer doesn't
know about it.

### 4. The add/strip composition

The channels layer's read path strips the 8-byte header and hands the
payload to the handler. The write path prepends the 8-byte header
(`add_channel_id`) onto the handler's output. The handler never sees
the `channel_id`; it sees only its own framing (the payload bytes).

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

The composition is uniform — the same shape at every level. A
`alknet/channels`-inside-`alknet/channels` recursive composition is the
outer layer stripping its 8-byte header, the inner layer parsing its own
8-byte header from the payload — same code, same shape, each level.

### 5. What does NOT change

- **ADR-092's `BiStream` leaf** — unchanged. This ADR is the
  channels-layer consequence of ADR-092: `accept_bi` yields a `BiStream`,
  handlers sub-multiplex it. The two ADRs compose (ADR-092 settles the
  transport leaf; this ADR settles the multiplexing layer above it).
- **`ProtocolHandler` trait shape** (ADR-002) — unchanged. Handlers
  receive a `Connection` and call `accept_bi()`.
- **Channel 0 pre-negotiated as `alknet/call`** (ADR-072) — unchanged.
  Channel 0's chunks have `channel_id = 0` in the 8-byte header. The call
  protocol's `EventEnvelope` framing is the payload; the channels layer
  carries it transparently.
- **Channel lifecycle operations** (ADR-073) — unchanged. The four
  operations (`channel/open`/`close`/`control`/`resources/subscribe`) and
  their `direction` semantics are call-protocol operations on channel 0,
  not channels-wire-format concerns.
- **`ChannelsAdapter` / `ChannelManager` split** (ADR-075) —
  structurally unchanged. The demux loop reads 8-byte headers (not
  9-byte); the `ChannelManager` is ALPN-blind, auth-blind,
  transport-blind. The `stream_types` field on `channel/open` and
  `ChannelState` is removed (the channels layer doesn't track
  per-stream-type reassembly buffers; it tracks one reassembly buffer
  per `channel_id`, yielding a `BiStream`).
- **Backpressure, channel limits, ID reuse** (ADR-076) — unchanged. The
  bounded-buffer backpressure is per-`channel_id` (was per-
  `(channel_id, stream_type)`; now per-`channel_id` since there's one
  reassembly buffer per channel). The 256-channel cap, 1 MiB default,
  and monotonic-ID-with-wrap strategy are unchanged.
- **Two-pump shutdown-on-completion** (ADR-078) — unchanged. Tunnel/SSH
  handlers call `tokio::io::split(bidi)` for their two pump halves; the
  shutdown-on-completion contract applies to the `ReadHalf` /
  `WriteHalf` unchanged.
- **Hub relay** (ADR-079) — unchanged in contract. The hub translates
  `channel/open` on channel 0 and byte-forwards data channels with
  `channel_id` rewrite. The relay reads 8-byte headers (not 9-byte) and
  rewrites the `channel_id` field (a 4-byte rewrite within the 8-byte
  header, not a 9-byte header). The relay does not parse the payload.
- **`ChannelClient`** (ADR-080) — unchanged in API. `from_connection`
  primary, `open_channel` returns a `Channel`. The `stream_types` field
  on `open_channel` and `Channel` is removed (the channels layer doesn't
  negotiate per-stream-type sets; the handler owns its sub-stream
  multiplexing). The `channel:stream_type_unavailable` error code is
  removed (the channels layer can't refuse a `stream_type` it doesn't
  know about).
- **Sub-crate decomposition** (ADR-081) — unchanged. `channels-core`
  (pure multiplexer, depends on `alknet-core` only) / `channels-call`
  (channel 0 pre-negotiation + lifecycle op registrations, depends on
  `channels-core` + `alknet-call`). The 8-byte wire format, demux/mux,
  and `ChannelBidiStreamSource` are in `channels-core`; the call-protocol
  coupling is in `channels-call`.
- **`BidiStreamSource` trait** (ADR-070) — unchanged in shape.
  `ChannelBidiStreamSource` implements it; `accept_bi` yields a
  `BiStream` (per ADR-092, already landed).

## Consequences

**Positive:**

- **Clean separation of concerns.** The channels layer has no
  `stream_type` concept — not in its header, not in its code, not in its
  mental model. The handler owns its framing entirely. This dissolves
  the mod 2/mod 3/mod 4 question at the channels layer (there's nothing
  to decompose) and fixes the "control isn't actually bidirectional" TTY
  flaw at the TTY layer (where it lives, not the channels layer).
- **Uniform across all handlers.** Tunnel, call, SSH, and TTY all
  receive the same shape: a `BiStream`. No handler gets a `stream_type`
  byte it doesn't use; no handler needs a second accessor
  (`into_sub_streams`) to reach its sub-streams. The channels layer's
  API surface is `accept_bi -> BiStream`, period.
- **Recursive composition is literal.** A `alknet/channels` channel runs
  another channels demux on its `BiStream`. The outer layer strips its
  8-byte header; the inner layer parses its own 8-byte header from the
  payload. Same code, same shape, each level. This is a property, not a
  feature — the primary use case is one level of multiplexing, but the
  add/strip composition makes the recursion cleaner than ADR-071's
  group framing did.
- **The `into_sub_streams()` accessor and its consuming handler code are
  removed.** This is a net simplification: one accessor, one handler
  path, no downcast / extension trait / "two paths" ergonomics question
  (which ADR-074 left as an implementation detail). The handler crate
  destructures its `BiStream` via its own framing (TTY's 5-byte format),
  not via a channels-crate-provided typed accessor.
- **TTY's `wire.rs` runs unchanged in both modes.** Direct mode and
  inside-channels mode use the same code; only the `BiStream` source
  differs. ADR-077's `drive_session_direct` / `drive_session_channels`
  split collapses to one `drive_session` function. The `channels` feature
  on `alknet-tty` becomes a thin wrapper that gets the `BiStream` from a
  channels-backed `Connection` instead of a top-level one.
- **The channels layer is WASM-compatible by construction.** The 8-byte
  header's core is pure byte manipulation (the sync core compiles under
  `wasm32-unknown-unknown`, validated by the POC). The 8-byte format is
  simpler than the 9-byte (one fewer field to parse), strengthening the
  WASM-clean property.

**Negative:**

- **5 extra bytes per TTY chunk.** The total header for a TTY chunk
  inside channels is 13 bytes (8 channels + 5 TTY), not 9. The two length
  fields are close but not identical (`ch_len = tty_len + 5`). For
  typical TTY chunks (4 KiB+), this is ~0.1% overhead. For extreme
  multiplexing scenarios, the clean separation is worth the trade-off;
  for high-throughput bulk transfer, the escape hatch is multi-connection
  (one channels connection per leg), not stripping the header. This is
  the documented cost of the clean separation; the alternative (9-byte
  header with `stream_type` in the channels layer) carries a concept the
  channels layer doesn't own, which is the root cause this ADR addresses.
- **TTY's `wire.rs` needs a small adaptation.** `ChunkReader` currently
  reads from an `AsyncRead` (the transport stream). Inside channels, it
  reads from a payload buffer (`&[u8]` or `Cursor<Bytes>`) — the bytes
  the channels layer handed it after stripping its 8-byte header. The
  framing logic (stream_type constants, length validation, control
  message parsing) is unchanged. This is a bounded, well-scoped
  implementation change, not an architecture change. The same adaptation
  applies to any handler that parses its own framing from a payload
  buffer (call's `EventEnvelope` framing already reads from a buffer;
  tunnel and SSH don't parse the payload, so no adaptation).
- **`channel/open` loses the `stream_types` field.** ADR-073's
  `channel/open` input included `stream_types: [u8]` (the active sub-stream
  set) and the response echoed the negotiated set. Under this ADR, the
  channels layer doesn't negotiate sub-stream sets — the handler owns
  its sub-stream multiplexing. The `stream_types` field is removed from
  `channel/open` (and from the `channel:stream_type_unavailable` error
  code). The `alpn` and `params` fields remain; the handler's sub-stream
  set is implicit in its ALPN's wire format. This is a small wire-format
  change to `channel/open` (one field removed); since the channels crate
  isn't implemented yet, there's no migration cost.
- **`ChannelState.streams: HashMap<u8, ReassemblyBuffer>` becomes
  `ChannelState.reassembly: ReassemblyBuffer` (one per channel, not per
  `(channel_id, stream_type)`).** This is an internal simplification
  (fewer reassembly buffers, simpler drain logic) but is an
  implementation change, not an architecture one. The bounded-buffer
  backpressure (ADR-076) is per-`channel_id` now, not per-
  `(channel_id, stream_type)` — the 1 MiB default and the 256-channel cap
  are unchanged; the per-channel memory ceiling is 1 MiB (was up to 5 MiB
  for a TTY channel with 5 active stream_types). This is a net
  improvement (lower memory ceiling per channel), not a regression.

## Door type

**One-way (wire format, accessor removal, two-mode reversal).** The 8-byte
chunk header layout (`channel_id:u32 + length:u32`), the removal of
`stream_type` from the channels header, and the removal of
`into_sub_streams()` are wire-format and API commitments. Changing them
after the channels crate is implemented and handlers are written against
them requires a version migration. Since the channels crate doesn't exist
yet, the one-way door is being cast now, before implementation — the
right time to cast a one-way door.

The reversal of ADR-077 (TTY always uses its 5-byte format) is one-way in
the same sense: once TTY's `wire.rs` runs in both modes (direct and
inside-channels), re-introducing a separate inside-channels mode would be
a rewrite of TTY's session driver. The trade is one unified session
driver now vs. two-mode maintenance forever.

The add/strip API shape (OQ-68) is a **two-way door** — whether the
header add/strip is built into the read/write path or exposed as a
standalone utility is an implementation detail that can change without
breaking the wire format or the handler contract.

## References

- ADR-071: channels wire format (amended — wire format is 8 bytes, not
  9; `stream_type` removed from the channels header; the stream_type
  decomposition is removed from the channels layer)
- ADR-074: ChannelBidiStreamSource (amended — `into_sub_streams()`
  removed; `accept_bi` is the only accessor, yields one `BiStream` per
  channel)
- ADR-077: TTY inside channels (reversed — TTY always uses its 5-byte
  format; the channels layer carries it transparently in the payload;
  the two-mode design is preserved but differs only in `BiStream`
  source, not in parsing)
- ADR-092: `BiStream` as the handler leaf (the transport-leaf layer this
  ADR builds on — `accept_bi` returns `BiStream`; `from_bidi` is the only
  public stream constructor)
- ADR-070: `BidiStreamSource` trait (the extension point
  `ChannelBidiStreamSource` implements; `accept_bi` yields `BiStream`)
- ADR-072: channel 0 pre-negotiated `alknet/call` (unchanged — channel 0's
  chunks have `channel_id = 0` in the 8-byte header; the call protocol's
  framing is the payload)
- ADR-073: channel lifecycle operations (amended — `stream_types` field
  removed from `channel/open`; `channel:stream_type_unavailable` error
  code removed)
- ADR-075: `ChannelsAdapter` and `ChannelManager` (structurally
  unchanged — demux reads 8-byte headers; one reassembly buffer per
  channel)
- ADR-076: backpressure, channel limits, ID reuse (unchanged —
  bounded-buffer is per-`channel_id`; 256-channel cap, 1 MiB default,
  monotonic IDs)
- ADR-078: two-pump shutdown-on-completion (unchanged — the contract
  applies to `tokio::io::split(bidi)` halves)
- ADR-079: hub relay (unchanged in contract — 8-byte header, 4-byte
  `channel_id` rewrite, payload byte-forwarded)
- ADR-080: `ChannelClient` (amended — `stream_types` field removed from
  `open_channel` and `Channel`)
- ADR-081: sub-crate decomposition (unchanged — 8-byte wire format in
  `channels-core`; call-protocol coupling in `channels-call`)
- ADR-052: alknet-tty wire format (the 5-byte format carried
  transparently in the channels payload; the control channel split
  from Phase 7 is TTY-internal)
- `docs/research/stream-unification/findings.md` — the research that
  surfaced the structural question and the resolution this ADR commits
- `docs/research/alknet-crate-extraction/findings.md` Phase 8 — the
  spec-cleanup phase this ADR is the substance of
- `/workspace/alknet-channels-poc/` — the POC that validated the
  per-`channel_id`/`stream_type` routing mechanism (the mechanism
  supports any convention; this ADR says the channels layer doesn't have
  a convention, the handler does)