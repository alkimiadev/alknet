---
status: plan
last_updated: 2026-07-12
---

# alknet-channels: De-Risk POC Plan

**Status:** Plan. The POC has not been built. This document specifies what to
build, in what order, and what each step must demonstrate before proceeding.
It is the concrete answer to OQ-CH-07 in `phase-0-findings.md`.
**Date:** 2026-07-12
**Scope:** A standalone POC that validates the three highest-leverage
unknowns of the channels layer: (1) the 9-byte chunk format decomposes and
recombines N concurrent streams correctly, (2) each reassembled channel
presents as a `Connection` that a `ProtocolHandler` can drive unchanged,
(3) the same shape works as a tunnel proxy for a local TCP port. The POC
deliberately avoids the call protocol and the real adapters — it isolates
the channels layer's core mechanics.

---

## Executive Summary

The POC is structured as three independently-runnable steps, each building
on the previous, each with a clear pass/fail criterion. All three run over
a `tokio::io::duplex` stand-in transport (no QUIC, no TLS) to keep the POC
self-contained and WASM-buildable.

1. **Chunk format + N-channel demux/mux** — generalize TTY's
   `ChunkReader`/`ChunkWriter` to the 9-byte format, build a demux that
   routes chunks to per-channel `mpsc` channels and a mux that frames
   per-channel bytes back onto the transport. Validates the
   decompose→stream→recompose round-trip for 3+ concurrent channels.
2. **Per-channel `Connection` presentation** — wrap each reassembled
   channel as `Connection::from_stream(send, recv, alpn, addr)` and run a
   minimal `ProtocolHandler` (echo) through the full demux→Connection→
   handler→mux path. Validates that the existing `Connection` abstraction
   (landed in the transport-generalization work) is sufficient; no core
   changes needed for the POC.
3. **Tunnel handler** — a `channel/open` with a target address opens a
   `TcpStream` and pumps bidirectionally between the channel and the TCP
   socket. Validates that the bidirectional pump pattern (same as TTY's
   `pump_session`) works as a generic port proxy and that the handler sees
   a `Connection` without knowing it's inside channels.

**Stretch goal:** run the demux/mux sync core under wasm32-unknown-unknown
to validate the "no platform dependencies" claim (WASM compatibility by
construction).

The POC lives at `/workspace/@alkdev/alknet-channels-poc/` (mirroring the
`alknet-tty-poc` convention), depends on `alknet-core` for `Connection`/
`SendStream`/`RecvStream`/`ProtocolHandler`, and does NOT depend on
`alknet-tty`, `alknet-call`, or any other handler crate. The echo handler
(Step 2) and tunnel handler (Step 3) are POC-local stubs, not the real
adapters.

---

## Design Principles (carried from the TTY POC)

### The sync core / async shell split

The TTY POC's REQ-TTY-01 insight (`docs/research/alknet-tty/phase-0-findings.md`
§REQ-TTY-01) was that `portable_pty` is blocking `std::io`, and the POC
bridged it via std threads + tokio mpsc. The channels demux has an analogous
split, but simpler — the "sync" part is pure byte manipulation, not
blocking I/O:

