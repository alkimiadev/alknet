# ADR-080: ChannelClient — the Client Side of a Channels Connection

## Status

Accepted

## Context

Both sides of a channels connection do the demux/mux work. The server side
is a `ProtocolHandler` (`ChannelsAdapter::handle`, ADR-075). The client side
needs a symmetric type — `ChannelClient` — that opens a transport, runs the
demux/mux, and exposes `open_channel(alpn, params) -> Channel` to the
application. This is the channels analogue of `CallClient` (server:
`CallAdapter`; client: `CallClient`) in the call protocol.

The phase-0 research (`docs/research/alknet-channels/phase-0-findings.md`
§OQ-CH-14) clarified that there are two concerns here:

1. **`ChannelClient` (channels-specific):** the client type for channels
   connections. Decision-ready — build it in `alknet-channels`, same shape
   as `CallClient`.
2. **`AlknetClient` (core, transport-polymorphic):** a general downstream-
   facing client that crates use to connect to an alknet endpoint. Genuinely
   deferred — blocked on a second *transport's* client existing (OQ-55
   tracks this correctly). `ChannelClient` over QUIC does not unblock
   `AlknetClient` because it's the same transport shape as `CallClient`.

This ADR decides #1. #2 stays deferred per OQ-55.

## Decision

### `ChannelClient` in `alknet-channels`

```rust
pub struct ChannelClient {
    manager: ChannelManager,
    // The transport-side demux/mux, running in a background task.
    ...
}

impl ChannelClient {
    /// Open a channels connection to a peer. Dials the transport (QUIC
    /// initially), establishes the channels connection, preinstalls channel
    /// 0 (alknet/call), and returns the client.
    pub async fn connect(addr: SocketAddr, credentials: CallCredentials)
        -> Result<Self, ChannelError>;

    /// Open a data channel with the given ALPN and params. Sends
    /// `channel/open` on channel 0, waits for the response, and returns
    /// the channel's sub-streams.
    pub async fn open_channel(
        &self,
        alpn: &str,
        stream_types: &[u8],
        params: Value,
        direction: ChannelDirection,
    ) -> Result<Channel, ChannelError>;

    /// The call-protocol connection on channel 0, for invoking channel
    /// lifecycle operations and any other call ops the peer exposes.
    pub fn call(&self) -> &CallConnection;
}

pub struct Channel {
    pub channel_id: u32,
    pub stream_types: Vec<u8>,
    /// The sub-streams, accessible via the BidiStreamSource (accept_bi) or
    /// into_sub_streams() — ADR-074.
    pub source: ChannelBidiStreamSource,
}
```

### QUIC-only initially

`ChannelClient::connect` dials a QUIC connection (via the same `quinn`
endpoint `CallClient` uses) and wraps it as a channels connection. This is
the same transport shape as `CallClient`. When a second transport's client
exists (HTTP, TCP+TLS, WebTransport — per OQ-55), the dial can be
generalized. Until then, `ChannelClient` is QUIC-only — the same posture as
`CallClient`.

### Bidirectionality preserved

The channels protocol is bidirectional — either side can open a channel
(ADR-073 §direction semantics). `ChannelClient::open_channel` supports both
`ChannelDirection::InitiatorToResponder` and
`ChannelDirection::ResponderToInitiator`. The client is not "the client
side" in the sense of only initiating — it can also receive `channel/open`
requests from the peer (the peer initiates, the client's `ChannelManager`
responds). This mirrors the call protocol's operation overlay (each side
populates what operations they expose).

This means `ChannelClient` is not purely a "client" in the request/response
sense — it's one endpoint of a bidirectional channels connection. The name
`ChannelClient` follows the `CallClient` convention (the side that dialed),
not a request/response role.

### Relationship to `AlknetClient` (OQ-55 — deferred)

`ChannelClient` is a standalone client, not a specialization of a core
`AlknetClient`. The `AlknetClient` extraction (OQ-55) is genuinely deferred:
blocked on a second *transport's* client existing, not on a second client
existing. `ChannelClient` over QUIC is a second client but the same
transport shape as `CallClient` — it doesn't give enough information to
extract the transport-polymorphic dial seam. Extracting a QUIC-shaped
connector to core and naming it `AlknetClient` would bake QUIC in as *the*
establishment shape — the same welding ADR-065 unwound on the server side.

When `AlknetClient` is eventually extracted (after a second transport's
client exists), `ChannelClient` and `CallClient` both refactor onto it.
Until then, they are independent clients with duplicated boilerplate (each
rebuilds verifier selection — ~20 lines). The friction is duplicated
boilerplate, not a missing capability.

## Consequences

**Positive:**
- `ChannelClient` gives the channels crate a symmetric client/server pair,
  matching the call protocol's `CallAdapter`/`CallClient` shape.
- Bidirectionality is preserved — the client can both initiate and receive
  `channel/open`.
- The `AlknetClient` deferral (OQ-55) is not blocked by `ChannelClient` —
  they are independent concerns. `ChannelClient` builds standalone; the
  core extraction happens later when the blocker clears.

**Negative:**
- `ChannelClient` duplicates ~20 lines of verifier-selection boilerplate
  from `CallClient`. This is the known cost of not extracting `AlknetClient`
  yet (OQ-55). Acceptable until the second transport's client exists.
- `ChannelClient` is QUIC-only. A non-QUIC channels client (e.g., a browser
  over WebTransport) builds separately until `AlknetClient` is extracted.
  This is the same posture as `CallClient` and is not a channels-specific
  limitation.

## Door type

**One-way.** The `ChannelClient::connect` / `open_channel` / `call` /
`subscribe_resources` API is the handler-facing surface; changing it after
consumers exist is a rewrite.

The `AlknetClient` extraction is a **deferred decision** (OQ-55,
deferred(scope)), not a door-type attribute. Its door type is two-way (the
extraction is a refactor, not a wire-format change), but it is not decided
in this ADR — see OQ-55 for the blocking condition.

## References

- ADR-073: channel lifecycle operations (`open_channel` sends `channel/open`)
- ADR-074: ChannelBidiStreamSource (what `Channel.source` wraps)
- ADR-075: ChannelManager (the shared state `ChannelClient` holds)
- OQ-55: AlknetClient / client establishment extraction (the deferred core
  concern this ADR does NOT block on)
- `docs/research/alknet-channels/phase-0-findings.md` §OQ-CH-14 (the
  research-scope question this ADR carries forward)
- `docs/architecture/crates/call/client-and-adapters.md` — `CallClient` (the
  shape `ChannelClient` mirrors)