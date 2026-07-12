# ADR-074: ChannelConnection — BidiStreamSource over Chunk Reassembly

## Status

Accepted

## Context

ADR-070 landed the `BidiStreamSource` trait and `Connection::from_source`
extension point so downstream crates can implement their own connection
shapes without a core edit. The channels crate is the first downstream
consumer: a channels connection carries N logical channels, each a
bidirectional byte stream presented to a `ProtocolHandler` as a `Connection`.

The phase-0 research (`docs/research/alknet-channels/phase-0-findings.md`
§The Channel Connection Abstraction, §OQ-CH-10) proposed that
`ChannelConnection` *implements* the `Connection` interface (for recursion
and generic handlers) **and** can be destructured into typed sub-stream
handles (`TtyChannel { stdin, stdout, stderr, control }`). The research
recommended "the TTY crate destructures; channels exposes `(channel_id,
stream_type) → (SendStream, RecvStream)` accessors" but did not pin the
exact API shape. This ADR pins it.

The de-risk POC (`docs/research/alknet-channels/poc-summary.md` §POC Target
2) validated that `Connection::from_stream` (the yield-once path) is
sufficient — an echo `ProtocolHandler` runs through the full
demux→Connection→handler→mux path with zero channels-layer awareness. But
the POC deliberately used the yield-once path (one `Connection` per channel)
rather than the N-stream `ChannelBidiStreamSource` shape. This ADR commits
to the N-stream shape that ADR-070 unblocked.

## Decision

### `ChannelBidiStreamSource` implements `BidiStreamSource`

The channels crate defines a `ChannelBidiStreamSource` that implements
`alknet-core`'s `BidiStreamSource` trait (ADR-070). One
`ChannelBidiStreamSource` instance represents **one channel** (not the
whole channels connection). Its `accept_bi()` yields one bidi stream — the
`(stream_type 0, stream_type 1)` pair for that channel — then returns
`ConnectionClosed` on subsequent calls (yield-once per channel, matching
the POC's validated shape).

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
    async fn accept_bi(&self) -> Result<(SendStream, RecvStream), StreamError> {
        // Yields the (stream_type 0, stream_type 1) pair on first call,
        // ConnectionClosed on subsequent calls. This is the yield-once
        // contract per channel, matching the POC's validated shape.
    }
    async fn open_bi(&self) -> Result<(SendStream, RecvStream), StreamError> {
        // StreamClosed — a single channel cannot open new application
        // streams (same as ADR-065's Stream backend). Additional sub-streams
        // (stream_type 2, 3) are accessed via sub_streams(), not open_bi().
    }
    fn remote_addr(&self) -> Option<SocketAddr> { ... }
    fn close(&self, _code: u32, _reason: &str) { ... }
}
```

Each channel is presented to its handler as a `Connection` constructed via
`Connection::from_source(ChannelBidiStreamSource::new(...), alpn)`. The
handler calls `accept_bi()` once, gets the main data pair, and drives its
session — exactly as the POC's `EchoHandler` and `TtyAdapter` do today.

### Sub-stream accessor for typed destructure (OQ-CH-10)

Some handlers need access to `stream_type` 2 (stderr), 3 (control in), and
4 (control out) in addition to the main 0/1 pair. The `Connection`
interface alone (accept_bi) only exposes the 0/1 pair. The channels crate
provides a typed-accessor extension:

```rust
// In alknet-channels:
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
    /// Returns the typed sub-streams for this channel, keyed by stream_type.
    /// Consumes the source — call this instead of accept_bi() if the handler
    /// needs direct access to stream_types 2/3/4. For handlers that only need
    /// the main 0/1 pair, accept_bi() is the path (and sub_streams() is not
    /// called).
    pub fn into_sub_streams(self) -> ChannelSubStreams { ... }
}
```

The handler crate (e.g., `alknet-tty`) destructures `ChannelSubStreams` into
its typed names:

```rust
// In alknet-tty (inside-channels mode, ADR-077):
let sub = channel_source.into_sub_streams();
let stdin = sub.get_send(0).unwrap();    // SendStream (write, client→server)
let stdout = sub.get_recv(1).unwrap();   // RecvStream (read, server→client)
let stderr = sub.get_recv(2);           // Option<RecvStream> (read, optional)
let ctrl_in = sub.get_send(3).unwrap(); // SendStream (write, client→server)
let ctrl_out = sub.get_recv(4).unwrap();// RecvStream (read, server→client)
```

