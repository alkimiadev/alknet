---
status: draft
last_updated: 2026-07-12
---

# channels-adapter.md — ChannelsAdapter and ChannelManager

The two internal components of the channels crate: the read/demux half
(`ChannelsAdapter`) and the reassemble/allocate half (`ChannelManager`).
ADR-075 is the decision; this doc specifies the contracts and the demux/mux
invariants.

## The split

| Component | Role | What it knows |
|-----------|------|---------------|
| `ChannelsAdapter` | `ProtocolHandler` on `alknet/channels`; reads 9-byte chunk headers off every bidi stream the transport yields and routes to `ChannelManager`. Substrate-agnostic (ADR-071 §substrate modes). | The transport stream(s); the `ChannelManager` handle. ALPN-blind. |
| `ChannelManager` | Shared state; holds `channel_id → ChannelState`, `HandlerRegistry`. Constructs `ChannelBidiStreamSource` per channel. What `channel/open` closes over (in `channels-call`). | The channel map; the handler registry for ALPN lookup. ALPN-blind (looks up ALPNs, doesn't parse their protocols). |

The split mirrors the TTY crate's `ChunkReader`/`ChunkWriter` + adapter
pattern, generalized to N channels: the adapter drives N channels, and
channel 0 is special only in that it's pre-allocated (by `channels-call`).

## `ChannelsAdapter::handle` (substrate-agnostic)

```rust
#[async_trait]
impl ProtocolHandler for ChannelsAdapter {
    fn alpn(&self) -> &'static [u8] { b"alknet/channels" }

    async fn handle(&self, connection: Connection, auth: &AuthContext)
        -> Result<(), HandlerError>
    {
        // 1. Channel 0 is pre-negotiated (ADR-072). The first bidi stream
        //    the transport yields is channel 0. The consumer (channels-call)
        //    installs the CallAdapter on it.
        let (send, recv) = connection.accept_bi().await?;
        self.manager.preinstall_channel_0(send, recv, auth).await?;

        // 2. Accept remaining bidi streams and read 9-byte headers off each.
        //    On an in-line transport, accept_bi() yields once and the header
        //    demuxes N channels from that stream. On QUIC native, accept_bi()
        //    yields repeatedly — each stream carries one logical channel.
        //    Same code path, same wire format (ADR-071 §substrate modes).
        self.manager.run_demux_loop(connection).await
    }
}
```

The `preinstall_channel_0` step (provided by `channels-call`, ADR-081)
constructs the reassembly buffers for `channel_id = 0` using stream_types
[0, 1] (ADR-072), wraps them as a `Connection` via `Connection::from_source`
with a `ChannelBidiStreamSource` (ADR-074), and hands that `Connection` to
the `CallAdapter`. The `ChannelsAdapter` in `channels-core` exposes the
hook; `channels-call` provides the implementation.

`run_demux_loop` continues accepting bidi streams from the transport. For
each stream, it reads 9-byte headers and routes payloads to the matching
`(channel_id, stream_type)` reassembly buffer. On an in-line transport,
there is only one stream (channel 0 rides inside it via the header); the
header demuxes all channels. On QUIC, each subsequent stream is a new
channel; the header's `channel_id` correlates it. The loop is the same;
only the transport's stream count differs.

## `ChannelManager`

```rust
// In alknet-channels-core:
pub struct ChannelManager {
    channels: Mutex<HashMap<u32, ChannelState>>,
    handlers: Arc<HandlerRegistry>,
    // Note: no call_ops field — the call-protocol coupling lives in
    // channels-call (ADR-081). The ChannelManager is ALPN-blind and
    // call-protocol-blind.
    next_id: AtomicU32,       // monotonic; wraps at u32::MAX
    buffer_cap: usize,        // default 1 MiB (ADR-076)
    max_channels: usize,      // default 256 (ADR-076)
}

struct ChannelState {
    alpn: String,
    streams: HashMap<u8, ReassemblyBuffer>,
    handler_task: JoinHandle<()>,
    stream_types: Vec<u8>,
}
```

`ChannelManager` is `Clone` (cheap — `Arc` internally) so the
`ChannelsAdapter`, the `channel/open` operation handler, and relay logic can
all hold a handle.

> **Type-name convention:** `ChannelManager`, `ChannelsAdapter`,
> `ChannelBidiStreamSource`, `ChannelSubStreams`, and `ChannelClient` are
> the public API surface (contract). `ReassemblyBuffer`, `Demux`,
> `MuxHandle`/`MuxRunner`, `MpscSendStream`/`MpscRecvStream`, and
> `ChannelOperations` are illustrative internal type names — the channels
> crate's implementation may name them differently. The contracts are the
> invariants (REQ-CH-01..04, 06) and the public API; the internal names are
> not contractual.

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

## The `channel/open` handler

The `channel/open` (and `channel/close`, `channel/control`,
`channel/resources/subscribe`) operations are registered on the call
protocol's `OperationRegistry` at assembly time:

```rust
let channel_ops = ChannelOperations::new(manager.clone());
channel_ops.register_on(&mut call_registry)?;
```

The `channel/open` handler (ADR-073):
1. ACL is already checked by `OperationRegistry::invoke` before this handler
   runs.
2. Looks up the ALPN in `HandlerRegistry` → `channel:unknown_alpn` if
   missing.
3. Allocates the `channel_id` via `next_id.fetch_add(1, Relaxed)` (DP-1:
   server-assigned).
4. Constructs the `ChannelBidiStreamSource` (ADR-074) for the negotiated
   `stream_types`.
5. Spawns the handler task — `tokio::spawn(handler.handle(conn, &auth))`.
   Identical to what `TtyAdapter::handle` does today, but on a
   channels-backed `Connection`.
6. Records the `ChannelState`.
7. Returns the `channel_id`.

## Demux invariants (REQ-CH-02, 04)

### REQ-CH-02: transport close → all channel senders drop → all handlers see EOF

On transport EOF, `run_demux_loop` clears the `channels` map, dropping all
`ReassemblyBuffer` senders. Every handler's reassembled `RecvStream` sees
EOF even without an explicit zero-length sentinel on the wire. Without this,
`read_to_end` / `tokio::io::copy` in handlers hangs forever waiting for a
sender that never drops. This is a teardown invariant of the
`ChannelsAdapter::handle` contract.

### REQ-CH-04: lenient unknown-`channel_id` handling

A chunk with an unallocated `channel_id` (or `stream_type`) is dropped with
a debug log and an error counter (exposed via `Demux::stats()`), and the
demux continues. This matches SSH's behavior and survives transient
mis-ordering during teardown. Validated by the POC
(`demux_unknown_channel_drops_lenient`).

## Mux invariants (REQ-CH-03)

### REQ-CH-03: dynamic registration (handle/runner split)

The mux frames per-channel bytes back onto the transport. The POC surfaced
that `Mux::run(self, transport)` (consume, run pre-registered pumps) does
not compose with the dynamic `channel/open` model — channels are opened
after the run loop starts.

The mux is split into:

- **`MuxHandle`** — clone-able, `register(channel_id, stream_type) ->
  Sender<Bytes>` callable at any time after the runner starts.
- **`MuxRunner`** — owns the transport, `select!`s on new-pump registrations
  and per-channel write pumps.

The runner's `select!` loop exits when all `MuxHandle` clones drop (the
`new_pumps` sender closes) — the natural shutdown signal. This matches the
dynamic `channel/open` model.

