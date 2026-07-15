---
status: draft
last_updated: 2026-07-15
---

# alknet-client

The native client dial seam — the client-side analogue of
`AlknetEndpoint`. A multi-transport dialer that takes pre-built
transport handles (quinn, TCP+TLS, iroh), dials a remote `AlknetEndpoint`
on a chosen ALPN, and produces a `Connection` for the protocol
take-overs (`CallClient::spawn_dispatch`,
`ChannelClient::from_connection`) to consume. It is the Rust native
client; it does not run protocols, manage peer lifecycle, or supervise
reconnection. It dials and produces a `Connection`.

## What

`AlknetClient` is the dial. Before this crate, each protocol client
(`CallClient::connect`, `ChannelClient::connect_quic`) built its own
QUIC dial inline — building a `TlsClientConfig`, constructing a
`quinn::Endpoint`, calling `connect_with`, wrapping as a `Connection`.
The dial boilerplate was duplicated, and there was no place for a
second transport's dial (TCP+TLS, iroh) to live without each protocol
client growing its own per-transport dial helper.

`alknet-client` extracts the dial the same way ADR-083 extracted the
accept loop on the server side: one type that takes pre-built transport
handles and produces a `Connection`, with the transport choice as a
parameter. The protocol take-overs are unchanged — they consume the
`Connection` and do not know `AlknetClient` produced it.

### The three concept layers (ADR-089 §"The tangle this ADR also names")

Three concept levels were conflated throughout the initial development.
`AlknetClient` is the fix for one of them (the establishment side);
naming all three is what makes the fix legible.

| Layer | Concepts | What it determines |
|-------|----------|--------------------|
| **Deployment role** | Hub / Worker / Hub-Worker | Who accepts, who dials — which side(s) you instantiate |
| **Establishment side** | `AlknetEndpoint` (server) / `AlknetClient` (client) | Accept-and-resolve-identity vs. dial-and-present-identity |
| **ALPN-level category** | Endpoint ALPN / Entry-point ALPN (ADR-086 §2) | Whether identity is required at the TLS layer vs. per-request |

The layers are orthogonal. A **hub** uses an `AlknetEndpoint` (server)
AND an `AlknetClient` (client, when dialing workers). A **worker** uses
an `AlknetClient` (client) AND may use an `AlknetEndpoint` (server, if
it accepts inbound). The role determines which side(s) you instantiate,
not what the side IS. `AlknetClient` is the client-side establishment
type — Layer 2 — independent of the deployment role that uses it and of
the ALPN-level category of the ALPN it dials.

### What `AlknetClient` IS

The client-side analogue of `AlknetEndpoint`: a multi-transport dialer
that produces `Connection`s for the protocol take-overs to consume.
Narrowed to the **native case**: dialing native endpoint types (QUIC +
TCP+TLS, both rustls-consuming via `TlsClientConfig`; iroh as the
key-not-config exception) over the native ALPNs (`alknet/register`,
`alknet/call`, `alknet/channels`).

### What `AlknetClient` is NOT

- **Not a protocol implementation.** It does not run the call protocol
  or the channels protocol. It produces a `Connection`;
  `CallClient`/`ChannelClient` take over from there. Analogue:
  `AlknetEndpoint` dispatches by ALPN; the handler runs the protocol.
- **Not a hub or worker.** Hub/Worker are deployment roles that *use*
  `AlknetClient` (and `AlknetEndpoint`). `AlknetClient` has no peer
  lifecycle, no aggregated env, no supervision loop, no relay. The
  hub's `supervise_worker` takes a `dial` closure that can call
  `AlknetClient` internally — the hub does not need to know
  `AlknetClient` exists.
- **Not the web/browser client.** Browsers dial via WebSocket/HTTP
  (ADR-044/048) — a different client surface (the JS SDK / wasm), not
  `AlknetClient`. `AlknetClient` is the Rust native client.
- **Not a replacement for `CallClient`/`ChannelClient`.** Those are
  the protocol take-overs. `AlknetClient` is the dial that feeds them.

## Why

The dial was deferred (OQ-55) because extracting a QUIC-shaped
connector would bake QUIC in as *the* establishment shape. ADR-089
resolves the deferral — three decisions (ADR-086, ADR-087, ADR-083)
collapsed the blocking conditions. See
[ADR-089](../../decisions/089-alknetclient-native-dial-seam.md) §"Why
the deferral has collapsed" for the full rationale.

