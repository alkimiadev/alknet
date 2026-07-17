# ADR-091: `ConnectionCredentials` — Decouple the Dial Credentials from the Call Protocol

## Status

Accepted (amends ADR-089 §3 and §5; amends ADR-087's `TlsClientConfig::new`
input framing; amended 2026-07-17 — `CallCredentials` is removed, not
retained in `alknet-call`; `from_call`'s `credentials_auth_token` dead
path removed; `auth_token` is a per-request payload field, not a
call-protocol credential)

## Context

ADR-089 extracted the dial into `AlknetClient` and moved `CallCredentials`
from `alknet-call` to `alknet-core` so the dial would not depend on the
call protocol. The three dial signatures were:

```rust
dial_quic(addr, server_name, alpn, credentials: &CallCredentials) -> Connection
dial_tcp_tls(host, addr, alpn, credentials: &CallCredentials) -> Connection
dial_iroh(node_id: iroh::NodeId, alpn, local_key: &Ed25519SecretKey) -> Connection
```

Two problems surfaced on review:

### Problem 1: the iroh dial signature is asymmetric

`dial_quic` and `dial_tcp_tls` take `&CallCredentials`; `dial_iroh` takes
a bare `&Ed25519SecretKey` + a separate `node_id: iroh::NodeId`. The
asymmetry exists because iroh has its own TLS (it shares the key, not
the rustls config — ADR-087 §3), so the iroh dial bypasses
`TlsClientConfig` and reads the key directly. But the asymmetry forces
the caller to know which dimension of the credential bundle each
transport consumes, and it leaves no path for the iroh dial to receive
the same inputs as the rustls dials — even though all three consume the
same two things: a local identity (key/cert) and an expected remote
identity (fingerprint).

### Problem 2: `CallCredentials` couples the dial to the call protocol

`CallCredentials` carries three dimensions (ADR-017 §7):

1. `tls_identity: Option<TlsIdentity>` — the local node's key/cert
2. `auth_token: Option<AuthToken>` — a call-protocol-level bearer token
3. `remote_identity: Option<RemoteIdentity>` — the expected remote fingerprint

The dial uses only dimensions 1 and 3 (the transport-identity layer).
Dimension 2 (`auth_token`) is a **call-protocol** concept: it correlates
a token to an identity via `IdentityProvider::resolve_from_token` — a
mechanism that exists for two hub-dependent cases where TLS-fingerprint
identity is unavailable:

- **Browsers** — no raw-key support, no client cert the hub can
  fingerprint; the browser authenticates via a bearer token over
  HTTP/WebSocket, and the hub's `IdentityProvider` resolves it.
- **`alknet/register`** — a native worker that hasn't been enrolled dials
  in with no prior peer relationship; a registration token (or open
  registration) establishes identity, not a TLS fingerprint.

Both depend on a **hub** running `IdentityProvider` with token-to-identity
mapping. A pure P2P connection (two nodes with raw-key identities) never
needs `auth_token` — the TLS fingerprint IS the identity.

`auth_token` is not a transport credential. It is a per-request field on
`call.requested` payloads (`Dispatcher::resolve_identity` reads
`payload.get("auth_token")`; the `from_call` forwarding handler sets it
via `build_forwarded_payload`). The dial never delivers it to the
protocol take-over — `spawn_dispatch(&self, connection: Connection)`
takes no credentials, and `Connection` (a `Box<dyn BidiStreamSource>`)
carries no `auth_token` field. The `auth_token` in `CallCredentials` is
unused by the dial and dropped after `connect()` in the current code.

By moving `CallCredentials` (with `auth_token` in it) to `alknet-core`
for the dial's benefit, ADR-089 §5 would drag a call-protocol concept
into the shared-types crate *for the dial's benefit* — when the dial
doesn't use it. The dial should consume a transport-level credential
bundle, not a call-protocol one.

### The two identity models

Underneath the three transports, there are two identity-consumption
models, both consuming the same two dimensions:

