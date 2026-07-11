---
status: draft
last_updated: 2026-07-10
---

# alknet-channels — Phase 0 Research Findings

This document captures Phase 0 (Exploration) findings for the `alknet-channels`
crate. The objective of Phase 0 per `docs/sdd_process.md` is: *"Capture vision
and guiding principles; research options; validate approaches; converge on a
recommended approach."* It is the input to Phase 1 (Architecture), where the
Architect will produce `docs/architecture/crates/channels/*.md` specs, ADRs,
and open questions.

This document was drafted 2026-07-10, emerging from a discussion about the
conceptual tangle between the call protocol, the TTY crate, and the coming
docker and SSH crates. The core issue: most "call clients" will use more than
just the call protocol, and not everything is well described by JSON. The TTY
crate's chunk format already solves sub-stream multiplexing for terminal
sessions — this document generalizes that pattern into a universal channel
multiplexer that the call protocol can orchestrate.

## Vision Recap

`alknet-channels` is a **multiplexing proxy** — a `ProtocolHandler` on
`alknet/channels` that decomposes a single bidirectional stream into multiple
logical channels, each carrying a different protocol (ALPN). It is the
generalization of two existing patterns:

1. **SSH's channel multiplexer** (RFC 4254): `ChannelId(u32)` with
   string-named types negotiated per channel, all traffic interleaved on one
   encrypted transport stream.
2. **alknet-tty's chunk format**: `[stream_type: u8][length: u32 be][payload]`
   with a fixed set of four sub-streams (stdin/stdout/stderr/control).

The generalization: a chunk header of `(channel_id: u32, stream_type: u8,
length: u32)` — 9 bytes — that multiplexes an arbitrary number of channels,
each with up to 256 sub-stream types, over a single transport. Channel 0 is
pre-negotiated as `alknet/call` — the call protocol. Every other channel is
opened dynamically via call operations on channel 0, and channel open is
**bidirectional**: either side can open a channel to the other, just like
the call protocol's operation overlay where each side populates what
operations they can call.

The guiding insight:

> **Streams are streams.** A TTY session, an SSH channel, a forwarded TCP
> connection, a QUIC bidi stream — they're all just `AsyncRead + AsyncWrite`
> handles. The differences are only in how they're *opened* (negotiation) and
> what *multiplexing layer* carries them. Once you normalize to
> `AsyncRead + AsyncWrite`, the architecture collapses into a simple shape:
> every channel is just an ALPN routed through the same `HandlerRegistry`,
> channel 0 is just `alknet/call` pre-negotiated, and ACL governs everything.

## The Problem This Solves

### The conceptual tangle

Today, alknet's architecture has three different multiplexing models that
don't compose well:

| Model | Where | Mechanism |
|-------|-------|-----------|
| Connection-level | ALPN router | One ALPN per QUIC connection |
| Stream-level | QUIC native | Many bidi streams per connection |
| Sub-stream-level | TTY chunk format | 4 logical channels within one bidi stream |

A docker client needs both JSON call operations (`docker/container/list`,
`docker/container/create`) and raw byte streams (interactive exec, container
attach). Today that means **two separate QUIC connections** with different
ALPNs (`alknet/call` and `alknet/tty`). The call protocol has no way to say
"for this operation, open a TTY stream." There's no cross-ALPN coordination.

### The deeper issue

The call protocol is positioned as the universal RPC layer, but it's
fundamentally **JSON-only**. The `EventEnvelope` framing can't carry raw
bytes. So anything non-JSON (TTY sessions, SSH channels, file transfers, git
pack protocols) needs its own ALPN and its own connection. The call protocol
can't even *reference* these other streams — there's no "stream token" or
"channel open" mechanism.

### What channels provides

A single `alknet/channels` connection carries:
- Channel 0: `alknet/call` (pre-negotiated — both sides route it to the `CallAdapter`)
- Channel N: `alknet/tty` (opened dynamically via `channel/open`)
- Channel M: `alknet/ssh` (opened dynamically via `channel/open`)
- Channel K: `alknet/tunnel` (opened dynamically via `channel/open`)

All on one QUIC connection (or one TCP connection, or one WebTransport
session). Channel 0 is not a special "control plane" — it's just the
`alknet/call` ALPN, pre-negotiated so both sides know to route it to the
`CallAdapter` without an explicit `channel/open` exchange. Every other
channel works exactly the same way: reassemble chunks into a stream, look up
the ALPN in the `HandlerRegistry`, hand off to the handler. Channel open is
bidirectional — either side can open a channel to the other, just like the
call protocol's operation overlay where each side populates what operations
they can call.