## Architecture

### `AlknetClient`

The central type. Holds pre-built transport handles, all optional — the
client dials with whichever transport the remote endpoint type implies.

```rust
pub struct AlknetClient {
    #[cfg(feature = "quinn")]
    quinn: Option<quinn::Endpoint>,
    #[cfg(feature = "tcp")]
    tcp_connector: Option<tokio_rustls::TlsConnector>,
    #[cfg(feature = "iroh")]
    iroh: Option<iroh::Endpoint>,
}

impl AlknetClient {
    pub fn new() -> Self;

    #[cfg(feature = "quinn")]
    pub fn with_quinn(mut self, endpoint: quinn::Endpoint) -> Self;

    #[cfg(feature = "tcp")]
    pub fn with_tcp_tls(mut self, connector: tokio_rustls::TlsConnector) -> Self;

    #[cfg(feature = "iroh")]
    pub fn with_iroh(mut self, endpoint: iroh::Endpoint) -> Self;
}
```

The builder mirrors `AlknetEndpoint`'s `with_quinn` / `with_iroh` /
`with_tcp_tls` (ADR-083) — the assembly layer builds the transport
handles and hands them to the client via builder methods. A native
client that needs QUIC-with-TCP+TLS-fallback holds both a quinn
endpoint and a TCP+TLS connector; a minimal iroh-only client holds only
the iroh endpoint.

### The three dials

```rust
impl AlknetClient {
    /// QUIC dial. Builds a `TlsClientConfig` from `credentials`
    /// (ADR-034 verifier selection + ADR-084 provider), dials `addr`
    /// on `alpn`, returns a `Connection` via
    /// `Connection::from_quinn_with_alpn`. The `server_name` is the
    /// TLS SNI / name (for X.509; ignored for raw-key pinning).
    /// Feature-gated on `quinn`.
    #[cfg(feature = "quinn")]
    pub async fn dial_quic(
        &self,
        addr: SocketAddr,
        server_name: &str,
        alpn: &[u8],
        credentials: &CallCredentials,
    ) -> Result<Connection, ClientDialError>;

    /// TCP+TLS dial. Builds a `TlsClientConfig` from `credentials`,
    /// connects a `TcpStream` to `addr`, wraps with `TlsConnector`
    /// using `host` as the SNI, returns a `Connection` via
    /// `Connection::from_bidi` (ADR-065). Feature-gated on `tcp`.
    #[cfg(feature = "tcp")]
    pub async fn dial_tcp_tls(
        &self,
        host: &str,
        addr: SocketAddr,
        alpn: &[u8],
        credentials: &CallCredentials,
    ) -> Result<Connection, ClientDialError>;

    /// Iroh dial. Dials `node_id` on `alpn` via the iroh endpoint.
    /// The iroh path does NOT use `TlsClientConfig` — iroh has its
    /// own TLS (shares the `Ed25519SecretKey`, not the rustls config —
    /// ADR-087 §3, ADR-089 §3). The verifier is iroh's `NodeId` match
    /// (fingerprint pin by another name — ADR-034 §3). An unknown
    /// iroh remote fails closed (no CA). Feature-gated on `iroh`.
    #[cfg(feature = "iroh")]
    pub async fn dial_iroh(
        &self,
        node_id: iroh::NodeId,
        alpn: &[u8],
        local_key: &alknet_core::config::Ed25519SecretKey,
    ) -> Result<Connection, ClientDialError>;
}
```

The two rustls dials (`dial_quic`, `dial_tcp_tls`) share
`TlsClientConfig::new` — the ADR-034 verifier selection (fingerprint
pin for a known peer, CA-verify for an unknown X.509 remote, fail-closed
for an unknown raw-key remote) and the ADR-084 crypto provider
(`aws_lc_rs`). The iroh dial is the exception: iroh has its own TLS and
takes the `Ed25519SecretKey` directly, not a `rustls::ClientConfig`.
The consistency is in the rule (ADR-034), not in the type — the same
exception as the server side (ADR-082, ADR-087 §3).

### What the dial does NOT do

- **No protocol take-over.** The dial returns a `Connection`; the
  caller hands it to `CallClient::spawn_dispatch` or
  `ChannelClient::from_connection`. `AlknetClient` does not spawn the
  dispatch loop or install channel 0.
