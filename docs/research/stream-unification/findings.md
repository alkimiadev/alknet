---
status: draft
last_updated: 2026-07-18
---

# stream-unification — Findings: the stream_type convention and the TTY/channels convergence

**Status:** Draft findings, iterating. Per the research-then-sync
pattern, this doc iterates in `docs/research/`; we fix inter-document
drift here, then sync to `docs/architecture/` and the ADRs only after
it settles.

**Scope:** The multiplexing layer — the `stream_type` space within a
channel. This is *above* the transport leaf (ADR-092, settled) and
*below* the channel-protocol layer (ADR-072/073). The transport leaf
(`BiStream` as the handler-facing duplex type) is settled in ADR-092
and is not re-litigated here.

**Date:** 2026-07-18

---

## Layering (to keep the questions separate)

| Layer | Question | Status |
|-------|----------|--------|
| Transport leaf | What type does `accept_bi` return? How does a handler get a duplex byte stream? | **Settled — ADR-092** (drafted, pushed). `accept_bi` returns `BiStream`; `from_bidi` is the only public stream constructor; the split never crosses a crate boundary as part of a constructor. |
| Multiplexing (this doc) | How are `stream_type` values assigned within a channel? What's the addressing convention? | **In progress.** ADR-071's mod-3 group framing vs the proposed mod-2/mod-4 instance framing. |
| Channel protocol | How are channels opened/closed? What's channel 0? | **Settled — ADR-072/073.** Channel 0 = `alknet/call` (hardcoded); channels 1..N opened via `channel/open`. Not re-litigated here. |

The two findings-doc questions that got conflated in the previous
draft, separated:

