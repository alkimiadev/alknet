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

The channels wire format composes with TTY's wire format by
construction: the 9-byte channels header is the 5-byte TTY header with
`channel_id:u32` prepended (`[channel_id][stream_type][length][payload]`
= `[channel_id]` + TTY's `[stream_type][length][payload]`). The
channels layer adds `channel_id` on write, strips it on read, hands
the inner 5 bytes to the TTY handler. TTY's `wire.rs` works as-is.
The "double-chunking" objection (ADR-077's reason for rejecting
sub-multiplex inside channels) was about a 14-byte double-header; the
actual composition is 9 bytes total, shared across both layers.

This dissolves the mod 2/3/4 question at the channels layer (the
channels layer has no `stream_type` concept), fixes the "control
isn't actually bidirectional" TTY flaw at the TTY layer (TTY owns
its sub-streams), and makes recursive composition literal (each
layer strips its header, the inner layer adds its own).

**No production/backward-compat constraint.** The develop branch is a
rewrite; no one is using this version yet. If TTY's `wire.rs` needs
rework, it needs rework — no time constraint, no duct tape. If it
doesn't need rework, no need to spend the resource. The decision is
purely "what's cleanest," not "what's least disruptive."

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
  The outer layer strips its `channel_id`; the inner layer adds its
  own. Each level is the same shape — `BiStream → accept_bi → N
  BiStreams`.

---

## How the wire formats compose (the add/strip insight)

The channels wire format and TTY's wire format compose by construction:

```
channels:  [channel_id:u32 BE][stream_type:u8][length:u32 BE][payload]
                        = [channel_id]  +  TTY's [stream_type][length][payload]
                            4 bytes             5 bytes
```

The 9-byte channels header is the 5-byte TTY header with `channel_id`
prepended. The channels layer adds `channel_id` on write, strips it
on read, hands the inner 5 bytes to the TTY handler. TTY's `wire.rs`
parses those 5 bytes exactly as it does today. No double-chunking;
the 9 bytes serve both layers because the `length` prefix is shared
(both layers use it to frame the payload).

### The add/strip utility

Each layer has its own add/strip pair:

- **Channels layer**: `add_channel_id(channel_id, inner_chunk) -> chunk`
  on write; `strip_channel_id(chunk) -> (channel_id, inner_chunk)` on
  read.
- **TTY layer** (inside the handler): parses the inner 5-byte chunk
  per its existing `wire.rs`. Doesn't know or care that a `channel_id`
  was stripped before it saw the bytes.

The composition is uniform — the same shape at every level. This is
SSH's model (layered headers, each layer strips its own at its
boundary), applied to channels. A `alknet/channels`-inside-
`alknet/channels` recursive composition is the outer layer stripping
its `channel_id`, the inner layer adding its own — same code, same
shape, each level.

### What this means for ADR-077's rejection

ADR-077 rejected sub-multiplex inside channels on the grounds of
"double-chunking (5-byte inside 9-byte)." The actual composition is
not 14 bytes — it's 9 bytes, shared. The 5-byte TTY format is the
*inner* format; the 9-byte channels format is the *outer* format; the
outer format's `stream_type`/`length` fields ARE the inner format's
header. There is no double-header, only a prefix.

ADR-077's rejection was based on a misunderstanding of how the layers
compose. The add/strip composition makes the 9-byte channels header
carry TTY's 5-byte header transparently — no waste, no double-chunk.
The rejection is reversed by this resolution.

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

## The one open question: 8 bytes vs 9 bytes

The channels wire format can be either:

- **9 bytes** (`[channel_id:u32][stream_type:u8][length:u32][payload]`):
  preserves TTY's 5-byte format exactly (strip 4 bytes → TTY's native
  format). The `stream_type` byte is the inner layer's framing,
  carried transparently by the channels layer. Non-TTY inner layers
  (tunnel, call) carry a `stream_type` byte they don't use (set to 0,
  ignored by the inner layer, or simply not parsed). 1 byte of
  overhead per chunk for non-TTY inner layers.
- **8 bytes** (`[channel_id:u32][length:u32][payload]`): the channels
  layer carries only `channel_id` + `length`. The inner layer's
  framing is entirely within the payload. No unused byte for non-TTY
  inner layers. But TTY's `wire.rs` doesn't compose by stripping —
  TTY's 5-byte format would need rewriting to fit inside the 8-byte
  payload (the length prefix position changes).

**The 9-byte composition is elegant because the strip is literal** —
the channels layer reads 9 bytes, hands the inner 5 to TTY, TTY parses
them as its native format. TTY's `wire.rs` works unchanged.