## The two-pump pattern (ADR-078 — documented here for handler authors)

Handlers with a two-pump shape (two `tokio::io::copy` pumps, one per
direction — tunnel, SSH `direct-tcpip`) MUST shut down the opposite sink
when one pump completes. `tokio::try_join!` alone deadlocks: each pump
waits for the other's EOF, which only comes after the opposite pump shuts
down its sink.

```rust
let c2t = async {
    tokio::io::copy(&mut recv, &mut tcp_write).await?;
    tcp_write.shutdown().await.ok();  // shut down the peer's sink
    Ok::<_, std::io::Error>(())
};
let t2c = async {
    tokio::io::copy(&mut tcp_read, &mut send).await?;
    send.shutdown().await.ok();  // shut down the peer's sink (emits sentinel — REQ-CH-01)
    Ok::<_, std::io::Error>(())
};
tokio::try_join!(c2t, t2c)?;
```

The three-pump pattern (TTY's `pump_session`, coordinating via the
`exit_code` future) does not have this deadlock — the `exit_code` future is
the third signal. The two-pump pattern is documented in ADR-078; the
shutdown-on-completion contract is a handler-level concern, not a
channels-layer one.

## The hub relay interface

The hub relay (ADR-079) uses the `ChannelManager`'s interface to bridge two
channels connections:

```rust
// For channel_id=7 on browser side, channel_id=12 on spoke side:
tokio::spawn(async move {
    let (b_send, b_recv) = browser_mgr.open_channel_stream(7, stream_type).await;
    let (s_send, s_recv) = spoke_mgr.open_channel_stream(12, stream_type).await;
    tokio::join!(
        pump(b_recv, s_send),  // browser → spoke (with channel_id rewrite)
        pump(s_recv, b_send),  // spoke → browser (with channel_id rewrite)
    );
});
```

The relay reads opaque bytes off one `ChannelManager`'s reassembled stream
and writes them onto the other's write-half, which re-chunks them with the
other leg's `channel_id`. The relay does not parse the bytes — it doesn't
know if they're TTY chunks, SSH frames, or tunnel data. The hub translates
`channel/open` on channel 0 (re-issues on the spoke leg with
`forwarded_for`); data channels are byte-forwarded with `channel_id`
rewrite. See ADR-079 for the full relay contract.

## Design Decisions

All design decisions are documented as ADRs in [decisions/](../../decisions/).

| ADR | Decision | Summary |
|-----|----------|---------|
| [075](../../decisions/075-channelsadapter-and-channelmanager.md) | ChannelsAdapter and ChannelManager | The split; the contracts |
| [076](../../decisions/076-backpressure-channel-limits-id-reuse.md) | Backpressure, Limits, ID Reuse | Bounded-buffer, 256-channel cap, monotonic IDs |
| [078](../../decisions/078-two-pump-shutdown-on-completion.md) | Two-Pump Pattern | Shutdown-on-completion contract |
| [079](../../decisions/079-hub-relay-translate-not-forward.md) | Hub Relay | Translate channel 0, byte-forward data channels |

## References

- ADR-075: ChannelsAdapter and ChannelManager (the decision)
- ADR-072: channel 0 pre-negotiated (the `preinstall_channel_0` step)
- ADR-073: channel lifecycle operations (the ops registered on `call_ops`)
- ADR-074: ChannelBidiStreamSource (what the manager constructs per channel)
- ADR-076: backpressure and limits (`buffer_cap`, `max_channels`)
- `docs/research/alknet-channels/poc-summary.md` §Issues Surfaced #4-#7
  (REQ-CH-01..04, the two-pump deadlock)