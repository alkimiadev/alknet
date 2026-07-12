---
status: draft
last_updated: 2026-07-12
---

# channel-client.md — ChannelClient

The client side of a channels connection. ADR-080 is the decision; this doc
specifies the API.

## What

`ChannelClient` is the symmetric counterpart to `ChannelsAdapter` (ADR-075).
The server side is a `ProtocolHandler` (`ChannelsAdapter::handle`); the
client side dials a transport, establishes the channels connection, runs the
demux/mux, and exposes `open_channel(alpn, params) -> Channel` to the
application.

This is the channels analogue of `CallClient` (server: `CallAdapter`;
client: `CallClient`) in the call protocol.

## API

```rust
pub struct ChannelClient {
    manager: ChannelManager,
    // The transport-side demux/mux, running in a background task.
    ...
}

impl ChannelClient {
    /// Open a channels connection to a peer. Dials the transport (QUIC
    /// initially), establishes the channels connection, preinstalls
    /// channel 0 (alknet/call), and returns the client.
    pub async fn connect(addr: SocketAddr, credentials: CallCredentials)
        -> Result<Self, ChannelError>;

    /// Open a data channel with the given ALPN and params. Sends
    /// `channel/open` on channel 0, waits for the response, and returns
    /// the channel.
    pub async fn open_channel(
        &self,
        alpn: &str,
        stream_types: &[u8],
        params: Value,
        direction: ChannelDirection,
    ) -> Result<Channel, ChannelError>;

    /// Subscribe to the peer's resource updates. Returns a stream of
    /// resource-set events (ADR-073 channel/resources/subscribe). Each
    /// event carries the JSON `output.resources` array from ADR-073's
    /// `channel/resources/subscribe` response shape.
    pub async fn subscribe_resources(&self)
        -> Result<BoxStream<ResourceEvent>, ChannelError>;

    /// The call-protocol connection on channel 0, for invoking channel
    /// lifecycle operations and any other call ops the peer exposes.
    pub fn call(&self) -> &CallConnection;
}

pub enum ChannelDirection {
    InitiatorToResponder,
    ResponderToInitiator,
}

pub struct Channel {
    pub channel_id: u32,
    pub stream_types: Vec<u8>,
    /// The sub-streams, accessible via accept_bi() (ADR-074 generic path)
    /// or into_sub_streams() (ADR-074 typed path).
    pub source: ChannelBidiStreamSource,
}

/// One event from `channel/resources/subscribe`. Wraps the JSON `output`
/// object from ADR-073's subscribe response — the `resources` array
/// describing what ALPNs the peer exposes and with what `access` preview.
/// The channels crate maps the JSON to this typed struct; the fields mirror
/// ADR-073's response shape.
pub struct ResourceEvent {
    pub resources: Vec<ResourceEntry>,
}

pub struct ResourceEntry {
    pub alpn: String,
    pub backends_or_targets: Vec<String>, // ALPN-specific enumeration
    pub access: Value,                     // preview of AccessControl (advisory)
}
```

## QUIC-only initially

`ChannelClient::connect` dials a QUIC connection (via the same `quinn`
endpoint `CallClient` uses) and wraps it as a channels connection. This is
the same transport shape as `CallClient`. When a second transport's client
exists (HTTP, TCP+TLS, WebTransport — per OQ-55), the dial can be
generalized. Until then, `ChannelClient` is QUIC-only — the same posture as
`CallClient`.

## Bidirectionality preserved

The channels protocol is bidirectional — either side can open a channel
(ADR-073 §direction semantics). `ChannelClient::open_channel` supports both
`ChannelDirection::InitiatorToResponder` and
`ChannelDirection::ResponderToInitiator`. The client is not "the client
side" in the request/response sense — it can also receive `channel/open`
requests from the peer (the peer initiates, the client's `ChannelManager`
responds). This mirrors the call protocol's operation overlay (each side
populates what operations they expose).

`ChannelClient` is one endpoint of a bidirectional channels connection. The
name follows the `CallClient` convention (the side that dialed), not a
request/response role.

## Relationship to `AlknetClient` (OQ-55 — deferred)

`ChannelClient` is a standalone client, not a specialization of a core
`AlknetClient`. The `AlknetClient` extraction (OQ-55) is genuinely deferred:
blocked on a second *transport's* client existing, not on a second client
existing. `ChannelClient` over QUIC is a second client but the same
transport shape as `CallClient` — it doesn't give enough information to
extract the transport-polymorphic dial seam.

When `AlknetClient` is eventually extracted (after a second transport's
client exists), `ChannelClient` and `CallClient` both refactor onto it.
Until then, they are independent clients with duplicated boilerplate (each
rebuilds verifier selection — ~20 lines). The friction is duplicated
boilerplate, not a missing capability.

## Design Decisions

All design decisions are documented as ADRs in [decisions/](../../decisions/).

| ADR | Decision | Summary |
|-----|----------|---------|
| [080](../../decisions/080-channelclient.md) | ChannelClient | Client side; QUIC-only; `AlknetClient` deferred (OQ-55) |

## Open Questions

- **OQ-55** (deferred(scope)): `AlknetClient` core extraction — blocked on
  a second *transport's* client. `ChannelClient` does not unblock it.

## References

- ADR-080: ChannelClient (the decision)
- ADR-073: channel lifecycle operations (`open_channel` sends `channel/open`)
- ADR-074: ChannelBidiStreamSource (what `Channel.source` wraps)
- ADR-075: ChannelManager (the shared state `ChannelClient` holds)
- OQ-55: AlknetClient / client establishment extraction
- `docs/architecture/crates/call/client-and-adapters.md` — `CallClient` (the
  shape `ChannelClient` mirrors)