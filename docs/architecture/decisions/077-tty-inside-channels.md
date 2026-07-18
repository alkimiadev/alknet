# ADR-077: TTY Inside Channels — Sub-Streams, Not Wire Format

## Status

Accepted (**reversed 2026-07-18 by ADR-093: TTY always uses its 5-byte
format; the channels layer carries it transparently in the payload —
see "Reversal (ADR-093, 2026-07-18)" below**)

## Reversal (ADR-093, 2026-07-18)

The two-mode TTY design (direct vs inside-channels, with different
sub-stream access paths) is **reversed**. TTY's 5-byte format
(`[stream_type:u8][length:u32][payload]`, ADR-052) is TTY's internal
format, used in **both** direct mode and inside-channels mode. The two
modes differ only in *where the `BiStream` comes from* (a top-level
`alknet/tty` connection vs a `channel/open` with ALPN `alknet/tty`), not
in *how TTY parses it*. The same `wire.rs` code runs in both modes.

When TTY is inside channels, the channels layer strips its 8-byte header
(ADR-093) and hands TTY the payload bytes. TTY parses its 5-byte header
from the payload. The channels layer carries TTY's 5-byte chunks
transparently in its payload — no shared fields, no leaked abstraction,
no double-chunking concern (the 13-byte total header is 8 channels + 5
TTY, not 8 + 9; the channels `length` is always `tty_len + 5`).

The `channels` feature on `alknet-tty` becomes "run TTY's sub-demux on a
channels-backed `BiStream`" — the same code as direct mode, different
`BiStream` source. The control channel split (`STREAM_CTRL_IN` /
`STREAM_CTRL_OUT`, Phase 7) is TTY-internal; the channels layer doesn't
know about it. ADR-074's `into_sub_streams()` (the accessor this ADR's
two-mode design relied on) is removed by ADR-093; TTY sub-demuxes its
`BiStream` via its own 5-byte format instead.

The body below describes the **original** (two-mode) shape; the reversal
above is the operative decision. The two-mode description is kept as
the historical context for the reversal. See ADR-093 for the resolution
rationale (the channels layer has no `stream_type` concept; the handler
owns its sub-stream multiplexing) and the cross-ADR impacts.

## Context

ADR-052 defines the alknet-tty wire format: `[stream_type: u8][length: u32
be][payload]`, a 5-byte chunk header for four sub-streams (stdin/stdout/
stderr/control) within one bidi stream. This format is stable, implemented
(`crates/alknet-tty/src/wire.rs`), and used for direct `alknet/tty`
connections.

The phase-0 research (`docs/research/alknet-channels/phase-0-findings.md`
§DP-3, §OQ-CH-02) recommended that the TTY chunk format be "absorbed into
channels" and that `alknet/tty` remain as a "direct-connect shortcut." But
the research did not pin what changes in the TTY crate when a TTY session
runs *inside* a channels connection. This is gap #5 from the architecture
assessment — a real integration question the research hand-waved.

The problem: the `TtyAdapter`'s current `handle()` loops `accept_bi()`,
spawning a `drive_session` task per bidi stream that parses 5-byte TTY
chunks off the stream. Inside a channels connection, the stream is *already
de-chunked* by the channels layer's 9-byte format — the handler sees an
`AsyncRead + AsyncWrite` pair, not a chunk-encoded stream. If the TTY
adapter tries to parse 5-byte chunks off an already-de-chunked stream, it
breaks.

## Decision

### Two modes for TTY, one adapter

The `TtyAdapter` operates in two modes, determined by how it receives its
`Connection`:

| Mode | When | Wire format | How the adapter gets sub-streams |
|------|------|-------------|---------------------------------|
| **Direct (`alknet/tty` ALPN)** | Top-level QUIC/TCP connection with ALPN `alknet/tty` | TTY's 5-byte format (ADR-052, amended — see below) | `accept_bi()` → parse 5-byte chunks → split into stream_types 0-4 |
| **Inside channels** | `channel/open` with ALPN `alknet/tty` on a channels connection | Channels' 9-byte format (ADR-071) — the channels layer de-chunks | `into_sub_streams()` (ADR-074) → five named handles for stream_types 0-4 |

In both modes, the `TtyBackend` trait and `TtyHandle` are unchanged
(ADR-053). The backend allocates a PTY and returns a `TtyHandle`; the
adapter pumps data between the handle and the sub-streams. The difference is
only in how the adapter gets the sub-streams — 5-byte chunk parsing (direct)
vs. `into_sub_streams()` (channels).

