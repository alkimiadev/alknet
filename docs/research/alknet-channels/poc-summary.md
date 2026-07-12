---
status: complete
last_updated: 2026-07-12
---

# alknet-channels: POC Research Summary

**Status:** Research complete — all three high-leverage unknowns validated. The
approach is viable; the remaining unknowns are spec-scope, not feasibility.
**Re-verified 2026-07-12** against the post-refactor `alknet-core`
(`BidiStreamSource` trait, ADR-070): all 28 tests still pass, clippy fully
clean, and the POC now uses `AuthContext::anonymous`. The three `alknet-core`
issues the POC surfaced (#1, #2, #3 below) are resolved by ADR-070 /
commit `60cce22`; the four `alknet-channels`-side issues (#4–#7) remain for
Phase 1.
**Date:** 2026-07-12
**Scope:** Captures what the POC proved about the 9-byte chunk format, N-channel
demux/mux, per-channel `Connection` presentation, and the tunnel handler — and
what it surfaced for the coming `alknet-channels` crate spec and the light
`alknet-core` refactor that should land alongside it.

---

## Executive Summary

A POC (`alknet-channels-poc`, `/workspace/alknet-channels-poc`) validated the
three highest-leverage unknowns of the channels layer over a
`tokio::io::duplex` stand-in transport:

1. **Chunk format + N-channel demux/mux** — the 9-byte format
   (`[channel_id:u32 be][stream_type:u8][length:u32 be][payload]`) is a clean
   generalization of TTY's 5-byte format. The sync core (`parse_header`/
   `write_header`) is pure, WASM-compatible by construction. The mpsc-bridged
   async shell scales to N concurrent channels with per-channel order
   preservation and cross-channel isolation, and bounded-buffer backpressure
   (DP-5 option c) works for the simple case.
2. **Per-channel `Connection` presentation** — `Connection::from_stream` (from
   the transport-generalization work) is sufficient to present each channel as
   a `Connection` a `ProtocolHandler` can drive unchanged. A minimal echo
   handler runs through the full demux→Connection→handler→mux path with zero
   channels-layer awareness. The yield-once `accept_bi` contract composes for
   handlers that loop `accept_bi` (like `TtyAdapter`).
3. **Tunnel handler** — the bidirectional pump pattern (same shape as TTY's
   `pump_session`, two pumps instead of three) works as a generic port proxy
   through channels. A `TunnelHandler` of ~15 lines proxies a channel to a
   local TCP echo server with no chunk-format or channels-layer awareness, and
   a tunnel channel coexists with an echo channel on one channels connection
   without interference.

**28 tests pass** (5 wire codec + 7 mpsc stream adapters + 5 demux + 3 mux + 5
echo handler + 3 tunnel handler). Clippy is fully clean for the POC's own code
and for the updated `alknet-core` dependency (the ADR-065 `Connection::close`
unused-arg warnings the POC originally surfaced are resolved by ADR-070 — see
§"Issues Surfaced" #2).

**Stretch goal validated:** the sync core (`wire.rs`) compiles cleanly under
`wasm32-unknown-unknown`. The "WASM compatibility by construction" claim holds
for the pure-byte-manipulation core; the async shell and the `alknet-core`
dependency graph are NOT WASM-clean yet (transitive `getrandom`/`rand` deps),
which is a Phase 1 concern, not a POC concern.

The POC depends on `alknet-core` only (path dependency,
`default-features = false` — no QUIC needed for the POC), and uses
`tokio::io::duplex` as the stand-in transport. It does NOT depend on
`alknet-tty`, `alknet-call`, or any other handler crate. The echo and tunnel
handlers are POC-local stubs, not the real adapters.

---

## The Sync Core / Async Shell Split (Confirmed)

The POC confirms the split proposed in `poc-plan.md` §"Design Principles":

- **Sync core (`wire.rs`)**: `parse_header(&[u8; 9]) -> ChunkHeader` and
  `write_header(channel_id, stream_type, length, &mut [u8; 9])`. Pure
  functions, no async, no platform deps. Validates `MAX_CHUNK_LEN` (matches
  TTY's 16 MiB cap, preserving the framing-disambiguation soundness property
  from ADR-052 §5). Compiles under `wasm32-unknown-unknown`.
- **Async shell (`demux.rs` / `mux.rs`)**: `Demux::run` reads 9-byte headers +
  payloads off the transport via `read_exact`, routes payloads to per-
  `(channel_id, stream_type)` bounded `mpsc::Sender<Bytes>`. `Mux` frames
  `Bytes` from per-channel `mpsc::Receiver<Bytes>` back onto the transport.
- **Per-channel reassembly (`mpsc_stream.rs`)**: `MpscRecvStream`
  (`mpsc::Receiver<Bytes>` → `AsyncRead`, ~50 lines) and `MpscSendStream`
  (`mpsc::Sender<Bytes>` → `AsyncWrite` with backpressure, ~60 lines). These
  back the `RecvStream`/`SendStream` the handler sees via
  `RecvStream::from_stream` / `SendStream::from_stream`.

The split is as clean as the TTY POC's was. The "don't have to async" pattern
lives in the demux/mux shell; the core byte manipulation is pure and
WASM-portable.

---

## POC Target 1: Chunk Format + N-Channel Demux/Mux

**Question:** Does the 9-byte format decompose and recombine N concurrent
streams correctly, with per-channel order preservation and cross-channel
isolation?

**Answer:** Yes. The demux routes chunks to per-`(channel_id, stream_type)`
bounded `mpsc::Sender`s; the mux frames `Bytes` from per-channel receivers
back onto the transport. Three concurrent channels with interleaved writes
round-trip with no cross-channel contamination and per-channel order
preserved (`demux_three_concurrent_channels_no_cross_contamination`,
`mux_demux_round_trip_three_concurrent_channels`).

**Lenient unknown-`channel_id` handling (OQ-CH-12):** a chunk with an
unallocated `channel_id` (or an unallocated `stream_type` on an allocated
channel) is dropped with a debug log and an error counter, and the demux
continues (`demux_unknown_channel_drops_lenient`). This matches SSH's
behavior and survives transient mis-ordering during teardown. The error
counter is exposed via `Demux::stats()` for observability. This is the
recommended v1 behavior from the plan; the POC validates it works.

**Zero-length sentinel (TTY convention carried forward):** a zero-length
chunk is delivered as an empty `Bytes`, which `MpscRecvStream` interprets as
EOF (`demux_zero_length_sentinel_delivered`, `recv_zero_length_sentinel_is_eof`).
This is the same convention as TTY's zero-length stdin/stdout chunks. See
§"Issues Surfaced" below for a subtlety about how shutdown propagates this.

**Chunk-too-large:** a chunk with `length > MAX_CHUNK_LEN` returns
`ChunkTooLarge` and does not corrupt the stream
(`demux_chunk_too_large_does_not_panic`).

**Backpressure (DP-5 option c):** each `(channel_id, stream_type)` has an
independent bounded `mpsc` buffer. A slow reader on one channel does not block
another channel's reads — the demux's per-chunk `route` awaits the matching
sender without holding a global lock. The 1 MiB tunnel test
(`tunnel_large_payload`) exercises this end-to-end and does not deadlock.

---

## POC Target 2: Per-Channel `Connection` Presentation

**Question:** Is `Connection::from_stream` sufficient to present each channel
as a `Connection` a `ProtocolHandler` can drive unchanged?

**Answer:** Yes. The `EchoHandler` (~5 lines of handler code) runs through the
full demux→Connection→handler→mux path with zero channels-layer awareness.
It calls `connection.accept_bi().await`, gets a `(SendStream, RecvStream)`
pair, and pumps via `tokio::io::copy`. The handler does not know it is inside
a channels connection.

**Wiring (`ChannelEndpoint`):** the POC's local equivalent of the symmetric
client/server types (OQ-CH-14). `channel_endpoint()` constructs both sides of
a channels connection (server demux/mux + client demux/mux, connected by two
`tokio::io::duplex` pipes). `open_echo_channel(channel_id)` allocates a
channel on both sides: the server side wraps `MpscSendStream`/`MpscRecvStream`
as `Connection::from_stream(..., b"alknet/echo", None)` and spawns
`EchoHandler::handle` with an `AuthContext::anonymous(b"alknet/echo")`;
the client side returns a `ClientChannel { send, recv }` the test drives. This
is the POC-local shape Phase 1 will generalize into `ChannelsAdapter::handle`
(server) and `ChannelClient` (client).

**ALPN threading:** `handler_sees_alpn` validates that `Connection::from_stream`'s
`alpn` parameter threads through correctly — the handler's
`connection.remote_alpn()` returns the ALPN the channel was opened with.

**Yield-once contract:** `handler_loops_accept_bi_gets_one_session` validates
that a handler that loops `accept_bi` (like `TtyAdapter`) gets exactly one
session per channel, then `ConnectionClosed` on the second call. The
yield-once `Connection::from_stream` path composes correctly for looping
handlers.

---

## POC Target 3: Tunnel Handler

**Question:** Does the bidirectional pump pattern (same shape as TTY's
`pump_session`, two pumps instead of three) work as a generic port proxy
through channels?

**Answer:** Yes. The `TunnelHandler` (~15 lines of handler code) opens a
`TcpStream` to a fixed target and runs two `tokio::io::copy` pumps:
`recv → tcp_write` (client → target) and `tcp_read → send` (target → client).
The handler contains zero chunk-format or channels-layer awareness — it
only sees `Connection`/`SendStream`/`RecvStream`.

**Shutdown-on-completion (subtlety):** the naive `tokio::try_join!(c2t, t2c)`
deadlocks — `t2c` waits forever for the TCP echo server to close, which only
happens once the handler closes `tcp_write`, which only happens after
`try_join` returns. The POC fixes this by shutting down the opposite sink
when each pump completes: `c2t` shuts down `tcp_write` on EOF, `t2c` shuts
down `send` on EOF. This is the `pump_session` shape with an explicit
shutdown-on-completion step that the TTY adapter doesn't need (because TTY's
three pumps coordinate via the exit-code future). The tunnel's two-pump
shape needs it. See §"Issues Surfaced" below.

**Tests:**
- `tunnel_echo_round_trip` — 1 KiB round-trips through the tunnel to a TCP
  echo server.
- `tunnel_large_payload` — 1 MiB round-trips without corruption or deadlock,
  exercising the bounded-buffer backpressure path (the tunnel handler's
  `AsyncRead` side and the channels demux must not deadlock when the TCP
  echo server is slower than the channel writer).
- `tunnel_concurrent_with_echo_channel` — one tunnel channel and one echo
  channel on the same channels connection, both running concurrently, neither
  blocks the other. The multiplexing is transparent.

---

## Issues Surfaced (For the Spec and the Core Refactor)

These are the things the POC ran into that should be addressed in Phase 1 or
in the light `alknet-core` refactor landing alongside it. They are grouped by
where the fix likely lives.

> **Update 2026-07-12 (post-refactor):** issues #1, #2, and #3 below are
> **resolved** by ADR-070 (`BidiStreamSource` trait) and commit `60cce22`
> (`refactor(core): implement BidiStreamSource trait + AuthContext::anonymous`).
> The POC was re-verified against the updated core: all 28 tests still pass,
> clippy is now fully clean (the upstream `Connection::close` unused-arg
> warnings are gone), and the POC's handler tests were updated to use the new
> `AuthContext::anonymous(alpn)` constructor (removing the four-`None`-field
> literal that recurred across `echo_handler.rs` and `tunnel_handler.rs`).
> The POC still uses `Connection::from_stream` (the yield-once path) — see
> the note on #1 below for why that remains the correct POC scope.

### In `alknet-core` (the refactor)

#### 1. `Connection::from_stream` is yield-once — `BidiStreamSource` would be cleaner (OQ-CH-13, confirmed +EV) — ✅ RESOLVED by ADR-070

The POC validates the yield-once path is sufficient, as the plan predicted.
But wiring `ChannelEndpoint::open_echo_channel` made the awkwardness concrete:
each channel constructs a fresh `Connection::from_stream` that yields one bidi
stream, and the endpoint holds a bag of these connections (one per channel)
rather than a single `ChannelConnection` that yields N streams. The
`BidiStreamSource` trait proposed in OQ-CH-13 would make `ChannelConnection`
a first-class peer of QUIC (many bidi streams) instead of a bag of yield-once
connections.

**Resolved.** ADR-070 landed the `BidiStreamSource` trait
(`crates/alknet-core/src/types.rs:364`) with `accept_bi`/`open_bi`/
`remote_addr`/`close`, and `Connection` now holds `Box<dyn BidiStreamSource>`.
`Connection` is open for extension: the channels crate (Phase 1) will
implement `ChannelBidiStreamSource` in its own crate, without a core edit.
The `StreamBidiStreamSource` (yield-once) impl is the compatibility path —
existing callers and this POC keep working via `Connection::from_stream`,
which now wraps a `StreamBidiStreamSource`.

> **POC scope note:** the POC keeps `Connection::from_stream` (the yield-once
> path) rather than implementing `BidiStreamSource` directly. This was the
> correct POC scope — the POC's job was to validate the yield-once path is
> *sufficient* (it is) and surface the +EV refactor (ADR-070 confirms it).
> Building a `ChannelConnection` that yields N streams is Phase 1's job, now
> unblocked. ADR-070's constructor table originally implied a public
> `Connection::from_source(impl BidiStreamSource)` constructor for downstream
> crates that wasn't yet exposed; that gap was surfaced by this POC and has
> since been closed by commit `e8bbc74` (`pub fn from_source(source: impl
> BidiStreamSource, alpn: Vec<u8>)` at `types.rs:574`). The POC was
> deliberately **not retrofitted** to use `from_source` + a
> `ChannelBidiStreamSource`: the POC's objective (de-risk the three
> high-leverage unknowns, surface the +EV changes) was reached, the issues it
> surfaced are resolved, and rewiring the POC to the N-stream shape now would
> be rework that the Phase 1 crate will do authoritatively anyway. The POC
> stands as the de-risk artifact; Phase 1 builds on the now-unblocked trait
> and constructor.