- **Sync core (WASM-compatible, no async)**: `parse_chunk_header(&[u8; 9])
  -> (channel_id, stream_type, length)` and `frame_chunk(channel_id,
  stream_type, payload) -> Vec<u8>`. Pure functions. These are the
  generalization of TTY's `ChunkReader::read_chunk` / `ChunkWriter::
  write_chunk` with the `channel_id` field added.
- **Async shell**: one `read_exact` on the transport to get the 9-byte
  header, one `read_exact` for the payload, then route the payload to the
  right per-channel `mpsc::Sender<Bytes>`. That's the demux loop — one
  async task per transport.
- **Per-channel reassembly**: `mpsc::Receiver<Bytes>` wrapped as
  `AsyncRead` (small adapter, ~30 lines), fed to the handler as a
  `RecvStream` via `RecvStream::from_stream`. The handler's writes go to
  an `mpsc::Sender<Bytes>` wrapped as `AsyncWrite`, which the channels
  layer's per-channel write pump reads from and frames back onto the
  transport.

The channels layer is structured exactly like the TTY POC's bridge: sync
byte manipulation at the core, async shell around it, mpsc channels
connecting the two worlds. The POC validates this is as clean as it was
for TTY.

### Streams are streams

The guiding insight from `phase-0-findings.md`: a TTY session, an SSH
channel, a forwarded TCP connection — they're all just
`AsyncRead + AsyncWrite` handles. The differences are only in how they're
*opened* and what *multiplexing layer* carries them. The POC validates the
multiplexing-layer half; the open-negotiation half is validated by the call
protocol (already in production) and is out of scope for this POC.

### The tunnel reuses the TTY shape, trivially

A tunnel handler receives a channel (a `Connection::from_stream` yielding
one bidi stream), calls `accept_bi()`, gets the `SendStream`/`RecvStream`
pair, opens a `TcpStream` to the local port, and runs two pumps:

- `RecvStream` → `TcpStream` write half (client → target)
- `TcpStream` read half → `SendStream` (target → client)

That's the TTY `pump_session` pattern with two pumps instead of three (no
control, no stderr). The chunk format is the channels layer's concern, not
the tunnel handler's — the handler just sees opaque bytes on its
`SendStream`/`RecvStream`. `TcpStream` is natively async (tokio), so no
blocking→async bridge is needed for the tunnel itself. The "don't have to
async" pattern lives in the channels demux, not in the tunnel handler.

---

## Step 1: Chunk Format + N-Channel Demux/Mux

**Goal:** validate the decompose→stream→recompose round-trip for N
concurrent channels.

### What to build

```rust
// wire.rs — pure sync core, no async, no platform deps

const CHUNK_HEADER_LEN: usize = 9; // channel_id:u32 + stream_type:u8 + length:u32
const MAX_CHUNK_LEN: u32 = 16 * 1024 * 1024;

pub struct ChunkHeader {
    pub channel_id: u32,
    pub stream_type: u8,
    pub length: u32,
}

pub fn parse_header(buf: &[u8; 9]) -> Result<ChunkHeader, ChunkError> { ... }
pub fn write_header(channel_id: u32, stream_type: u8, length: u32, out: &mut [u8; 9]) { ... }

// demux.rs — async shell, one task per transport

pub struct Demux {
    // channel_id → (tx per stream_type)
    channels: HashMap<u32, HashMap<u8, mpsc::Sender<Bytes>>>,
}

impl Demux {
    pub async fn run<R: AsyncRead + Unpin>(&self, transport: R) { ... }
}

pub struct Mux {
    // reads from per-channel mpsc::Receiver<Bytes>, frames chunks onto transport
}

impl Mux {
    pub async fn run<W: AsyncWrite + Unpin>(&self, transport: W) { ... }
}
```

The `Demux` reads 9-byte headers, looks up `(channel_id, stream_type)` in
its map, and sends the payload to the matching `mpsc::Sender`. The `Mux`
reads from per-channel `mpsc::Receiver`s and writes framed chunks onto the
transport. Both sides use `tokio::io::duplex` as the stand-in transport.

Per-channel reassembly: `mpsc::Receiver<Bytes>` wrapped as `AsyncRead`
(`MpscRecvStream` — ~30 lines, mirrors the TTY POC's channel-bridge
adapter). The write side: `mpsc::Sender<Bytes>` wrapped as `AsyncWrite`
(`MpscSendStream` — ~30 lines). These are the POC's `RecvStream`/`SendStream`
backings, constructed via `RecvStream::from_stream` / `SendStream::from_stream`.

### Tests

- `round_trip_single_channel`: one channel, write distinct data on each
  `stream_type`, read back in order, assert integrity.
- `round_trip_three_concurrent_channels`: three channels, each with
  different `stream_type` sets, concurrent writers and readers, assert no
  cross-channel contamination and per-channel order preservation.
- `zero_length_sentinel`: a zero-length chunk is delivered as an empty
  `Bytes` (the EOF sentinel, same as TTY).
- `chunk_too_large`: a chunk with `length > MAX_CHUNK_LEN` returns
  `ChunkTooLarge`, doesn't corrupt the stream.
- `unknown_channel_id`: a chunk with an unallocated `channel_id` is
  dropped with a debug log (or returned as an error — decide in the POC;
  see OQ-CH-12 below).
- `backpressure`: a slow reader on channel A does not block channel B's
  reads (the bounded-buffer property from DP-5).

### Pass criterion

All six tests pass. The sync core (`parse_header`/`write_header`) compiles
under `wasm32-unknown-unknown` (stretch: run the same tests in a WASM test
harness).

### What this validates

- The 9-byte format is a clean generalization of TTY's 5-byte format.
- The sync core is pure and platform-independent.
- The mpsc-bridged async shell scales to N concurrent channels with
  per-channel order preservation and cross-channel isolation.
- The bounded-buffer backpressure (DP-5 option c) works for the simple
  case.

### What this does NOT validate

- `Connection` presentation (Step 2).
- Handler integration (Step 2).
- Real transport (QUIC/TCP/WebTransport).
- Call-protocol orchestration (channel/open etc.).

---

## Step 2: Per-Channel `Connection` Presentation

**Goal:** validate that the existing `Connection::from_stream` is
sufficient to present each channel as a `Connection` a `ProtocolHandler`
can drive unchanged.

### What to build

```rust
// echo_handler.rs — a minimal ProtocolHandler
pub struct EchoHandler;