- **Transport-leaf split/recombine** ("can `tokio::io::join`/`split`
  recombine halves from different sources?") — ADR-092's layer.
  Answer: yes, stdlib `tokio::io::join`/`split` is a pure type
  combinator; the halves don't need to come from the same source.
  Settled; not this doc's concern.
- **Stream_type split/recombine** ("if stderr is its own instance
  with an unused write half, does the unused half cause problems?")
  — this doc's layer. Answer (verified in the existing POC code,
  `alknet-channels-poc/src/demux.rs:91-109, 161-181` and
  `mux.rs:58-63, 152-177`): the demux/mux is per-`stream_type`
  independent — there is **no pairing assumption** in the mechanism.
  An unused `stream_type` is an idle mpsc channel, not a wart. The
  mod-2 framing is trivially clean. **No POC needed** — the
  load-bearing property is already proven by the 28-test POC.

---

## The core question: stream_type assignment convention

The mechanism (per-`stream_type` independent demux/mux, validated by
the channels POC) routes by `(channel_id, stream_type)` regardless of
convention. The convention choice — *how stream_type values are
assigned to roles* — is a documentation/ergonomics choice, not a
mechanism choice. Both conventions work; the question is which is
cleaner.

### ADR-071's current convention: mod-3 groups

| Group | stream_type | direction | purpose |
|-------|-------------|-----------|---------|
| Data | 0 | write | in (stdin) |
| | 1 | read | out (stdout) |
| | 2 | read | err (stderr) |
| Control | 3 | write | ctrl_in |
| | 4 | read | ctrl_out |
| | 5 | read | ctrl_err (optional) |
| Future | 6/7/8, 9/10/11, ... | write/read/read | next groups |

Formula: `stream_type % 3 == 0` → write, `% 3 == 1` → read,
`% 3 == 2` → diagnostic read (err).

**The flaw: stderr is structural.** The `+2 = err` slot bakes a
*unidirectional* role (stderr is server→client only) into a
*bidirectional* group structure (in/out/err). Every group either
allocates an err slot it may not use (tunnel has no err) or skips the
group structure. The group concept (data vs control) is real, but the
3-slot shape is asymmetric — bidirectional things are two halves, but
the group has three slots, one of which is a unidirectional leaf.

### Proposed convention: mod-2/mod-4 by instance

An **instance** is a self-contained bidirectional unit, addressed as
a contiguous block of `stream_type` values within a channel.

**No control channel** — mod 2:

| Instance | in (write) | out (read) |
|----------|-----------|-----------|
| 0 | 0 | 1 |
| 1 | 2 | 3 |
| 2 | 4 | 5 |
| ... | | |
| 127 | 254 | 255 |

128 instances per channel. Instance K uses `stream_type = 2K` (in)
and `2K+1` (out).

**With control channel** — mod 4:

| Instance | in (write) | out (read) | ctrl_in (write) | ctrl_out (read) |
|----------|-----------|-----------|-----------------|-----------------|
| 0 | 0 | 1 | 2 | 3 |
| 1 | 4 | 5 | 6 | 7 |
| 2 | 8 | 9 | 10 | 11 |
| ... | | | | |
| 63 | 252 | 253 | 254 | 255 |

64 instances per channel. Instance K uses `4K` (in), `4K+1` (out),
`4K+2` (ctrl_in), `4K+3` (ctrl_out).

**The `stream_type` space is the instance address space, not a role
space.** A handler talks about "instance K of my channel" as a
contiguous block of stream_types. This is richer than ADR-071's
"data group / control group" framing — the instance is the addressing
unit, and a handler can have N independent bidirectional sub-streams
on one channel, each with or without its own control.

**Combined address space:** `channel_id × instance`. Channel 0 is
hardcoded `alknet/call` (ADR-072), so channels 1..255 are dynamic —
~255 × 128 (no control) or ~255 × 64 (with control) logical
sub-streams per channels connection. Huge; the load-bearing count
for the recursive-multiplexing property below.

### Why mod 2 wins

1. **Uniformity.** Every bidirectional thing is exactly two
   stream_types (write, read). Control is a pair; io is a pair;
   future sub-streams are pairs. No "err is a third half" asymmetry.
2. **The "unused write half" wart is trivially clean** (verified in
   the existing POC code — see "Stream_type split/recombine" above).
   An unused `stream_type` is an idle mpsc channel: no dangling
   sender, no premature EOF (EOF fires on sender drop or zero-length
   sentinel), no flow-control weirdness (independent bounded
   buffers). The lenient unknown-`stream_type` path
   (`demux.rs:161-174`) means even a stray write to an unallocated
   `stream_type` is dropped with a counter bump, not a panic.
3. **Stderr is just another instance.** PTY mode (stderr merged into
   stdout server-side) declares instance 0 only. Pipe mode (separate
   stderr) declares instance 0 (io) + instance 1 (stderr-as-bidirectional,
   write half unused). The unused write half costs one idle mpsc
   channel; cheap. No structural asymmetry.
4. **The demux/mux doesn't care.** The convention is documentation;
   the mechanism routes by `(channel_id, stream_type)` regardless.
   Both conventions work; mod 2 is the cleaner documentation.

**The mod-2-vs-mod-3 question is settled by the existing POC
evidence.** No new POC needed. The mechanism supports both; mod 2
wins on uniformity, and the "unused write half" property that made
mod 2 look like a wart is trivially clean in the actual code.

---

## The TTY control channel flaw (separate, already specified)

`crates/alknet-tty/src/wire.rs:13,32-33`:
```
STREAM_CONTROL: u8 = 3   // "bidirectional, JSON control message"
InvalidStreamType > 3
```

One `stream_type` both sides write to — the "control isn't actually
bidirectional" flaw. ADR-071 §stream_type decomposition already fixes
this at the wire-format level (stream_types 3 = ctrl_in, 4 = ctrl_out);
ADR-077 amends ADR-052 to add stream_type 4. The TTY code hasn't been
updated — it still has the old "bidirectional 3" comment and the `> 3`
bound.

**This is an implementation-lag, not a design question.** The fix is
specified; the code needs to catch up. The mod-2/mod-4 instance
framing subsumes this fix: control becomes instance 0's ctrl_in/ctrl_out
pair (types 2/3 under mod 4), not a standalone "bidirectional" stream_type.

---

## TTY/channels convergence

Channels was written after TTY as a natural extension: TTY's 5-byte
header + `channel_id:u32` prefix = the 9-byte channels header. The
convergence has two parts.

### Semantic convergence (independent of format)

Regardless of format choice, TTY and channels present the same shape
to handlers: a set of unidirectional sub-streams declared per-channel
at `channel/open` time, paired into bidirectional instances where the
handler wants a joined `BiStream`. `into_sub_streams()` (ADR-074)
returns the declared stream_types as `Vec<(u8, SubStreamHandle)>` with
`SubStreamHandle::Send` (write half) or `Recv` (read half). The
handler joins the pairs it wants via `tokio::io::join`. The channels
layer exposes the leaves; the handler composes them.

ADR-074's two paths (`accept_bi` for the joined 0/1 pair,
`into_sub_streams` for the typed leaves) become the same data,
presented differently. `accept_bi` is a convenience that joins 0/1 for
the common case (tunnel, SSH, echo, HTTP); `into_sub_streams` is the
primary accessor (the unidirectional leaves).

