---
status: draft
last_updated: 2026-07-18
---

# stream-unification — Findings: channels as pure channel multiplexing

**Status:** Draft findings, iterating. Per the research-then-sync
pattern, this doc iterates in `docs/research/`; we fix inter-document
drift here, then sync to `docs/architecture/` and the ADRs only after
it settles.

**Scope:** The multiplexing layer — how channels, TTY, and other
handlers compose their framing. This is *above* the transport leaf
(ADR-092, settled) and *below* the channel-protocol layer (ADR-072/
073, settled). The transport leaf (`BiStream` as the handler-facing
duplex type) is settled in ADR-092 and is not re-litigated here.

**Date:** 2026-07-18

---

## TL;DR

The previous framing — "mod 2 vs mod 3 vs mod 4 for the `stream_type`
space within a channel" — was a symptom. The actual question is the
separation of concerns between the channels layer and the handler,
and the resolution is **layered framing via strip/add**: the channels
layer routes by `channel_id` only; handlers own their sub-multiplexing
on the `BiStream` the channels layer gives them. Every channel is a
`BiStream`. The "pass a stream to/from any ALPN" objective becomes
universal, not qualified.

The channels wire format is **8 bytes**: `[channel_id:u32 BE][length:u32 BE]`
followed by an opaque payload. The channels layer owns `channel_id`
and `length`; the payload is the handler's framing, carried
transparently. The channels layer strips its 8-byte header on read,
hands the payload to the handler. The handler parses its own framing
from the payload — TTY's `[stream_type:u8][length:u32 BE][payload]`,
call's length-prefixed JSON, tunnel's raw bytes.

This means the total wire format for a TTY chunk inside channels is:
`[channel_id:u32][ch_len:u32][stream_type:u8][tty_len:u32][payload]`
(13 bytes of header: 8 channels + 5 TTY). The two length fields are
close but not identical (`ch_len = tty_len + 5`). This is a small
amount of waste per chunk, but the trade-off is clean separation:
the channels layer has no `stream_type` concept — not in its header,
not in its code, not in its mental model. The handler owns its
framing entirely. For extreme multiplexing scenarios this clean
separation is worth the few extra bytes.

This dissolves the mod 2/3/4 question at the channels layer (the
channels layer has no `stream_type` concept), fixes the "control
isn't actually bidirectional" TTY flaw at the TTY layer (TTY owns
its sub-streams), and makes recursive composition literal (each
layer strips its header, the inner layer parses its own framing
from the payload).

**No production/backward-compat constraint.** The develop branch is a
rewrite of main (which is labeled pre-alpha). The decision is purely
"what's cleanest," not "what's least disruptive."

---

## Layering (to keep the questions separate)

| Layer | Question | Status |
|-------|----------|--------|
| Transport leaf | What type does `accept_bi` return? How does a handler get a duplex byte stream? | **Settled — ADR-092** (drafted, pushed). `accept_bi` returns `BiStream`; `from_bidi` is the only public stream constructor; the split never crosses a crate boundary as part of a constructor. |
| Multiplexing (this doc) | How do channels, TTY, and other handlers compose their framing? Who owns sub-stream multiplexing? | **In progress — this resolution.** Channels layer routes by `channel_id`; handlers own their sub-multiplexing on the `BiStream` they receive. |
| Channel protocol | How are channels opened/closed? What's channel 0? | **Settled — ADR-072/073.** Channel 0 = `alknet/call` (hardcoded); channels 1..N opened via `channel/open`. Not re-litigated here. |

---

## The structural question (the actual tangle)

The channels layer has two objectives in tension:

1. **"Pass a stream to/from any ALPN"** — every channel is a `BiStream`;
   any handler gets `accept_bi()` and treats the channel as a duplex
   stream. Uniform, transport-agnostic, recursive-composition-friendly.
2. **"Channels carry N sub-streams"** — a TTY channel carries
   stdin/stdout/stderr/control; the handler destructs via
   `into_sub_streams()`. Carries what the source produces.

The tension is real when a sub-stream is *unidirectional* (stderr).
You can't represent stderr as a `BiStream` without wasting the write
half; you can't make it a "third half" (mod 3) without breaking pair
symmetry; you can't make the channel a single `BiStream` without
losing the stdout/stderr distinction.

ADR-074's current design resolves this with two access paths
(`accept_bi` for clean-pair channels, `into_sub_streams` for
multi-stream channels), making the "pass a stream to/from any ALPN"
objective *qualified* — it applies to single-stream channels, not
multi-stream channels. The mod 2/mod 3/mod 4 question was a numbering
symptom of this qualified design.

### The resolution: channels layer is pure channel multiplexing

The channels layer's job is "one connection carries N channels,
routed by `channel_id`." It does not know about TTY's sub-streams,
SSH's channel protocol, or how call frames its JSON. Handlers own
their sub-multiplexing on the `BiStream` the channels layer gives
them.