**The 8-byte composition is more uniform across inner layers** — the
channels layer doesn't carry a `stream_type` byte it doesn't know
about. But TTY's format needs rewriting, and the layered composition
is no longer "strip a prefix" but "parse the inner framing from the
payload."

**The decision is "what's cleanest," not "what's least disruptive"**
(no production constraint, no backward-compat). Considerations:

- 9 bytes: preserves TTY's `wire.rs` as-is. The strip/add is literal.
  The cost is 1 byte per chunk for non-TTY inner layers (tunnel,
  call), where the `stream_type` byte is unused.
- 8 bytes: the channels layer is uniform — it carries only
  `channel_id` + `length`, nothing else. The cost is TTY's `wire.rs`
  needs rewriting (the length prefix position changes; TTY's 5-byte
  format doesn't compose by stripping).

My read: 9 bytes. The 1-byte overhead for non-TTY is noise (chunks
are not tiny for tunnel/call). The literal strip/add is the elegant
property that makes the layered composition uniform — every layer's
header is a prefix, strip the prefix → inner layer's format. The 8-
byte option breaks the strip-prefix property and forces TTY to
re-derive its framing from the payload. But this is your call —
you know the use cases and the chunk-size tradeoffs.

---

## What goes where (ADR plan)

| ADR | Scope | Status |
|-----|-------|--------|
| **ADR-092** | Transport leaf: `BiStream` as the handler leaf; `accept_bi` returns `BiStream`; `from_stream` removed; `from_bidi` is the only public stream constructor. | **Drafted, pushed** (`f8d4650`, `528cfa0`). Load-bearing, separable. |
| **ADR-093** | Channels layer as pure channel multiplexing: routes by `channel_id` only; handlers own sub-multiplexing on the `BiStream` they receive; `into_sub_streams()` removed; every channel is a `BiStream`. The add/strip composition. Amends ADR-071 (channels layer has no `stream_type` concept), ADR-074 (`into_sub_streams` removed, `accept_bi` is the only accessor), ADR-077 (reversed — TTY always uses its 5-byte format, the channels layer carries it transparently). | **Ready to draft.** The structural question is resolved; the one open sub-question is 8 vs 9 bytes (below). |
| **ADR-094** | (Optional) The 8-vs-9-byte decision, if it warrants a separate ADR. Or folded into ADR-093 if the decision is straightforward. | **Pre-ADR**, drafting with or after ADR-093. |

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

- The channels layer routes by `channel_id` only. `stream_type` is
  the inner layer's framing, carried transparently. The channels
  layer has no `stream_type` concept.
- `into_sub_streams()` is removed. `accept_bi` is the only accessor;
  it yields one `BiStream` per channel.
- The wire format is `[channel_id][...inner layer's framing...]
  [payload]`. The inner layer's framing is whatever the handler
  expects (TTY's 5-byte, call's length-prefix, tunnel's raw bytes).
- The add/strip utility: `add_channel_id` on write,
  `strip_channel_id` on read.
- Recursive composition is literal: an `alknet/channels` channel
  runs another channels demux on its `BiStream`. The outer layer
  strips its `channel_id`; the inner layer adds its own.

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
- The 5-byte format is the inner format. The channels layer (when
  TTY is inside channels) strips its `channel_id` and hands TTY the
  inner 5 bytes. TTY's `wire.rs` parses them as its native format.
- ADR-077 is reversed: the 5-byte format is NOT scoped to direct —
  it's TTY's internal format, carried transparently by the channels
  layer. The two-mode TTY design (direct vs inside-channels) is
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
its `BiStream` — the outer layer strips its `channel_id`, the inner
layer adds its own. Each level is the same shape: `BiStream →
accept_bi → N BiStreams`. The recursion is unbounded and uniform at
every level.

This is a property, not a feature. The primary use case is one level
of multiplexing. But the add/strip composition makes it cleaner than
ADR-071's group framing did — the recursion is the same operation
(strip a prefix) at every level, not a different framing per level.

---

## Open questions

- **8 bytes vs 9 bytes.** The one open sub-question. 9 bytes
  preserves TTY's `wire.rs` via literal strip/add; 8 bytes is more
  uniform across inner layers but requires rewriting TTY's format.
  Default assumption: 9 bytes (the strip/add property is the elegant
  one). Decision is "what's cleanest" — no production constraint.
- **Recursive composition.** Deferred. The add/strip composition
  makes it cleaner, but it's not a goal. Low-leverage POC if ever
  needed.

---

## References

- ADR-092: `BiStream` as the handler leaf (the transport-leaf layer,
  settled; this doc is the layer above it)
- ADR-071: channels wire format (amended by this resolution — the
  channels layer routes by `channel_id` only; `stream_type` is the
  inner layer's framing, carried transparently)
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