#[async_trait]
impl ProtocolHandler for EchoHandler {
    fn alpn(&self) -> &'static [u8] { b"alknet/echo" }

    async fn handle(&self, connection: Connection, _auth: &AuthContext)
        -> Result<(), HandlerError>
    {
        let (mut send, mut recv) = connection.accept_bi().await?;
        // echo: read from recv, write to send
        tokio::io::copy(&mut recv, &mut send).await?;
        Ok(())
    }
}
```

The demux loop, on allocating a new channel, constructs a
`Connection::from_stream(send, recv, alpn, remote_addr)` — where `send`
and `recv` are the `MpscSendStream`/`MpscRecvStream` from Step 1 — and
hands it to `EchoHandler::handle`. The handler calls `accept_bi()` once
(yield-once contract), gets the stream pair, and pumps. The handler does
not know it's inside a channels connection.

The test wires both sides: a client-side `Mux`/`Demux` pair and a
server-side `Mux`/`Demux` pair, connected by two `tokio::io::duplex`
pipes (one for each direction). The client writes to a channel's
`SendStream`; the server's `EchoHandler` reads from its `RecvStream` and
writes back to its `SendStream`; the client reads the echo from its
`RecvStream`.

### Tests

- `echo_single_channel`: write 1 KiB to channel 1's `SendStream`, read
  the echo back from channel 1's `RecvStream`, assert equality.
- `echo_three_concurrent_channels`: three channels, each echoing
  concurrently, assert each gets its own data back.
- `handler_sees_alpn`: the `Connection`'s `remote_alpn()` returns the
  ALPN the channel was opened with (validates `from_stream`'s `alpn`
  parameter threads through correctly).
- `handler_loops_accept_bi_gets_one_session`: a handler that loops
  `accept_bi` (like `TtyAdapter`) gets exactly one session per channel,
  then `ConnectionClosed` — the yield-once contract.

### Pass criterion

All four tests pass. The `EchoHandler` is transport-agnostic — it compiles
and runs unchanged whether the `Connection` is a real QUIC connection or a
channels reassembled stream.

### What this validates

- `Connection::from_stream` (landed in transport-generalization) is
  sufficient for per-channel presentation. No new `ConnectionKind` is
  needed for the POC.
- The yield-once contract composes correctly for handlers that loop
  `accept_bi`.
- The `Connection` abstraction's `alpn` and `remote_addr` fields thread
  through correctly for channels.

### What this does NOT validate

- The tunnel proxy (Step 3).
- Real transport.
- Multiple channels of different ALPN types on one connection (that's a
  Step 3 / stretch concern).

---

## Step 3: Tunnel Handler

**Goal:** validate that the bidirectional pump pattern (same as TTY's
`pump_session`) works as a generic port proxy through channels.

### What to build

```rust
// tunnel_handler.rs — a ProtocolHandler that proxies to a TCP target
pub struct TunnelHandler;

#[async_trait]
impl ProtocolHandler for TunnelHandler {
    fn alpn(&self) -> &'static [u8] { b"alknet/tunnel" }