- **Every channel is a `BiStream`.** `accept_bi()` yields one
  `BiStream` per channel. No `into_sub_streams()`, no second-class
  accessor.
- **Handlers sub-multiplex their `BiStream` however they want.** TTY
  sub-demuxes `stream_type` from its `BiStream` (5-byte format).
  Tunnel uses the `BiStream` as raw bytes. Call length-prefixes JSON.
  SSH runs its own channel protocol. The channels layer carries the
  bytes transparently.
- **The mod 2/mod 3/mod 4 question dissolves at the channels layer.**
  The channels layer has no `stream_type` concept. `stream_type` is
  the inner layer's framing byte, carried transparently.
- **The control channel is handler-internal.** TTY sub-demuxes
  control from its io `BiStream` using its 5-byte format. The channels
  layer doesn't carry control. The "control isn't actually
  bidirectional" flaw is fixed at the TTY layer (stream_type 3 =
  ctrl_in, 4 = ctrl_out), not the channels layer.
- **Recursive composition is literal.** A channel with ALPN
  `alknet/channels` runs another channels demux on its `BiStream`.
  The outer layer strips its 8-byte header; the inner layer parses
  its own 8-byte header from the payload. Each level is the same
  shape — `BiStream → accept_bi → N BiStreams`.

---

## How the wire formats compose (the add/strip insight)

The channels wire format and TTY's wire format compose by layering:

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
This is a small amount of waste per chunk (the channels `length` is
always 5 bytes more than TTY's `length`), but the trade-off is clean
separation of concerns. The channels layer has no `stream_type` concept
— not in its header, not in its code, not in its mental model.

### The add/strip utility

Each layer has its own add/strip pair:

- **Channels layer**: `add_channel_id(channel_id, payload_bytes) -> chunk`
  on write (prepends `[channel_id:u32][len(payload_bytes):u32]`);
  `strip_channel_id(chunk) -> (channel_id, payload_bytes)` on read.
- **TTY layer** (inside the handler): parses its 5-byte header from
  the payload bytes per its existing `wire.rs`. Doesn't know or care
  that a `channel_id` was stripped before it saw the bytes.

The composition is uniform — the same shape at every level. This is
SSH's model (layered headers, each layer strips its own at its
boundary), applied to channels. A `alknet/channels`-inside-
`alknet/channels` recursive composition is the outer layer stripping
its 8-byte header, the inner layer parsing its own 8-byte header from
the payload — same code, same shape, each level.

### What this means for ADR-077's rejection

ADR-077 rejected sub-multiplex inside channels on the grounds of
"double-chunking (5-byte inside 9-byte)." The actual composition under
this resolution is 13 bytes total (8 channels + 5 TTY), not 14. The
two length fields are close but not identical — the channels `length`
is always `tty_len + 5`. This is a small amount of waste per chunk,
but the trade-off is clean separation: the channels layer has no
`stream_type` concept, and the handler owns its framing entirely.

ADR-077's rejection was based on a misunderstanding of how the layers
compose. The add/strip composition makes the channels layer carry the
handler's framing transparently in the payload — no shared fields, no
leaked abstraction. The rejection is reversed by this resolution.

---

## The "merge and split" trap (what was conflated in earlier rounds)

Earlier rounds of this discussion got confused by "can we merge and
split stderr." The vocabulary is now clear:

- **Untagged interleave** (`StreamExt::merge(stdout, stderr)`) — bytes
  arrive interleaved, no way to tell which came from where. The
  distinction is lost. This is "true merge" — what PTY mode does, what
  Docker `Tty: true` does. Once merged this way, stderr is
  unrecoverable. No amount of "interleaved reading" recovers it
  without a tag.
- **Tagged interleave** — each chunk carries a tag (stream_type), so
  the receiver demuxes. This is what the channels layer already is
  (for `channel_id`) and what TTY's sub-multiplex is (for `stream_type`).
  The tag IS the framing.

"Interleaved reading" doesn't avoid stream_type; it IS stream_type.
The channels layer is tagged interleave by construction. The design
question is not "how to avoid stream_type" but "which layer owns
which tag" — and the resolution is "channels layer owns `channel_id`,
handler owns `stream_type`."

---

## Stderr under this resolution

Stderr is a handler concern, not a channels concern. Two cases:

- **PTY mode** (Docker `Tty: true`): the PTY merges stdout and stderr
  into one output stream. The TTY handler sees one output stream,
  sub-demuxes nothing for stderr (there is no stderr). One channel,
  one `BiStream`, no stderr stream_type. Mod 2 at the TTY sub-stream
  level (one pair: in/out). Clean.