- **No identity resolution.** The client *presents* its identity (via
  the client cert in `TlsClientConfig`) and *verifies* the remote (via
  the ADR-034 verifier). It does not resolve the remote's identity into
  a `PeerId` — that happens inside the protocol take-over (the
  `CallAdapter` / `CallConnection` resolves the fingerprint via
  `IdentityProvider`).
- **No reconnection / supervision.** The dial is one-shot. A caller
  that needs reconnect-with-backoff wraps the dial in a supervision
  loop (the hub's `supervise_worker` pattern — a closure that produces
  a `Connection`).
- **No transport fallback.** A caller that needs
  QUIC-with-TCP+TLS-fallback dials QUIC, catches the error, and dials
  TCP+TLS. `AlknetClient` provides both dials; the fallback policy is a
  caller concern (or a future `dial_with_fallback` helper —
  two-way-door).

### Credentials

`AlknetClient`'s dials take a `CallCredentials` bundle — the existing
type from `alknet-call` (the local `TlsIdentity`, the optional
`auth_token`, and the `RemoteIdentity` for verifier selection). The
credentials come from `Capabilities` (ADR-014), never from environment
variables — the no-env-vars invariant. The assembly layer derives them
from the vault at startup and passes them to each dial. The credential
type's crate location is a two-way-door detail — see
[ADR-089](../../decisions/089-alknetclient-native-dial-seam.md) §"What
this does NOT change" and the Dependencies section below.

### The dialable ALPNs

`AlknetClient` dials any ALPN the remote endpoint advertises. The
native ALPNs (ADR-086):

| ALPN | Category (ADR-086 §2) | Protocol take-over | Identity |
|------|----------------------|--------------------|----------|
| `alknet/register` | entry point | (registration handshake — deferred, OQ-66) | None at TLS; per-request token (or open) |
| `alknet/call` | endpoint | `CallClient::spawn_dispatch` | Fingerprint (raw key / client cert) or bearer token (first frame) |
| `alknet/channels` | endpoint | `ChannelClient::from_connection` | Fingerprint or bearer token (resolved on channel 0 — ADR-072) |

The dial is the same for all three — the difference is the protocol that
runs on the resulting `Connection`. For `alknet/register`, the protocol
is the registration handshake (deferred — see "`alknet/register`"
below). For `alknet/call` and `alknet/channels`, the protocol is the
call / channels take-over, which the caller invokes after the dial.

### `alknet/register`

The native registration entry point, parallel to HTTP registration
(OQ-58) but without the HTTP layer. A native worker that has no HTTP
client dials `alknet/register` directly. The connection is an **entry
point** (ADR-086 §2): accepted without an established peer identity,
authenticated per-request by the registration token (or open for
no-token registration). The dial produces the `Connection`; the
registration handshake runs on it.

Two registration cases (token / no-token), both hub concerns and both
optional — see [ADR-089](../../decisions/089-alknetclient-native-dial-seam.md)
§6 for the full description.

The `alknet/register` **wire protocol** (the handshake on the
`Connection` — what frames the client sends, what the hub returns) ties
into the call crate's ACL and the OQ-58 enrollment model. It is
**deferred** to a dedicated ADR. This spec names the ALPN and its
entry-point role; the HTTP registration endpoint (OQ-58) remains the
first implementation. See OQ-66.

### Relationship to `CallClient` / `ChannelClient`

The dial produces a `Connection`; the protocol take-overs consume it:

```rust
// Dial QUIC, take over as channels:
let conn = client.dial_quic(addr, "alknet", b"alknet/channels", &creds).await?;
let channels = ChannelClient::from_connection(conn).await?;

// Dial TCP+TLS, take over as call:
let conn = client.dial_tcp_tls("hub.example", addr, b"alknet/call", &creds).await?;
let call = CallClient::new(registry, idp).spawn_dispatch(conn);
```

The existing convenience constructors (`CallClient::connect`,
`ChannelClient::connect_quic`) become thin wrappers over
`AlknetClient::dial_quic` — they build an ephemeral `AlknetClient` (or
accept one), dial QUIC, and call `spawn_dispatch` / `from_connection`.
They remain for the "I just want QUIC, no `AlknetClient` wiring" case.
A caller that needs transport selection (QUIC with TCP+TLS fallback)
uses `AlknetClient` directly. See
[ADR-089](../../decisions/089-alknetclient-native-dial-seam.md) §5.

