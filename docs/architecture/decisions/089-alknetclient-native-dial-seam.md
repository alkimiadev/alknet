# ADR-089: AlknetClient — the Native Client Dial Seam

## Status

Accepted (resolves OQ-55)

## Context

### The deferral and why it was valid

OQ-55 deferred `AlknetClient::dial()` — the transport-polymorphic client
dial seam — blocked on "a second transport's real dial existing." The
reasoning (recorded in the OQ-55 file and in
[channel-client.md](../crates/channels/channel-client.md) §"Relationship
to `AlknetClient`") was sound at the time: extracting a QUIC-shaped
connector and naming it `AlknetClient` would bake QUIC in as *the*
establishment shape — the same welding ADR-065 unwound on the server
side. Only one transport dial existed (`CallClient::connect` /
`ChannelClient::connect_quic`, both QUIC). The transport-polymorphic
seam was not extractable from two *different* transport implementations;
it was guessable from one.

### Why the deferral has collapsed

Three decisions landed since the deferral, each removing a blocker:

1. **ADR-086 (endpoint types)** named the native endpoint type and gave
   it **two rustls-consuming transports**: QUIC (primary) and TCP+TLS
   (fallback when UDP is blocked). Both consume `TlsClientConfig`; both
   produce a `Connection` via `Connection::from_quinn_with_alpn` /
   `Connection::from_bidi`. The native endpoint type also includes iroh
   (key-based, not rustls-consuming). That is **three dial shapes within
   one endpoint type** — two sharing `TlsClientConfig`, one using the
   raw key directly. The OQ-55 blocking condition ("a second
   transport's real dial existing") is met *within one endpoint type*,
   not across two unrelated transports.

2. **ADR-087 (`TlsClientConfig`)** broke the circular hedge that linked
   the TLS config to the dial. The client-side TLS config is extracted
   and buildable today; it is a **prerequisite** for the dial, not a
   consequence of it. Each transport-specific dial helper builds a
   `TlsClientConfig` and passes it to its transport's connector. The
   dial no longer waits on the TLS config; the TLS config is shared.

3. **ADR-083 (endpoint as accept-loop runner)** made the server side a
   clean accept-loop runner that takes pre-built transports via
   `with_quinn` / `with_iroh` / `with_tcp_tls`. The client-side analogue
   — a dialer that takes pre-built transport handles and produces a
   `Connection` — is now guessable by symmetry, not a shot in the dark.
   The server side separates "build the transport" (assembly layer)
   from "run the accept loop" (endpoint); the client side separates
   "build the transport handle" (assembly layer) from "dial + produce
   `Connection`" (`AlknetClient`).

The three together remove every blocker the deferral named. The dial
seam is extractable from two different rustls-consuming transport
implementations (QUIC + TCP+TLS) plus the key-based iroh path — three
real shapes, not one. The TLS config is shared. The server-side shape
gives the client-side shape by symmetry.

### The tangle this ADR also names

Three concept levels were conflated throughout the initial development,
contributing to the confusion that made `AlknetClient` hard to spec:

1. **Deployment role** — Hub / Worker / Hub-Worker. *Who accepts, who
   dials, in the hub-and-spoke topology.* A hub accepts inbound and may
   dial outbound (hub-as-client). A worker dials outbound and may
   accept inbound (a hub-worker). A pure worker only dials.
2. **Establishment side** — `AlknetEndpoint` (server) / `AlknetClient`
   (client). *Server-side accept vs. client-side dial.* The endpoint
   accepts connections and resolves identity from the incoming
   connection; the client dials and presents identity (client cert)
   while verifying the remote (ADR-034).
3. **ALPN-level category** — endpoint ALPN / entry-point ALPN
   (ADR-086 §2). *Identity-gated vs. bootstrap, at the TLS layer.*

These are orthogonal. A hub *uses* an `AlknetEndpoint` (server side) AND
*uses* an `AlknetClient` (client side, when dialing workers). A worker
*uses* an `AlknetClient` (client side) AND *may* use an `AlknetEndpoint`
(server side, if it accepts inbound). The role determines which side(s)
you instantiate, not what the side IS. `AlknetClient` is the client-side
establishment type — Layer 2 — independent of the deployment role that
uses it and of the ALPN-level category of the ALPN it dials.

## Decision

### 1. `alknet-client` is a new crate

`AlknetClient` lives in a new crate `alknet-client`, not in
`alknet-core` or `alknet-tls`. The dependency profile rules out the
alternatives:

- **`alknet-core` is ruled out by a cycle.** `AlknetClient` needs
  `TlsClientConfig` from `alknet-tls`, and `alknet-tls` depends on
  `alknet-core`. Putting `AlknetClient` in core creates
  `alknet-core → alknet-tls → alknet-core` — a circular dependency.
- **`alknet-tls` is the wrong scope.** That crate is "TLS config + cert
  sharing" (`TlsServerConfig` / `TlsClientConfig`), not "dial +
  transport establishment." The dial calls `quinn::Endpoint::connect`,
  `TcpStream::connect` + `TlsConnector::connect`, and
  `iroh::Endpoint::connect` — transport-connection establishment, not
  TLS config. Putting the dial in `alknet-tls` would weld transport
  establishment to cert config, the same conflation ADR-082 untangled
  on the server side.
- **Folding into `alknet-hub` / `alknet-worker`** would make both
  depend on each other or duplicate the dial. Both roles need the dial;
  the dial is not a role concern.

`alknet-client` depends on `alknet-core` (for `Connection`,
`CallCredentials`, `RemoteIdentity`, types) + `alknet-tls` (for
`TlsClientConfig`) + transport crates (quinn, tokio-rustls, iroh —
feature-gated). The DAG is clean:
`alknet-client → alknet-tls → alknet-core`. `alknet-hub` and
`alknet-worker` (and any assembly layer) depend on `alknet-client` for
the dial; `alknet-call` and `alknet-channels-call` do not — their
take-over APIs (`spawn_dispatch`, `from_connection`) consume the
`Connection` the dial produces, without knowing `AlknetClient` produced
it.

### 2. `AlknetClient` is the client-side analogue of `AlknetEndpoint`

`AlknetEndpoint` (ADR-083) is a multi-transport **accept-loop runner**:
it takes pre-built transport endpoints and runs their accept loops,
dispatching by ALPN. `AlknetClient` is a multi-transport **dialer**: it
takes pre-built transport handles and dials a remote endpoint on a
chosen ALPN, producing a `Connection` for the protocol take-overs to
consume.

The symmetry:

| Concern | `AlknetEndpoint` (server) | `AlknetClient` (client) |
|---------|---------------------------|-------------------------|
| Transports | `with_quinn` / `with_iroh` / `with_tcp_tls` — pre-built by the assembly layer | `with_quinn` / `with_iroh` / `with_tcp_tls` — pre-built by the assembly layer |
| Per-connection work | Accept → extract ALPN + fingerprint → `Connection` → `dispatch` | Dial → TLS handshake → `Connection` (ALPN + fingerprint carried) |
| Identity | Resolved *from* the incoming connection (fingerprint from client cert, or token on channel 0) | *Presented* (local `TlsIdentity` as client cert) + remote *verified* (ADR-034 — fingerprint pin or CA) |
| What it does NOT do | Run protocols — handlers do | Run protocols — `CallClient` / `ChannelClient` do |
| Config | `TlsServerConfig` (per endpoint type, built by assembly) | `TlsClientConfig` (per-dial, built from `CallCredentials`) |

`AlknetClient` produces a `Connection`; the protocol take-overs
(`CallClient::spawn_dispatch`, `ChannelClient::from_connection`) take
over from there. This is the exact analogue of `AlknetEndpoint`
producing a `Connection` for `ProtocolHandler::handle`.

### 3. Three dial methods, one per transport family

```rust
pub struct AlknetClient {
    // Pre-built transport handles, all optional — the client dials
    // with whichever transport the remote endpoint type implies.
    #[cfg(feature = "quinn")]
    quinn: Option<quinn::Endpoint>,
    #[cfg(feature = "tcp")]
    tcp_connector: Option<tokio_rustls::TlsConnector>,
    #[cfg(feature = "iroh")]
    iroh: Option<iroh::Endpoint>,
}

impl AlknetClient {
    /// QUIC dial. Builds a `TlsClientConfig` from `credentials`
    /// (ADR-034 verifier selection + ADR-084 provider), dials `addr`
    /// on `alpn`, returns a `Connection` via
    /// `Connection::from_quinn_with_alpn`. Feature-gated on `quinn`.
    #[cfg(feature = "quinn")]
    pub async fn dial_quic(
        &self,
        addr: SocketAddr,
        server_name: &str,
        alpn: &[u8],
        credentials: &CallCredentials,
    ) -> Result<Connection, ClientDialError>;

    /// TCP+TLS dial. Builds a `TlsClientConfig` from `credentials`,
    /// connects `TcpStream`, wraps with `TlsConnector`, returns a
    /// `Connection` via `Connection::from_bidi`. Feature-gated on `tcp`.
    #[cfg(feature = "tcp")]
    pub async fn dial_tcp_tls(
        &self,
        host: &str,
        addr: SocketAddr,
        alpn: &[u8],
        credentials: &CallCredentials,
    ) -> Result<Connection, ClientDialError>;

    /// Iroh dial. Dials `node_id` on `alpn` via the iroh endpoint. The
    /// iroh path does NOT use `TlsClientConfig` — iroh has its own TLS
    /// (shares the `Ed25519SecretKey`, not the rustls config —
    /// ADR-087 §3). The verifier is iroh's `NodeId` match (fingerprint
    /// pin by another name). Feature-gated on `iroh`.
    #[cfg(feature = "iroh")]
    pub async fn dial_iroh(
        &self,
        node_id: iroh::NodeId,
        alpn: &[u8],
        local_key: &alknet_core::config::Ed25519SecretKey,
    ) -> Result<Connection, ClientDialError>;
}
```

The three dials share `CallCredentials` (the local identity + remote
identity + auth token bundle, from `Capabilities`). The two rustls dials
(QUIC, TCP+TLS) build a `TlsClientConfig` from the credentials; the
iroh dial uses the raw `Ed25519SecretKey` directly. This mirrors the
server side's "iroh shares the key, not the config" (ADR-082, ADR-087
§3) — the consistency is in the rule (ADR-034 verifier selection), not
in the type.

### 4. The dial is transport-polymorphic across the native endpoint type

The native endpoint type (ADR-086) has QUIC + TCP+TLS (both
rustls-consuming) + iroh (key-based). `AlknetClient` dials all three.
The two rustls dials share `TlsClientConfig::new`; the iroh dial is the
exception. The dial is transport-polymorphic within the native endpoint
type — a native client can reach a native endpoint over QUIC, TCP+TLS
(when UDP is blocked), or iroh (relay-assisted p2p). The transport
choice is the caller's, driven by network conditions and the remote
endpoint's reachability.

### 5. `CallClient::connect` / `ChannelClient::connect_quic` are removed

The existing QUIC convenience constructors on `CallClient` and
`ChannelClient` (`connect` / `connect_quic`) are **removed**, not
delegated. Keeping them as thin wrappers over `AlknetClient::dial_quic`
would make `alknet-call` / `alknet-channels-call` depend on
`alknet-client` — contradicting the dep graph (§1: the protocol crates
are parallel to the dial, not downstream of it) and re-coupling every
`CallClient` user to `quinn` + `rustls` + the TLS verifier machinery,
the exact welding the extraction undoes. Duplicating the dial inline in
each convenience constructor would preserve the dep graph but defeats
the point of centralizing the dial.

The dial is a distinct concern from the protocol take-over.
`AlknetClient` is the single home for the dial; `CallClient` /
`ChannelClient` are the single home for the take-over. A caller that
wants the old one-liner shape composes two lines:
`client.dial_quic(...).await?` then
`CallClient::new(...).spawn_dispatch(conn)` (or
`ChannelClient::from_connection(conn).await?`). The one-way-door
surface is the `AlknetClient` dial + take-over pattern; the
per-protocol convenience constructors are gone, not retained as
two-way-door sugar.

This is a breaking change to `CallClient` / `ChannelClient`'s public
APIs. It is expected — the develop branch is a total rewrite addressing
issues not feasible to fix inline against `main`; there are no external
consumers to preserve compatibility for. The migration plan handles
the call-site updates.

**Consequence: `CallCredentials` / `RemoteIdentity` move to
`alknet-core`.** These types were in `alknet-call` because
`CallClient::connect` consumed them. With `connect` removed, the dial
(`AlknetClient`) is the consumer, and the dial must not depend on
`alknet-call` (§1). Both the call and channels clients need them (the
channels client takes `CallCredentials` for its own removed
`connect_quic`, and the dial takes them for all three dials). They move
to `alknet-core` — the shared-types crate, alongside `TlsIdentity` and
`AuthToken` which already live there. This is the cleaner of the two
options the original ADR-089 draft called a "two-way-door
implementation detail"; it is not implementation detail — it determines
the dep graph, and the dep graph requires it.

**Consequence: `FingerprintPinVerifier` moves to `alknet-tls`.** With
`connect` removed and the verifier-selection logic centralized in
`TlsClientConfig::new` (ADR-087), `FingerprintPinVerifier` has no
remaining home in `alknet-call`. It is a TLS concern (it implements
`rustls::client::danger::ServerCertVerifier`); moving it to
`alknet-tls` lets `alknet-call` shed its direct `rustls`,
`rustls-pemfile`, and `rustls-native-certs` deps entirely — `CallClient`
becomes a pure protocol crate (`{registry, identity_provider}` +
`spawn_dispatch`). See ADR-087 §5 (amended).

**Consequence: `ClientError` is removed.** The existing
`ClientError { Transport, TlsSetup, ConnectionClosed }` was produced
only by `connect` (`Transport` and `TlsSetup`) and by no current
`spawn_dispatch` path (`ConnectionClosed` is a `FrameError`/`StreamError`
variant internal to the dispatch loop, not a `CallClient` API error).
With `connect` gone, `ClientError` has no producing call site. It is
removed rather than left as a vestigial enum. If `spawn_dispatch` ever
gains a failure path, a fresh error type is cleaner than retrofitting
this one.

### 6. `alknet/register` is a dialable ALPN (entry point, wire protocol deferred)

`AlknetClient::dial_quic` / `dial_tcp_tls` can dial the `alknet/register`
ALPN — the native registration entry point, parallel to HTTP
registration (OQ-58) but without the HTTP layer. The connection is an
**entry point** (ADR-086 §2): accepted without an established peer
identity, authenticated per-request by the registration token (or open
for no-token registration). The dial is the same as any other ALPN; the
difference is the protocol that runs on the resulting `Connection`.

Two registration cases, both hub concerns and both optional:

- **Token registration** — a freshly-provisioned worker (docker,
  vast.ai, runpod) generates its local identity, dials the hub on
  `alknet/register`, presents the one-time registration token, and
  enrolls its key. The hub creates a `PeerEntry` and returns a session
  credential.
- **No-token (open) registration** — a hub that hosts public services
  over channels, or a relay/gateway, accepts registration without a
  token. The enrollment creates a `PeerEntry` with no token
  requirement.

The `alknet/register` **wire protocol** (the handshake on the
`Connection` after the dial — what frames the client sends, what the
hub returns) ties into the call crate's ACL and the OQ-58 enrollment
model. It is **deferred** to a dedicated ADR — this ADR names the ALPN
and its entry-point role; it does not specify the wire protocol. The
HTTP registration endpoint (OQ-58) remains the first implementation;
`alknet/register` is the native analogue that removes the HTTP
dependency for workers that have no HTTP client.

### 7. OQ-55 is resolved for the native dial

OQ-55's blocking condition ("a second transport's real dial existing")
is met: the native endpoint type has two rustls-consuming transports
(QUIC + TCP+TLS) + iroh (key-based) — three dial shapes, two sharing
`TlsClientConfig`. The transport-polymorphic dial seam is extractable
from two different transport implementations. `AlknetClient` is that
seam, for the native case.

The **web/browser client** (WebSocket, HTTP — the browser bidirectional
path per ADR-044/048) was never what OQ-55 was about. The browser path
is a different client surface (the JS SDK / wasm), not a Rust dial. It
does not use `AlknetClient`; it negotiates TLS via the browser's
network stack and speaks the wire protocol over WebSocket. OQ-55
deferred the Rust transport-polymorphic dial; the browser path is out
of scope and always was. The non-Rust native clients (Node/Deno/Bun,
Python, wasm) that can negotiate TLS against an X.509 endpoint and
implement the wire protocols directly are also out of scope for
`AlknetClient` — `AlknetClient` is the Rust native client, one of
several possible native clients sharing the same wire protocols.

## What this does NOT change

- **`AlknetEndpoint` (ADR-083)** — the server side is unchanged. The
  client is a new type, not a modification to the endpoint.
- **`TlsClientConfig` (ADR-087)** — the client-side TLS config is
  unchanged. `AlknetClient` calls `TlsClientConfig::new` per-dial; the
  config is a prerequisite, not a consequence of the dial (the
  relationship ADR-087 established).
- **`CallClient::spawn_dispatch` / `ChannelClient::from_connection`**
  — the take-over APIs are unchanged. They consume the `Connection`
  the dial produces; they do not know `AlknetClient` produced it.
- **`CallCredentials` / `RemoteIdentity`** — **moved to `alknet-core`**
  (see §5). The shape is unchanged; the location changes from
  `alknet-call` to `alknet-core` so the dial does not depend on the
  call protocol. Both the call and channels clients consume them from
  core.
- **The channels substrate (ADR-071)** — unchanged. The dial produces a
  `Connection`; the channels protocol runs on it.
- **ADR-086 (endpoint types / entry points)** — the endpoint-type model
  is unchanged. `AlknetClient` is the client-side consumer of the
  native endpoint type. The entry-point vs. endpoint ALPN distinction
  (§2) governs which ALPNs the client can dial and whether identity is
  required — `AlknetClient` dials both; the protocol on the resulting
  `Connection` differs.
- **The hub's `supervise_worker` (hub README §"Dial")** — the hub's
  supervision loop takes a `dial` closure that produces a
  `Connection`. That closure can call `AlknetClient::dial_quic` /
  `dial_tcp_tls` internally. The hub does not need to know
  `AlknetClient` exists — the closure seam is preserved. The hub spec
  is updated to note `AlknetClient` as the recommended dial producer
  for the closure.

## Consequences

**Positive:**

- **OQ-55 is resolved.** The transport-polymorphic dial seam is
  extracted, for the native case. The duplicated dial boilerplate
  (the removed convenience constructors each rebuilt
  `TlsClientConfig::new` + their transport's connector) is centralized
  in `AlknetClient`. The friction the deferral accepted is removed.
- **`alknet-call` becomes a pure protocol crate.** With `connect`
  removed and `FingerprintPinVerifier` moved to `alknet-tls` (§5),
  `alknet-call` sheds its direct `quinn`, `rustls`, `rustls-pemfile`,
  and `rustls-native-certs` deps. `CallClient` is `{registry,
  identity_provider}` + `spawn_dispatch` — no TLS, no transport. Every
  handler crate that uses `CallClient` stops transitively linking the
  TLS/transport stack.
- **The client-side shape is symmetric with the server side.** A
  reader who understands `AlknetEndpoint` (accept + dispatch) can
  understand `AlknetClient` (dial + produce `Connection`) by
  symmetry. The concept layers (role / side / ALPN-category) are
  named, reducing the tangle that made the client hard to spec.
- **The hub-as-client case is first-class.** A hub that dials workers
  (or another hub) uses `AlknetClient` — the same type a worker uses
  to dial a hub. The role asymmetry (hub vs. worker) does not produce
  a type asymmetry; both use the same client.
- **Transport selection is the caller's.** A native client that needs
  QUIC-with-TCP+TLS-fallback dials QUIC first, falls back to TCP+TLS
  on connection failure. `AlknetClient` provides both dials; the
  fallback policy is a caller concern (or a future `dial_with_fallback`
  helper — two-way-door).
- **`alknet/register` is named.** The native registration entry point
  has a home in the ALPN registry, parallel to HTTP registration. The
  wire protocol is deferred, but the ALPN and its entry-point role are
  decided — a worker that has no HTTP client can register natively.

**Negative:**

- **Breaking change: `CallClient::connect` / `ChannelClient::connect_quic`
  removed; `CallCredentials` / `RemoteIdentity` / `FingerprintPinVerifier`
  relocated; `ClientError` removed.** Call sites that used the
  convenience constructors must switch to `AlknetClient::dial_*` +
  `spawn_dispatch` / `from_connection`. Import paths for
  `CallCredentials` / `RemoteIdentity` change from `alknet_call` to
  `alknet_core`. This is expected — the develop branch is a total
  rewrite; there are no external consumers to preserve compatibility
  for. The migration plan handles the call-site + import updates.
- **A new crate.** `alknet-client` is one more crate in the workspace.
  The cost is low (the dial is narrow), and the dependency profile
  rules out the alternatives, but it is a new entry in the crate
  graph.
- **The iroh dial is the exception.** It does not use
  `TlsClientConfig` — iroh has its own TLS. The dial helper applies
  the same ADR-034 rule via iroh's API (NodeId match). The
  consistency is in the rule, not in the type. This is the same
  exception as the server side (ADR-082, ADR-087 §3) — unavoidable,
  and isolated to one dial method.
- **The `alknet/register` wire protocol is still deferred.** This ADR
  names the ALPN and its role; the handshake protocol (token/no-token,
  the frames, the `PeerEntry` creation, the session credential return)
  is a separate ADR tied to OQ-58. A worker cannot register natively
  until that ADR lands; the HTTP path (OQ-58) remains the first
  implementation.
- **The ADR-086 entry-point/endpoint terminology is under-specified
  as a general abstraction.** This ADR uses the current terms
  (entry-point = no identity at TLS; endpoint = identity required) but
  does not re-litigate them. The broader abstraction — that all
  top-level ALPNs are "entry points to the endpoint," each handling
  auth in its own way — is a separate conceptual refinement, not
  this ADR's scope.

## Door type

**One-way (crate existence + dial seam).** `alknet-client` as the
shared client dial crate is structural — every outbound-dialing role
(hub, worker, hub-worker) depends on it. Reversing would mean
re-distributing the dial across crates, reintroducing the duplicated
boilerplate. The three-dial API (`dial_quic` / `dial_tcp_tls` /
`dial_iroh`) is one-way — changing the signatures after consumers exist
is a rewrite. The internal implementation (how `CallCredentials` feeds
`TlsClientConfig::new`, how the iroh dial maps the `Ed25519SecretKey`)
is two-way. The `alknet/register` ALPN name is one-way (wire
compatibility); its wire protocol is two-way until the dedicated ADR
lands.

## References

- OQ-55 (resolved by this ADR) — `AlknetClient` / client establishment
  extraction
- [ADR-083](083-endpoint-as-accept-loop-runner.md) — `AlknetEndpoint`
  as multi-transport accept-loop runner; the server-side shape this
  ADR mirrors on the client side
- [ADR-086](086-endpoint-types-and-entry-points.md) — endpoint types
  (native has QUIC + TCP+TLS + iroh); entry-point vs. endpoint ALPN
  distinction (§2)
- [ADR-087](087-tlsclientconfig-not-blocked-on-dial.md) —
  `TlsClientConfig` not blocked on the dial seam; breaks the circular
  hedge; the TLS config is a prerequisite for the dial
- [ADR-082](082-alknet-tls-extraction.md) — `TlsServerConfig` /
  `TlsClientConfig` in `alknet-tls`; "iroh shares the key, not the
  config"
- [ADR-065](065-connection-from-stream-generic-single-stream.md) —
  `Connection::from_stream` / `from_bidi`; the server-side
  generalization whose client-side analogue this ADR completes
- [ADR-034](034-outgoing-only-x509-and-three-peer-roles.md) —
  client-side verifier selection (fingerprint pin vs CA vs fail-closed)
- [ADR-084](084-aws-lc-rs-crypto-provider.md) — aws-lc-rs crypto
  provider on all paths
- [ADR-080](080-channelclient.md) — `ChannelClient::from_connection`
  (the take-over `AlknetClient` feeds)
- [ADR-017](017-call-protocol-client-and-adapter-contract.md) —
  `CallClient::spawn_dispatch` (the take-over `AlknetClient` feeds)
- OQ-58 — worker registration flow (the HTTP path; `alknet/register`
  is the native analogue)
- `docs/architecture/crates/channels/channel-client.md` §"Relationship
  to `AlknetClient`" — the deferral this ADR resolves