- **Pipe mode** (Docker `Tty: false`): the OS gives the handler two
  distinct streams (child.stdout, child.stderr). The TTY handler
  sub-demuxes these onto its 5-byte format's stream_types (0 = in,
  1 = out, 2 = err). The channels layer carries the resulting chunks
  transparently. The "stderr as a unidirectional read" asymmetry is
  inside TTY's sub-stream space, not the channels layer's.

The channels layer never sees stderr. It sees bytes. The TTY handler
owns the stdout/stderr distinction entirely.

---

## The wire format decision: 8 bytes

The channels wire format is **8 bytes**: `[channel_id:u32 BE][length:u32 BE]`
followed by an opaque payload. The channels layer owns `channel_id`
and `length`; the payload is the handler's framing, carried
transparently.

The 9-byte alternative (`[channel_id:u32][stream_type:u8][length:u32]`)
was considered and rejected. The 9-byte format puts `stream_type` in
the channels header, which means the channels layer carries a concept
it doesn't own. For TTY this composes cleanly (the 9-byte header is
TTY's 5-byte header with `channel_id` prepended), but for non-TTY
handlers (tunnel, call, SSH) the `stream_type` byte is dead weight —
the channels layer carries a byte it doesn't understand, and the
handler ignores a byte in a header it doesn't control.

The 8-byte format is uniform across all handlers: the channels layer
carries `channel_id` + `length` + opaque payload. Every handler
parses its own framing from the payload. The cost is that TTY's
`wire.rs` needs to be called from a payload buffer rather than
directly from the wire, and the total header for a TTY chunk is
13 bytes (8 channels + 5 TTY) instead of 9. The two length fields
are close but not identical (`ch_len = tty_len + 5`).

**Why 8 bytes wins:**

- **Clean separation of concerns.** The channels layer has no
  `stream_type` concept — not in its header, not in its code, not in
  its mental model. The handler owns its framing entirely.
- **Uniform across all handlers.** Tunnel, call, SSH, and TTY all
  receive the same shape: a payload buffer. No handler gets a
  `stream_type` byte it doesn't use.
- **The waste is small.** 5 extra bytes per TTY chunk (the channels
  `length` field is always `tty_len + 5`). For typical TTY chunks
  (4 KiB+), this is ~0.1% overhead. For extreme multiplexing
  scenarios, the clean separation is worth the trade-off.
- **TTY's `wire.rs` rework is bounded.** TTY already has a `ChunkReader`
  that reads from an `AsyncRead`. Adapting it to read from a `&[u8]`
  payload buffer (or a `Cursor<Bytes>`) is a small, well-scoped change.
  The framing logic (stream_type constants, length validation, control
  message parsing) is unchanged.

---

## What goes where (ADR plan)

| ADR | Scope | Status |
|-----|-------|--------|
| **ADR-092** | Transport leaf: `BiStream` as the handler leaf; `accept_bi` returns `BiStream`; `from_stream` removed; `from_bidi` is the only public stream constructor. | **Drafted, pushed** (`f8d4650`, `528cfa0`). Load-bearing, separable. |
| **ADR-093** | Channels layer as pure channel multiplexing: routes by `channel_id` only; handlers own sub-multiplexing on the `BiStream` they receive; `into_sub_streams()` removed; every channel is a `BiStream`. The 8-byte wire format (`[channel_id:u32][length:u32][payload]`). The add/strip composition. Amends ADR-071 (channels layer has no `stream_type` concept; wire format is 8 bytes, not 9), ADR-074 (`into_sub_streams` removed, `accept_bi` is the only accessor), ADR-077 (reversed — TTY always uses its 5-byte format; the channels layer carries it transparently in the payload). | **Ready to draft.** |

---

## What changes in each crate

### `alknet-core` (ADR-092's changes, plus this resolution's implications)

- ADR-092's changes: `accept_bi` returns `BiStream`; `from_stream`
  removed; `from_bidi` is the only public stream constructor;
  `SendStream`/`RecvStream` collapse to thin newtypes.
- This resolution doesn't change core beyond ADR-092. `BiStream` is
  the handler leaf; the channels layer yields `BiStream`s; handlers
  parse them per their ALPN. Core is not aware of the layered framing.

### `alknet-channels` (the bulk of this resolution)

- The channels layer routes by `channel_id` only. It has no
  `stream_type` concept — not in its header, not in its code.
- `into_sub_streams()` is removed. `accept_bi` is the only accessor;
  it yields one `BiStream` per channel.
- The wire format is `[channel_id:u32 BE][length:u32 BE][payload]`
  (8-byte header). The payload is opaque to the channels layer; the
  handler parses its own framing from the payload.
- The add/strip utility: `add_channel_id(channel_id, payload_bytes) -> chunk`
  on write (prepends 8-byte header); `strip_channel_id(chunk) -> (channel_id, payload_bytes)`
  on read (strips 8-byte header, returns payload).
