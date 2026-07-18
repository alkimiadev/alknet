---
status: draft
last_updated: 2026-07-18
---

# stream-unification — Findings: the leaf, the instance, the convergence

**Status:** Draft findings, iterating. This doc is the working scratch
for the stream/multiplexing redesign that surfaced during the
`alknet-crate-extraction` Phase 6 deep dive. It is **not** synced to the
architecture specs — per the research-then-sync pattern, we iterate
here, fix inter-document drift, and only then sync to
`docs/architecture/` and the ADRs. ADR-092 (`BiStream` as the handler
leaf) has been drafted and pushed because it's load-bearing and
separable; everything else in this doc is pre-ADR and will produce
ADR-093 (the multiplexing/stream_type redesign) and possibly ADR-094
(retire the 5-byte TTY-direct format) once it settles.

**Date:** 2026-07-18
**Scope:** (a) the transport-leaf unification (ADR-092, drafted); (b)
the stream_type / multiplexing redesign that ADR-092 enables; (c) the
TTY/channels convergence; (d) the POC candidates that would take
guesswork out before committing.

---

## TL;DR

The deep dive surfaced a layered tangle:

1. **Transport leaf is split.** `accept_bi` returns `(SendStream,
   RecvStream)`; every handler that wants a duplex stream re-joins
   (HTTP's `QuicStream`, 44 lines) or bypasses (WS's `WsStream`).
   Five abstractions exist for "a bidirectional byte stream."
   **Resolved by ADR-092** (drafted, pushed): `accept_bi` returns one
   `BiStream`; the join moves into core's quinn/iroh impls once; the
   split never crosses a crate boundary as part of a constructor
   (`Connection::from_stream` removed, `from_bidi` is the only public
   stream constructor).

2. **The control channel "isn't actually bidirectional"** in the
   current TTY code (`wire.rs:13,32-33`: `STREAM_CONTROL = 3` is one
   stream_type both sides write to). ADR-071's mod-3 stream_type
   decomposition already fixes this at the wire-format level
   (stream_types 3 = ctrl_in, 4 = ctrl_out), but the TTY code hasn't
   been updated. ADR-077 amends ADR-052 to add stream_type 4, but the
   implementation lags.

3. **ADR-071's mod-3 stream_type decomposition is structural but
   asymmetric.** "Data group 0/1/2 = in/out/err; control group 3/4/5 =
   ctrl_in/ctrl_out/ctrl_err" bakes stderr into the group structure.
   The cleaner framing is **mod 2 by instance**: an *instance* is a
   self-contained bidirectional unit, addressed as a contiguous block
   of stream_types. No control: instance K uses `2K`/`2K+1` (128
   instances/channel). With control: instance K uses
   `4K`/`4K+1`/`4K+2`/`4K+3` (64 instances/channel). The stream_type
   space is the *instance address space*, not a *role* space. This is
   richer than "control is just another pair" — the instance is the
   addressing unit.

4. **TTY and channels should converge on one format.** Channels was
   written after TTY and was a natural extension — TTY's 5-byte header
   + a `channel_id:u32` prefix = the 9-byte channels header. TTY
   should be rewritten to use the channels format. The one hardcoded
   `channel_id` is channel 0 = `alknet/call` (ADR-072); everything
   else is dynamic. Whether TTY-direct retires the 5-byte format
   entirely (using 9-byte with `channel_id = 0` or `channel_id = 1`)
   is a bigger call — backward compat — flagged as a POC candidate
   and a possible ADR-094.

5. **The recursive multiplexing property follows.** A channels
   connection with N channels, each with up to 128 (or 64) instances,
   has N×128 (or N×64) logical sub-streams. Combined address space
   (channel_id × instance) is ~255×128 or ~255×64. An instance can
   itself be a channels connection (recursive composition, ADR-074).
   The "channel within a channel within a channel" shape is unbounded,
   and the mod-2/mod-4 instance framing makes each level uniform.

