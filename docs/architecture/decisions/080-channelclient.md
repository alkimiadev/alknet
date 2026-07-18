# ADR-080: ChannelClient — the Client Side of a Channels Connection

## Status

Accepted (amended 2026-07-12 — see "Amendment: transport-agnostic API"
below; amended 2026-07-16 — `connect_quic` removed per ADR-089 §5, see
"Amendment: `connect_quic` removed" below; **amended 2026-07-18 by
ADR-093 — `stream_types` field removed from `open_channel` and `Channel`;
the channels layer has no `stream_type` concept, see "Amendment
(ADR-093, 2026-07-18)" below**)

## Amendment (ADR-093, 2026-07-18)

The `stream_types: &[u8]` field is **removed** from `open_channel`'s
signature, and `pub stream_types: Vec<u8>` is **removed** from the
`Channel` struct. The channels layer has no `stream_type` concept
(ADR-093) — the handler owns its sub-stream multiplexing on the
`BiStream` it receives via `Channel.source` (a `ChannelBidiStreamSource`
whose `accept_bi` yields a `BiStream` per ADR-092). The handler's
sub-stream set is implicit in its ALPN's wire format (e.g., TTY's 5-byte
format declares its own `stream_type` set internally; the channels
layer carries the bytes transparently). The `into_sub_streams()` reference
in the `Channel.source` doc comment is moot — `into_sub_streams()` is
removed by ADR-093 (amending ADR-074).

The body below describes the **original** (with `stream_types`) shape;
the amendment above is the operative decision. See ADR-093 for the
resolution rationale and the cross-ADR impacts.

## Amendment: `connect_quic` removed (2026-07-16, per ADR-089 §5)

The `connect_quic(addr, credentials)` convenience constructor is
**removed**. Keeping it as a thin wrapper over
`AlknetClient::dial_quic` would make `alknet-channels-call` depend on
`alknet-client`, contradicting the dep graph (the protocol crates are
parallel to the dial, not downstream of it). Callers compose
`AlknetClient::dial_quic(...).await?` + `ChannelClient::from_connection(conn).await?`.
The `from_connection` primary constructor (the 2026-07-12 amendment
below) is unchanged and remains the one-way-door surface. The
`connect_quic` references in the body of this ADR are the historical
shape; they do not survive into the implementation. See ADR-089 §5 for
the full rationale and the breaking-change acknowledgment.

## Amendment: transport-agnostic API (2026-07-12)