#### 2. `Connection::close` has unused params (`code`, `reason`) for the stream backend — ✅ RESOLVED by ADR-070 (REQ-CORE-02)

`crates/alknet-core/src/types.rs:500` — the `Stream` backend of `Connection::close`
took `code: u32, reason: &str` and used neither (clippy flagged both as
unused). The stream backend just dropped the inner stream. This was a leak of
the QUIC-centric `close(code, reason)` shape into a backend that has no such
concept.

**Resolved.** ADR-070 §REQ-CORE-02 chose option (b): keep the QUIC-shaped
signature on the trait (so `Connection::close(code, reason)` callers are
unchanged), and the `StreamBidiStreamSource::close` impl prefixes the args
with `_code`/`_reason` and documents why they're ignored ("the drop is the
close — ADR-065 §Negative"). The clippy warning under
`--no-default-features` (the POC's build mode) is gone — verified by
`cargo clippy -p alknet-core --no-default-features` returning clean.

#### 3. `AuthContext` construction is verbose for tests and POCs — ✅ RESOLVED (REQ-CORE-03)

Every POC handler test constructed an `AuthContext` literal with four `None`
fields and a hardcoded ALPN. The real assembly layer gets one from the
endpoint; POCs and tests repeated the boilerplate. A
`AuthContext::anonymous(alpn)` helper in `alknet-core` would remove this.

**Resolved.** `crates/alknet-core/src/auth.rs:90` adds
`pub fn anonymous(alpn: impl Into<Vec<u8>>) -> Self` (no `test-utils` feature
gate — it's a plain `pub fn` useful for any caller that constructs an
`AuthContext` outside the endpoint's resolution path). The POC's
`echo_handler.rs` and `tunnel_handler.rs` were updated to use it, replacing
three four-`None`-field literals with `AuthContext::anonymous(b"alknet/echo")`
/ `AuthContext::anonymous(b"alknet/tunnel")`.

### In `alknet-channels` (the spec / Phase 1)

#### 4. `MpscSendStream::poll_shutdown` must emit a zero-length sentinel

The POC's `MpscSendStream::poll_shutdown` sends an empty `Bytes` (the EOF
sentinel) before dropping the sender. Without this, the demux never sees EOF
on the channel's `stream_type`, and `tokio::io::copy` in the handler never
completes — the test hangs. The TTY crate's `pump_session` emits the
zero-length stdout sentinel explicitly via `Chunk::stdout(Bytes::new())`;
the channels layer's per-channel write pump does NOT forward a sentinel on
sender-drop, so the send adapter must do it.

**Implication for Phase 1:** the `ChannelConnection` write-half contract
must specify that `AsyncWrite::shutdown` (or the `BidiStreamSource`
equivalent) emits a zero-length sentinel so the peer sees EOF. This is a
wire-level invariant, not just an implementation detail — both sides must
agree, or channels will hang on clean shutdown. The TTY crate already
follows this convention; the channels crate should codify it.

#### 5. The mux needs dynamic registration (handle/runner split)

The plan's `Mux::run(self, transport)` shape (consume the mux, run pumps for
pre-registered channels) does not compose with the "open a channel after the
run loop started" API that `ChannelEndpoint::open_echo_channel` needs. The
POC split `Mux` into `MuxHandle` (clone-able, `register(channel_id,
stream_type) -> Sender<Bytes>` at any time) and `MuxRunner` (owns the
transport, `select!`s on new-pump registrations). This is the shape Phase 1
should adopt — the alternative (pre-registering all channels before run)
doesn't match the dynamic `channel/open` model.