- Recursive composition is literal: an `alknet/channels` channel
  runs another channels demux on its `BiStream`. The outer layer
  strips its 8-byte header; the inner layer parses its own 8-byte
  header from the payload.

### `alknet-tty`

- TTY always uses its 5-byte format — direct mode and inside-channels
  mode. The `channels` feature on `alknet-tty` becomes "run TTY's
  sub-demux on a channels-backed `BiStream`" — the same code as
  direct mode, different `BiStream` source.
- The control channel is sub-demuxed by TTY, not the channels layer.
  The "control isn't actually bidirectional" flaw is fixed at the TTY
  layer: stream_type 3 = ctrl_in (write), 4 = ctrl_out (read). TTY's
  `wire.rs` needs updating (the `STREAM_CONTROL = 3` "bidirectional"
  comment and the `InvalidStreamType > 3` bound are the implementation
  lag ADR-077 already specified).
- The 5-byte format is TTY's internal format. When TTY is inside
  channels, the channels layer strips its 8-byte header and hands TTY
  the payload bytes. TTY parses its 5-byte header from the payload.
  TTY's `wire.rs` needs a small adaptation to read from a payload
  buffer (`&[u8]` or `Cursor<Bytes>`) rather than directly from the
  wire, but the framing logic (stream_type constants, length
  validation, control message parsing) is unchanged.
- ADR-077 is reversed: the 5-byte format is NOT scoped to direct —
  it's TTY's internal format, carried transparently in the channels
  payload. The two-mode TTY design (direct vs inside-channels) is
  preserved, but the modes differ only in *where the `BiStream` comes
  from*, not in *how TTY parses it*. The same `wire.rs` code runs in
  both modes.

### `alknet-call`, `alknet-ssh`, etc.

- The call protocol's `EventEnvelope` framing is the inner layer's
  format. The channels layer carries it transparently. No change to
  the call protocol itself.
- SSH runs its own channel protocol on the `BiStream` the channels
  layer gives it. No change to SSH.
- Tunnel uses the `BiStream` as raw bytes. No sub-multiplexing.

---

## The recursive multiplexing property (made cleaner)

A channels connection carries N channels, each a `BiStream`. A
channel with ALPN `alknet/channels` runs another channels demux on
its `BiStream` — the outer layer strips its 8-byte header, the inner
layer parses its own 8-byte header from the payload. Each level is
the same shape: `BiStream → accept_bi → N BiStreams`. The recursion
is unbounded and uniform at every level.

This is a property, not a feature. The primary use case is one level
of multiplexing. But the add/strip composition makes it cleaner than
ADR-071's group framing did — the recursion is the same operation
(strip an 8-byte header) at every level, not a different framing per
level.

---

## Open questions

- **Recursive composition.** Deferred. The add/strip composition
  makes it cleaner, but it's not a goal. Low-leverage POC if ever
  needed.

---

## References

- ADR-092: `BiStream` as the handler leaf (the transport-leaf layer,
  settled; this doc is the layer above it)
- ADR-071: channels wire format (amended by this resolution — the
  channels layer routes by `channel_id` only; wire format is 8 bytes
  (`[channel_id:u32][length:u32][payload]`), not 9; `stream_type` is
  the inner layer's framing, carried transparently in the payload)
- ADR-074: `ChannelBidiStreamSource` / `into_sub_streams` (amended —
  `into_sub_streams` removed; `accept_bi` is the only accessor, yields
  one `BiStream` per channel)
- ADR-077: TTY inside channels (reversed — TTY always uses its 5-byte
  format; the channels layer carries it transparently; the two-mode
  design is preserved but differs only in `BiStream` source, not in
  parsing)
- ADR-072: channel 0 pre-negotiated as `alknet/call` (the hardcoded
  `channel_id` constraint)
- ADR-073: channel lifecycle operations (the `channel/open` operation
  that allocates `channel_id`s)
- `crates/alknet-tty/src/wire.rs:13,32-33,150-151` — the `STREAM_CONTROL
  = 3` "bidirectional" flaw and the `InvalidStreamType > 3` bound
  (implementation lag; fix specified in ADR-077, subsumed by this
  resolution's "TTY owns its sub-streams")
- `/workspace/alknet-channels-poc/src/demux.rs:91-109, 161-181` — the
  demux's per-`stream_type` routing (no pairing assumption; the
  mechanism supports any convention; this resolution says the channels
  layer doesn't have a convention, the handler does)
- `crates/alknet-tty-local/tests/pipe.rs:363 separate_stderr` — the
  test that proves separate stderr is a real feature (resolves as a
  TTY-layer concern, not a channels-layer concern)
- `docs/research/alknet-channels/poc-summary.md` — the channels POC
  (28 tests) that validated the per-`channel_id`/`stream_type`
  routing mechanism the channels layer uses