The original Decision named `connect(addr: SocketAddr, credentials)` as the
primary constructor and framed it as "QUIC-only initially" — to be
generalized when a second transport's client exists. That framing welded
the client-side one-way-door API to QUIC, the same welding ADR-065 unwound
on the server side, and masked it as a two-way-door deferral
(anti-patterns #8, #9, #11). "Can be generalized later" meant "can be
rewritten later" — the expensive reversal the one-way-door classification
exists to prevent.

The channels protocol is transport-agnostic by design (ADR-071 substrate
modes; `Connection::from_stream`/`from_bidi`/`from_source` accept any
`AsyncRead + AsyncWrite`). The client side is half of that protocol and
must not be coupled to a transport. This amendment splits the constructor
surface:

- **`from_connection(connection: Connection)`** — the transport-agnostic
  primary constructor and the one-way-door API. Takes a pre-established
  `Connection` (produced by any transport — TCP+TLS via `from_bidi`,
  WebTransport `BiStream`, SSH `direct-tcpip`, a quinn connection, a
  WebSocket carrying `alknet/channels` per ADR-044), installs channel 0,
  spawns the demux/mux, returns the client. Mirrors the server-side
  `ChannelsAdapter::handle(Connection)` (substrate-agnostic) and the
  existing `CallClient::spawn_dispatch(Connection)` pattern.
- **`connect_quic(addr, credentials)`** — a QUIC convenience constructor:
  dial QUIC, then `from_connection`. Additive and two-way-door.
  `connect_tcp_tls`, `connect_webtransport`, etc. join it as transports
  are added, without touching the one-way-door surface.

The dial+TLS seam (the transport-specific work each dial helper does —
verifier selection per ADR-034, handshake, produce a `Connection`) is the
correct scope of OQ-55's deferral. `AlknetClient` is the eventual shared
*dial*; `from_connection` is the shared *channels-take-over*. Separating
them now, before the one-way-door API is cast, is the point — not a
deferral. The "QUIC-only initially" framing is removed; it was the
anti-pattern this amendment corrects.

The door-type classification is unchanged: `from_connection` is one-way
(the handler-facing surface), `connect_quic` is two-way (additive
convenience). The `AlknetClient` extraction remains deferred (OQ-55) — but
what is deferred is the shared *dial*, not a QUIC-welded client API.

## Context

Both sides of a channels connection do the demux/mux work. The server side
is a `ProtocolHandler` (`ChannelsAdapter::handle`, ADR-075). The client side
needs a symmetric type — `ChannelClient` — that takes over an established
transport `Connection`, runs the demux/mux, and exposes
`open_channel(alpn, params) -> Channel` to the application. This is the
channels analogue of `CallClient` (server: `CallAdapter`; client:
`CallClient`) in the call protocol.

The phase-0 research (`docs/research/alknet-channels/phase-0-findings.md`
§OQ-CH-14) clarified that there are two concerns here:

1. **`ChannelClient` (channels-specific):** the client type for channels
   connections. Decision-ready — build it in `alknet-channels`, same shape
   as `CallClient`. The *take-over* half is transport-agnostic
   (`from_connection`); the *dial* half is transport-specific
   (`connect_quic` and future transport helpers).
2. **`AlknetClient` (core, transport-polymorphic):** the shared *dial+TLS*
   seam — the transport-specific work (open socket, TLS handshake, ADR-034
   verifier selection, produce a `Connection`) that each transport's dial
   helper rebuilds. Genuinely deferred — blocked on a second *transport's*
   dial existing (OQ-55 tracks this correctly). A single QUIC dial
   (`connect_quic`) does not give enough information to extract the
   transport-polymorphic dial seam; two different transport dials do.

This ADR decides #1 — `ChannelClient`, with `from_connection` as the
transport-agnostic primary constructor and `connect_quic` as a transport-
specific dial helper. #2 (the shared `AlknetClient` dial+TLS seam) stays
deferred per OQ-55.

## Decision

### `ChannelClient` in `alknet-channels`

```rust
pub struct ChannelClient {
    manager: ChannelManager,
    // The transport-side demux/mux, running in a background task.
    ...
}

impl ChannelClient {
    /// Transport-agnostic primary constructor. Takes a pre-established
    /// `Connection` (any transport — TCP+TLS via `from_bidi`,
    /// WebTransport BiStream, SSH direct-tcpip, a quinn connection, a
    /// WebSocket per ADR-044), installs channel 0 (alknet/call), spawns
    /// the demux/mux, and returns the client. Mirrors the server-side
    /// `ChannelsAdapter::handle(Connection)`. This is the one-way-door
    /// API surface — it must not be coupled to a transport (ADR-071,
    /// ADR-065).
    pub async fn from_connection(connection: Connection)
        -> Result<Self, ChannelError>;

    /// QUIC convenience constructor. Dials a QUIC connection to `addr`
    /// on ALPN `alknet/channels` (credentials → TLS handshake,
    /// ADR-034 verifier selection), then calls `from_connection`.
    /// Additive and two-way-door — `connect_tcp_tls`,
    /// `connect_webtransport`, etc. join it as transports are added.
    ///
    /// **REMOVED per ADR-089 §5.** The dial is extracted into
    /// `AlknetClient`; `connect_quic` is deleted, not delegated.
    /// Callers compose `AlknetClient::dial_quic` + `from_connection`.
    /// The `CallCredentials` parameter is moot — `CallCredentials` is
    /// removed per ADR-091 (amended 2026-07-17); the dial consumes
    /// `ConnectionCredentials` from `alknet-core`.
    pub async fn connect_quic(
        addr: SocketAddr,
        credentials: CallCredentials,  // REMOVED — CallCredentials is removed
    ) -> Result<Self, ChannelError>;

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

    /// Subscribe to the peer's resource updates. Returns a stream of
    /// resource-set events (ADR-073 channel/resources/subscribe). Part of
    /// the one-way-door handler-facing surface (see Door type below).
    pub async fn subscribe_resources(&self)
        -> Result<BoxStream<ResourceEvent>, ChannelError>;

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

### Transport-agnostic by construction

`ChannelClient` is the client side of the channels protocol, which is
transport-agnostic (ADR-071 substrate modes; ADR-065 `from_stream`/`from_bidi`). The primary constructor — `from_connection(connection: Connection)` — takes a pre-established `Connection` from any
transport and takes over channels establishment. This mirrors the
server-side `ChannelsAdapter::handle(Connection)`, which is
substrate-agnostic by the same mechanism: the server receives a
`Connection` (QUIC-native, TCP+TLS via `from_bidi`, WebTransport, SSH
`direct-tcpip`, …) and runs the demux loop unchanged; the client receives
a `Connection` the same way and runs the same logic from the dialing side.

`connect_quic(addr, credentials)` is a convenience over `from_connection`:
dial QUIC, then `from_connection`. It is additive and two-way-door.
Transport-specific dial helpers (`connect_tcp_tls`, `connect_webtransport`,
…) join it as transports are added — none of which touch the
`from_connection` contract. The dial helper set is open-ended by design.

This is the client-side analogue of the server-side generalization ADR-065
made. Welding the client's one-way-door API to QUIC would repeat the
welding ADR-065 explicitly unwound.

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

`ChannelClient`'s *API* is transport-agnostic — `from_connection` takes
a pre-established `Connection`. What is deferred (OQ-55) is the shared
*dial+TLS* seam (`AlknetClient`): the transport-specific work each dial
helper does — open a socket, run the TLS handshake, apply ADR-034's
verifier-selection rule, produce a `Connection`. That dial is genuinely
transport-specific (QUIC, TCP+TLS, WebTransport, raw TCP, SSH), and we have
one shape implemented (QUIC, in `connect_quic`). Extracting a QUIC-shaped
connector now and naming it `AlknetClient` would bake QUIC in as *the*
establishment shape — the same welding ADR-065 unwound on the server side.

This is why `from_connection` is the one-way-door surface and
`connect_quic` is a two-way-door convenience over it. `AlknetClient` (when
extracted, after a second transport's dial exists) becomes the shared
*dial*; `from_connection` stays the shared *channels-take-over*. The two
concerns are separated now, before the one-way-door API is cast.

The friction while `AlknetClient` is deferred is duplicated
verifier-selection boilerplate across dial helpers (~20 lines each) — not
duplicated capability and not a QUIC-welded client API.

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
- Each transport-specific dial helper duplicates ~20 lines of
  verifier-selection boilerplate (from `connect_quic`). This is the known
  cost of not extracting `AlknetClient` yet (OQ-55). Acceptable until the
  second transport's dial exists, at which point `AlknetClient` extracts
  the shared dial+TLS seam. The `from_connection` API — the one-way-door
  surface — is unaffected; only the dial helpers carry the duplication.

## Door type

**One-way.** The `ChannelClient::from_connection` / `open_channel` /
`call` / `subscribe_resources` API is the handler-facing surface;
changing it after consumers exist is a rewrite. `from_connection` is the
one-way-door primary constructor (transport-agnostic).

`connect_quic` (and future `connect_tcp_tls` / `connect_webtransport` /
…) are **two-way** doors — additive convenience constructors over
`from_connection`. Adding, removing, or changing a dial helper is cheap
and does not touch the one-way-door surface.

The `AlknetClient` extraction is a **deferred decision** (OQ-55,
deferred(scope)), not a door-type attribute. Its door type is two-way (the
extraction is a refactor, not a wire-format change), but it is not decided
in this ADR — see OQ-55 for the blocking condition. What is deferred is the
shared *dial+TLS* seam; `from_connection`'s transport-agnostic contract is
decided now.

## References

- ADR-073: channel lifecycle operations (`open_channel` sends `channel/open`)
- ADR-074: ChannelBidiStreamSource (what `Channel.source` wraps, as
  amended by ADR-093 — `accept_bi` yields a `BiStream`)
- ADR-093: channels pure channel multiplexing (`stream_types` field
  removed from `open_channel` and `Channel`; handler owns sub-stream
  multiplexing)
- ADR-075: ChannelManager (the shared state `ChannelClient` holds)
- OQ-55: AlknetClient / client establishment extraction (the deferred core
  concern this ADR does NOT block on)
- `docs/research/alknet-channels/phase-0-findings.md` §OQ-CH-14 (the
  research-scope question this ADR carries forward)
- `docs/architecture/crates/call/client-and-adapters.md` — `CallClient` (the
  shape `ChannelClient` mirrors)