The split adds one `mpsc::UnboundedSender` + `Arc<Mutex<HashMap>>` per mux,
which is cheap. The runner's `select!` loop exits when all `MuxHandle` clones
drop (the `new_pumps` sender closes), which is the natural shutdown signal.

#### 6. The demux must drop all channel senders on transport EOF

`Demux::run` clears its `channels` map on exit (transport closed) so every
handler's `MpscRecvStream` sees EOF even without an explicit zero-length
sentinel arriving on the wire. Without this, `read_to_end` / `tokio::io::copy`
in handlers hangs forever waiting for a sender that never drops because the
demux task is holding the map. This is a teardown invariant: transport close
→ all channel senders drop → all handlers see EOF → all handler tasks exit.
Phase 1 should specify this as part of the `ChannelsAdapter::handle` contract.

#### 7. The tunnel's two-pump shape needs explicit shutdown-on-completion

`tokio::try_join!(c2t, t2c)` deadlocks for the tunnel: each pump waits for the
other's EOF, which only comes once the *opposite* pump completes and shuts
down its sink. The fix (shut down the peer's sink when one pump completes) is
small but easy to get wrong. The TTY adapter doesn't have this problem
because its three pumps coordinate via the `exit_code` future, which is a
third signal. The tunnel has no such signal — it's purely pump-driven.

**Implication for Phase 1:** the tunnel handler (and any future handler
with a pump-driven two-direction shape) must shut down the opposite sink on
pump completion, not just `try_join` the two pumps. This is worth a
documented pattern in the channels spec, or — now that `BidiStreamSource`
has landed (ADR-070) — a helper in `alknet-core` that encapsulates the
"two-pump with shutdown-on-completion" shape so handlers don't reimplement
it. The TTY crate's `pump_session` is the three-pump version; the two-pump
version is the tunnel's shape and will recur (e.g. an SSH `direct-tcpip`
channel is the same two-pump shape).