**POC candidates** (ordered by leverage): stderr split/recombine
(load-bearing for the mod-2 vs mod-3 decision); TTY-direct-as-channels
(load-bearing for the format-convergence decision); recursive
composition (low leverage — already implied by the abstraction, just
needs validation).

---

## The tangle, restated

Five abstractions for "a bidirectional byte stream" before ADR-092:

| # | Abstraction | Where | Status |
|---|-------------|-------|--------|
| 1 | `BiStream` trait (`AsyncRead + AsyncWrite + Send + Unpin`) | `crates/alknet-core/src/types.rs:226` | Vestigial in code — zero consumers. Resurrected by ADR-092 as a concrete newtype. |
| 2 | `Connection` (yields `(SendStream, RecvStream)` via `accept_bi`) | `crates/alknet-core/src/types.rs:507` | Handler-facing. Leaf split. ADR-092 changes `accept_bi` to return `BiStream`. |
| 3 | `SendStream` (AsyncWrite-only) + `RecvStream` (AsyncRead-only) | `crates/alknet-core/src/types.rs:228-294` | Quinn-welded enums. ADR-092 collapses to thin newtypes, retained for `into_sub_streams()`. |
| 4 | `WsStream` trait (recv/send `axum::ws::Message`) | `crates/alknet-http/src/websocket/upgrade.rs:44` | Bypasses `Connection`. ADR-092 replaces with `WsBidiStream` through `Connection::from_bidi`. |
| 5 | `MpscSendStream` / `MpscRecvStream` (channels POC) | `/workspace/alknet-channels-poc/src/mpsc_stream.rs` | Split mpc-backed halves. ADR-092: channels crate joins via `tokio::io::join` before `from_bidi`. |

The two `findings.md` Phase 6 issues (QuicStream wrapper, WS bespoke
dispatch) are symptoms of the split leaf. ADR-092 addresses the
transport seam; this doc addresses the multiplexing seam that ADR-092
exposes.

---

## ADR-092 recap (what's settled)

- `accept_bi` / `open_bi` return `BiStream`, not `(SendStream,
  RecvStream)`.
- `BiStream` is a concrete newtype (internal `Box<dyn AsyncReadWrite +
  Send + Unpin>`), not the bare ADR-007 trait (which is removed; the
  bounds survive as implied bounds).
- The join moves into core's quinn/iroh `BidiStreamSource` impls once
  (via `tokio::io::join` or equivalent).
- `Connection::from_bidi` is the only public stream constructor.
  `Connection::from_stream(send, recv, ...)` is removed — the split
  never crosses a crate boundary as part of a constructor.
- `SendStream` / `RecvStream` collapse to thin newtypes over
  `Box<dyn Async* + Send + Unpin>`, retained only for `into_sub_streams`
  (ADR-074) and the channels reassembly path's `SubStreamHandle` leaves.
- `HttpAdapter::handle` drops `QuicStream` (44 lines) and
  `QuicStreamDuplex` (38 lines).
- WS runs through `Connection::from_bidi(WsBidiStream)` + the
  call-protocol handler; `WsStream` trait, `drive_ws_session` loop,
  and ~150 lines of dispatch glue removed.