| Model | Transports | Consumes | What the transport does |
|-------|-----------|----------|------------------------|
| **rustls config** | QUIC (quinn), TCP+TLS (tokio-rustls) | `local_identity` → `TlsClientConfig` (client cert); `remote_identity` → verifier (`FingerprintPinVerifier` / `WebPkiServerVerifier`) | Builds `rustls::ClientConfig`, hands to transport connector |
| **key-native** | iroh, SSH (future — `docs/research/references/ssh/russh/06-usage-patterns.md`) | `local_identity` → `Ed25519SecretKey` → transport's key type (`iroh::SecretKey`, russh key); `remote_identity` → fingerprint → transport's verifier (`NodeId` match, known_hosts) | Reads the key directly; transport handles identity internally |

The difference is *how* each model consumes the dimensions, not *what*
they are. A unified credential bundle carrying just those two dimensions
lets every dial extract what its transport's identity layer needs,
without call-protocol coupling.

## Decision

### `ConnectionCredentials` — the dial's credential bundle

A new type in `alknet-core`, carrying the two transport-identity
dimensions every dial consumes:

```rust
/// Transport-level credentials for an outbound dial. Consumed by
/// `AlknetClient`'s dial methods and (for the server side) by the
/// assembly layer when building transports. Carries only the dimensions
/// the transport's identity layer needs — the local identity (key/cert
/// presented to the transport) and the expected remote identity
/// (fingerprint, driving verifier selection per ADR-034).
///
/// This is NOT the call-protocol credential bundle. The call-protocol
/// `auth_token` (hub-correlated bearer for browsers / `alknet/register`)
/// is a per-request field on `call.requested` payloads, not a
/// transport credential. It stays in the call-protocol layer.
pub struct ConnectionCredentials {
    /// The local node's identity (RFC 7250 raw key or X.509), presented
    /// to the transport's identity layer. rustls dials → `TlsClientConfig`
    /// (client cert via `RawKeyClientCertResolver`); iroh/SSH dials →
    /// key directly (`iroh::SecretKey::from_bytes`, russh key).
    pub local_identity: Option<TlsIdentity>,

    /// Expected identity of the remote node. `Some(fingerprint)` → pin
    /// (known peer); `None` → CA verification for X.509 remotes or
    /// fail-closed for Ed25519 raw-key remotes (ADR-034 §2/§3). `None`
    /// is the public-X.509-endpoint state, not a missing field.
    pub remote_identity: Option<RemoteIdentity>,
}
```

`RemoteIdentity` moves with `ConnectionCredentials` to `alknet-core`
(both are transport-level types; the dial and the server-side transport
construction both consume them).

### Unified dial signatures

All three dials take `&ConnectionCredentials`:

```rust
impl AlknetClient {
    #[cfg(feature = "quinn")]
    pub async fn dial_quic(
        &self,
        addr: SocketAddr,
        server_name: &str,
        alpn: &[u8],
        creds: &ConnectionCredentials,
    ) -> Result<Connection, ClientDialError>;

    #[cfg(feature = "tcp")]
    pub async fn dial_tcp_tls(
        &self,
        host: &str,
        addr: SocketAddr,
        alpn: &[u8],
        creds: &ConnectionCredentials,
    ) -> Result<Connection, ClientDialError>;

    #[cfg(feature = "iroh")]
    pub async fn dial_iroh(
        &self,
        alpn: &[u8],
        creds: &ConnectionCredentials,
    ) -> Result<Connection, ClientDialError>;
}
```

The `node_id: iroh::NodeId` parameter on `dial_iroh` is removed — it is
derived from `creds.remote_identity.fingerprint` (`ed25519:<hex>` →
`NodeId::from_bytes`), the same way the rustls dials derive their
verifier from `remote_identity`. The consistency is now in both the rule
(ADR-034) and the type.

Each dial extracts what its transport's identity layer needs:

- **rustls dials** (`dial_quic`, `dial_tcp_tls`): `creds.local_identity`
  → `TlsClientConfig::new` (client cert); `creds.remote_identity` →
  `ClientVerifierContext` (verifier selection).
- **iroh dial** (`dial_iroh`): `creds.local_identity` →
  `Ed25519SecretKey` → `iroh::SecretKey::from_bytes`;
  `creds.remote_identity.fingerprint` → `NodeId` (verifier).

### `CallCredentials` is removed (amendment 2026-07-17)

> **This section supersedes the original "CallCredentials stays in
> `alknet-call`" decision.** The original rationale rested on a code
> path that does not exist. The trace below is the correction.

