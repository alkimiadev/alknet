# ADR-075: ChannelsAdapter and ChannelManager

## Status

Accepted (amended 2026-07-18 by ADR-093 — demux reads 8-byte headers, not
9-byte; one reassembly buffer per `channel_id` (not per
`(channel_id, stream_type)`); `ChannelState.stream_types` removed; the
channels layer has no `stream_type` concept — see "Amendment (ADR-093,
2026-07-18)" below)

## Amendment (ADR-093, 2026-07-18)

The demux loop reads **8-byte headers** (not 9-byte). `ChannelState` has
**one reassembly buffer per `channel_id`** (not per
`(channel_id, stream_type)`), yielding a `BiStream` to the handler. The
`stream_types: Vec<u8>` field on `ChannelState` is **removed**. The
`ChannelManager` has no `stream_type` concept — it routes by `channel_id`
only, and the handler owns its sub-stream multiplexing on the `BiStream`
it receives (per ADR-093, the channels-layer consequence of ADR-092's
`BiStream` handler leaf).

The body below describes the **original** (9-byte, per-stream_type) shape;
the amendment above is the operative decision. See ADR-093 for the
resolution rationale and the cross-ADR impacts.

## Context

The channels crate has two internal components, split by responsibility
(`docs/research/alknet-channels/phase-0-findings.md` §Channel Manager and
Connection Internals):

1. **`ChannelsAdapter`** — implements `ProtocolHandler` for
   `alknet/channels`. Its `handle()` receives one `Connection` (the
   transport), reads 9-byte chunk headers, and routes each chunk. It is the
   read/demux half.

2. **`ChannelManager`** — the shared state both halves touch. It holds the
   map of `channel_id → ChannelState`, the `HandlerRegistry` reference, and
   the `OperationRegistry` reference. It is the reassemble/allocate half.
   It is what the `channel/open` operation handler closes over.

The de-risk POC (`docs/research/alknet-channels/poc-summary.md` §Issues
Surfaced) surfaced three invariants the spec must pin: the mux needs dynamic
registration (handle/runner split — REQ-CH-03), the demux must drop all
channel senders on transport EOF (REQ-CH-02), and the `AsyncWrite::shutdown`
must emit a zero-length sentinel (REQ-CH-01). This ADR pins these as
contracts.

## Decision

### `ChannelsAdapter` — the read/demux half

```rust
#[async_trait]
impl ProtocolHandler for ChannelsAdapter {
    fn alpn(&self) -> &'static [u8] { b"alknet/channels" }

    async fn handle(&self, connection: Connection, auth: &AuthContext)
        -> Result<(), HandlerError>
    {
        // 1. Channel 0 is pre-negotiated as alknet/call (ADR-072).
        //    The first bidi stream the transport yields is channel 0.
        let (send, recv) = connection.accept_bi().await?;
        self.manager.preinstall_channel_0(send, recv, auth).await?;

        // 2. Accept remaining bidi streams and read 9-byte headers off each.
        //    On an in-line transport (TCP+TLS, WebTransport), accept_bi()
        //    yields once and the header demuxes N channels from that stream.
        //    On QUIC native, accept_bi() yields repeatedly — each stream
        //    carries one logical channel, and the header provides
        //    stream_type + channel_id correlation. Same code path, same
        //    wire format (ADR-071 §substrate modes).
        self.manager.run_demux_loop(connection).await
    }
}
```

The `preinstall_channel_0` step constructs the reassembly buffers for
`channel_id = 0` using stream_types [0, 1] (ADR-072), wraps them as a
`Connection` (via `Connection::from_source` with a `ChannelBidiStreamSource`
— ADR-074), and hands that `Connection` to the `CallAdapter` — exactly as if
`alknet/call` had been the top-level ALPN. The `CallAdapter` is looked up in
the same `HandlerRegistry` as every other ALPN.

`run_demux_loop` continues accepting bidi streams from the transport. For
each stream, it reads 9-byte headers and routes payloads to the matching
`(channel_id, stream_type)` reassembly buffer. On an in-line transport,
there is only one stream (channel 0 rides inside it via the header); the
header demuxes all channels. On QUIC, each subsequent stream is a new
channel; the header's `channel_id` correlates it. The loop is the same;
only the transport's stream count differs.