## The Wire Format

### Chunk header

```
[channel_id: u32 be][stream_type: u8][length: u32 be][payload bytes]
```

9 bytes of header, compared to TTY's current 5 bytes (`[stream_type: u8]
[length: u32 be]`). The `channel_id` field is the addition — it's what turns
a fixed 4-channel multiplexer into an arbitrary N-channel multiplexer.

### Channel 0: pre-negotiated as `alknet/call`

Channel 0 is not a special "control plane" with its own framing. It is
simply the `alknet/call` ALPN, pre-negotiated: both sides know that channel
0 is routed to the `CallAdapter` without an explicit `channel/open`
exchange. The call protocol runs on channel 0 exactly as it runs on a
top-level `alknet/call` QUIC connection — `EventEnvelope` frames on a
bidirectional stream.

Channel 0's chunks use `stream_type` to disambiguate framing:

| stream_type | purpose |
|-------------|---------|
| 0 | `EventEnvelope` frame (JSON, length-prefixed — the call protocol wire format) |
| 1-255 | reserved for future sub-streams |

### Data channels (1..N)

Every other channel works exactly the same way as channel 0: reassemble
chunks into a stream, look up the ALPN in the `HandlerRegistry`, hand off to
the handler. The `stream_type` byte decomposes the channel into sub-streams,
following the TTY crate's proven model:

| stream_type | direction | purpose |
|-------------|-----------|---------|
| 0 | write half | data flowing in (stdin equivalent) |
| 1 | read half | data flowing out (stdout equivalent) |
| 2 | read half (optional) | error/diagnostic output (stderr equivalent) |
| 3 | bidirectional | control messages (protocol-specific JSON) |
| 4-255 | reserved | future sub-stream types |

Not all channels use all sub-streams. A TTY session uses 0-3. A raw tunnel
might use only 0 and 1. An SSH connection uses only 0 and 1 (SSH multiplexes
internally). The `stream_type` set is fixed per channel type, declared at
open time.

### Channel lifecycle

1. **Open**: either side sends a `channel/open` call operation on channel 0
   with `{ alpn, params }`. The receiving side validates ACL, looks up the
   ALPN in the `HandlerRegistry`, allocates the handler, and returns
   `{ channel_id, stream_types }`. The `stream_types` field declares which
   sub-streams are active for this channel (e.g., `[0, 1, 2, 3]` for TTY,
   `[0, 1]` for a tunnel). Channel open is **bidirectional** — a hub can
   expose a resource a worker consumes, or a worker can expose a resource
   the hub consumes, just like the call protocol's operation overlay.
2. **Data**: both sides read/write chunks on the assigned `channel_id`. Each
   side reassembles chunks for a given `(channel_id, stream_type)` into a
   stream. The reassembled streams are presented to the handler as
   `AsyncRead + AsyncWrite` handles.
3. **Control**: channel-specific control messages ride on `stream_type 3` as
   JSON. For TTY channels this is resize/signal/eof. For tunnel channels
   this might be keepalive or connection-close. The call protocol on channel
   0 can also send control operations scoped to a specific channel (e.g.,
   `tty/resize` with a `channel_id` parameter).
4. **Close**: either side sends a `channel/close` call operation on channel
   0, or the channel's handler signals completion (e.g., TTY exit). The
   channel's chunks stop; the `channel_id` may be reused.

### Framing disambiguation

The TTY crate's framing disambiguation trick (ADR-052 §5) carries forward.
Channel 0 is just another channel — its chunks have `channel_id=0` in the
header. The disambiguation between channel 0 (call protocol) and data
channels is by `channel_id`, not by a special first-byte trick. Within a
channel, `stream_type` 0 (stdin) from the server is invalid, so `0x00` as
the first byte of a chunk payload from the server is unambiguous.

## The Channel Connection Abstraction

### Reassembling chunks into streams

On each end of a channels connection, chunks for a given `(channel_id,
stream_type)` are reassembled into a byte stream. The reassembly is
order-preserving within a `(channel_id, stream_type)` pair — chunks arrive
in order because they ride on a single ordered transport (QUIC stream, TCP
connection).

Channel 0 is reassembled exactly like any other channel. The reassembled
stream for `(channel_id=0, stream_type=0)` is handed to the `CallAdapter`
as a `Connection`. The `CallAdapter` doesn't know it's inside a channels
connection — it sees a `Connection` yielding bidi streams, same as if it
were a top-level QUIC connection.

For data channels (1..N), the reassembled streams are presented through the
same `Connection` abstraction that alknet-core already defines
(`crates/alknet-core/src/types.rs`):

```rust
// A channels connection presents the same interface as a QUIC connection
impl ChannelConnection {
    async fn accept_bi(&self) -> Result<(SendStream, RecvStream), StreamError>;
    async fn open_bi(&self) -> Result<(SendStream, RecvStream), StreamError>;
}
```

Internally, `accept_bi` / `open_bi` map to `channel/open` call operations on
channel 0, and the returned `SendStream` / `RecvStream` are backed by chunk
reassembly on the assigned `channel_id`.

### ALPN routing inside channels

A `ChannelConnection` holds a reference to the same `HandlerRegistry` that
the outer `AlknetEndpoint` uses. When a `channel/open` arrives with an ALPN
string, the channels handler looks up the ALPN in the registry, validates
ACL, and hands the reassembled stream to the handler. The handler doesn't
know it's inside a channels connection — it sees a `Connection` yielding bidi
streams, same as if it were a top-level QUIC connection.

Channel 0 is not special — it's just `alknet/call` pre-negotiated. Both
sides know to route channel 0 to the `CallAdapter` without an explicit
`channel/open` exchange. Every other channel is opened dynamically via
`channel/open` on channel 0, and the open is bidirectional: either side can
initiate.

This means every existing and future `ProtocolHandler` works inside channels
without modification. The `CallAdapter`, `TtyAdapter`, the future
`SshAdapter`, a tunnel handler — they all just see a `Connection`. The
channels layer is a transparent proxy.

### Recursive composition

A `ChannelConnection` is itself a `Connection`. A channels handler can open a
sub-channels connection on a data channel. This is recursive composition:
`alknet/channels` inside `alknet/channels`. It's powerful (arbitrary nesting
of multiplexing layers) and potentially confusing (infinite recursion). The
recommendation is to allow it but not encourage it — the primary use case is
one level of multiplexing. Recursive composition is a natural consequence of
the `Connection` abstraction, not a feature to design for.

## Relationship to Existing Crates

### alknet-tty

The TTY crate's chunk format becomes a special case of the channels format
(single-channel, no `channel_id` header). Two paths forward:

**Path A (recommended): TTY becomes a channel consumer.** The `TtyAdapter`
no longer owns its own chunk format or its own ALPN. It receives a
`Connection` from the channels layer and drives sessions the same way it does
today. The chunk format becomes the channels layer's concern, not TTY's. The
`alknet/tty` ALPN remains for direct connections (no channels layer), but the
primary path is through channels.

**Path B: TTY keeps its own ALPN and chunk format.** The channels layer and
TTY coexist. TTY's chunk format is a fixed 4-channel subset of the channels
format. A future migration could unify them. This is lower-risk but leaves
two chunk formats in the codebase.

The `TtyBackend` trait and `TtyHandle` are unchanged. The `DockerTtyBackend`
and `LocalTtyBackend` don't need to know about channels — they still
implement `TtyBackend::allocate()` and return a `TtyHandle`. The channels
layer handles the wire format; the TTY adapter handles the session
lifecycle.

### alknet-call

The call protocol is unchanged. It remains JSON-only, `EventEnvelope`-based.
It runs on channel 0 exactly as it runs on a top-level `alknet/call` QUIC
connection — the `CallAdapter` receives a `Connection` and dispatches
operations. The only difference is that the `Connection` is backed by chunk
reassembly on channel 0 rather than by a QUIC bidi stream.

What changes is that the call protocol gains a new class of operations:
channel lifecycle operations.

New call operations:
- `channel/open` — open a data channel with a given ALPN and params.
  Bidirectional: either side can initiate.
- `channel/close` — close a data channel
- `channel/control` — send a control message on a channel's stream_type 3

Existing call operations are unchanged. The call protocol doesn't need to
know about raw bytes — it just hands out channel tokens and dispatches
control operations. The `from_call` operation overlay (where each side
populates what operations they can call) extends naturally to channels:
either side can expose resources the other can open channels to.

### alknet-ssh

The SSH crate's relationship to channels is twofold:

1. **SSH as a channel type**: an `alknet/ssh` channel carries the SSH binary
   protocol over stream_types 0 and 1. The channels layer hands the
   reassembled stream to `SshAdapter`, which feeds it to russh. Russh
   multiplexes SSH channels inside. This is SSH-over-channels — one level of
   multiplexing (channels) carrying another (SSH).

2. **SSH as a channels transport**: an SSH `direct-tcpip` channel could
   carry a channels connection. This is channels-over-SSH — the reverse
   composition. Useful for tunneling alknet through an SSH bastion.

The SSH crate doesn't need to know about channels. It implements
`ProtocolHandler` for `alknet/ssh` and accepts a `Connection`. Whether that
`Connection` is a top-level QUIC connection or a channels data channel is
transparent to the handler.

### alknet-docker

The docker crate registers call operations on the call protocol
(`docker/container/list`, `docker/container/create`, etc.) and a
`DockerTtyBackend` for interactive sessions. With channels:

- Docker lifecycle operations are call operations on channel 0 (unchanged
  from ADR-058).
- Interactive exec/attach opens a TTY channel via `channel/open` with ALPN
  `alknet/tty` and backend `docker`. The channels layer routes to the
  `TtyAdapter`, which dispatches to `DockerTtyBackend`.
- No separate `alknet/tty` connection needed. One `alknet/channels`
  connection handles both JSON operations and raw TTY sessions.

## Transport Agnosticism

The channels wire format works over any ordered, reliable bidirectional byte
stream:

| Transport | How |
|-----------|-----|
| QUIC bidi stream | `alknet/channels` ALPN on a QUIC connection; one bidi stream carries all channels |
| TCP+TLS | `alknet/channels` ALPN on a TLS connection; the TCP stream carries all channels |
| WebTransport | `alknet/channels` session proxied through the h3 handler (ADR-040) |
| SSH channel | channels connection riding inside an SSH `direct-tcpip` channel |
| Another channels connection | recursive composition (channel type `alknet/channels`) |

The same wire format, the same chunk reassembly, the same `Connection`
abstraction. The transport is a parameter, not a design constraint.

## WASM Compatibility

The channels wire format is pure byte manipulation: read a 9-byte header,
split the payload, route to the right reassembly buffer. No platform
dependencies, no tokio-specific types in the core splitting/recombining
logic. A WASM build can:

1. Read chunks from a WebTransport `BiStream`
2. Reassemble `(channel_id, stream_type)` streams
3. Present them as `AsyncRead + AsyncWrite` handles to WASM-compatible
   handlers

The handlers themselves may or may not be WASM-compatible (russh's client is;
`portable_pty` is not). But the channels layer — the multiplexing proxy —
is WASM-compatible by construction. A browser can run a channels client in
WASM over WebTransport, opening TTY channels, SSH channels, and tunnel
channels, with the call protocol on channel 0 orchestrating everything.

## ACL and Security Model

### Channel open is gated by the call protocol's ACL

Every `channel/open` operation goes through the call protocol's existing
`OperationContext` (identity, scopes, capabilities, ownership). No channel
opens without authorization. The ACL check happens before the handler is
invoked.

### No open ports by default

A core design principle carried from the SSH research (DP-5: default-deny
baseline): alknet endpoints never expose ports, sockets, or listeners unless
explicitly configured. Tunnels are channels, not ports. A `tunnel/open`
operation targets a resource (`container:abc123`, `service:postgres`), not a
port number. The hub resolves the resource to a stream. No listener, no
socket, no port — just a channel.

This is the "VPN-like without being a VPN" property: bidirectional stream
proxying through channels, gated by ACL, with no network-level exposure.

### Resources, not addresses

Forwarding destinations and tunnel targets are expressed as resources in the
alknet identity/ownership model, not as IP:port pairs. A docker container
hosting a service is a resource (`container:abc123`). Access is granted
through ownership or scopes, not through firewall rules. This avoids the
global trend of VPN restrictions — there are no VPN ports to block because
there are no VPN ports.

## Decision Points

### DP-1: Channel ID allocation — server-assigned vs client-assigned vs negotiated

*(Recommended: server-assigned, returned in the `channel/open` response)*

Three options:
- **(a) Client-assigned**: client picks a `channel_id` in the open request.
  Risk of collision if two clients open channels simultaneously.
- **(b) Server-assigned**: server returns the `channel_id` in the open
  response. No collision risk. Slight latency (one round-trip before data
  can flow).
- **(c) Negotiated ranges**: client and server each own half the ID space
  (e.g., client uses odd IDs, server uses even). SSH uses this. More
  complex, but zero-round-trip channel open.

**Recommendation**: **(b) server-assigned** for simplicity. The round-trip
for channel open is acceptable — it's the same round-trip the call protocol
already makes for every operation. If zero-round-trip channel open becomes a
performance requirement, option (c) can be added as an extension (two-way
door).

### DP-2: Channel 0 — pre-negotiated `alknet/call`, not a special control plane

*(Recommended: channel 0 is just `alknet/call` pre-negotiated — no special framing)*

Channel 0 is not a "control plane" with its own wire format. It is simply
the `alknet/call` ALPN, pre-negotiated: both sides know that channel 0 is
routed to the `CallAdapter` without an explicit `channel/open` exchange. The
call protocol runs on channel 0 exactly as it runs on a top-level
`alknet/call` QUIC connection — `EventEnvelope` frames on a bidirectional
stream backed by chunk reassembly.

This means:
- Channel 0 uses the same chunk format as every other channel
  (`channel_id=0` in the header)
- The `CallAdapter` receives a `Connection` and dispatches operations —
  unchanged from today
- Channel lifecycle operations (`channel/open`, `channel/close`) are just
  call operations on channel 0, dispatched through the `OperationRegistry`
- No special framing disambiguation is needed — `channel_id` already
  distinguishes channel 0 from data channels

**Recommendation**: channel 0 is `alknet/call` pre-negotiated. No special
framing, no separate control-plane wire format. The channels layer is a
transparent proxy: every channel (including channel 0) is just an ALPN
routed through the `HandlerRegistry`.

### DP-3: TTY chunk format — absorb into channels vs keep separate

*(Recommended: absorb into channels, keep `alknet/tty` ALPN as a direct-connect shortcut)*

The TTY crate's chunk format (`[stream_type: u8][length: u32 be]`) is a
subset of the channels format (it's a channels chunk with `channel_id`
implicitly 0 and only stream_types 0-3). Two paths:

- **(a) Absorb**: TTY sessions use the channels format. The `TtyAdapter`
  becomes a channel consumer. The `alknet/tty` ALPN remains as a
  direct-connect shortcut (no channels layer needed for simple cases), but
  the primary path is through channels.
- **(b) Keep separate**: TTY keeps its own chunk format and ALPN. Channels
  and TTY coexist as separate multiplexing strategies. Two chunk formats in
  the codebase.

**Recommendation**: **(a) absorb**. The TTY crate's chunk format was always
described as "a deliberately impoverished version of SSH's channel
multiplexer" (alknet-tty phase-0-findings.md). Channels is the
non-impoverished version. The `alknet/tty` ALPN stays for direct connections
(useful for simple deployments, browser terminals), but the chunk format
unifies under channels.

### DP-4: Control messages — call protocol operations vs stream_type 3 chunks

*(Recommended: both, with clear division of responsibility)*

Channel-specific control can flow through two paths:
- **(a) Call protocol operations**: `tty/resize`, `tty/signal`,
  `channel/close` — dispatched through the `OperationRegistry` on channel 0.
  Benefits from the call protocol's existing auth, routing, and
  observability.
- **(b) stream_type 3 chunks**: inline control messages on the data
  channel. Lower latency (no channel 0 round-trip), but requires the data
  channel handler to parse and dispatch them.

**Recommendation**: **both**. Use call protocol operations for
channel lifecycle (open, close) and infrequent control (resize, signal). Use
stream_type 3 for control that must be ordered relative to data (EOF before
exit). The TTY crate's exit-chunk-is-last invariant (ADR-055) is an example
of ordering-sensitive control that belongs on the data channel. The division
is: call protocol for orchestration, stream_type 3 for data-ordered control.

### DP-5: Stream reassembly — backpressure and flow control

*(Recommended: rely on the transport's flow control; add channel-level
windowing only if needed)*

QUIC provides per-stream flow control. TCP provides connection-level flow
control. The channels layer adds a third multiplexing level. If one data
channel's consumer is slow, it could block all other channels on the same
transport (head-of-line blocking).

Options:
- **(a) No channel-level flow control**: rely on the transport's flow
  control. Simple, but susceptible to head-of-line blocking.
- **(b) Channel-level windowing**: each channel has its own flow-control
  window (like SSH's `ChannelParams.window_size`). Prevents head-of-line
  blocking but adds complexity.
- **(c) Bounded channels with backpressure**: each channel's reassembly
  buffer has a maximum size; when full, the sender stops reading chunks for
  that channel. Simpler than full windowing, prevents unbounded memory
  growth.

**Recommendation**: **(c) bounded channels** for v1. A configurable maximum
buffer size per channel (default 1 MiB). When a channel's buffer is full, the
channels layer stops reading chunks for that `channel_id` until the consumer
drains it. This prevents memory exhaustion without the complexity of SSH's
sliding-window protocol. If head-of-line blocking becomes a real problem
(which it likely won't for the intended use cases — TTY sessions and tunnels
are not high-throughput), full windowing can be added as an extension
(two-way door).

### DP-6: Crate structure — new crate vs extension of alknet-tty

*(Recommended: new `alknet-channels` crate)*

The channels layer is a generalization of TTY's chunk format, but it's a
different thing: a multiplexing proxy, not a terminal session handler. Two
options:
- **(a) New crate `alknet-channels`**: depends on `alknet-core` (for
  `ProtocolHandler`, `Connection`, `HandlerRegistry`). Defines the chunk
  format, the `ChannelConnection`, and the `ChannelsAdapter`. TTY becomes a
  consumer.
- **(b) Extend `alknet-tty`**: add `channel_id` to the chunk format, add
  the `ChannelConnection` abstraction. TTY is the base; channels is the
  generalization. Simpler crate graph, but blurs the conceptual boundary.

**Recommendation**: **(a) new crate**. The channels layer is a distinct
concept (multiplexing proxy) from TTY (terminal session handler). Keeping
them separate follows the existing pattern (ADR-003: no-handler-depends-on-
another-handler) and makes the dependency graph clear: `alknet-channels`
depends on `alknet-core`; `alknet-tty` depends on `alknet-core` (unchanged);
both register on the same `HandlerRegistry`.

## Straightforward Parts

### 1. The chunk format is a thin extension of a proven design

The TTY crate's chunk format (`[stream_type: u8][length: u32 be]`) is
validated by two POCs (alknet-docker-poc, alknet-tty-poc) and implemented in
production code (`crates/alknet-tty/src/wire.rs`). Adding a `channel_id: u32`
prefix is a 4-byte extension to a proven format. The `ChunkReader`/
`ChunkWriter` pattern, the framing disambiguation trick, and the
bidirectional pump pattern all carry forward.

### 2. The `Connection` abstraction already supports this

`alknet-core`'s `Connection` type already wraps multiple backends (QUIC,
iroh, raw stream) behind a uniform `accept_bi()` / `open_bi()` interface. A
`ChannelConnection` is a new backend for the same interface. The
`HandlerRegistry` and all `ProtocolHandler` implementations work unchanged.

### 3. The call protocol needs no wire-format changes

The call protocol stays JSON-only, `EventEnvelope`-based. The new
`channel/open`, `channel/close`, and channel-scoped control operations are
just new operation types in the `OperationRegistry`. No new framing, no new
carriage types, no protocol version bump.

### 4. WASM compatibility is by construction

Chunk splitting and reassembly is pure byte manipulation. The channels layer
has no platform dependencies. A WASM build can read chunks from a
WebTransport stream, reassemble them, and present `AsyncRead + AsyncWrite`
handles to WASM-compatible handlers. The browser case (xterm.js over
WebTransport to a TTY channel) works without the browser implementing SSH.

### 5. ACL is inherited from the call protocol

Channel open goes through the call protocol's existing `OperationContext`.
Identity, scopes, capabilities, and ownership checks apply to channel open
exactly as they apply to any other call operation. No new auth machinery.

## Less Straightforward Parts

### The TTY control channel is currently not bidirectional

The TTY crate's `ControlMessage` enum has server→client variants (`Exit`)
and client→server variants (`Resize`, `Signal`, `Eof`), but the adapter
ignores `Exit` from the client. The control channel *could* be fully
bidirectional — there's no wire-format reason it isn't. With channels, the
call protocol on channel 0 handles bidirectional control, and stream_type 3
on the data channel handles data-ordered control. The TTY crate's
`ControlMessage` enum may need to be split or generalized.

### The TTY adapter's session lifecycle assumes it owns the stream

The `TtyAdapter::handle()` currently loops `accept_bi()`, spawning a
`drive_session` task for each stream. Inside a channels connection, the
adapter receives a `Connection` that yields bidi streams — but those streams
are already channel-demuxed. The adapter doesn't need to know this. The
question is whether the adapter's current "one session per bidi stream"
model maps cleanly to "one session per channel." It should — a channel is
just a bidi stream with extra metadata.

### The exit-chunk-is-last invariant (ADR-055) with channel close

The TTY crate guarantees the exit chunk is the last chunk before stream
close. With channels, the exit chunk is the last chunk on that channel, and
then a `channel/close` operation on channel 0 signals completion. The
ordering between the exit chunk and the close operation must be preserved:
the exit chunk must be written and flushed before the close operation is
sent. This is an implementation constraint, not a design change.

## Open Questions to Carry into Phase 1

- **OQ-CH-01 (channel open round-trip)**: server-assigned channel IDs mean
  one round-trip before data can flow. Is this acceptable for all channel
  types? For TTY sessions the negotiation frame already requires a
  round-trip. For tunnels it adds latency to connection establishment. If
  zero-round-trip open is needed, negotiated ID ranges (DP-1 option c) can
  be added.

- **OQ-CH-02 (TTY migration path)**: if the TTY chunk format is absorbed
  into channels, what's the migration path for existing `alknet/tty`
  deployments? The `alknet/tty` ALPN stays as a direct-connect shortcut.
  Does the TTY adapter's wire format change, or does it keep its current
  chunk format for direct connections and use the channels format when
  inside a channels connection?

- **OQ-CH-03 (channel-level flow control)**: is bounded-buffer backpressure
  (DP-5 option c) sufficient, or will head-of-line blocking be a real
  problem? The intended use cases (TTY sessions, tunnels, SSH connections)
  are not high-throughput. If a use case emerges that saturates a channel
  (e.g., file transfer over a tunnel), channel-level windowing may be
  needed.

- **OQ-CH-04 (channel ID reuse)**: after a channel is closed, can its
  `channel_id` be reused? SSH allows reuse. The channels format should
  allow it too, but the reassembly buffers must be fully drained before
  reuse to prevent data from the old channel leaking into the new one.

- **OQ-CH-05 (maximum channels per connection)**: is there a limit? SSH
  uses `u32` channel IDs, effectively unlimited. The channels format should
  match. The practical limit is memory (reassembly buffers per channel) and
  the transport's flow control.

- **OQ-CH-06 (channel open denial-of-service)**: an authenticated peer
  could open many channels and never read from them, exhausting memory.
  Bounded per-channel buffers (DP-5) limit the damage. A per-connection
  channel limit (configurable, default e.g. 256) provides defense in depth.

- **OQ-CH-07 (POC scope)**: what should a Phase 0 POC validate? Minimum:
  chunk format round-trip with multiple channels, channel open/close via
  call protocol, TTY session inside a channel. Stretch: SSH connection
  inside a channel, tunnel inside a channel, recursive composition.

## Recommended Approach

### Crate

`alknet-channels`, depends on `alknet-core` (for `ProtocolHandler`,
`Connection`, `HandlerRegistry`, `SendStream`, `RecvStream`). Defines:

- The chunk format (`[channel_id: u32 be][stream_type: u8][length: u32 be]`)
- `ChunkReader` / `ChunkWriter` (generalized from TTY's wire module)
- `ChannelConnection` (implements the `Connection` interface over chunk
  reassembly)
- `ChannelsAdapter` (`ProtocolHandler` for `alknet/channels`)
- Channel open/close call operations

Does not depend on `alknet-tty`, `alknet-call`, `alknet-ssh`, or any other
handler crate. The dependency direction is: handlers depend on
`alknet-core`; `alknet-channels` depends on `alknet-core`; nothing depends
on `alknet-channels` except the assembly layer that registers it on the
`HandlerRegistry`.

### Build order

**Step 1: Wire format + mock channels.**
- Implement the 9-byte chunk format with `channel_id`.
- Implement `ChunkReader` / `ChunkWriter` (generalized from TTY's
  `wire.rs`).
- Implement `ChannelConnection` with mock channels (in-memory pipes).
- Validate chunk round-trip with multiple channels, concurrent reads/writes.
- **Result**: a working chunk multiplexer with no real handlers.

**Step 2: Channel open/close via call protocol.**
- Implement `channel/open` and `channel/close` call operations.
- Integrate with `HandlerRegistry` — `channel/open` looks up the ALPN,
  validates ACL, invokes the handler.
- Validate: open a channel, pump data, close the channel.
- **Result**: call protocol orchestrating channel lifecycle.

**Step 3: TTY inside channels.**
- Route `alknet/tty` channel opens to the `TtyAdapter`.
- Validate: `channel/open` with ALPN `alknet/tty` → TTY session inside a
  channel.
- **Result**: TTY sessions work through channels. The docker use case (call
  operations + TTY sessions on one connection) is validated.

**Step 4: Tunnel channels.**
- Implement a simple tunnel handler: `channel/open` with ALPN
  `alknet/tunnel` and a target resource → bidirectional byte proxy.
- Validate: tunnel a TCP connection through a channel.
- **Result**: the "VPN-like without being a VPN" property is proven.

**Step 5: SSH inside channels (stretch).**
- Route `alknet/ssh` channel opens to the `SshAdapter`.
- Validate: SSH connection inside a channel.
- **Result**: SSH-over-channels works. The full stack (call + TTY + SSH +
  tunnel on one connection) is validated.

### De-risk POC

A Phase 0 POC should validate the core claim: **one connection, multiple
channel types, orchestrated by the call protocol.** Minimum scope:

1. Chunk format round-trip with 3+ concurrent channels
2. Channel open/close via call protocol operations
3. TTY session inside a channel (reuse the existing `alknet-tty-poc`
   test infrastructure)
4. Bidirectional byte pumping on a data channel

Stretch goals:
5. Two different channel types (TTY + tunnel) on the same connection
6. WASM build of the chunk splitting/recombining logic

The POC can be built as an extension to the existing `alknet-tty-poc` or as
a standalone POC in `/workspace/alknet-channels-poc/`.

## References

- `docs/research/alknet-tty/phase-0-findings.md` — the TTY crate's chunk
  format, control channel, and backend trait. The seed of the channels
  generalization.
- `docs/research/alknet-ssh/phase-0-findings.md` — SSH's channel
  multiplexer, the channel decomposition (Layers 1-7), the default-deny
  baseline.
- `docs/research/alknet-docker/poc-summary.md` — the docker POC that
  validated the raw chunk format and the two-carriage model.
- `crates/alknet-tty/src/wire.rs` — the current chunk format implementation
  (`ChunkReader`, `ChunkWriter`, `[stream_type: u8][length: u32 be]`).
- `crates/alknet-tty/src/control.rs` — the `ControlMessage` enum (resize,
  signal, eof, exit).
- `crates/alknet-tty/src/adapter.rs` — the `TtyAdapter` and `drive_session`
  bidirectional pump.
- `crates/alknet-core/src/types.rs` — `ProtocolHandler`, `Connection`,
  `SendStream`, `RecvStream`, `HandlerRegistry`.
- `crates/alknet-call/src/protocol/wire.rs` — `EventEnvelope`,
  `FrameFramedReader` / `FrameFramedWriter`.
- `crates/alknet-call/src/protocol/dispatch.rs` — `Dispatcher::run_loop`,
  `handle_stream`, `pump_stream`.
- `/workspace/russh/russh/src/channels/` — russh's channel implementation
  (`ChannelMsg`, `Channel`, `ChannelReadHalf`, `ChannelWriteHalf`,
  `WindowSizeRef`).
- `docs/architecture/decisions/001-alpn-protocol-dispatch.md` — ALPN
  dispatch.
- `docs/architecture/decisions/002-protocol-handler-trait.md` —
  ProtocolHandler.
- `docs/architecture/decisions/003-crate-decomposition.md` —
  no-handler-depends-on-another-handler.
- `docs/architecture/decisions/040-webtransport-alpn-stream-proxy.md` —
  WebTransport stream proxy (the browser path for channels).
- `docs/architecture/decisions/052-tty-wire-format.md` — TTY wire format
  (ADR-052).
- `docs/architecture/decisions/055-tty-exit-chunk-ordering.md` — exit-chunk-
  is-last invariant (ADR-055).
- `docs/architecture/decisions/058-docker-call-alpn.md` — docker on
  `alknet/call` (ADR-058).