### Cosmetic / clippy

#### 8. "Very complex type" for the `reserve_owned` future

`MpscSendStream` stores an in-flight `reserve_owned()` future for
backpressure. The concrete future type is anonymous (async fn), so it's
boxed as `Pin<Box<dyn Future<Output = Result<OwnedPermit<Bytes>, SendError<()>>> + Send>>`.
Clippy flags this as "very complex type." The POC factored it into a
`type ReserveFut = ...` alias, which silences clippy but doesn't reduce the
complexity. The clean fix is `tokio_util::io::PollSender`-style helper that
wraps an `mpsc::Sender<T>` as `AsyncWrite` for `T: Into<Bytes>` — but that
requires `tokio-util` as a dep and a small adapter. Phase 1 should consider
whether to extract this adapter (it's ~50 lines and will recur in any
mpsc-backed stream) or keep it POC-local. Low priority.

---

## What the POC Does NOT Validate

Following the docker POC summary's pattern:

1. **The call protocol.** `channel/open`, `channel/close`, `channel/control`,
   `channel/resources` are call-protocol operations. The POC uses a POC-local
   channel-allocation mechanism (`Demux::allocate_channel` /
   `MuxHandle::register`), not the real `OperationRegistry` path. The call
   protocol is already in production; its integration with channels is Phase 1's
   concern, not a POC unknown.