### Format convergence (the open question)

**Option A (current, ADR-077): two formats coexist.** TTY-direct keeps
the 5-byte format; TTY-inside-channels uses 9-byte. The 4-byte
`channel_id` overhead is paid only inside channels.

**Option B (retire the 5-byte format): TTY-direct uses 9-byte.** One
demux implementation, one set of stream_type semantics, one crate.
Sub-options for the `channel_id`:
- **B1:** TTY-direct is a channels connection with `channel_id = 0` =
  `alknet/tty` (breaks ADR-072's "channel 0 is call" rule).
- **B2:** TTY-direct is a channels connection with `channel_id = 0` =
  `alknet/call` (unused, no `channel/open` issued) + `channel_id = 1` =
  `alknet/tty` (pre-allocated). Pays one unused channel for rule
  consistency. Channel 0 is *always* call, even when unused.

**Option B is the bigger call.** It's a wire-format change with
backward-compat implications. It's the one open question that benefits
from a POC — see below.

---

## The recursive multiplexing property

A channels connection with N channels, each with up to 128 (or 64)
instances, has N×128 (or N×64) logical sub-streams. An instance can
itself be a channels connection (recursive composition, ADR-074 — a
channel with ALPN `alknet/channels` inside `alknet/channels`). The
"channel within a channel within a channel" shape is unbounded, and
the mod-2/mod-4 instance framing makes each level uniform — every
level is "pairs of stream_types (or 4-tuples with control), declared
per-channel, addressed by instance."

This is a property, not a feature. The primary use case is one level
of multiplexing. Recorded here because the instance framing makes it
cleaner than ADR-071's group framing did — the instance is the
recursive unit; the group was not.

---

## POC candidates

### POC 1: stderr split/recombine — **already answered, no POC needed**

**Original framing:** "If stderr is its own instance (mod 2, write
half unused), does the unused write half cause problems — dangling
sender, premature EOF, flow-control weirdness?"

**Answer (verified in existing POC code):** No. The demux/mux is
per-`stream_type` independent — there is no pairing assumption in the
mechanism. An unused `stream_type` is an idle mpsc channel. The
lenient unknown-`stream_type` path drops stray writes with a counter
bump. The 28-test channels POC already validated per-`stream_type`
independence (`demux_three_concurrent_channels_no_cross_contamination`,
`demux_unknown_channel_drops_lenient`, `recv_dropped_sender_is_eof`).

**Status:** Confirmatory, not exploratory. The load-bearing property
is already proven. The mod-2 framing is trivially clean. No new POC.

### POC 2: TTY-direct-as-channels — the one open question

**Question:** Is Option B1 (TTY-direct = channels with `channel_id = 0`
= `alknet/tty`, breaking ADR-072) or Option B2 (channel 0 = call
unused, channel 1 = TTY) feasible and clean? Or is the 4-byte
`channel_id` overhead per chunk in TTY-direct mode a real concern
for terminal I/O? And is backward-compat with existing TTY-direct
deployments a real constraint, or are there no existing deployments
to break?

**Why it matters:** If Option B is clean and backward-compat is
non-binding, the 5-byte format retires, the demux implementation
unifies, and the TTY crate's direct mode becomes a thin wrapper. If
backward-compat is binding or the overhead is a concern, Option A
(two formats coexist per ADR-077) stands.

**Scope:** ~200 lines. A POC that runs the TTY adapter over a
channels-format demux with `channel_id = 0` (B1) and `channel_id = 1`
(B2), using the existing TTY `TtyBackend` (the local pipe backend).
Verify the `pty.rs` / `pipe.rs` tests still pass in shape. Measure
chunk overhead if relevant.