`CallCredentials` is **removed**, not retained. Once the transport
dimensions (`local_identity`, `remote_identity`) move to
`ConnectionCredentials` in `alknet-core`, `CallCredentials` would
reduce to a one-field struct `{ auth_token: Option<AuthToken> }` — and
that field has **no reader**.

**The trace (why the original rationale was wrong).** The original
section claimed the call protocol uses `CallCredentials.auth_token`
because "the `from_call` forwarding handler populates `auth_token` on
outgoing `call.requested` payloads." That chain does not connect:

- `from_call`'s signature is `from_call(connection: &CallConnection,
  config: FromCallConfig)` — no `CallCredentials` parameter.
  `FromCallConfig` has no credential field.
- The `auth_token` the `from_call` forwarding handlers *can* set on
  payloads is sourced from `OpSummary.credentials_auth_token`, an
  `Option<String>` that is **hardcoded to `None` at every construction
  site** (`from_call.rs:185, 748, 757`). It is not read from
  `CallCredentials.auth_token`, and it is a different type
  (`Option<String>` vs `Option<AuthToken>`). The two were never
  connected, even in intent.
- The consuming side — `Dispatcher::resolve_identity`
  (`dispatch.rs:119`) — reads `payload.get("auth_token").as_str()` from
  the per-request call payload. It does not read `CallCredentials`.

**Where `auth_token` actually originates.** It is a per-request payload
field, populated by two real paths, neither of which touches
`CallCredentials`:

- **Browsers over WebSocket** — the browser sends `auth_token` directly
  in the `call.requested` JSON payload (`websocket/mod.rs:202–206`); the
  WS layer (`upgrade.rs:178–181`) passes `envelope.payload` straight to
  `dispatch_requested`. The browser is the originator; the WS layer is
  a transparent passthrough.
- **HTTP gateway (bearer)** — `gateway/dispatch.rs` resolves the
  `Authorization: Bearer` header to an `Identity` at the HTTP boundary
  (`resolve_bearer`, line 58) and passes the `Identity` into
  `build_root_context`. The call protocol sees the resolved `Identity`,
  not the token. `auth_token` does not enter the call payload on this
  path.

So `CallCredentials.auth_token` is a write-only field (it has a setter,
`with_auth_token`, and zero readers). `connect()` — `CallCredentials`'s
only consumer — is removed in Phase 5 of the migration. With `connect`
gone, nothing constructs or reads `CallCredentials` except the tests.

**`auth_token`'s two real use cases (confirming no call-protocol
credential bundle is needed):**

1. **HTTP auth** — the inbound case. The HTTP gateway resolves the
   bearer token to an `Identity` via `IdentityProvider::resolve_from_token`
   at the HTTP boundary. The call protocol receives the `Identity`, not
   the token.
2. **Registration** (`alknet/register` native ALPN, `/register` HTTP
   endpoint) — a client not yet associated with a hub presents a
   one-time registration token; the hub creates a `PeerEntry` (a new
   identity based on the fingerprint). Outbound, the vault manages the
   token on the client side; inbound, the hub's registration handler
   consumes it. Neither path involves `CallCredentials`.

A hub does not "forward with its own token" in the way the original
rationale assumed. Where the hub authenticates to an outside service
(another hub's HTTP interface, an external API), the vault manages that
outbound token — it is not a call-protocol credential. The
`from_call` `credentials_auth_token` path was a future hatch for a
use case that dissolved once `IdentityProvider::resolve_from_token`
solved the inbound identity problem: the hub authenticates as itself
(its `Identity` is on the connection), and the spoke authorizes the hub
as the direct caller. No per-forwarded-call token is needed.

**`from_call`'s `credentials_auth_token` is removed too.** It is the
same family of dead code — an always-`None` field of a different type
than `CallCredentials.auth_token`, never connected to anything. The
`credentials_auth_token` field on `OpSummary`, the `credentials_auth_token`
parameters on `make_forwarding_handler` / `make_streaming_forwarding_handler`,
and the `auth_token` parameter on `build_forwarded_payload` are removed.
The forwarding handlers stop emitting `auth_token` in payloads (which
they never did in practice — the source was always `None`). The two
`from_call` tests asserting the `Some` path
(`build_forwarded_payload_sets_auth_token_when_provided`,
`streaming_forwarding_handler_sets_auth_token_when_provided`) are
removed — they test a code path never exercised in production. If a
future hub needs its own token on forwarded payloads, that is a fresh,
end-to-end-wired feature, not a vestigial path.

**What does NOT move to `alknet-core`:** `ConnectionCredentials` and
`RemoteIdentity` move (the original decision). `CallCredentials` does
not move — it is removed. ADR-089 §5's move of `CallCredentials` to
core is superseded twice over: first by the original ADR-091 (move
`ConnectionCredentials` instead), and now by this amendment (remove
`CallCredentials` entirely). There is no call-protocol credential
bundle; `auth_token` is a per-request payload field, full stop.

### `TlsClientConfig::new` input framing

`TlsClientConfig::new` (ADR-087) takes a `ClientVerifierContext` derived
from the credential bundle's `remote_identity`. The rustls dials extract
`creds.local_identity` and `creds.remote_identity` from
`ConnectionCredentials` and build `ClientVerifierContext` from the latter
— the same extraction ADR-087 described, just from
`ConnectionCredentials` instead of `CallCredentials`. The `auth_token`
dimension is simply not present in `ConnectionCredentials`, so the
"stripped at the TLS boundary" framing (ADR-089's claim that the token
"travels with the Connection") is no longer needed — the token was never
in the dial's credential bundle to strip.

### Future `dial_ssh` validates the shape

An SSH dial (`docs/research/references/ssh/russh/06-usage-patterns.md`)
consumes the same two dimensions:

- `check_server_key(&mut self, key: &ssh_key::PublicKey)` — the verifier
  (fingerprint pin against known_hosts = `remote_identity`)
- `authenticate_publickey("user", PrivateKeyWithHashAlg::new(...))` —
  local identity (the Ed25519 key = `local_identity`)
- `channel_open_session()` → `Connection::from_bidi` (ADR-065)

`dial_ssh(addr, alpn, creds: &ConnectionCredentials)` fits the same
signature. The SSH host-key verification is fingerprint-pinning
(known_hosts), which is what `remote_identity` carries. The local SSH
key is the same Ed25519 key iroh and raw-key quinn use. The pattern is
general — `ConnectionCredentials` covers it without call-protocol
coupling. SSH itself is unspecced (not yet specced — comes after
channels, tunnels, TTY rework), but the russh usage patterns confirm
the credential dimensions.

## Consequences

**Positive:**

- **The dial is fully decoupled from the call protocol.**
  `ConnectionCredentials` carries only transport-identity dimensions;
  `alknet-client` has no call-protocol coupling in its credential type.
  There is no call-protocol credential bundle — `auth_token` is a
  per-request payload field, not a credential.
- **All three dial signatures are unified.** A caller no longer needs to
  know that iroh takes a bare key while quinn/tcp take a credential
  bundle — all take `&ConnectionCredentials`. The `node_id` parameter on
  `dial_iroh` is derived from `remote_identity`, the same extraction
  pattern the rustls dials use for the verifier.
- **The `auth_token` spec inaccuracy is fixed.** ADR-089 claimed the
  `auth_token` "travels with the `Connection` into the protocol
  take-over, where it is sent as the first call-protocol frame." This
  was aspirational — `Connection` carries no `auth_token`, and
  `spawn_dispatch` takes no credentials. With `CallCredentials` removed,
  the claim is not merely unneeded; the field it described was never
  read. `auth_token` is a per-request field on `call.requested` payloads,
  set by browsers (in the WS payload) or resolved by the HTTP gateway at
  its boundary (bearer → `Identity`).
- **`dial_ssh` fits the same shape when it arrives.** The credential
  dimensions SSH needs (local key + expected host key) are exactly what
  `ConnectionCredentials` carries. No future ADR needed for the SSH dial
  signature.
- **A dead credential type and a dead forwarding-token path are removed
  (amendment 2026-07-17).** `CallCredentials` is removed (its
  `auth_token` field had no reader). `from_call`'s
  `credentials_auth_token` is removed (always `None`, different type
  than `CallCredentials.auth_token`, never connected). Both were future
  hatches from the era before `IdentityProvider::resolve_from_token`
  solved the inbound identity problem; the hatches dissolved once it
  did. See the amended §"`CallCredentials` is removed" above for the
  trace.

**Negative:**

- **`CallCredentials` is removed (a public type).** Callers that
  constructed `CallCredentials` (the integration test; any future
  assembly-layer code) switch to `ConnectionCredentials` for the dial.
  `auth_token`, where needed, is a per-request payload field (browsers
  send it in the WS payload; the HTTP gateway resolves bearer →
  `Identity` at its boundary). This is expected — `connect()` was
  `CallCredentials`'s only consumer and is removed in the same
  migration. There are no external consumers (develop branch is a total
  rewrite).
- **The assembly layer builds one credential bundle, not two.** Where
  ADR-089 had the assembly layer build one `CallCredentials` (and the
  original ADR-091 reframed it as two — `ConnectionCredentials` for the
  dial + a per-request `auth_token`), the assembly layer now builds
  `ConnectionCredentials` for the dial only. `auth_token` is not a
  credential the assembly layer constructs; it is a per-request payload
  field the browser (or the HTTP gateway's bearer resolution) supplies.
  This is fewer types at the assembly site, not more.
- **ADR-089 §5's "CallCredentials moves to core" is superseded twice.**
  The original ADR-091 reframed the move target as `ConnectionCredentials`
  (not `CallCredentials`); this amendment removes `CallCredentials`
  entirely. What moves to `alknet-core`: `ConnectionCredentials` +
  `RemoteIdentity`. What does not move: `CallCredentials` (removed, not
  relocated). This affects the extraction plan's Phase 0 (additive
  credentials move) and Phase 5 (the call prune now removes
  `CallCredentials` and the `from_call` dead path, not just `connect`
  and the TLS helpers).

## Door type

**One-way.** The dial signatures (`dial_quic` / `dial_tcp_tls` /
`dial_iroh` all taking `&ConnectionCredentials`) are the public API
surface of `alknet-client`. The credential-type decoupling
(`ConnectionCredentials` in core, no call-protocol credential bundle)
determines the dep graph (`alknet-client` depends on `alknet-core` for
`ConnectionCredentials`, not on `alknet-call`). Reversing would mean
re-coupling the dial to the call protocol's credential type and
re-asymmetrizing the iroh dial. The `CallCredentials` removal
(amendment 2026-07-17) is the same door — removing a public type whose
only consumer (`connect`) is removed in the same migration. The crate
is greenfield (Phase 3 of the extraction plan), so the door is still
open now — this ADR records the decisions before implementation.

## References

- ADR-089 — `AlknetClient` native dial seam (§3 dial signatures amended
  — all take `&ConnectionCredentials`; §5 move amended —
  `ConnectionCredentials`/`RemoteIdentity` move to core, not
  `CallCredentials`; §5 further amended 2026-07-17 — `CallCredentials`
  removed, not retained in `alknet-call`)
- ADR-087 — `TlsClientConfig` not blocked on dial (input framing
  amended — `ClientVerifierContext` derived from
  `ConnectionCredentials.remote_identity`, not `CallCredentials`)
- ADR-034 — client-side verifier selection (the rule
  `ConnectionCredentials.remote_identity` drives — unchanged)
- ADR-017 §7 — the three credential dimensions (the historical source of
  `CallCredentials`'s three fields; the transport dimensions moved to
  `ConnectionCredentials`, the `auth_token` dimension is a per-request
  payload field, and `CallCredentials` itself is removed)
- `crates/alknet-call/src/protocol/dispatch.rs` —
  `Dispatcher::resolve_identity` reads `payload.get("auth_token")`
  (per-request, not connection-level — the consumer of `auth_token`)
- `crates/alknet-call/src/client/from_call.rs` — the
  `credentials_auth_token` field on `OpSummary` and the
  `auth_token` parameter on `build_forwarded_payload` (the removed dead
  path; always `None`, different type than `CallCredentials.auth_token`,
  never connected)
- `crates/alknet-http/src/gateway/dispatch.rs` — `resolve_bearer` (the
  HTTP path: bearer → `Identity` at the boundary; the call layer sees
  the identity, not the token)
- `crates/alknet-http/src/websocket/mod.rs` — the WS path:
  `auth_token` in the browser's call payload, passed through to
  `dispatch_requested` unchanged
- `docs/research/references/ssh/russh/06-usage-patterns.md` — the SSH
  client usage patterns (check_server_key + authenticate_publickey)
  validating the `ConnectionCredentials` shape for a future `dial_ssh`