- "VPN-like without being a VPN" over WS in v1 becomes real
  (`webtransport.md`'s path, over WS).
- WebTransport h3 extraction is recorded as a future channels-variant
  move enabled by the unification (out of scope per ADR-044).

ADR-092 is pushed (`f8d4650`, amended `528cfa0`). It is load-bearing
and separable from the multiplexing redesign below.

---

## What ADR-092 exposes: the multiplexing seam

With the transport leaf unified, the remaining asymmetry is in the
*stream_type* space — the multiplexing layer above the transport.

### ADR-071's mod-3 decomposition (current)

ADR-071 groups stream_types in threes:

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
`% 3 == 2` → diagnostic read.

**The flaw: stderr is structural.** The "err is the third half"
bakes stderr into the group shape. The "group" concept is real (data
vs control) but the `+2 = err` slot is asymmetric — a bidirectional
thing is two halves, but the group has three slots, one of which is
"err." This forces every group to either allocate an err slot it may
not use (tunnel has no err) or skip the group structure entirely.

### The control channel flaw in the TTY code

`crates/alknet-tty/src/wire.rs:13,32-33`:
```
STREAM_CONTROL: u8 = 3   // "bidirectional, JSON control message"
InvalidStreamType > 3
```

One stream_type both sides write to. That's the "control isn't
actually bidirectional" flaw ADR-071 §stream_type decomposition calls
out: *one* stream both sides write loses independent flow control,
independent EOF, and clean separation. ADR-077 amends ADR-052 to add
stream_type 4 (ctrl_out), splitting control into 3 (in) + 4 (out).
The TTY code hasn't been updated — it still has the old "bidirectional
3" comment and the `> 3` bound. The update is needed regardless of
the mod-2/mod-3 decision.

### The implicit split (the key insight)

Bidirectionality is *always* two unidirectional halves. There is no
"bidirectional stream_type" — bidirectionality is two stream_types
(write, read), the same way QUIC bidi streams are two unidirectional
halves, the same way `tokio::io::split(BiStream)` is two halves, the
same way `SendStream`/`RecvStream` after ADR-092 is two halves. The
stream_type space is *inherently* unidirectional; the bidirectional
thing is the pair.

This means: TTY and channels present the same shape to their
handlers. A set of unidirectional sub-streams, paired into
bidirectional channels where the handler wants a joined pair. TTY
wants five unidirectional leaves (`stdin`, `stdout`, `stderr`,
`ctrl_in`, `ctrl_out`); the tunnel wants two paired into one
`BiStream` (`tokio::io::join(send_0, recv_1)`). The handler chooses
how to compose the leaves; the channels layer just exposes them.

---

## The instance framing (mod 2 / mod 4)

An **instance** is a self-contained bidirectional unit, addressed as
a contiguous block of stream_types within a channel.

**No control channel** — mod 2:

| Instance | in (write) | out (read) |
|----------|-----------|-----------|
| 0 | 0 | 1 |
| 1 | 2 | 3 |
| 2 | 4 | 5 |
| ... | | |
| 127 | 254 | 255 |

128 instances per channel. Instance K uses stream_types `2K` (in) and
`2K+1` (out).

**With control channel** — mod 4:

| Instance | in (write) | out (read) | ctrl_in (write) | ctrl_out (read) |
|----------|-----------|-----------|-----------------|-----------------|
| 0 | 0 | 1 | 2 | 3 |
| 1 | 4 | 5 | 6 | 7 |
| 2 | 8 | 9 | 10 | 11 |
| ... | | | | |
| 63 | 252 | 253 | 254 | 255 |

64 instances per channel. Instance K uses stream_types `4K` (in),
`4K+1` (out), `4K+2` (ctrl_in), `4K+3` (ctrl_out).

**The stream_type space is the instance address space, not a role
space.** A handler talks about "instance K of my channel" as a
contiguous block of stream_types. This is richer than ADR-071's "data
group / control group" framing — the instance is the addressing unit,
and the handler can have N independent bidirectional sub-streams on
one channel, each with or without its own control.

**Combined address space:** channel_id × instance. Channel 0 is
hardcoded as `alknet/call` (ADR-072), so channels 1..255 are dynamic
— ~255 × 128 (no control) or ~255 × 64 (with control) logical
sub-streams per channels connection. That's a lot, and it's the
load-bearing count for the "recursive multiplexing" point below.

**The "ignore stderr for a moment" resolution.** Stderr is the one
feature that breaks the pure mod-2 framing — it's a unidirectional
read (server→client), not a bidirectional pair. Three options:

1. **Stderr as its own instance (mod 2).** Instance 0 = io, instance
   1 = stderr-as-bidirectional (write half unused, declared but never
   written). The demux routes by `(channel_id, stream_type)`
   regardless of convention; the per-channel declaration at
   `channel/open` time (ADR-073 already has this field) is the only
   truth. PTY mode (stderr merged) declares instance 0 only; pipe
   mode declares instances 0 and 1. One unused stream_type per
   stderr-carrying channel; cheap.
2. **Stderr as a unidirectional leaf (the asymmetric case).** The
   per-channel declaration allows unidirectional leaves — not every
   stream_type has to be part of a pair. Stderr rides a single
   stream_type (e.g., type 5), declared read-only. The demux doesn't
   care; the convention is "pairs are the default, unidirectional
   leaves are explicitly declared." This keeps mod 2 as the
   bidirectional convention without forcing stderr into a pair.
3. **Combine stderr into stdout (the Docker PTY choice).** Always
   merge; lose separate stderr. This loses the pipe/non-PTY mode
   feature — `tty-local/pipe.rs:363 separate_stderr` test asserts
   stderr arrives as distinct chunks; the Docker `exec`/`attach` API
   exposes `Tty: true` (merge) vs `Tty: false` (separate). Not
   acceptable for the non-PTY use case.

**The stderr question is load-bearing for mod 2 vs mod 3.** If
stderr-as-its-own-instance (option 1) is clean, mod 2 wins — every
bidirectional thing is a pair, stderr is a pair with one unused half,
no asymmetry. If the unused half is a real wart, the per-channel
declaration with unidirectional leaves (option 2) wins, and the
mod-2/mod-3 distinction is less load-bearing than "the declaration is
the truth, the convention is a hint." This is a POC candidate (see
below).