2. **Real transport.** `tokio::io::duplex` stands in for QUIC/TCP/WebTransport.
   The transport-generalization work already validated `Connection::from_stream`
   over real transports; the POC reuses that.
3. **ACL.** The call protocol's `AccessControl::check` gates `channel/open`
   in the real system; the POC has no auth.
4. **Real adapters.** `EchoHandler` and `TunnelHandler` are POC-local stubs.
   The real `TtyAdapter` / `SshAdapter` / `DockerTtyBackend` are unchanged and
   integrate in Phase 1.
5. **Recursive composition.** `alknet/channels` inside `alknet/channels` is a
   natural consequence of the `Connection` abstraction but is not a POC goal.
6. **Hub relay.** The stretch goal (two `Demux`/`Mux` pairs bridged by a byte
   pump with `channel_id` remapping, OQ-CH-11) was not built — the three primary
   targets were the higher-leverage unknowns. The POC's demux/mux shape would
   support it, but the relay sketch is left for Phase 1.
7. **WASM test harness.** The sync core compiles under
   `wasm32-unknown-unknown`; running the same tests in a WASM test harness
   (`wasm-bindgen-test`) was the secondary stretch and was not set up. The
   compile check validates the "no platform dependencies" claim for the core;
   the async shell and `alknet-core` dep graph are not WASM-clean yet (transitive
   `getrandom`/`rand`), which is a Phase 1 concern.