### Iroh — shares the key, not the config (client side too)

The iroh client dial, like the iroh server side (ADR-082, ADR-087 §3),
does not consume a `rustls::ClientConfig`. It takes the
`Ed25519SecretKey` directly and feeds it to
`iroh::SecretKey::from_bytes`. Iroh handles TLS internally. The
verifier is iroh's `NodeId` match — the remote's `NodeId` (Ed25519
public key) is verified against the expected `NodeId`, which is
fingerprint-pinning by another name. An unknown iroh remote fails
closed (no CA to fall back to — ADR-034 §3, Assumption 1).

The `dial_iroh` method takes the `Ed25519SecretKey` as a parameter
rather than pulling it from `CallCredentials` because iroh does not use
the `TlsClientConfig` path. The assembly layer reads the key from
`StaticConfig` (in core) and passes it directly, same as the server
side's iroh endpoint construction.

### Non-Rust native clients (out of scope)

The wire protocols (channels 9-byte chunk format — ADR-071; call
`EventEnvelope` — ADR-012/064) are language-agnostic. When the endpoint
uses X.509 (the web endpoint type, or a native endpoint with X.509
instead of raw keys), non-Rust native clients (Node/Deno/Bun, Python,
wasm) can negotiate TLS with standard library TLS stacks and implement
the wire protocols directly. A wasm implementation of the wire
protocols is reusable both in-browser and server-side (Deno, etc.),
reducing the need for per-language native adapters.

`AlknetClient` is the **Rust** native client — one of several possible
native clients sharing the same wire protocols. The non-Rust clients
are out of scope for this crate; they implement the wire protocols in
their own languages. The X.509 endpoint type is what makes this
possible — raw-key (RFC 7250) endpoints require a TLS stack that
supports raw public keys, which most non-Rust runtimes do not (browsers
definitely do not — ADR-086).

### `ClientDialError`

The error type for all three dial methods. A single
`#[non_exhaustive]` enum, one variant per failure category:

```rust
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ClientDialError {
    /// TLS config construction (TlsClientConfig::new failure —
    /// verifier build, cert load, provider init). Wraps `TlsError`
    /// from alknet-tls.
    #[error("TLS config construction: {0}")]
    TlsConfig(#[from] alknet_tls::TlsError),

    /// Transport connect failure — quinn connect, TcpStream::connect,
    /// or iroh connect. The transport's own error type, stringified.
    #[error("transport connect: {0}")]
    Connect(String),

    /// TLS handshake failure — the handshake started but failed
    /// (rejected cert, ALPN mismatch, unknown raw-key remote
    /// fail-closed). Distinct from TlsConfig (which is pre-handshake).
    #[error("TLS handshake: {0}")]
    Handshake(String),

    /// No transport handle configured for the requested dial — e.g.,
    /// `dial_quic` called but `with_quinn` was not set.
    #[error("no transport handle configured for {transport}")]
    NoTransport { transport: &'static str },
}
```

`TlsConfig` wraps `alknet_tls::TlsError` (ADR-088) — the config
construction errors. `Connect` and `Handshake` are transport-level
failures (pre- and post-handshake). `NoTransport` is a wiring error
(calling a dial without the matching `with_*`).

**`Handshake` resolves an ADR-088 §6 deferral.** ADR-088 §6 explicitly
scoped `TlsError` to *config-construction* errors and deferred the
handshake-error surfacing question to "the dial-seam ADR" (OQ-55, then
deferred). ADR-089 is that ADR. `ClientDialError::Handshake` is the
resolution: handshake-time errors (rejected cert, ALPN mismatch,
unknown-raw-key fail-closed) surface through the dial's error type as
`Handshake(String)`, not through `TlsError` (which stays
config-construction-only). This keeps ADR-088's scope boundary intact
while giving the dial a single error enum for all failure categories.

`Connect(String)` and `Handshake(String)` take `String` rather than
wrapping the concrete transport error types (`quinn::ConnectError`,
`io::Error`, `rustls::Error`) because the three transports' error types
are non-unifiable — the dial is transport-polymorphic, and there is no
single source type that covers quinn, tokio-rustls, and iroh. The
category is in the variant (`Connect` vs `Handshake`); the detail is in
the string. This differs from ADR-088's `TlsError` (which wraps concrete
types via `#[from]`) because `TlsError` has one source crate (rustls +
pemfile + rcgen), while `ClientDialError` spans three transport crates.
The variant granularity is decided; the exact string contents are an
implementation detail.