### The control channel is now properly bidirectional

The phase-0 findings flagged that the TTY control channel "isn't actually
bidirectional… the adapter ignores Exit from the client." The root cause:
`stream_type 3` was "bidirectional" — one stream both sides wrote to, which
is not properly multiplexed.

ADR-071's stream_type decomposition fixes this: **every stream_type is
unidirectional.** Control is now two halves:

| stream_type | direction | purpose |
|-------------|-----------|---------|
| 3 | write (client→server) | control in: resize, signal, eof |
| 4 | read (server→client) | control out: exit, keepalive response |

The TTY adapter writes resize/signal/eof to `ctrl_in` (stream_type 3) and
reads exit/keepalive from `ctrl_out` (stream_type 4). Each has its own
reassembly buffer, its own flow control, its own EOF. The control channel
is *actually* bidirectional — two unidirectional streams, not one shared
stream both sides write to.

This amends ADR-052's stream_type assignments for direct mode too: direct
`alknet/tty` connections now use stream_types [0, 1, 2, 3, 4] (data in/out/
err + control in/out), not [0, 1, 2, 3]. The 5-byte format's `stream_type`
field gains value 4; the `ControlMessage` enum is unchanged (the JSON shape
is the same; the stream_type it rides on splits from 3 into 3+4).

### What changes in alknet-tty

1. **The adapter's session-driving code splits into two entry points:**
   - `drive_session_direct(send, recv, backends, ...)` — the existing path:
     parse 5-byte chunks, split into stream_types 0-4, pump. Used for direct
     `alknet/tty` connections.
   - `drive_session_channels(sub_streams, backends, ...)` — the new path:
     receive `ChannelSubStreams` (five named handles: stdin=SendStream,
     stdout=RecvStream, stderr=Option<RecvStream>, ctrl_in=SendStream,
     ctrl_out=RecvStream), pump directly without chunk parsing. Used when
     the channel's `Connection` is backed by `ChannelBidiStreamSource`.

