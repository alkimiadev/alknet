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
client side takes over an established transport `Connection`, runs the
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
    /// Construct a `ChannelClient` over a pre-established transport
    /// `Connection` on ALPN `alknet/channels`. This is the
    /// transport-agnostic primary constructor: the caller (or a
    /// transport-specific dial helper) produces the `Connection` —
    /// via `Connection::from_stream`/`from_bidi` (TCP+TLS,
    /// WebTransport, SSH `direct-tcpip`), a quinn connection, or any
    /// other `AsyncRead + AsyncWrite` source — and this method takes
    /// over: installs channel 0 (`alknet/call`), spawns the demux/mux,
    /// and returns the client. Mirrors the server side's
    /// transport-agnostic `ChannelsAdapter::handle(Connection)` and
    /// `CallClient::spawn_dispatch(Connection)`.
    ///
    /// This is the one-way-door API surface (ADR-080). It must not be
    /// coupled to a transport — the channels protocol is
    /// transport-agnostic (ADR-071, ADR-065), and the client side is
    /// half of that protocol.
    pub async fn from_connection(connection: Connection)
        -> Result<Self, ChannelError>;

    /// QUIC convenience constructor. Dials a QUIC connection to `addr`
    /// on ALPN `alknet/channels` (using `credentials` for the TLS
    /// handshake — ADR-034 verifier selection), then calls
    /// `from_connection`. This is the "I just want QUIC" one-liner;
    /// it is additive over `from_connection` and is a two-way door —
    /// `connect_tcp_tls`, `connect_webtransport`, etc. can be added
    /// alongside it without touching the one-way-door surface.
    pub async fn connect_quic(
        addr: SocketAddr,
        credentials: CallCredentials,
    ) -> Result<Self, ChannelError>;

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

## Transport-agnostic by construction

`ChannelClient` is the client side of the channels protocol. The channels
protocol is transport-agnostic (ADR-071 substrate modes;
`Connection::from_stream`/`from_bidi`/`from_source` from ADR-065/070 take
any `AsyncRead + AsyncWrite`). The client side must not be welded to a
transport — that would repeat the server-side welding ADR-065 explicitly
unwound.

`from_connection(connection: Connection)` is the primary constructor and
the one-way-door API surface. It takes a pre-established `Connection` and
takes over channels establishment. The transport is the caller's concern:
`Connection::from_bidi(tls_stream, ...)` for TCP+TLS, a quinn `Connection`,
a WebTransport `BiStream`, an SSH `direct-tcpip` channel wrapped via
`from_stream`, a WebSocket carrying `alknet/channels` (the browser path per
ADR-044) — all produce a `Connection` that `from_connection` accepts
unchanged. This mirrors the server side's `ChannelsAdapter::handle(Connection)`, which is substrate-agnostic by the same mechanism.

`connect_quic(addr, credentials)` is a **convenience** constructor — dial
QUIC, then `from_connection`. It is additive and two-way-door: transport-specific dial helpers (`connect_tcp_tls`, `connect_webtransport`, …)
join it as transports are added, none of which touch the `from_connection`
contract. The dial helper set is open-ended by design.

The credential/verifier-selection rule (ADR-034) lives in the transport's
own dial path, not in `from_connection` — `from_connection` receives an
already-established, already-authenticated `Connection`, exactly as
`ChannelsAdapter::handle` does on the server side. The ~20 lines of
verifier-selection boilerplate each dial helper rebuilds is the known
duplicated cost of not having `AlknetClient` (OQ-55) extracted yet;
`from_connection` keeps that boilerplate on the *transport-specific dial*
side, not on the channels protocol's one-way-door surface.

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

## Relationship to `AlknetClient` (ADR-089 — resolved)

`ChannelClient`'s *API* is transport-agnostic — `from_connection` takes a
pre-established `Connection`. The shared *dial+TLS* seam
(`AlknetClient`, OQ-55) is now extracted: [`alknet-client`](../client/README.md)
provides `AlknetClient` with three dial methods (`dial_quic` /
`dial_tcp_tls` / `dial_iroh`), each producing a `Connection` that
`from_connection` consumes. The dial is transport-specific (QUIC,
TCP+TLS, iroh); the take-over (`from_connection`) is
transport-agnostic. The two concerns are separated.

`connect_quic` becomes a thin wrapper over `AlknetClient::dial_quic` —
dial QUIC, then `from_connection`. A caller that needs transport
selection (QUIC with TCP+TLS fallback) uses `AlknetClient` directly;
the fallback policy is a caller concern. See
[ADR-089](../../decisions/089-alknetclient-native-dial-seam.md) for the
full decision and [OQ-55](../../questions/055-alknetclient-establishment-extraction.md)
(resolved).

## Design Decisions

All design decisions are documented as ADRs in [decisions/](../../decisions/).

| ADR | Decision | Summary |
|-----|----------|---------|
| [080](../../decisions/080-channelclient.md) | ChannelClient | Client side; transport-agnostic `from_connection` primary, `connect_quic` convenience; `AlknetClient` dial-seam extracted (ADR-089, resolves OQ-55) |

## Open Questions

- **OQ-55** (resolved by ADR-089): `AlknetClient` core **dial+TLS seam**
  — extracted as `alknet-client` with three dial methods.
  `ChannelClient`'s API is transport-agnostic (`from_connection`); the
  dial is the shared seam, now extracted. See
  [ADR-089](../../decisions/089-alknetclient-native-dial-seam.md).

## References

- ADR-080: ChannelClient (the decision)
- ADR-073: channel lifecycle operations (`open_channel` sends `channel/open`)
- ADR-074: ChannelBidiStreamSource (what `Channel.source` wraps)
- ADR-075: ChannelManager (the shared state `ChannelClient` holds)
- OQ-55: AlknetClient / client establishment extraction
- `docs/architecture/crates/call/client-and-adapters.md` — `CallClient` (the
  shape `ChannelClient` mirrors)