---

## TTY/channels convergence

Channels was written after TTY and was a natural extension: TTY's
5-byte header + `channel_id:u32` prefix = the 9-byte channels header.
The convergence has two parts.

### Format convergence

TTY-direct uses the 5-byte format (ADR-052, scoped to direct by
ADR-077). Channels uses the 9-byte format (ADR-071). TTY-inside-channels
uses the 9-byte format. The 5-byte format is the 9-byte format with
the `channel_id` prefix elided (because TTY-direct has only one
channel).

**Option A (current, ADR-077): two formats coexist.** TTY-direct keeps
the 5-byte format; TTY-inside-channels uses 9-byte. The 4-byte
`channel_id` overhead is paid only inside channels.

**Option B (retire the 5-byte format): TTY-direct uses 9-byte with
`channel_id = 0`.** One demux implementation, one set of stream_type
semantics, one crate. The 4-byte overhead per chunk is noise for the
TTY use case (terminal I/O, not bulk transfer). But: backward-compat
with existing TTY-direct deployments, and the "channel 0 is always
call" rule (ADR-072) conflicts — TTY-direct's single channel is
`alknet/tty`, not `alknet/call`. Resolutions:
- **B1:** TTY-direct is a channels connection with `channel_id = 0` =
  `alknet/tty` (breaking ADR-072's "channel 0 is call" rule).
- **B2:** TTY-direct is a channels connection with `channel_id = 0` =
  `alknet/call` (unused, no `channel/open` issued) + `channel_id = 1` =
  `alknet/tty` (pre-allocated). Pays one unused channel for rule
  consistency. Channel 0 is *always* call, even when unused.
- **B3:** TTY-direct is not a channels connection at all; it keeps the
  5-byte format (Option A). The 9-byte format applies only inside
  channels. (Same as current ADR-077.)

**Option B is the bigger call.** It's a wire-format change with
backward-compat implications. It's a POC candidate (validate B1/B2 in
a minimal implementation) and a possible ADR-094. Not in scope for the
mod-2/mod-4 stream_type redesign (ADR-093).

### Semantic convergence (independent of format)

Regardless of Option A/B, TTY and channels should present the same
shape to their handlers: a set of unidirectional sub-streams
declared per-channel, paired into bidirectional instances where the
handler wants a joined `BiStream`. The `into_sub_streams()` accessor
(ADR-074) returns the declared stream_types as
`Vec<(u8, SubStreamHandle)>` with `SubStreamHandle::Send` (write half)
or `Recv` (read half). The handler joins the pairs it wants via
`tokio::io::join`. The channels layer exposes the leaves; the handler
composes them.