---

## Test Coverage

```
running 28 tests
test wire::tests::parse_max_length ... ok
test wire::tests::parse_preserves_channel_id_max ... ok
test wire::tests::parse_round_trip ... ok
test wire::tests::parse_too_large ... ok
test wire::tests::parse_zero_length ... ok
test mpsc_stream::tests::recv_dropped_sender_is_eof ... ok
test mpsc_stream::tests::recv_partial_then_leftover ... ok
test mpsc_stream::tests::recv_reads_bytes_in_order ... ok
test mpsc_stream::tests::recv_zero_length_sentinel_is_eof ... ok
test mpsc_stream::tests::send_after_close_errors_broken_pipe ... ok
test mpsc_stream::tests::send_backpressure_pends_then_completes ... ok
test mpsc_stream::tests::send_writes_then_shutdown_closes ... ok
test demux::tests::demux_chunk_too_large_does_not_panic ... ok
test demux::tests::demux_routes_to_allocated_channel ... ok
test demux::tests::demux_three_concurrent_channels_no_cross_contamination ... ok
test demux::tests::demux_unknown_channel_drops_lenient ... ok
test demux::tests::demux_zero_length_sentinel_delivered ... ok
test mux::tests::mux_demux_round_trip_three_concurrent_channels ... ok
test mux::tests::mux_frames_single_chunk_round_trip ... ok
test mux::tests::mux_zero_length_sentinel_round_trips ... ok
test echo_handler::tests::echo_single_channel ... ok
test echo_handler::tests::echo_single_channel_1kib ... ok
test echo_handler::tests::echo_three_concurrent_channels ... ok
test echo_handler::tests::handler_loops_accept_bi_gets_one_session ... ok
test echo_handler::tests::handler_sees_alpn ... ok
test tunnel_handler::tests::tunnel_concurrent_with_echo_channel ... ok
test tunnel_handler::tests::tunnel_echo_round_trip ... ok
test tunnel_handler::tests::tunnel_large_payload ... ok

test result: ok. 28 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

The 1 MiB `tunnel_large_payload` test exercises the bounded-buffer
backpressure path end-to-end (channel writer faster than TCP echo server,
no deadlock). The three `echo_*` tests validate the full
demux→Connection→handler→mux path. The five `wire::*` tests are pure sync
core, WASM-portable.

---

## POC Structure

```
alknet-channels-poc/
  Cargo.toml          — depends on alknet-core (path, no default features), tokio, tokio-util, bytes, async-trait, thiserror, tracing
  src/
    lib.rs            — module docs; the three-step POC overview
    wire.rs           — sync core: 9-byte ChunkHeader, parse_header, write_header (WASM-clean)
    mpsc_stream.rs    — MpscRecvStream (AsyncRead), MpscSendStream (AsyncWrite + backpressure + sentinel-on-shutdown)
    demux.rs          — Demux: route chunks to per-(channel,stream_type) bounded mpsc senders; lenient on unknown ids; drop senders on EOF
    mux.rs            — MuxHandle/MuxRunner: dynamic per-channel write pumps; frames Bytes onto the shared transport
    echo_handler.rs   — EchoHandler (ProtocolHandler) + ChannelEndpoint (POC-local client/server wiring) + open_echo_channel
    tunnel_handler.rs — TunnelHandler (two-pump TcpStream proxy) + tcp echo server test helper
