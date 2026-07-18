---
status: draft
last_updated: 2026-07-18
---

# channels-connection.md — ChannelBidiStreamSource and `BiStream` Access

How a reassembled channel is presented to its handler as a `Connection`.
ADR-074 (amended by ADR-093) is the decision; this doc specifies the API
shape — one accessor, one `BiStream` per channel.

## What

Each channel is reassembled into a `BiStream` — a single duplex
(`AsyncRead + AsyncWrite`) byte stream. The channels layer strips its
8-byte header (`channel_id` + `length`) on read, hands the payload to the
reassembled `BiStream`, and the handler parses its own framing from the
payload. The handler sub-multiplexes its `BiStream` however it wants —
TTY sub-demuxes `stream_type` from its `BiStream` via its 5-byte format,
tunnel uses the `BiStream` as raw bytes, call length-prefixes JSON, SSH
runs its own channel protocol.

The `BiStream` is wrapped in a `ChannelBidiStreamSource` that implements
`alknet-core`'s `BidiStreamSource` trait (ADR-070), and a `Connection` is
constructed from it via `Connection::from_source(source, alpn)`. The
handler receives a `Connection`, calls `accept_bi()` once (yield-once per
channel), gets a `BiStream`, and drives its session — identical to how it
works on a top-level QUIC connection.

## `ChannelBidiStreamSource`

```rust
// In alknet-channels:

pub struct ChannelBidiStreamSource {
    // The reassembly buffer for this channel's payload bytes (one per
    // channel_id, not per (channel_id, stream_type) — the channels layer
    // has no stream_type concept), plus the mux handle for writing back
    // onto the transport. Constructed by ChannelManager::build_channel_connection
    // (ADR-075).
    ...
}

#[async_trait]
impl BidiStreamSource for ChannelBidiStreamSource {
    async fn accept_bi(&self)
        -> Result<BiStream, StreamError>
    {
        // Yields the channel's BiStream on first call,
        // ConnectionClosed on subsequent calls. Yield-once per channel,
        // matching the POC's validated shape.
    }

    async fn open_bi(&self)
        -> Result<BiStream, StreamError>
    {
        // StreamClosed — a single channel cannot open new application
        // streams (same as ADR-065's Stream backend). The handler owns
        // its sub-stream multiplexing on the BiStream it received.
    }

    fn remote_addr(&self) -> Option<SocketAddr> { ... }

    fn close(&self, _code: u32, _reason: &str) { ... }
}
```

One `ChannelBidiStreamSource` instance represents **one channel** (not the
whole channels connection). The `ChannelManager` (ADR-075) constructs one
per channel at `channel/open` time and wraps it in a `Connection` via
`from_source`.

## The single path: `accept_bi()`

Every handler — TTY, tunnel, SSH, call — receives a `Connection`, calls
`accept_bi()` once, gets a `BiStream`, and sub-multiplexes it however it
wants. There is one accessor; the two-accessor design
(`accept_bi` vs `into_sub_streams`) from ADR-074's original shape is
removed by ADR-093.

```rust
// Tunnel handler — ~15 lines, zero channels-layer awareness
async fn handle(&self, connection: Connection, _auth: &AuthContext)
    -> Result<(), HandlerError>
{
    let mut bidi = connection.accept_bi().await?;
    let mut tcp = TcpStream::connect(target).await?;
    let (mut tcp_read, mut tcp_write) = tcp.into_split();
    let (mut recv, mut send) = tokio::io::split(&mut bidi);

    // Two-pump with shutdown-on-completion (ADR-078)
    let c2t = async {
        tokio::io::copy(&mut recv, &mut tcp_write).await?;
        tcp_write.shutdown().await.ok();
        Ok::<_, std::io::Error>(())
    };
    let t2c = async {
        tokio::io::copy(&mut tcp_read, &mut send).await?;
        send.shutdown().await.ok();  // emits zero-length sentinel (REQ-CH-01)
        Ok::<_, std::io::Error>(())
    };
    tokio::try_join!(c2t, t2c)?;
    Ok(())
}
```