This means ADR-074's two paths (`accept_bi` for the joined 0/1 pair,
`into_sub_streams` for the typed leaves) become the same data,
presented differently. `accept_bi` is a convenience that joins 0/1 for
the common case (tunnel, SSH, echo, HTTP); `into_sub_streams` is the
primary accessor (the unidirectional leaves). Both are the same data;
the handler chooses how to compose.

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
of multiplexing. Recursive composition is a natural consequence of the
abstraction, not a goal. Recorded here because the instance framing
makes it cleaner than ADR-071's group framing did — the instance is
the recursive unit, the group was not.

---

## POC candidates

Ordered by leverage (the load-bearing unknowns that, once validated,
make the ADR drafting mechanical rather than speculative).

### POC 1: stderr split/recombine (load-bearing for mod 2 vs mod 3)

**Question:** Can a `tokio::io::split` half be recombined with a
different half cleanly? Specifically: if stderr is its own instance
(mod 2, write half unused), does the unused write half cause any
real problem — dangling sender, premature EOF on the read half,
flow-control weirdness? Or does declaring "instance 1 = stderr,
write half unused" just work?

**Why it matters:** If the unused write half is clean, mod 2 wins
(every bidirectional thing is a pair, stderr is a pair with one
unused half, no asymmetry, no mod 3). If it's a wart, the
per-channel declaration with unidirectional leaves (option 2) wins,
and the mod-2/mod-3 distinction is less load-bearing than "the
declaration is the truth."

**Scope:** ~100 lines. A test-only POC in the channels POC's shape:
declare a channel with two instances, instance 0 = io pair (types
0/1), instance 1 = stderr (types 2/3, write half never written).
Verify the read half on instance 1 delivers stderr chunks; the write
half never errors; the demux doesn't choke on an unused write
stream_type. Run it over the POC's `tokio::io::duplex` stand-in.

**Output:** A `docs/research/alknet-channels/poc-stderr-split.md`
summary with a yes/no answer and the code path. If yes, ADR-093
drafts with mod 2. If no, ADR-093 drafts with per-channel
declaration as the primary, mod 2 as a convention hint.

### POC 2: TTY-direct-as-channels (load-bearing for format convergence)

**Question:** Is Option B1 (TTY-direct = channels with `channel_id = 0`
= `alknet/tty`, breaking ADR-072) or Option B2 (channel 0 = call
unused, channel 1 = TTY) feasible and clean? Or is the 4-byte
`channel_id` overhead per chunk in TTY-direct mode a real concern
for terminal I/O? And is the backward-compat with existing
TTY-direct deployments a real constraint, or are there no existing
deployments to break?

**Why it matters:** If Option B is clean and backward-compat is
non-binding, the 5-byte format retires, the demux implementation
unifies, and the TTY crate's direct mode becomes a thin wrapper. If
backward-compat is binding or the overhead is a concern, Option A
(two formats coexist per ADR-077) stands.

**Scope:** ~200 lines. A POC that runs the TTY adapter over a
channels-format demux with `channel_id = 0` (B1) and `channel_id =
1` (B2), using the existing TTY `TtyBackend` (the local pipe backend).
Verify the `pty.rs` / `pipe.rs` tests still pass in shape. Measure
chunk overhead if relevant.

**Output:** A `docs/research/alknet-channels/poc-tty-direct-as-channels.md`
summary. Recommends Option A, B1, or B2. If B, becomes the input to
ADR-094 (retire the 5-byte format). If A, ADR-077 stands and the
5-byte format is kept.

### POC 3: recursive composition (low leverage, deferred)