    async fn handle(&self, connection: Connection, _auth: &AuthContext)
        -> Result<(), HandlerError>
    {
        let (mut send, mut recv) = connection.accept_bi().await?;
        // In the POC, the target address is fixed (or passed via a
        // POC-local mechanism — NOT the real channel/open params, which
        // are a call-protocol concern and out of scope).
        let target = "127.0.0.1:0"; // the test's echo server
        let mut tcp = TcpStream::connect(target).await?;
        // Two pumps:
        let (mut tcp_read, mut tcp_write) = tcp.into_split();
        let c2t = tokio::io::copy(&mut recv, &mut tcp_write);
        let t2c = tokio::io::copy(&mut tcp_read, &mut send);
        tokio::try_join!(c2t, t2c)?;
        Ok(())
    }
}
```

The test spins up a local TCP echo server (`tokio::net::TcpListener` with
a per-connection echo task), opens a channel with ALPN `alknet/tunnel`,
writes data to the channel's `SendStream`, and reads the echo back from
the channel's `RecvStream`. The tunnel handler forwards bytes between the
channel and the `TcpStream` — two `tokio::io::copy` pumps, no chunk-format
awareness, no channels-layer awareness.

### Tests

- `tunnel_echo_round_trip`: write 1 KiB to channel's `SendStream` →
  tunnel handler forwards to TCP echo server → response flows back through
  channel to `RecvStream` → assert equality.
- `tunnel_large_payload`: write 1 MiB, assert it round-trips without
  corruption (exercises the bounded-buffer backpressure path — the tunnel
  handler's `AsyncRead` side and the channels demux must not deadlock when
  the TCP echo server is slower than the channel writer).
- `tunnel_concurrent_with_echo_channel`: one tunnel channel and one echo
  channel on the same channels connection, both running concurrently,
  assert neither blocks the other.

### Pass criterion

All three tests pass. The `TunnelHandler` is ~15 lines of handler code and
contains zero chunk-format or channels-layer awareness — it only sees
`Connection`/`SendStream`/`RecvStream`.

### What this validates

- The same concepts behind the TTY crate work as a generic port proxy.
- The bidirectional pump pattern (`pump_session` shape) reuses cleanly
  with two pumps instead of three.
- The handler sees a `Connection` and doesn't know it's inside channels.
- A tunnel channel and an echo channel coexist on one channels connection
  without interference — the multiplexing is transparent.

### What this does NOT validate

- Real `channel/open` with `params: { target: "..." }` — the POC uses a
  fixed target or a POC-local mechanism, not the call-protocol operation
  (which is out of scope).
- TLS, SSH, or WebTransport transports.
- ACL gating (the call protocol's `AccessControl::check` — out of scope).

---

## Stretch Goals

### WASM build of the sync core

`parse_header` / `write_header` compile under `wasm32-unknown-unknown`.
The demux/mux async shell compiles under WASM with `wasm-bindgen-futures`.
This validates the "WASM compatibility by construction" claim — the core
byte manipulation has no platform dependencies, and a browser can run a
channels client in WASM over WebTransport.

### Two different channel types on one connection

Step 3's `tunnel_concurrent_with_echo_channel` already does this
partially. The stretch is to run a TTY-shaped channel (4 stream_types:
0/1/2/3) alongside a tunnel channel (2 stream_types: 0/1) on the same
channels connection, validating that the `stream_types` field at open
time correctly sizes the reassembly and that mixed stream_type sets
coexist.

### Hub relay sketch

Two `Demux`/`Mux` pairs bridged by a byte pump, validating that a chunk
arriving on channel `N` on leg A is re-framed with `channel_id = M` on
leg B (OQ-CH-11). This is a POC-level validation of the hub relay concept
from `phase-0-findings.md` §"The hub relay: ChannelManager-to-
ChannelManager". The POC version does not run the call protocol — it just
validates the byte-pump relay works with `channel_id` remapping.

---

## What the POC Deliberately Does NOT Do

To keep scope de-risked and bounded:

- **No call protocol.** `channel/open`, `channel/close`,
  `channel/control`, `channel/resources` are call-protocol operations
  (`phase-0-findings.md` §Channel Open Negotiation). The POC uses a
  POC-local channel-allocation mechanism (direct `Demux::allocate_channel`
  calls), not the real `OperationRegistry` path. The call protocol is
  already in production; its integration with channels is Phase 1's
  concern, not a POC unknown.
- **No real transport.** `tokio::io::duplex` stands in for QUIC/TCP/
  WebTransport. The transport-generalization work already validated
  `Connection::from_stream` over real transports; the POC reuses that.
- **No ACL.** The call protocol's `AccessControl::check` gates
  `channel/open` in the real system; the POC has no auth.
- **No real adapters.** `EchoHandler` and `TunnelHandler` are POC-local
  stubs. The real `TtyAdapter` / `SshAdapter` / `DockerTtyBackend` are
  unchanged and integrate in Phase 1.
- **No recursive composition.** `alknet/channels` inside
  `alknet/channels` is a natural consequence of the `Connection`
  abstraction but is not a POC goal. The POC validates one level of
  multiplexing.

---

## Open Questions Surfaced by the POC Plan

These are added to `phase-0-findings.md` §Open Questions (OQ-CH-12,
OQ-CH-13, OQ-CH-14):

- **OQ-CH-12 (unknown channel_id on demux)**: when the demux receives a
  chunk with a `channel_id` it has not allocated, what does it do? Options:
  (a) drop with a debug log (lenient — survives transient mis-ordering
  during channel teardown), (b) return a protocol error and close the
  transport (strict — catches bugs but is fragile during teardown). SSH
  is lenient. Recommendation: lenient for v1, with an error counter for
  observability.

- **OQ-CH-13 (core trait for bidi-stream sources)**: the POC uses
  `Connection::from_stream` per channel, which is yield-once. A Phase 1
  refactor may add a `BidiStreamSource` trait to `alknet-core` so
  `ChannelConnection` (many channels, each a bidi stream) is a first-class
  peer of QUIC (many bidi streams) rather than a bag of yield-once
  Connections:

  ```rust
  trait BidiStreamSource: Send + Sync {
      async fn accept_bi(&self) -> Result<(SendStream, RecvStream), StreamError>;
      async fn open_bi(&self) -> Result<(SendStream, RecvStream), StreamError>;
  }
  ```

  with `Connection` holding `Box<dyn BidiStreamSource>`, and QUIC/Iroh/
  Stream/Channels all implementing it. This is additive (existing callers
  keep working via a blanket impl or a `from_stream`-backed default) and
  not a major/pita break. The POC does NOT need this — it validates the
  yield-once path is sufficient — but Phase 1 should evaluate whether the
  trait makes the channels layer and the client-side endpoint (OQ-CH-14)
  cleaner.

- **OQ-CH-14 (client-side channels endpoint)**: both sides of a channels
  connection do the demux/mux work. The server side is a `ProtocolHandler`
  (`ChannelsAdapter::handle`). The client side needs a symmetric type —
  something like `ChannelClient` that opens a transport, runs the demux/
  mux, and exposes `open_channel(alpn, params) -> Channel` to the
  application. This is the channels analogue of `AlknetEndpoint` (server)
  vs `CallClient` (client) in the call protocol. The POC uses a
  POC-local client type; Phase 1 must decide whether this lives in
  `alknet-channels` or is a thin wrapper over `alknet-core`'s endpoint
  types. The `BidiStreamSource` trait (OQ-CH-13) may factor into this —
  if `AlknetEndpoint` and `ChannelClient` both produce `BidiStreamSource`s,
  the client/server symmetry is cleaner.

---

## References

- `docs/research/alknet-channels/phase-0-findings.md` — the research doc
  this POC derisks.
- `docs/research/alknet-tty/phase-0-findings.md` §REQ-TTY-01 — the sync
  core / async shell split pattern this POC generalizes.
- `docs/research/transport-generalization/findings.md` — the
  `Connection::from_stream` / `Connection::from_bidi` work this POC
  builds on.
- `crates/alknet-tty/src/wire.rs` — the 5-byte chunk format
  (`ChunkReader`/`ChunkWriter`) this POC generalizes to 9 bytes.
- `crates/alknet-tty/src/adapter.rs` — `TtyAdapter::handle` and
  `drive_session`/`pump_session` — the per-stream dispatch and
  bidirectional pump pattern the tunnel handler reuses.
- `crates/alknet-core/src/types.rs` — `Connection::from_stream`,
  `SendStream::from_stream`, `RecvStream::from_stream`, `ProtocolHandler`
  — the integration points the POC validates.
- `docs/research/alknet-docker/poc-summary.md` — the docker POC this doc
  mirrors in structure and tone.