### `ChannelManager` — the shared state

```rust
pub struct ChannelManager {
    /// channel_id → per-channel state. Channel 0 is pre-inserted at
    /// construction by preinstall_channel_0.
    channels: Mutex<HashMap<u32, ChannelState>>,
    /// The handler registry for looking up ALPNs on channel/open.
    handlers: Arc<HandlerRegistry>,
    /// The call protocol's operation registry, so channel/open etc. can be
    /// registered at assembly time.
    call_ops: Arc<OperationRegistry>,
    /// Next server-assigned channel_id. Monotonic; wraps at u32::MAX.
    next_id: AtomicU32,
    /// Per-channel reassembly buffer cap (ADR-076). Default 1 MiB.
    buffer_cap: usize,
    /// Per-connection channel limit (ADR-076). Default 256.
    max_channels: usize,
}

struct ChannelState {
    /// The ALPN this channel carries, for routing and observability.
    alpn: String,
    /// Reassembly buffers per active stream_type.
    streams: HashMap<u8, ReassemblyBuffer>,
    /// The handler task driving this channel. Dropping this aborts it.
    handler_task: JoinHandle<()>,
    /// Which stream_types are active (from the open negotiation).
    stream_types: Vec<u8>,
}
```

`ChannelManager` is `Clone` (cheap — `Arc` internally) so the
`ChannelsAdapter`, the `channel/open` operation handler, and relay logic
can all hold a handle.

### The demux loop — REQ-CH-02 and REQ-CH-04

`run_demux_loop` reads 9-byte headers, looks up `channel_id` in `channels`,
and pushes the payload into the right `ReassemblyBuffer` for `(channel_id,
stream_type)`.

**REQ-CH-04 (lenient unknown-channel_id):** a chunk with an unallocated
`channel_id` (or `stream_type`) is dropped with a debug log and an error
counter (exposed via `Demux::stats()`), and the demux continues. This
matches SSH's behavior and survives transient mis-ordering during teardown.
Validated by the POC (`demux_unknown_channel_drops_lenient`).

**REQ-CH-02 (transport close → all handlers see EOF):** on transport EOF,
the demux loop clears its `channels` map, dropping all `ReassemblyBuffer`
senders. Every handler's reassembled `RecvStream` sees EOF even without an
explicit zero-length sentinel on the wire. Without this, `read_to_end` /
`tokio::io::copy` in handlers hangs forever waiting for a sender that never
drops. This is a teardown invariant of the `ChannelsAdapter::handle`
contract. Validated by the POC.

### The mux — REQ-CH-03 (handle/runner split)

The mux frames per-channel bytes back onto the transport. The POC surfaced
that the plan's `Mux::run(self, transport)` shape (consume the mux, run
pumps for pre-registered channels) does not compose with the dynamic
`channel/open` model — channels are opened after the run loop starts.

**REQ-CH-03 (dynamic registration):** the mux is split into:

- **`MuxHandle`** — clone-able, `register(channel_id, stream_type) ->
  Sender<Bytes>` callable at any time (after the runner has started).
- **`MuxRunner`** — owns the transport, `select!`s on new-pump registrations
  and per-channel write pumps.

The runner's `select!` loop exits when all `MuxHandle` clones drop (the
`new_pumps` sender closes), which is the natural shutdown signal. This
matches the dynamic `channel/open` model. The split adds one
`mpsc::UnboundedSender` + `Arc<Mutex<HashMap>>` per mux — cheap. Validated
by the POC.

### `ChannelManager` is ALPN-blind and auth-blind

The `ChannelManager` deliberately does **not** hold:

- **No `ProtocolHandler` implementations.** It holds a `HandlerRegistry`
  reference for ALPN lookup, but it doesn't *be* a handler. Handlers live in
  their crates and register on the same registry.
- **No ALPN-specific parsing.** It does not parse `NegotiateRequest` JSON,
  SSH frames, or tunnel target strings. It hands `params` JSON to the
  handler and gets back a handler task; it hands `stream_type 3` JSON to the
  handler's control handle.
