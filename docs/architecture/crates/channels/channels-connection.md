---
status: draft
last_updated: 2026-07-12
---

# channels-connection.md — ChannelBidiStreamSource and Sub-Stream Access

How a reassembled channel is presented to its handler as a `Connection`.
ADR-074 is the decision; this doc specifies the API shape and the two
access paths.

## What

Each channel is reassembled into a set of **unidirectional** handles — one
per active `stream_type` (declared at `channel/open` time, ADR-073). Every
stream_type is unidirectional (ADR-071 §stream_type decomposition);
bidirectionality is two stream_types (write + read), not one shared
"bidirectional" stream. Write stream_types (`% 3 == 0`) carry a
`SendStream`; read stream_types (`% 3 == 1 or 2`) carry a `RecvStream`.

These handles are wrapped as a `ChannelBidiStreamSource` that implements
`alknet-core`'s `BidiStreamSource` trait (ADR-070), and a `Connection` is
constructed from it via `Connection::from_source(source, alpn)`.

The handler receives a `Connection` and can either:
1. Call `accept_bi()` once to get the main data pair (`stream_type` 0/1) —
   the generic handler path (tunnel, SSH).
2. Call `into_sub_streams()` on the `ChannelBidiStreamSource` to get all
   active sub-streams as typed `(stream_type, SubStreamHandle)` tuples —
   the typed handler path (TTY, which needs stdin/stdout/stderr/control-in/
   control-out).

Both paths operate on the same reassembly buffers; the difference is how the
handler accesses them.

## `ChannelBidiStreamSource`

```rust
// In alknet-channels:

pub struct ChannelBidiStreamSource {
    // The reassembly buffers for this channel's active stream_types,
    // plus the mux handle for writing back onto the transport.
    // Constructed by ChannelManager::build_channel_connection (ADR-075).
    ...
}

#[async_trait]
impl BidiStreamSource for ChannelBidiStreamSource {
    async fn accept_bi(&self)
        -> Result<(SendStream, RecvStream), StreamError>
    {
        // Yields the (stream_type 0, stream_type 1) pair on first call,
        // ConnectionClosed on subsequent calls. Yield-once per channel,
        // matching the POC's validated shape.
    }

    async fn open_bi(&self)
        -> Result<(SendStream, RecvStream), StreamError>
    {
        // StreamClosed — a single channel cannot open new application
        // streams (same as ADR-065's Stream backend). Additional sub-streams
        // (stream_type 2, 3) are accessed via into_sub_streams(), not
        // open_bi().
    }

    fn remote_addr(&self) -> Option<SocketAddr> { ... }

    fn close(&self, _code: u32, _reason: &str) { ... }
}
```

One `ChannelBidiStreamSource` instance represents **one channel** (not the
whole channels connection). The `ChannelManager` (ADR-075) constructs one
per channel at `channel/open` time and wraps it in a `Connection` via
`from_source`.

## The generic path: `accept_bi()`

For handlers that only need the main data pair (`stream_type` 0 = data-in,
`stream_type` 1 = data-out):

```rust
// Tunnel handler — ~15 lines, zero channels-layer awareness
async fn handle(&self, connection: Connection, _auth: &AuthContext)
    -> Result<(), HandlerError>
{
    let (mut send, mut recv) = connection.accept_bi().await?;
    let mut tcp = TcpStream::connect(target).await?;
    let (mut tcp_read, mut tcp_write) = tcp.into_split();

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

The handler calls `accept_bi()` once, gets the `(SendStream, RecvStream)`
pair, and pumps. It does not know it's inside a channels connection — the
`Connection` looks like any other. This is the path the POC's `EchoHandler`
and `TunnelHandler` validated.

`accept_bi()` is yield-once: the first call returns the 0/1 pair; subsequent
calls return `ConnectionClosed`. This matches the POC's validated shape and
the `StreamBidiStreamSource` yield-once contract (ADR-070).

## The typed path: `into_sub_streams()`

For handlers that need `stream_type` 2 (stderr) or 3 (control) in addition
to 0/1:

```rust
// In alknet-channels-core:
pub struct ChannelSubStreams {
    /// (stream_type, handle) for each active stream_type. Each handle is
    /// unidirectional: write stream_types (0, 3, 6, ...) carry a SendStream;
    /// read stream_types (1, 2, 4, 5, 7, ...) carry a RecvStream.
    /// See ADR-071 §stream_type decomposition.
    pub streams: Vec<(u8, SubStreamHandle)>,
}