### Feature gates

```toml
[features]
default = []
quinn = ["dep:quinn", "alknet-tls/quinn"]
tcp = ["dep:tokio-rustls", "alknet-tls/tcp"]
iroh = ["dep:iroh"]
```

A deployment that dials QUIC only enables `quinn`. A deployment that
dials TCP+TLS enables `tcp`. A deployment that dials iroh enables
`iroh`. A full native client (QUIC + TCP+TLS fallback + iroh) enables
all three. The `quinn` and `tcp` features pull the corresponding
features on `alknet-tls` (for `TlsClientConfig::for_quinn` /
`for_tcp_tls`). The `iroh` feature does not pull `alknet-tls` features
— iroh has its own TLS.

### Dependencies

```
alknet-client
├── alknet-core       (Connection, CallCredentials/RemoteIdentity if
│                     moved here, Ed25519SecretKey, types)
├── alknet-tls        (TlsClientConfig — for quinn + tcp dials)
├── quinn             (optional — dial_quic)
├── tokio-rustls      (optional — dial_tcp_tls)
├── tokio             (TcpStream, spawn)
├── iroh              (optional — dial_iroh)
└── thiserror         (ClientDialError)
```

`alknet-client` depends on `alknet-tls` (for `TlsClientConfig`) and
`alknet-core` (for `Connection` and types). It does **not** depend on
`alknet-call` or `alknet-channels-call` — the dial is below the
protocol. If `CallCredentials` / `RemoteIdentity` stay in
`alknet-call`, `alknet-client` depends on `alknet-call` for the type
only; the cleaner option (moving the credential type to `alknet-core`
or `alknet-client`) keeps the dial below the protocol. See
[ADR-089](../../decisions/089-alknetclient-native-dial-seam.md) §5 —
this is a two-way-door implementation detail.

## Crate dependencies (in the dep graph)

```
alknet-client
├── alknet-tls (TlsClientConfig)
│   └── alknet-core
└── alknet-core (Connection, types)

alknet-hub (uses AlknetClient for outbound worker dials)
├── alknet-client (the dial — the hub's dial_worker closure calls it)
├── alknet-channels-call (ChannelClient — the take-over)
├── alknet-call (CallAdapter, Dispatcher)
└── alknet-endpoint (AlknetEndpoint)

alknet-worker (uses AlknetClient to dial a hub)
├── alknet-client (the dial)
├── alknet-channels-call (ChannelClient — the take-over)
└── alknet-core (AlknetEndpoint — if the worker accepts inbound)
```

`alknet-call` and `alknet-channels-call` do **not** depend on
`alknet-client`. Their take-over APIs (`spawn_dispatch`,
`from_connection`) consume a `Connection` from any source —
`AlknetClient` is one producer, but a test can hand them a
`Connection::from_stream` directly. The dependency direction is:
`alknet-client → alknet-tls → alknet-core`; the protocol crates are
parallel, not downstream of the dial.

## Assembly layer integration

A downstream worker or hub uses `alknet-client` like this:

```rust
// 1. Build the transport handles (assembly layer — same pattern as
//    the server side's AlknetEndpoint builder).
let quinn_endpoint = quinn::Endpoint::client("0.0.0.0:0".parse()?)?;

// 2. Build the AlknetClient with the transport handles it needs.
let client = AlknetClient::new()
    .with_quinn(quinn_endpoint);
    // .with_tcp_tls(tls_connector) — if TCP+TLS fallback is needed
    // .with_iroh(iroh_endpoint)   — if iroh is needed

// 3. Derive credentials from the vault (ADR-014 — no env vars).
let creds = CallCredentials::new()
    .with_tls_identity(TlsIdentity::RawKey(local_key))
    .with_remote_identity(RemoteIdentity {
        fingerprint: hub_fingerprint,  // known peer → fingerprint pin
    });

// 4. Dial the hub on alknet/channels, take over as channels.
let conn = client
    .dial_quic(hub_addr, "alknet", b"alknet/channels", &creds)
    .await?;
let channels = ChannelClient::from_connection(conn).await?;

// 5. Discover the hub's operations via from_call on channel 0.
let bundles = from_call(channels.call(), FromCallConfig::new()).await?;
channels.register_imported_all(bundles);
```