```

No `tests/` directory — tests are inline per-module (`#[cfg(test)] mod tests`),
matching the alknet-core / alknet-tty convention (vs. the docker POC's
separate `tests/integration.rs`, which matched bollard's style).

---

## Key Code-to-Concept Mappings

| POC concept | alknet-core equivalent | alknet-tty equivalent |
|---|---|---|
| `wire::parse_header`/`write_header` | — | `wire::ChunkReader::read_chunk` / `ChunkWriter::write_chunk` (5-byte, no `channel_id`) |
| `MpscRecvStream` (`AsyncRead`) | backs `RecvStream::from_stream` | the channel-bridge adapter in the TTY POC |
| `MpscSendStream` (`AsyncWrite`) | backs `SendStream::from_stream` | `TestStdinSink` in TTY adapter tests |
| `Demux` | — | (new — TTY has no per-channel demux; it's single-channel) |
| `MuxHandle`/`MuxRunner` | — | (new — TTY has `ChunkWriter` directly, no mux) |
| `ChannelEndpoint` | the symmetric client/server types (OQ-CH-14) | — |
| `Connection::from_stream(send, recv, alpn, addr)` | `types.rs:405` | `TtyAdapter::handle` receives a `Connection` the same way |
| `EchoHandler::handle` | `ProtocolHandler::handle` | `TtyAdapter::handle` (loops `accept_bi`) |
| `TunnelHandler::handle` two pumps | — | `pump_session` (three pumps — stdout/stderr + client→backend + exit) |
| zero-length sentinel = EOF | — | `wire.rs` §Sentinels (ADR-052) |

---

## References

- POC plan: `docs/research/alknet-channels/poc-plan.md` — the detailed
  de-risk POC plan this summary reports against.
- Phase 0 findings: `docs/research/alknet-channels/phase-0-findings.md` — the
  research doc this POC derisks (OQ-CH-12/13/14 all surfaced there).
- TTY wire format: `crates/alknet-tty/src/wire.rs` — the 5-byte
  `ChunkReader`/`ChunkWriter` this POC generalizes to 9 bytes.
- TTY adapter: `crates/alknet-tty/src/adapter.rs` — `TtyAdapter::handle` and
  `drive_session`/`pump_session` (the per-stream dispatch and three-pump
  bidirectional pattern the tunnel handler reuses with two pumps).
- Core types: `crates/alknet-core/src/types.rs` — `Connection::from_stream`
  (`:541`), `Connection::from_source` (`:574`, ADR-070's downstream extension
  point — `impl BidiStreamSource` → `Connection`, no core edit), `SendStream::
  from_stream` (`:267`), `RecvStream::from_stream` (`:289`), `ProtocolHandler`
  (`:220`), `BidiStreamSource` trait (`:364`, ADR-070) — the integration
  points the POC validates. The POC uses `Connection::from_stream`
  (yield-once); the channels crate (Phase 1) will use `from_source` with its
  own `ChannelBidiStreamSource` impl.
- Core auth: `crates/alknet-core/src/auth.rs` — `AuthContext::anonymous`
  (`:90`, REQ-CORE-03), used by the POC's echo and tunnel handler tests.
- ADR-070: `docs/architecture/decisions/070-bidistreamsource-trait.md` — the
  `BidiStreamSource` trait decision this POC's findings prompted (resolves
  Issues #1 and #2 below; REQ-CORE-01/02).
- ADR-065: `docs/architecture/decisions/065-connection-from-stream-generic-single-stream.md`
  — the `Connection::from_stream` yield-once contract the POC validated and
  the `StreamBidiStreamSource` impl now preserves.
- Docker POC summary: `docs/research/alknet-docker/poc-summary.md` — the
  POC summary this doc mirrors in structure and tone.
- TTY POC: `/workspace/alknet-tty-poc/` — the POC convention this POC follows
  (sibling-of-`@alkdev` location, standalone Cargo crate, inline tests).