- **No auth state.** Auth lives in the `OperationContext` that the call
  protocol passes to `channel/open`. The `ChannelManager` doesn't check
  scopes or ownership — that's `AccessControl::check` in
  `OperationRegistry::invoke`, run before the `channel/open` handler.
- **No transport coupling.** It talks to the transport only through the
  `ChannelsAdapter`'s read loop and the per-channel write pumps, both of
  which use `AsyncRead + AsyncWrite`.

This is what makes the channels layer WASM-compatible and transport-agnostic
— the `ChannelManager` is pure byte routing with no platform or protocol
dependencies.

### The `channel/open` handler — threading into `OperationRegistry`

The `channel/open` (and `channel/close`, `channel/control`,
`channel/resources/subscribe`) operations are registered on the call
protocol's `OperationRegistry` at assembly time. The handler closures close
over a `ChannelManager` clone:

```rust
let channel_ops = ChannelOperations::new(manager.clone());
channel_ops.register_on(&mut call_registry)?;
```

The `channel/open` handler (ADR-073) looks up the ALPN in `HandlerRegistry`,
allocates the `channel_id` via `next_id.fetch_add(1, Relaxed)`, constructs
the `ChannelBidiStreamSource` (ADR-074), spawns the handler task, and
records the `ChannelState`. The key insight: spawning the handler task is
identical to what `TtyAdapter::handle` does today — `tokio::spawn` a
session-driving task. The only difference is the `Connection` passed in is
backed by chunk reassembly rather than a quinn connection.

## Consequences

**Positive:**
- The ChannelsAdapter/ChannelManager split mirrors the TTY crate's
  ChunkReader/ChunkWriter + adapter pattern, generalized to N channels.
- The demux/mux contracts (REQ-CH-01..04) are pinned as wire-level
  invariants, not implementation details. Both sides must agree, or channels
  hang on clean shutdown.
- The `ChannelManager` is ALPN-blind, auth-blind, and transport-blind — the
  channels layer is a re-framing proxy, not a protocol engine. This is what
  makes it reusable across TTY, SSH, tunnel, and future ALPNs.

**Negative:**
- The mux handle/runner split (REQ-CH-03) adds one `mpsc::UnboundedSender` +
  `Arc<Mutex<HashMap>>` per mux. Cheap, but more moving parts than the
  pre-register-all-then-run alternative. The alternative doesn't match the
  dynamic `channel/open` model, so the split is necessary, not optional.
- The demux loop is one task per transport. If the demux task panics, all
  channels on that transport lose their read side. The teardown invariant
  (REQ-CH-02) ensures handlers see EOF, not a hang — but a panic in the
  demux is still a transport-wide failure. This is the same property as any
  single-task read loop (including the call protocol's dispatch loop).

## Door type

**One-way (contracts) + two-way (internals).** The wire-level invariants
(REQ-CH-01..04) are one-way — both sides must agree, and changing them
after deployments exist is a protocol migration. The `ChannelManager`'s
internal structure (fields, `Arc<Mutex<HashMap>>` vs a concurrent map, etc.)
is two-way — implementation details that can change without breaking the
contract.

## References

- ADR-071: channels wire format (the chunks the demux reads, as amended
  by ADR-093 — 8-byte header)
- ADR-093: channels pure channel multiplexing (amends this ADR — 8-byte
  header, one reassembly buffer per channel, no `stream_type` concept)
- ADR-072: channel 0 pre-negotiated (the `preinstall_channel_0` step)
- ADR-073: channel lifecycle operations (the ops registered on `call_ops`)
- ADR-074: ChannelBidiStreamSource (the per-channel source the manager
  constructs, as amended by ADR-093 — `accept_bi` yields a `BiStream`)
- ADR-076: backpressure, channel limits, ID reuse (the `buffer_cap` /
  `max_channels` / reuse invariants)
- `docs/research/alknet-channels/poc-summary.md` §Issues Surfaced #4-#6
  (REQ-CH-01, 02, 03)