**Every stream_type is unidirectional** (ADR-071). Write stream_types
(`% 3 == 0`) carry a `SendStream`; read stream_types (`% 3 == 1 or 2`) carry
a `RecvStream`. There is no "bidirectional" stream_type — bidirectionality
is two halves (e.g., control is 3 write + 4 read). This resolves the TTY
control channel's "not actually bidirectional" flaw: the TTY adapter reads
exit/keepalive from `ctrl_out` (stream_type 4) and writes resize/signal/eof
to `ctrl_in` (stream_type 3), each with its own flow control and EOF.

**The channels crate does not know about TTY's `stream_type` semantics.**
It exposes `(stream_type, SubStreamHandle)` tuples. The handler crate maps
stream_types to its typed names. This preserves ADR-003's
no-handler-depends-on-another-handler rule and keeps the channels crate
ALPN-blind.

### When to use `accept_bi` vs `into_sub_streams`

| Handler shape | Path | Example |
|---------------|------|---------|
| Main data pair only (0/1) | `accept_bi()` | tunnel handler, SSH handler (SSH multiplexes internally) |
| Needs stderr/control (2/3) | `into_sub_streams()` | TTY handler (stdin/stdout/stderr/control) |

The handler chooses at construction time based on its ALPN's `stream_type`
set (declared at `channel/open` time, ADR-073). The `ChannelsAdapter` passes
the handler a `Connection` (via `from_source`); handlers that need sub-
streams downcast or receive the `ChannelBidiStreamSource` directly via a
channels-crate extension trait. The exact ergonomics (downcast vs. a
channels-crate constructor that hands the source directly to handlers that
opt in) are an implementation detail for the channels crate; the contract is
that both paths are available and the handler crate chooses.

### Recursive composition

A `ChannelBidiStreamSource` is a `BidiStreamSource`, and `Connection::
from_source` wraps it. A handler that is itself `alknet/channels` can open a
sub-channels connection on a data channel. This is recursive composition:
`alknet/channels` inside `alknet/channels`. It is allowed (the `Connection`
abstraction permits it) but not a feature designed for — the primary use
case is one level of multiplexing. Recursive composition is a natural
consequence of the abstraction, not a goal.

## Consequences

**Positive:**
- `ChannelConnection` is a first-class peer of QUIC: one
  `BidiStreamSource` impl per channel, constructed via `from_source` — no
  core edit (the ADR-070 extension point).
- Handlers that only need the main data pair use `accept_bi()` — identical
  to how they work on top-level QUIC connections. Zero handler changes for
  the tunnel/SSH shape.
- Handlers that need typed sub-streams (TTY) use `into_sub_streams()` — the
  channels crate provides the accessor, the handler crate maps to typed
  names. No channels-crate knowledge of TTY semantics.
- The POC's validated yield-once shape is preserved per-channel; the N-stream
  generalization is at the connection level (one channels connection = N
  channels = N `ChannelBidiStreamSource` instances), not per-channel.

**Negative:**
- Two paths to access channel data (`accept_bi` vs `into_sub_streams`). This
  is a necessary divergence: the `Connection` interface alone can't express
  "give me four named sub-streams" without four `accept_bi` calls (which
  would violate the yield-once contract). The two-path design is the
  minimum-complexity solution; the alternative (a new `Connection` variant
  with multi-stream semantics) would touch `alknet-core` and break the
  ADR-070 extension-point model.
- `into_sub_streams()` consumes the source, so a handler can't call both
  `accept_bi()` and `into_sub_streams()`. This is by design — the sub-
  streams include the 0/1 pair, so `into_sub_streams()` is the superset.

## Door type

**One-way.** The `ChannelBidiStreamSource` shape (one source per channel,
yield-once `accept_bi`, `into_sub_streams` accessor) is the handler-facing
API surface. Changing it after handlers exist (TTY, tunnel, SSH) is a
rewrite of those handlers' integration code. The trait impl is in the
channels crate (not core), so the one-way door is the channels crate's API,
not a core type.

The choice of `into_sub_streams()` returning `Vec<(u8, SendStream,
RecvStream)>` (vs a typed struct, vs a map) is a two-way-door implementation
detail — the return type can change without breaking the contract as long
as the handler crate's destructure code updates.

## References

- ADR-070: BidiStreamSource trait (the extension point this implements)
- ADR-065: Connection::from_stream (the yield-once path this generalizes for
  channels)
- ADR-071: channels wire format (the chunks this reassembles)
- ADR-075: ChannelsAdapter and ChannelManager (the components that construct
  `ChannelBidiStreamSource` instances)
- ADR-077: TTY inside channels (the primary consumer of `into_sub_streams`)
- `docs/research/alknet-channels/poc-summary.md` §POC Target 2, §Issues
  Surfaced #1