```rust
// TTY handler (inside-channels mode, ADR-077 reversed by ADR-093) —
// the SAME code as direct mode, just a different BiStream source.
async fn handle(&self, connection: Connection, _auth: &AuthContext)
    -> Result<(), HandlerError>
{
    let mut bidi = connection.accept_bi().await?;
    // drive_session reads the 5-byte TTY chunks off `bidi` — the same
    // code as direct mode. The channels layer stripped its 8-byte
    // header; TTY's 5-byte format is the payload.
    drive_session(bidi, backends, ownership, identity).await
}
```

The handler calls `accept_bi()` once, gets a `BiStream`, and pumps. It
does not know it's inside a channels connection — the `Connection` looks
like any other. This is the path the POC's `EchoHandler` and
`TunnelHandler` validated.

`accept_bi()` is yield-once: the first call returns the `BiStream`;
subsequent calls return `ConnectionClosed`. This matches the POC's
validated shape and the `StreamBidiStreamSource` yield-once contract
(ADR-070, ADR-092).

## Recursive composition

A `ChannelBidiStreamSource` is a `BidiStreamSource`, and
`Connection::from_source` wraps it. A handler that is itself
`alknet/channels` can open a sub-channels connection on a data channel —
`alknet/channels` inside `alknet/channels`. The outer layer strips its
8-byte header; the inner layer parses its own 8-byte header from the
payload. Each level is the same shape: `BiStream → accept_bi → N
BiStreams`. The recursion is unbounded and uniform at every level.

This is a property, not a feature. The primary use case is one level of
multiplexing. But the add/strip composition makes it cleaner than
ADR-071's group framing did — the recursion is the same operation
(strip an 8-byte header) at every level, not a different framing per
level.

## What does NOT change

- **`ProtocolHandler` trait** (ADR-002) — handlers still receive a
  `Connection` and call `accept_bi()`. The `ChannelBidiStreamSource` is
  internal to the channels crate; handlers see a `Connection`.
- **`BiStream`** (ADR-092) — the leaf type `accept_bi` returns. The
  channels layer yields `BiStream`s; handlers parse them per their ALPN.
- **`HandlerRegistry`** — unchanged. The channels layer looks up ALPNs in
  the same registry as top-level connections.

## Design Decisions

All design decisions are documented as ADRs in [decisions/](../../decisions/).

| ADR | Decision | Summary |
|-----|----------|---------|
| [074](../../decisions/074-channelconnection-bidistreamsource.md) | ChannelConnection | Per-channel `BidiStreamSource`; yield-once `accept_bi` (amended by ADR-093 — `into_sub_streams` removed, `accept_bi` is the only accessor) |
| [093](../../decisions/093-channels-pure-channel-multiplexing.md) | channels Pure Channel Multiplexing | The umbrella decision: 8-byte header, no `stream_type`, `into_sub_streams` removed, `BiStream`-only |
| [070](../../decisions/070-bidistreamsource-trait.md) | BidiStreamSource Trait | The extension point `ChannelBidiStreamSource` implements |
| [092](../../decisions/092-bistream-as-the-handler-leaf.md) | `BiStream` as the Handler Leaf | `accept_bi` returns `BiStream` (the transport-leaf decision this doc builds on) |
| [065](../../decisions/065-connection-from-stream-generic-single-stream.md) | `Connection::from_stream` | The yield-once path generalized for channels |

## References

- ADR-074: ChannelConnection (the decision, amended by ADR-093)
- ADR-093: channels pure channel multiplexing (the umbrella decision)
- ADR-070: BidiStreamSource trait
- ADR-092: `BiStream` as the handler leaf
- ADR-065: `Connection::from_stream` (the yield-once path generalized)
- ADR-077: TTY inside channels (reversed by ADR-093 — TTY always uses
  its 5-byte format, carried transparently in the channels payload)
- `docs/research/alknet-channels/poc-summary.md` §POC Target 2 (the
  yield-once `Connection::from_stream` validation)
- `docs/research/stream-unification/findings.md` — the research that
  surfaced the single-accessor resolution