pub enum SubStreamHandle {
    Send(SendStream),  // write half (stream_type % 3 == 0)
    Recv(RecvStream),  // read half (stream_type % 3 == 1 or 2)
}

impl ChannelBidiStreamSource {
    /// Returns all active sub-streams, keyed by stream_type. Consumes the
    /// source — call this instead of accept_bi() if the handler needs
    /// direct access to stream_types 2/3/4.
    pub fn into_sub_streams(self) -> ChannelSubStreams { ... }
}
```

The handler crate destructures `ChannelSubStreams` into its typed names:

```rust
// In alknet-tty (inside-channels mode, ADR-077):
let sub = channel_source.into_sub_streams();
let stdin = sub.get_send(0).unwrap();    // SendStream (write, client→server)
let stdout = sub.get_recv(1).unwrap();   // RecvStream (read, server→client)
let stderr = sub.get_recv(2);           // Option<RecvStream> (read, optional)
let ctrl_in = sub.get_send(3).unwrap(); // SendStream (write, client→server)
let ctrl_out = sub.get_recv(4).unwrap();// RecvStream (read, server→client)
```

**Every stream_type is unidirectional** (ADR-071). The channels crate
exposes `(stream_type, SubStreamHandle)` tuples. The handler crate maps
stream_types to its typed names. This preserves ADR-003's
no-handler-depends-on-another-handler rule and keeps the channels crate
ALPN-blind.

`into_sub_streams()` consumes the source — a handler can't call both
`accept_bi()` and `into_sub_streams()`. This is by design: the sub-streams
include the 0/1 pair, so `into_sub_streams()` is the superset.

## Choosing the path

| Handler shape | Path | Examples |
|---------------|------|---------|
| Main data pair only (0/1) | `accept_bi()` | tunnel, SSH (SSH multiplexes internally) |
| Needs stderr/control (2/3/4) | `into_sub_streams()` | TTY (stdin/stdout/stderr/ctrl-in/ctrl-out) |

The handler chooses based on its ALPN's `stream_type` set (declared at
`channel/open` time). The `ChannelsAdapter` (ADR-075) passes the handler a
`Connection` (via `from_source`); handlers that need sub-streams access the
`ChannelBidiStreamSource` via a channels-crate extension trait or downcast
(exact ergonomics are an implementation detail for the channels crate; the
contract is that both paths are available and the handler crate chooses).

## Recursive composition

A `ChannelBidiStreamSource` is a `BidiStreamSource`, and `Connection::
from_source` wraps it. A handler that is itself `alknet/channels` can open a
sub-channels connection on a data channel — `alknet/channels` inside
`alknet/channels`. This is allowed (the `Connection` abstraction permits it)
but not a feature designed for. The primary use case is one level of
multiplexing. Recursive composition is a natural consequence of the
abstraction, not a goal.

## What does NOT change

- **`ProtocolHandler` trait** (ADR-002) — handlers still receive a
  `Connection` and call `accept_bi()`. The `ChannelBidiStreamSource` is
  internal to the channels crate; handlers see a `Connection`.
- **`SendStream` / `RecvStream`** (ADR-007) — unchanged. They continue to
  wrap their internal sources. `ChannelBidiStreamSource` constructs them via
  the existing `from_stream` constructors, backed by mpsc reassembly
  buffers.
- **`HandlerRegistry`** — unchanged. The channels layer looks up ALPNs in
  the same registry as top-level connections.

## Design Decisions

All design decisions are documented as ADRs in [decisions/](../../decisions/).

| ADR | Decision | Summary |
|-----|----------|---------|
| [074](../../decisions/074-channelconnection-bidistreamsource.md) | ChannelConnection | Per-channel `BidiStreamSource`; yield-once `accept_bi`; `into_sub_streams()` accessor |
| [070](../../decisions/070-bidistreamsource-trait.md) | BidiStreamSource Trait | The extension point `ChannelBidiStreamSource` implements |
| [065](../../decisions/065-connection-from-stream-generic-single-stream.md) | `Connection::from_stream` | The yield-once path generalized for channels |

## References

- ADR-074: ChannelConnection (the decision)
- ADR-070: BidiStreamSource trait
- ADR-065: `Connection::from_stream`
- ADR-077: TTY inside channels (the primary consumer of `into_sub_streams`)
- `docs/research/alknet-channels/poc-summary.md` §POC Target 2 (the
  yield-once `Connection::from_stream` validation)