2. **The `TtyAdapter::handle()` branches on the `Connection`'s source type.**
   The channels crate's `ChannelBidiStreamSource` is a `BidiStreamSource`
   (ADR-070); the `Connection` wraps it. The adapter detects whether the
   `Connection` is channels-backed (via a downcast or a channels-crate
   extension trait — exact ergonomics per ADR-074's implementation detail)
   and calls `drive_session_channels` instead of `drive_session_direct`.

   **This is the one place alknet-tty knows about channels.** It is a
   branch on the connection source, not a dependency on channels' wire
   format. The branch can be feature-gated (`channels` feature on
   alknet-tty) so the direct-only path has no channels dependency.

3. **The 5-byte wire format (ADR-052) is unchanged for direct connections.**
   ADR-052's scope is now "the wire format for direct `alknet/tty`
   connections." The channels path does not use it. This amends ADR-052's
   scope — the format is not replaced, it's scoped.

4. **The control channel works the same in both modes, now properly
   bidirectional.** In direct mode, control-in JSON rides in 5-byte chunks
   with `stream_type=3` and control-out rides with `stream_type=4`. In
   channels mode, control-in rides in 9-byte chunks with `stream_type=3`
   (write) and control-out with `stream_type=4` (read) — but the channels
   layer de-chunks them, so the adapter reads raw JSON bytes from
   `ctrl_in`/`ctrl_out` in both cases. The `ControlMessage` enum (resize,
   signal, eof, exit) is unchanged — the JSON shape is the same; only the
   stream_type assignments change (3 splits into 3+4).

5. **The exit-chunk-is-last invariant (ADR-055) generalizes.** In direct
   mode, the exit chunk is the last 5-byte chunk on `stream_type=4` (read,
   server→client) before stream close (ADR-055, amended). In channels mode,
   the exit control message is the last data on `stream_type=4` before
   `channel/close` is sent on channel 0 (ADR-073 §channel/close). The
   ordering invariant is the same — exit before close — but the mechanism
   differs: 5-byte chunk ordering on stream_type 4 (direct) vs.
   `stream_type 4` ordering + `channel/close` after pump completion
   (channels, REQ-CH-06).

### What does NOT change

- **`TtyBackend` trait, `TtyHandle`, `TtyControl`** (ADR-053) — unchanged.
  Backends don't know about channels or direct mode.
- **`DockerTtyBackend`, `LocalTtyBackend`** — unchanged. They implement
  `TtyBackend::allocate()` and return a `TtyHandle`.
- **`ControlMessage` enum** — unchanged. The JSON shape is the same in both
  modes.
- **The `alknet/tty` ALPN string** — unchanged. Direct connections use it;
  channels `channel/open` requests it.

### Crate dependency

`alknet-tty` does **not** depend on `alknet-channels` unconditionally. The
channels-integration code is behind a `channels` feature on `alknet-tty`.
When the feature is off, `TtyAdapter` only supports direct mode (the
existing behavior). When the feature is on, the adapter branches into
channels mode for channels-backed connections. This preserves ADR-003's
no-handler-depends-on-another-handler rule for the default build; the
feature-gated dependency is opt-in, same as `alknet-docker`'s `tty` feature
(ADR-061).

## Consequences

**Positive:**
- The TTY crate's direct mode is unchanged — existing `alknet/tty`
  deployments (browser terminals over WebSocket, direct QUIC TTY) keep
  working with the 5-byte format.
- The channels path uses the channels layer's de-chunking — no double-
  chunking (5-byte inside 9-byte). The TTY adapter sees clean sub-streams.
- The `TtyBackend` trait is insulated — backends don't know which mode the
  adapter is in. Docker, SSH, and local backends work in both modes without
  changes.
- The control channel and exit-chunk invariant carry forward cleanly — the
  `ControlMessage` enum and ordering semantics are mode-independent.

**Negative:**
- `alknet-tty` has two session-driving entry points (`drive_session_direct`
  vs `drive_session_channels`). This is the necessary cost of supporting
  both direct and channels modes without double-chunking. The alternative
  (always use channels format, even for direct) would break existing direct
  deployments and add 4 bytes of overhead per chunk for no benefit.
- The `channels` feature on `alknet-tty` adds a dependency edge
  (`alknet-tty` → `alknet-channels`, feature-gated). This is the same
  pattern as `alknet-docker`'s `tty` feature (ADR-061) and is opt-in.
- ADR-052's scope is amended (from "the TTY wire format" to "the TTY wire
  format for direct connections"). This is a scope clarification, not a
  format change — the 5-byte format itself is unchanged.

## Door type

**One-way (scope amendment) + two-way (feature gate).** ADR-052's scope
amendment (direct-only) is one-way — once the channels path exists,
re-merging the formats would require unifying 5-byte and 9-byte chunk
handling, which is a rewrite. The `channels` feature gate is two-way — it
can be removed if channels integration is no longer needed.

**Reversed by ADR-093 (2026-07-18):** the two-mode design is reversed —
TTY always uses its 5-byte format, carried transparently in the channels
payload. The one-way door is re-cast (the channels crate is not yet
implemented, so this is the right time). See ADR-093 for the amended
door-type discussion.

## References

- **ADR-093**: channels pure channel multiplexing (reverses this ADR —
  TTY always uses its 5-byte format; the channels layer carries it
  transparently; the two-mode design is preserved but differs only in
  `BiStream` source, not in parsing)
- ADR-052: alknet-tty wire format (amended — scoped to direct connections
  by this ADR; **re-amended by ADR-093 — TTY always uses its 5-byte
  format, in both direct and inside-channels modes**)
- ADR-053: TtyBackend trait and TtyHandle (unchanged by this ADR)
- ADR-055: exit-chunk-is-last (generalized by this ADR + ADR-073)
- ADR-057: alknet-tty does not depend on alknet-call (preserved — the
  channels feature is on alknet-channels, not alknet-call)
- ADR-071: channels wire format (the 9-byte format the channels path
  uses; **amended by ADR-093 — 8-byte format, no `stream_type`**)
- ADR-074: ChannelBidiStreamSource / `into_sub_streams` (the accessor the
  channels path uses; **amended by ADR-093 — `into_sub_streams()`
  removed**)
- ADR-092: `BiStream` as the handler leaf (the transport-leaf decision
  that enables the reversal — `accept_bi` returns `BiStream`)
- ADR-061: DockerTtyBackend in alknet-docker (the feature-gated dependency
  pattern this ADR mirrors)
- `docs/research/alknet-channels/phase-0-findings.md` §DP-3, §OQ-CH-02,
  §Relationship to Existing Crates / alknet-tty