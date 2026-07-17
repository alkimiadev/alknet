# ADR-091: `ConnectionCredentials` — Decouple the Dial Credentials from the Call Protocol

## Status

Accepted (amends ADR-089 §3 and §5; amends ADR-087's `TlsClientConfig::new`
input framing)

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

### `CallCredentials` stays in `alknet-call`

`CallCredentials` remains the **call-protocol** credential bundle. Its
`auth_token` field stays — the call protocol uses it (the `from_call`
forwarding handler populates `auth_token` on outgoing `call.requested`
payloads; the hub's `Dispatcher::resolve_identity` resolves it via
`IdentityProvider::resolve_from_token`). The call protocol layer
assembles `CallCredentials` from `ConnectionCredentials` (the
transport-level dimensions) + the call-protocol `auth_token`, or the
caller provides the `auth_token` per-request via `call_with_payload`.

`CallCredentials` does **not** move to `alknet-core`. ADR-089 §5's move
of `CallCredentials` to core is superseded by this ADR: only
`ConnectionCredentials` and `RemoteIdentity` move to core. The call
protocol's own credential type stays in the call crate.

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
  The `auth_token` (a call-protocol / hub-layer concept) stays in
  `alknet-call` where it belongs.
- **All three dial signatures are unified.** A caller no longer needs to
  know that iroh takes a bare key while quinn/tcp take a credential
  bundle — all take `&ConnectionCredentials`. The `node_id` parameter on
  `dial_iroh` is derived from `remote_identity`, the same extraction
  pattern the rustls dials use for the verifier.
- **The `auth_token` spec inaccuracy is fixed.** ADR-089 claimed the
  `auth_token` "travels with the `Connection` into the protocol
  take-over, where it is sent as the first call-protocol frame." This
  was aspirational — `Connection` carries no `auth_token`, and
  `spawn_dispatch` takes no credentials. With `auth_token` out of the
  dial's credential bundle entirely, the claim is no longer needed. The
  token is a per-request field on `call.requested` payloads, set by the
  caller or the `from_call` forwarding handler.
- **`dial_ssh` fits the same shape when it arrives.** The credential
  dimensions SSH needs (local key + expected host key) are exactly what
  `ConnectionCredentials` carries. No future ADR needed for the SSH dial
  signature.
- **`CallCredentials` stays in `alknet-call` — its home.** The call
  protocol's own credential type is not dragged into core for the dial's
  benefit. Core gets `ConnectionCredentials` (transport-level); the call
  crate keeps `CallCredentials` (protocol-level).

**Negative:**

- **`CallCredentials` loses two fields.** `tls_identity` and
  `remote_identity` move to `ConnectionCredentials` in core;
  `CallCredentials` becomes `{auth_token: Option<AuthToken>}` (or is
  restructured — the call protocol may assemble it from
  `ConnectionCredentials` + `auth_token` at the take-over site, or the
  caller provides `auth_token` per-request). The exact restructure is a
  two-way-door implementation detail; the one-way decision is that the
  transport dimensions leave `CallCredentials`.
- **The assembly layer assembles two credential bundles, not one.** Where
  ADR-089 had the assembly layer build one `CallCredentials`, it now
  builds `ConnectionCredentials` (for the dial) and, separately,
  provides the `auth_token` to the call-protocol layer (for the
  per-request payload). This is the correct layering — the dial and the
  protocol consume different dimensions — but it is one more type at the
  assembly site.
- **ADR-089 §5's "CallCredentials moves to core" is superseded.** The
  move target changes from `CallCredentials` to `ConnectionCredentials`
  + `RemoteIdentity`. `CallCredentials` stays in `alknet-call`. This
  affects the extraction plan's Phase 0 (the additive credentials move).

## Door type

**One-way.** The dial signatures (`dial_quic` / `dial_tcp_tls` /
`dial_iroh` all taking `&ConnectionCredentials`) are the public API
surface of `alknet-client`. The credential-type decoupling
(`ConnectionCredentials` in core, `CallCredentials` in call) determines
the dep graph (`alknet-client` depends on `alknet-core` for
`ConnectionCredentials`, not on `alknet-call` for `CallCredentials`).
Reversing would mean re-coupling the dial to the call protocol's
credential type and re-asymmetrizing the iroh dial. The crate is
greenfield (Phase 3 of the extraction plan), so the door is still open
now — this ADR records the decision before implementation.

## References

- ADR-089 — `AlknetClient` native dial seam (§3 dial signatures amended
  — all take `&ConnectionCredentials`; §5 move amended —
  `ConnectionCredentials`/`RemoteIdentity` move to core, not
  `CallCredentials`)
- ADR-087 — `TlsClientConfig` not blocked on dial (input framing
  amended — `ClientVerifierContext` derived from
  `ConnectionCredentials.remote_identity`, not `CallCredentials`)
- ADR-034 — client-side verifier selection (the rule
  `ConnectionCredentials.remote_identity` drives — unchanged)
- ADR-017 §7 — the three credential dimensions (the source of
  `CallCredentials`'s three fields; this ADR splits the transport
  dimensions from the protocol dimension)
- `crates/alknet-call/src/protocol/dispatch.rs` —
  `Dispatcher::resolve_identity` reads `payload.get("auth_token")`
  (per-request, not connection-level)
- `crates/alknet-call/src/client/from_call.rs` —
  `build_forwarded_payload` sets `auth_token` on outgoing payloads
  (per-request, the hub's own token)
- `docs/research/references/ssh/russh/06-usage-patterns.md` — the SSH
  client usage patterns (check_server_key + authenticate_publickey)
  validating the `ConnectionCredentials` shape for a future `dial_ssh`