The hub's `supervise_worker` (hub README §"Dial") takes a `dial`
closure that produces a `Connection`. That closure can call
`client.dial_quic(...)` internally — the hub does not need to know
`AlknetClient` exists. The closure seam is preserved; `AlknetClient` is
the recommended dial producer for it.

## Design Decisions

All design decisions are documented as ADRs in
[decisions/](../../decisions/).

| ADR | Decision | Summary |
|-----|----------|---------|
| [089](../../decisions/089-alknetclient-native-dial-seam.md) | AlknetClient — native client dial seam | New crate `alknet-client`; client-side analogue of `AlknetEndpoint`; three dials (QUIC + TCP+TLS via `TlsClientConfig`, iroh via key); resolves OQ-55; `alknet/register` named, wire protocol deferred |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-55** (resolved by ADR-089): `AlknetClient` native dial seam —
  the transport-polymorphic dial is extracted. The deferral's blocking
  condition (a second transport's real dial) is met within the native
  endpoint type (QUIC + TCP+TLS + iroh). The web/browser client
  (WebSocket, HTTP) was never in scope. See
  [ADR-089](../../decisions/089-alknetclient-native-dial-seam.md).
- **OQ-66** (deferred(scope)): `alknet/register` wire protocol — the
  native registration handshake (token / no-token, the frames,
  `PeerEntry` creation, session credential return). Named as a
  dialable ALPN by ADR-089; the wire protocol ties into the call
  crate's ACL and OQ-58 (the token model is the shared blocker) and
  needs a dedicated ADR. The HTTP registration endpoint (OQ-58)
  remains the first implementation.

## References

- [ADR-089](../../decisions/089-alknetclient-native-dial-seam.md) —
  the decision this spec implements
- [ADR-083](../../decisions/083-endpoint-as-accept-loop-runner.md) —
  `AlknetEndpoint` (the server-side shape this spec mirrors)
- [ADR-086](../../decisions/086-endpoint-types-and-entry-points.md) —
  endpoint types (native = QUIC + TCP+TLS + iroh); entry-point vs.
  endpoint ALPN distinction
- [ADR-087](../../decisions/087-tlsclientconfig-not-blocked-on-dial.md)
  — `TlsClientConfig` (the prerequisite the dial consumes)
- [ADR-082](../../decisions/082-alknet-tls-extraction.md) —
  `TlsServerConfig` / `TlsClientConfig` in `alknet-tls`
- [ADR-065](../../decisions/065-connection-from-stream-generic-single-stream.md)
  — `Connection::from_stream` / `from_bidi` (the `Connection`
  constructors the dials use)
- [ADR-034](../../decisions/034-outgoing-only-x509-and-three-peer-roles.md)
  — client-side verifier selection (fingerprint pin vs CA vs
  fail-closed)
- [ADR-084](../../decisions/084-aws-lc-rs-crypto-provider.md) —
  aws-lc-rs crypto provider
- [ADR-080](../../decisions/080-channelclient.md) —
  `ChannelClient::from_connection` (the take-over the dial feeds)
- [ADR-017](../../decisions/017-call-protocol-client-and-adapter-contract.md)
  — `CallClient::spawn_dispatch` (the take-over the dial feeds)
- [`crates/endpoint/README.md`](../endpoint/README.md) — `AlknetEndpoint`
  (the server-side complement)
- [`crates/tls/README.md`](../tls/README.md) — `TlsClientConfig`
- [`crates/call/client-and-adapters.md`](../call/client-and-adapters.md)
  — `CallClient` (the protocol take-over)
- [`crates/channels/channel-client.md`](../channels/channel-client.md)
  — `ChannelClient` (the protocol take-over)
- [`crates/hub/README.md`](../hub/README.md) §"Dial (outbound workers)"
  — the hub-as-client case (the `supervise_worker` closure that calls
  the dial)
- OQ-55 (resolved) — `AlknetClient` / client establishment extraction
- OQ-58 — worker registration flow (the HTTP path; `alknet/register`
  is the native analogue)
- OQ-66 (deferred) — `alknet/register` wire protocol