**Output:** A `docs/research/alknet-channels/poc-tty-direct-as-channels.md`
summary. Recommends Option A, B1, or B2. If B, becomes the input to
ADR-094 (retire the 5-byte format). If A, ADR-077 stands and the
5-byte format is kept.

### POC 3: recursive composition — low leverage, deferred

**Question:** Does `alknet/channels` inside `alknet/channels` work
end-to-end? The abstraction permits it (ADR-074); the POC didn't
validate it (`poc-summary.md` §"What the POC Does NOT Validate" #5).

**Why it matters:** Low leverage — the instance framing makes it
cleaner, but the primary use case is one level. Recursive composition
is a property, not a feature.

**Status:** Deferred. Not a POC for this round.

---

## What goes where (ADR plan)

| ADR | Scope | Status |
|-----|-------|--------|
| **ADR-092** | Transport leaf: `BiStream` as the handler leaf; `accept_bi` returns `BiStream`; `from_stream` removed; `from_bidi` is the only public stream constructor. | **Drafted, pushed** (`f8d4650`, `528cfa0`). Load-bearing, separable. |
| **ADR-093** | Multiplexing: mod-2/mod-4 instance framing; per-channel stream_type declaration; `into_sub_streams()` as the primary accessor; demux convention-agnostic. Amends ADR-071 (stream_type decomposition), ADR-074 (`into_sub_streams`), ADR-077 (TTY uses mod 2; control channel fixed). | **Ready to draft.** The mod-2-vs-mod-3 question is settled by existing POC evidence; the control channel fix is already specified in ADR-077; the instance framing is the cleaner documentation. |
| **ADR-094** | Format convergence: retire the 5-byte TTY-direct format in favor of 9-byte channels format. Option A (keep 5-byte), B1 (channel 0 = TTY), or B2 (channel 0 = call unused, channel 1 = TTY). Backward-compat analysis. | **Pre-ADR**, drafting after POC 2 recommends an Option. |

---

## Open questions

- **5-byte format retirement.** POC 2 resolves. Default assumption:
  Option A (two formats coexist, ADR-077 stands) is safer; Option B
  (retire 5-byte) is cleaner but a wire-format change. POC validates
  feasibility and overhead.
- **`channel 0 is always call` (ADR-072) under TTY-direct-as-channels.**
  If POC 2 recommends B1 (TTY-direct channel 0 = `alknet/tty`),
  ADR-072 needs an exception or amendment. If B2 (channel 0 = call
  unused, channel 1 = TTY), ADR-072 stands. If A (keep 5-byte), no
  change.
- **Recursive composition.** Deferred. The instance framing makes it
  cleaner, but it's not a goal. POC 3 is low leverage.

---

## References

- ADR-092: `BiStream` as the handler leaf (the transport-leaf layer,
  settled; this doc is the layer above it)
- ADR-071: channels wire format (the mod-3 stream_type decomposition
  ADR-093 amends)
- ADR-074: `ChannelBidiStreamSource` / `into_sub_streams` (the
  per-channel sub-stream accessor ADR-093 amends)
- ADR-077: TTY inside channels (the two-mode TTY design ADR-093
  amends; the 5-byte format scoping ADR-094 may retire)
- ADR-072: channel 0 pre-negotiated as `alknet/call` (the hardcoded
  channel_id constraint)
- ADR-073: channel lifecycle operations (the `stream_types` field
  declared at `channel/open` time — the per-channel declaration that
  makes the demux convention-agnostic)
- `docs/research/alknet-channels/poc-summary.md` — the channels POC
  (28 tests) that validated per-`stream_type` independence
- `/workspace/alknet-channels-poc/src/demux.rs:91-109, 161-181` —
  the demux's per-`stream_type` routing (no pairing assumption)
- `/workspace/alknet-channels-poc/src/mux.rs:58-63, 152-177` — the
  mux's per-`(channel_id, stream_type)` independent pumps
- `crates/alknet-tty/src/wire.rs:13,32-33` — the `STREAM_CONTROL = 3`
  "bidirectional" flaw (implementation lag; fix specified in ADR-077,
  subsumed by the mod-4 instance framing)
- `crates/alknet-tty-local/tests/pipe.rs:363 separate_stderr` — the
  test that proves separate stderr is a real feature, not vestigial
  (resolves under mod 2 as instance 1 with unused write half)