**Question:** Does `alknet/channels` inside `alknet/channels` actually
work end-to-end? The abstraction permits it (ADR-074); the POC didn't
validate it (`poc-summary.md` §"What the POC Does NOT Validate" #5).

**Why it matters:** Low leverage — the instance framing makes it
cleaner, but the primary use case is one level. Recursive composition
is a property, not a feature.

**Scope:** Deferred. Not a POC for this round. Recorded for
completeness.

---

## What goes where (ADR plan)

| ADR | Scope | Status |
|-----|-------|--------|
| **ADR-092** | `BiStream` as the handler leaf; `accept_bi` returns `BiStream`; `Connection::from_stream` removed; `from_bidi` is the only public stream constructor | **Drafted, pushed** (`f8d4650`, amended `528cfa0`). Load-bearing, separable. |
| **ADR-093** | stream_type as the unidirectional leaf; mod 2 / mod 4 instance framing; per-channel stream_type declaration; `into_sub_streams()` as the primary accessor; demux convention-agnostic. Amends ADR-071 (wire format stream_type decomposition), ADR-074 (`into_sub_streams`), ADR-077 (TTY uses mod 2, control channel fixed). | **Pre-ADR**, drafting after POC 1 (stderr split) validates the mod-2 decision. |
| **ADR-094** | Retire the 5-byte TTY-direct format in favor of 9-byte channels format with `channel_id = 0` (or `channel_id = 1` with channel 0 = call unused). Backward-compat analysis. | **Pre-ADR**, drafting after POC 2 (TTY-direct-as-channels) recommends Option B. If POC 2 recommends Option A, ADR-094 is not drafted and ADR-077 stands. |

ADR-092 is separable from 093/094 — it's the transport seam, not the
multiplexing seam. ADR-093 is the multiplexing redesign that ADR-092
exposes. ADR-094 is the format-convergence call that ADR-093 enables
but doesn't require.

---

## Open questions (for the research iteration)

- **Stderr as instance vs unidirectional leaf.** POC 1 resolves this.
  Default assumption: stderr as instance (mod 2, unused write half is
  clean). POC validates.
- **Mod 2 vs mod 3.** If POC 1 confirms the unused-write-half is clean,
  mod 2 wins. If not, the per-channel declaration becomes primary and
  the mod distinction is a hint, not a rule.
- **5-byte format retirement.** POC 2 resolves this. Default
  assumption: Option A (two formats coexist, ADR-077 stands) is
  safer; Option B (retire 5-byte) is cleaner but a wire-format change.
  POC validates feasibility and overhead.
- **`channel 0 is always call` (ADR-072) under TTY-direct-as-channels.**
  If POC 2 recommends B1 (TTY-direct channel 0 = `alknet/tty`), ADR-072
  needs an exception or amendment. If B2 (channel 0 = call unused,
  channel 1 = TTY), ADR-072 stands. If A (keep 5-byte), no change.
- **Recursive composition.** Deferred. The instance framing makes it
  cleaner, but it's not a goal. POC 3 is low leverage.
- **WS-as-BiStream for the browser WASM path.** ADR-092 records
  `WsBidiStream` as the server-side adapter; the browser-side adapter
  is a separate concern (the WASM SDK, not alknet-http). Where does
  the browser-side adapter live? Default: separate, not alknet-http.
  Resolved at implementation time.

---

## References

- ADR-092: `BiStream` as the handler leaf (drafted, pushed)
- ADR-071: channels wire format (the mod-3 stream_type decomposition
  ADR-093 amends)
- ADR-074: `ChannelBidiStreamSource` / `into_sub_streams` (the
  per-channel sub-stream accessor ADR-093 amends)
- ADR-077: TTY inside channels (the two-mode TTY design ADR-093
  amends; the 5-byte format scoping ADR-094 may retire)
- ADR-070: `BidiStreamSource` trait (the extension point ADR-092
  amends)
- ADR-065: `Connection::from_stream` / `from_bidi` (amended by
  ADR-092 — `from_stream` removed, `from_bidi` only)
- ADR-078: two-pump shutdown-on-completion (preserved by ADR-092)
- `docs/research/alknet-crate-extraction/findings.md` Phase 6 — the
  deferred `alknet-http` rework; ADR-092 resolves the deferral
- `docs/research/alknet-channels/poc-summary.md` — the channels POC
  (28 tests, WASM compile check); the shape ADR-093 builds on
- `crates/alknet-tty/src/wire.rs:13,32-33` — the `STREAM_CONTROL = 3`
  "bidirectional" flaw ADR-093 fixes
- `crates/alknet-tty-local/tests/pipe.rs:363 separate_stderr` — the
  test that proves separate stderr is a real feature, not vestigial