---
status: reviewed
last_updated: 2026-07-17
---

# alknet-tls

Shared TLS configuration and certificate management — server and
client. Builds a `rustls::ServerConfig` (or an ACME state machine +
cert resolver) once and shares it across multiple transports — quinn,
`tokio-rustls` (TCP+TLS), and iroh — so one certificate identity serves
QUIC and TCP endpoints simultaneously. Builds a `rustls::ClientConfig`
with ADR-034 verifier selection and ADR-084 crypto provider, shared
across all outbound-dialing crates (hub, worker, `CallClient`,
`ChannelClient`). One ACME state machine, one cert, N transports;
one verifier rule, N clients.

## What

`alknet-tls` provides `TlsServerConfig` and `TlsClientConfig` —
shareable TLS setup types that a deployment builds once and hands to
whichever transports it runs:

```rust
pub struct TlsServerConfig {
    config: rustls::ServerConfig,          // Clone-safe — Arc internally
    acme_handle: Option<JoinHandle<()>>,   // one ACME task (see lifecycle below)
}

impl TlsServerConfig {
    pub async fn new(identity: &TlsIdentity, alpns: &[Vec<u8>]) -> Result<Self, TlsError>;

    /// Produce a quinn server config. Clones the inner rustls config
    /// (cheap — Arc-shared cert resolver) and wraps it for quinn.
    /// Feature-gated on `quinn`.
    #[cfg(feature = "quinn")]
    pub fn for_quinn(&self) -> Result<quinn::ServerConfig, TlsError>;

    /// Produce a tokio-rustls acceptor for TCP+TLS. Clones the inner
    /// rustls config. Feature-gated on `tcp`.
    #[cfg(feature = "tcp")]
    pub fn for_tcp_tls(&self) -> tokio_rustls::TlsAcceptor;

    /// Borrow the underlying rustls config, for any other consumer.
    pub fn rustls_config(&self) -> &rustls::ServerConfig;
}
```

`TlsServerConfig` is **not `Clone`** — it holds a `JoinHandle` for the
ACME task, which is not cloneable. Share it via `Arc<TlsServerConfig>`.
Each accessor (`for_quinn`, `for_tcp_tls`) clones the inner
`rustls::ServerConfig`, which is cheap (it holds `Arc`s to the cert
resolver and verifier, not the raw key material). The assembly layer
builds one `TlsServerConfig`, wraps it in `Arc`, and hands `Arc::clone()`
to each transport consumer. One cert, one ACME state machine, N
transports.

## Why

Without a shareable TLS config, a `rustls::ServerConfig` built for one
transport gets consumed into that transport's wrapper (e.g.
`quinn::ServerConfig`), making the cert unreusable for a TCP+TLS
listener. For ACME the problem is worse: the `AcmeState` task spawned
inside the quinn endpoint means a TCP+TLS listener would need a second
ACME state machine for the same domain (duplicate orders, divergent cert
caches, Let's Encrypt rate-limit risk). `alknet-tls` isolates TLS setup
from the transport so one config serves all transports. The full
rationale, including the cert-reuse problem, the ACME worst case, and
the three reasons a separate crate is the right shape (dependency
isolation, ACME weight, quinn/iroh having their own TLS), is in
[ADR-082](../../decisions/082-alknet-tls-extraction.md).

### The three endpoint types (ADR-086)

A hub composes a subset of three endpoint types, each with its own
identity model and transport(s). `alknet-tls` provides the
`TlsServerConfig`s; the assembly layer builds one per endpoint type
that uses a `rustls::ServerConfig` (iroh is the exception — it has its
own TLS).

| Endpoint type | Identity | `TlsServerConfig` | Transport(s) | Browsers? |
|---------------|----------|-------------------|--------------|-----------|
| **native** | RFC 7250 raw key (Ed25519) | raw-key config | QUIC (primary), TCP+TLS (fallback when UDP blocked) | No (browsers can't do raw keys) |
| **web** | X.509 (manual or ACME) | X.509/ACME config | TCP+TLS (HTTP, WebSocket), QUIC (WebTransport — deferred) | Yes (via HTTPS / WebSocket; WebTransport when revived) |
| **iroh** | RFC 7250 raw key (NodeId) | (no `TlsServerConfig` — iroh has its own TLS) | iroh (relay-assisted QUIC) | No |
| Development | Self-signed | self-signed config | Any | No (untrusted) |

The ALPN list each `TlsServerConfig` advertises is **split by endpoint
type** (ADR-086 §3, resolving OQ-62): the native config advertises the
native ALPNs (`alknet/channels`, `alknet/call`, `alknet/ssh` future);
the web config advertises the entry-point ALPNs (`h2`, `http/1.1`) +
`alknet/channels` (for WebSocket-carrying-channels, OQ-65) +
`acme-tls/1` (appended automatically). The assembly layer filters
`registry.alpn_strings()` per config. See
[ADR-086](../../decisions/086-endpoint-types-and-entry-points.md) for
the full ALPN-list table and the entry-point/endpoint distinction.

In all cases, TLS + ALPNs "just works" — the TLS handshake negotiates the
ALPN, the `HandlerRegistry` dispatches by ALPN, the transport is a
parameter. The TLS crate's job is to make the cert available to whichever
transports the deployment runs.

## Architecture

### Server-side contents

The server-side TLS setup — `rustls::ServerConfig` construction, cert
resolvers, the ACME state machine — is in `alknet-tls/src/server.rs`.
These components were originally part of `alknet-core`'s endpoint
module (quinn-gated); ADR-082 moved them into `alknet-tls` so a
`TlsServerConfig` is shareable across transports rather than consumed
into a single transport's wrapper.

| Component | Notes |
|-----------|-------|
| `TlsServerConfig` | The central type — wraps `rustls::ServerConfig` + the optional ACME task handle |
| `build_rustls_server_config()` | Unconditional; called by `TlsServerConfig::new` |
| `for_quinn()` | Wraps the rustls config in a `QuicServerConfig` (feature-gated on `quinn`) |
| `TlsSetup` / ACME path | The `TlsServerConfig::new` ACME branch spawns the state-machine task |
| `RawKeyCertResolver` | Presents an Ed25519 key as an RFC 7250 raw public key server cert |
| `Ed25519SigningKey` | One copy in `alknet-tls`, shared by server + client (see below) |
| `AcceptAnyCertVerifier` | Accepts any client cert and extracts the fingerprint (raw-key servers don't pin client certs) |
| `SelfSignedCert` / `generate_self_signed_cert()` | The dev `SelfSigned` identity path |
| `load_cert_chain()` / `load_private_key()` | In `pem.rs`; one copy, shared by server + client |

The config types `TlsIdentity` and `Ed25519SecretKey` live in
`alknet-core` (`config.rs`) — `StaticConfig` holds a `TlsIdentity`, and
config types belong in core. `alknet-tls` imports them. `fingerprint.rs`
lives in core because it is shared by both the server path (the
endpoint extracts the fingerprint from the client cert) and the client
path (`FingerprintPinVerifier`, in `alknet-tls`, matches the server's
cert against a pinned fingerprint). The production code in
`fingerprint.rs` uses only `sha2` and manual DER parsing; the
`rustls::sign` usage is in the test helper only. See OQ-59 — the
original dep-edge concern that motivated keeping `fingerprint.rs` in
core is dissolved by ADR-089 §5 (`FingerprintPinVerifier` is in
`alknet-tls`, so its consumers are co-located).

### Client-side contents

The client-side TLS setup — verifier selection, client-auth cert
presentation, provider wiring — is in `alknet-tls/src/client.rs`.
These components were originally part of `alknet-call`'s client
module (quinn-gated); ADR-087 / ADR-089 §5 moved them into `alknet-tls`
so `alknet-call` has no direct `rustls` dep and the verifier selection
is shared across all outbound dials.

| Component | Notes |
|-----------|-------|
| `TlsClientConfig::new` | Builds a `rustls::ClientConfig` from `ConnectionCredentials` + ALPN; runs ADR-034 verifier selection + ADR-084 provider wiring + client-auth cert presentation |
| `for_quinn()` | Wraps the rustls config in a `quinn::ClientConfig` (feature-gated on `quinn`) |
| `into_rustls_config()` | Returns the inner `rustls::ClientConfig` for consumers that build their own transport wrapper (e.g. `dial_tcp_tls` wraps it in a `TlsConnector`) |
| `build_client_auth()` | Constructs the client-auth cert resolver inside `TlsClientConfig::new` |
| `select_server_verifier()` | ADR-034 verifier selection (fingerprint pin / CA / fail-closed) inside `TlsClientConfig::new` |
| `load_platform_root_cert_store()` | The unknown-X.509-remote CA path inside `TlsClientConfig::new` |
| `FingerprintPinVerifier` | A TLS concern; `TlsClientConfig::new` constructs it. Locating it in `alknet-tls` lets `alknet-call` have no direct `rustls` dep (ADR-089 §5) |
| `RawKeyClientCertResolver` | Presents the local key as an RFC 7250 raw public key client cert |
| `NoClientCertResolver` | The no-client-cert path |
| `Ed25519SigningKey` | One copy in `alknet-tls` (`signing.rs`), shared by server + client |
| `load_cert_chain()` / `load_private_key()` | In `pem.rs`; one copy, shared by server + client |

`Ed25519SigningKey` and `load_cert_chain`/`load_private_key` are single
copies in `alknet-tls`, used by both `TlsServerConfig::new` and
`TlsClientConfig::new`. Before the extraction these were duplicated
across the server (in core's endpoint module) and the client (in call's
client module); the extraction consolidated them.

The dial is in `AlknetClient` (`alknet-client`, ADR-089);
`CallClient` keeps only `spawn_dispatch`, and `alknet-call` has no
TLS/transport deps.

### `TlsServerConfig`

The central type. Built once from a `TlsIdentity` + ALPN list, shared
across transports.

```rust
pub struct TlsServerConfig {
    /// The rustls server config. Clone-safe (holds Arcs to cert resolver
    /// and verifier, not raw key material).
    config: rustls::ServerConfig,
    /// The ACME state machine task, if ACME is active. One task, shared
    /// — dropping this handle does NOT stop the ACME state machine
    /// (it's owned by the config, not the endpoint).
    acme_handle: Option<tokio::task::JoinHandle<()>>,
}
```

Construction:

```rust
impl TlsServerConfig {
    /// Build a TLS server config from the given identity and ALPN list.
    /// For ACME, spawns the ACME state machine task and wires its
    /// resolver into the rustls config. For X509/RawKey/SelfSigned,
    /// loads the cert and builds the resolver directly.
    pub async fn new(
        identity: &TlsIdentity,
        alpns: &[Vec<u8>],
    ) -> Result<Self, TlsError>;
}
```

The ALPN list is the set of ALPNs the endpoint type advertises. For a
native config: `alknet/channels`, `alknet/call`, `alknet/ssh` (future).
For a web config: `h2`, `http/1.1`, `alknet/channels` (for
WebSocket-carrying-channels, OQ-65). The list is **split by endpoint
type** (ADR-086 §3) — the assembly layer filters
`registry.alpn_strings()` per `TlsServerConfig`, not passes the same
list to both. For ACME, the `acme-tls/1` ALPN is appended
automatically (for the TLS-ALPN-01 challenge, ADR-027 §7).

### `async fn new` — lifecycle semantics

`new` is `async` because the ACME path spawns a state-machine task
(`tokio::spawn`) before returning — the spawn itself is await-free, but
the function is async so the non-ACME paths share one signature.

**ACME path**: `new` spawns the `AcmeState` task, wires its `resolver()`
into the `rustls::ServerConfig`, and returns **immediately** — it does
**not** await the first certificate. The returned `TlsServerConfig` is
usable for `for_quinn()` / `for_tcp_tls()` right away; the resolver may
return no cert until the first ACME order completes, causing TLS
handshakes to fail transiently during that window. This matches the
current code's behavior (`TlsSetup::new_acme` spawns and returns).

**Non-ACME paths** (X509 / RawKey / SelfSigned): cert loading is
synchronous file I/O (`std::fs::read`) + in-memory construction; there
is no await point in the implementation. The `async` signature is for
API uniformity with the ACME path, not because the work is async. An
implementer who finds this objectionable may split a non-async
constructor — that is a two-way-door implementation detail, not an
architecture decision.

### Behavior-preservation invariants

These load-bearing TLS behaviors must be preserved. They originate from
[ADR-027](../../decisions/027-tls-identity-redesign-acme-rawkey-decoupling.md),
which established the `TlsIdentity` model, the `Acme` variant, and the
`acme-tls/1` ALPN challenge handling. Omitting any of them produces a
crate that compiles and passes type-checks but silently changes TLS
behavior:

- **`max_early_data_size = u32::MAX`** on all server config paths (X509,
  RawKey, SelfSigned, ACME). Enables 0-RTT / early data. Omitting it
  disables 0-RTT, silently breaking clients that use it.
- **`rustls::crypto::aws_lc_rs::default_provider()`** as the crypto
  provider on all paths. Do not switch to `ring` or the process-default
  provider without a new ADR — see
  [ADR-084](../../decisions/084-aws-lc-rs-crypto-provider.md) for the
  rationale (FIPS, platform matrix, iroh consistency).
- **`AcceptAnyCertVerifier`'s `supported_verify_schemes()`** returns
  ED25519 + ECDSA P-256/P-384 + RSA PSS/PKCS1 (SHA256/384/512). This
  list determines which client cert signature algorithms the server
  accepts. Must be preserved verbatim.
- **`acme-tls/1` ALPN append** for the ACME path only (ADR-027 §7). The
  TLS-ALPN-01 challenge requires the server to advertise `acme-tls/1` in
  its ALPN list. Appended in `TlsServerConfig::new`'s ACME branch, not by
  the caller.

Transport-specific accessors:

```rust
impl TlsServerConfig {
    /// Produce a `quinn::ServerConfig` for a QUIC listener. Clones the
    /// rustls config (cheap — Arc-shared cert resolver), wraps it in
    /// `QuicServerConfig`. Returns `Result` because
    /// `QuicServerConfig::try_from(rustls::ServerConfig)` can fail if
    /// the rustls config contains quinn-incompatible settings.
    /// Feature-gated on `quinn`.
    #[cfg(feature = "quinn")]
    pub fn for_quinn(&self) -> Result<quinn::ServerConfig, TlsError>;

    /// Produce a `tokio_rustls::TlsAcceptor` for a TCP+TLS listener.
    /// Clones the rustls config. Infallible —
    /// `TlsAcceptor::new(rustls::ServerConfig)` cannot fail.
    /// Feature-gated on `tcp` (pulls `tokio-rustls`).
    #[cfg(feature = "tcp")]
    pub fn for_tcp_tls(&self) -> tokio_rustls::TlsAcceptor;

    /// Borrow the underlying rustls config, for consumers that need to
    /// build their own transport-specific wrapper not covered by
    /// `for_quinn` / `for_tcp_tls`. No current consumer (iroh reads the
    /// `Ed25519SecretKey` directly, not the rustls config — see "Iroh:
    /// shares the key, not the rustls config" below); retained for
    /// transport wrappers that do not fit `for_quinn` / `for_tcp_tls`.
    pub fn rustls_config(&self) -> &rustls::ServerConfig;
}
```

### Iroh: shares the key, not the rustls config

Iroh is different from quinn and TCP+TLS: it has its own TLS built into
the `Endpoint`, using RFC 7250 raw keys. It does not consume a
`rustls::ServerConfig` — it takes an `iroh::SecretKey` and handles TLS
internally. So `alknet-tls` does not have a `for_iroh()` method. Instead,
the assembly layer reads the `Ed25519SecretKey` from `StaticConfig`
(lives in core) and passes it to iroh's `Endpoint::builder().secret_key()`
directly. `alknet-tls` is involved only when iroh is not the sole
transport — in that case, the same `Ed25519SecretKey` feeds both
`TlsServerConfig::new(TlsIdentity::RawKey(key), ...)` (for quinn/TCP) and
`iroh::SecretKey::from_bytes(key.as_bytes())` (for iroh).

The fingerprint is normalized across all three paths (ADR-030 §6):
`ed25519:<hex>` for raw keys, whether the cert came from quinn's
`RawKeyCertResolver`, iroh's built-in TLS, or a future TCP+TLS raw-key
path. `fingerprint.rs` (in core) handles this.

### Feature gates

```toml
[features]
default = []
quinn = ["dep:quinn"]      # for_quinn() — wraps rustls config for quinn
tcp = ["dep:tokio-rustls"] # for_tcp_tls() — wraps rustls config for TCP+TLS
acme = ["dep:rustls-acme"] # ACME state machine
```

A deployment that only uses quinn enables `quinn`. A deployment that
uses TCP+TLS enables `tcp`. A deployment that uses both enables both.
ACME is opt-in (heavy dep, long-running task). The `rustls` dep is always
present (it's the core TLS library).

### Dependencies

```
alknet-tls
├── alknet-core       (TlsIdentity, Ed25519SecretKey, fingerprint — re-exported)
├── rustls            (ServerConfig, ClientConfig, cert types — always present)
├── rustls-pki-types  (CertificateDer, PrivateKeyDer, etc. — via rustls re-export
│                     or direct dep; core lists it directly)
├── rustls-pemfile    (cert/key file loading — always present)
├── rustls-native-certs (platform root cert store — always present; the
│                       unknown-X.509-remote CA path in `TlsClientConfig::new`)
├── webpki-roots      (built-in CA roots fallback — always present; merged
│                       into the root store when the platform store is empty,
│                       so a containerized deployment with no system CA bundle
│                       can still verify public X.509 remotes — see ADR-088 §5)
├── rcgen             (self-signed cert generation — always present)
├── ed25519-dalek     (Ed25519 signing key — always present, via core)
├── sha2              (fingerprint computation — always present, via core)
├── tokio             (spawn for ACME task — always present)
├── futures           (StreamExt for ACME event loop — acme-gated)
├── tracing           (logging)
├── quinn             (optional — for_quinn())
├── tokio-rustls      (optional — for_tcp_tls())
└── rustls-acme       (optional — ACME state machine)
```

`rustls-native-certs` and `webpki-roots` are always-present deps (not
feature-gated) because the unknown-X.509-remote CA-verification path in
`TlsClientConfig::new` is needed by any client dialing a public X.509
endpoint, regardless of transport (QUIC or TCP+TLS). They are not gated
under `quinn`/`tcp` — a TCP+TLS-only or QUIC-only deployment both need
the CA path. `alknet-call` does not depend on them (the dial's TLS
deps are in `alknet-tls`/`alknet-client` now).

`alknet-core` does not depend on `rustls-pemfile`, `rcgen`, or
`rustls-acme` — cert-loading, self-signed generation, and the ACME
state machine are in `alknet-tls` (on `TlsServerConfig`, not on
`AlknetEndpoint`). Core has no `acme` feature. Core does keep `quinn`
and `iroh` (for `Connection::from_quinn` / `from_iroh` — the shared
constructors the endpoint and the dial both use),
`ed25519-dalek` (`Ed25519SecretKey` in `config.rs`), and `rustls` /
`rustls-pki-types` (`fingerprint.rs` uses `rustls::pki_types` in
production and `rustls::sign` in the test helper
`build_ed25519_spki_der` — see OQ-59).

> **Terminology — hub, worker, hub-worker.** A *hub* is a node that
> accepts inbound connections from workers and browsers (the central
> node in a hub-and-spoke topology — see
> [`crates/hub/README.md`](../hub/README.md)). A *worker* is a node
> that dials out to a hub. A *hub-worker* is a node that does both
> (accepts inbound and dials out). A *pure worker* has no inbound
> endpoints. These terms come from the hub topology (ADR-029, ADR-034);
> "assembly layer" (ADR-014) is the deployment binary that wires crates
> — in practice, today, usually a hub or hub-worker.

### What `AlknetEndpoint` (in `alknet-endpoint`) does

`AlknetEndpoint` takes **no TLS config at all** — it is a
multi-transport accept-loop runner. TCP+TLS is an owned transport (via
`with_tcp_tls`), not an external loop:

```rust
impl AlknetEndpoint {
    pub fn new(
        handlers: HandlerRegistry,
        dynamic: Arc<ArcSwap<DynamicConfig>>,
        identity_provider: Arc<dyn IdentityProvider>,
        drain_timeout: Duration,
    ) -> Self;

    pub fn with_quinn(mut self, endpoint: quinn::Endpoint) -> Self;
    pub fn with_iroh(mut self, endpoint: iroh::Endpoint) -> Self;

    /// TCP+TLS is a first-class owned transport — same `run()` loop,
    /// same `shutdown()` as quinn/iroh. Feature-gated on `tcp`.
    #[cfg(feature = "tcp")]
    pub fn with_tcp_tls(
        mut self,
        listener: tokio::net::TcpListener,
        acceptor: tokio_rustls::TlsAcceptor,
    ) -> Self;

    /// Public for SSH channels / future WT (connection-internal
    /// multiplexing, not listener transports).
    pub fn dispatch(
        &self,
        connection: Connection,
        alpn: Vec<u8>,
        fingerprint: Option<String>,
        remote_addr: Option<SocketAddr>,
    );

    pub async fn run(self: Arc<Self>);
    pub async fn shutdown(&self);
}
```

The assembly layer builds the `TlsServerConfig`(s), builds the
transports (`for_quinn()` → `quinn::Endpoint::server()`,
`for_tcp_tls()` → `TlsAcceptor` paired with a `TcpListener`,
`Ed25519SecretKey` → iroh), and hands them to `AlknetEndpoint` via
builder methods. A hub serving native clients and browsers holds two
`TlsServerConfig`s (raw key + X.509/ACME); the endpoint takes neither —
it takes the already-built transport endpoints. The TCP+TLS listener
is owned by the endpoint via `with_tcp_tls`; the endpoint runs its
accept loop inside `run()` and stops it on `shutdown()`. The ACME
handle lives on the `TlsServerConfig`, not the endpoint.

This resolves the single-`Arc<TlsServerConfig>` problem: the endpoint
has no "the TLS config" to take because a hub has two. It also means
shutdown is single-owner — the endpoint owns all its accept loops
(quinn, iroh, TCP+TLS); one `shutdown()` stops them all. See
[`crates/endpoint/README.md`](../endpoint/README.md) and
[ADR-083](../../decisions/083-endpoint-as-accept-loop-runner.md).

### The TCP+TLS accept loop (out of scope for this crate)

`alknet-tls` provides `for_tcp_tls() -> TlsAcceptor`. The actual TCP
accept loop (`TcpListener::accept` → `TlsAcceptor::accept` →
`Connection::from_bidi` → `endpoint.dispatch()`) lives in
`alknet-endpoint` behind a `tcp` feature, as an owned transport on
`AlknetEndpoint` (via `with_tcp_tls(listener, acceptor)` — see ADR-083,
Amendment 2026-07-15). `alknet-tls` is the cert provider, not the
accept loop. This keeps `alknet-tls` focused on TLS setup and cert
sharing, not transport accept logic.

### Client-side — `TlsClientConfig` (ADR-087)

`alknet-tls` provides a client-side config alongside `TlsServerConfig`.
A hub dials out to workers it supervises and to other hubs
(hub-as-client); `alknet-worker` dials a hub. Both need a
`rustls::ClientConfig` with ADR-034's verifier selection and ADR-084's
crypto provider. `TlsClientConfig` centralizes this, and is consumed by
`AlknetClient`'s QUIC and TCP+TLS dials (ADR-089).

There are exactly two clients in the alknet client surface as far as
`TlsClientConfig` and `AlknetClient` are concerned — **call**
(`CallClient`) and **channels** (`ChannelClient`, which is a proxy over
many ALPNs via channel 0). Both share `TlsClientConfig` via the dial;
the TLS config is shared across them, the dial is per-transport
per-client.

```rust
pub struct TlsClientConfig {
    rustls_config: rustls::ClientConfig,
}

impl TlsClientConfig {
    /// Build a client TLS config from `ConnectionCredentials` and the
    /// dial's ALPN. `ConnectionCredentials` (ADR-091, in `alknet-core`)
    /// carries the two dimensions the dial consumes:
    ///
    /// 1. `local_identity` — the local node's `TlsIdentity` (RFC 7250
    ///    raw key or X.509), presented as the client cert. `None` →
    ///    no client cert (the server gets nothing to fingerprint).
    ///    `SelfSigned` → no client cert (dev-only). `Acme` →
    ///    `TlsError::AcmeConfig` (server-only identity).
    ///
    /// 2. `remote_identity` — the inputs to ADR-034's server-cert
    ///    verifier selection:
    ///    - `Some(fingerprint)` (known peer, `PeerEntry` present) →
    ///      fingerprint pin (`FingerprintPinVerifier`)
    ///    - `None` + X.509 transport → CA verification
    ///      (`WebPkiServerVerifier`)
    ///    - `None` + raw key → fail closed at handshake (not a `new`-
    ///      time error; see ADR-088 §6)
    ///
    /// Applies ADR-084 crypto provider (aws_lc_rs::default_provider()).
    pub fn new(
        credentials: &ConnectionCredentials,
        alpn: &[u8],
    ) -> Result<Self, TlsError>;

    /// Consume the config and produce a `quinn::ClientConfig` for a
    /// QUIC dial. Returns `Result` because
    /// `QuicClientConfig::try_from(rustls::ClientConfig)` can fail with
    /// `NoInitialCipherSuite` — the same failure the server-side
    /// `for_quinn()` surfaces as `TlsError::QuinnWrap`. Feature-gated
    /// on `quinn`.
    #[cfg(feature = "quinn")]
    pub fn for_quinn(self) -> Result<quinn::ClientConfig, TlsError>;

    /// Consume the config and return the inner `rustls::ClientConfig`,
    /// for consumers that build their own transport-specific wrapper —
    /// e.g. `dial_tcp_tls` wraps it in a
    /// `tokio_rustls::TlsConnector::from(Arc::new(rustls_config))`. Not
    /// feature-gated; the raw rustls config is transport-agnostic.
    pub fn into_rustls_config(self) -> rustls::ClientConfig;
}
```

`TlsClientConfig::new` runs ADR-034's verifier selection directly off
`ConnectionCredentials.remote_identity` — there is no separate
`ClientVerifierContext` type; the credential bundle carries the
fingerprint (or its absence), which is all the verifier selection needs.
The call-protocol `auth_token` is not in `ConnectionCredentials` — it
is a per-request field on `call.requested` payloads (a call-protocol /
hub concept), not a transport credential; it never reaches
`TlsClientConfig`. The `TlsError` variant granularity (covering both
server and client errors) is decided — see
[ADR-088](../../decisions/088-tlserror-shape.md) and the
[`TlsError`](#tlserror) section below.

**Root store fallback (ADR-088 §5).** The unknown-X.509-remote
CA-verification path loads the platform's native root certs
(`rustls-native-certs`). If the platform store is empty (e.g. a
containerized deployment with no system CA bundle), the built-in
`webpki-roots` are merged in so the store is never empty. This makes
the `NoRootAnchors` failure mode unreachable in practice — a
containerized worker dialing a public X.509 hub succeeds without
requiring the operator to mount a CA bundle. Native-certs *load* errors
are logged, not returned; the fallback guarantees the store is
non-empty regardless. See ADR-088 §5.

`TlsClientConfig` produces a `rustls::ClientConfig`; the caller (the
transport-specific dial helper — `AlknetClient::dial_quic` /
`dial_tcp_tls`, ADR-089) passes it to the transport's connector. The
config is transport-agnostic; the dial is not. This is the client-side
analogue of ADR-065's server-side separation: the take-over
(`spawn_dispatch` / `from_connection`, transport-agnostic) is
transport-agnostic; the dial (transport-specific) is per-transport.
The transport-polymorphic dial is `alknet-client` (ADR-089, resolves
OQ-55) — `AlknetClient` builds the `TlsClientConfig` per-dial and
calls the transport's connector.

The client-side accessor API: `for_quinn()` (QUIC) and
`into_rustls_config()` (any other transport — `dial_tcp_tls` wraps the
rustls config in a `TlsConnector`). Iroh is the exception (see below).
`AlknetClient` (ADR-089) consumes `TlsClientConfig` via these accessors
for the QUIC and TCP+TLS dials; the iroh dial is the key-not-config
exception.

### Iroh — shares the key, not the config (client side too)

Iroh's client side, like its server side (above), does not consume a
`rustls::ClientConfig` — it takes an `iroh::SecretKey` and handles TLS
internally. The iroh client dial does not use `TlsClientConfig`. The
verifier selection for iroh is fingerprint-pinning by another name:
iroh's built-in TLS verifies the remote's `NodeId` (Ed25519 public key)
against the expected `NodeId`. An unknown iroh remote fails closed
(ADR-034 §3, Assumption 1 — no CA to fall back to). The iroh dial
helper applies the same ADR-034 rule via iroh's own API; the
consistency is in the rule, not in the type.

### `TlsError`

The error type for `TlsServerConfig::new`, `TlsClientConfig::new`, and
the `for_quinn()` accessors on both (`for_tcp_tls` is infallible —
`TlsAcceptor::new` / `TlsConnector::new` cannot fail). A single
`#[non_exhaustive]` enum with one variant per failure category, owned by
`alknet-tls`. The shape, the rationale for single-enum-over-thin-wrapper,
and the "what is NOT a variant" list are in
[ADR-088](../../decisions/088-tlserror-shape.md); this section is
the target shape per that ADR.

> **Implementation note.** The current `alknet-tls/src/lib.rs`
> `TlsError` is a simplified 3-variant enum (`Config(String)`,
> `Io(io::Error)`, `Cert(String)`) without `#[non_exhaustive]`. The
> full ADR-088 shape below (six typed variants, `#[non_exhaustive]`,
> `#[from` sources) is the target; the present code folds the
> finer-grained categories into `Config`/`Cert` strings. An implementer
> refining `TlsError` to match ADR-088 is a two-way-door change (the
> enum is crate-local, no external match arms).

```rust
/// Errors produced by `TlsServerConfig::new`, `TlsClientConfig::new`,
/// and the transport accessors (`for_quinn`; `for_tcp_tls` is
/// infallible). One variant per failure category — match on the variant
/// for the category, inspect the `#[source]` for the detail.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TlsError {
    /// Cert or key file read / PEM parse. `io::Error` is the type the
    /// pemfile BufRead APIs return (pemfile funnels its own non-`Error`
    /// type into `io::Error` — see ADR-088 §"Gotchas" #2).
    #[error("loading cert/key material: {0}")]
    CertLoad(#[from] std::io::Error),

    /// Self-signed cert generation (rcgen). Server `SelfSigned` path.
    #[error("generating self-signed cert: {0}")]
    SelfSigned(#[from] rcgen::Error),

    /// rustls server or client config construction
    /// (`with_safe_default_protocol_versions`, `with_single_cert`,
    /// `CertifiedKey::from_der`, `RootCertStore::add`). Both paths.
    #[error("building rustls config: {0}")]
    Rustls(#[from] rustls::Error),

    /// `WebPkiServerVerifier::build()` — the unknown-X.509-remote
    /// client path (empty root store, invalid CRL). Distinct type and
    /// remediation from `Rustls`.
    #[error("building webpki verifier: {0}")]
    VerifierBuild(#[from] rustls::webpki::VerifierBuilderError),

    /// `QuicServerConfig::try_from(rustls::ServerConfig)` — the one
    /// path where `for_quinn()` fails. Distinct type
    /// (`NoInitialCipherSuite`, not `rustls::Error`); quinn-gated.
    #[cfg(feature = "quinn")]
    #[error("wrapping rustls config for quinn: {0}")]
    QuinnWrap(#[from] quinn::crypto::rustls::NoInitialCipherSuite),

    /// ACME config mismatch: "feature not enabled but `Acme`
    /// configured" (server), or "ACME identity is server-only; cannot
    /// be used for client auth" (client). A config error, not a
    /// wrapped third-party error.
    #[error("ACME configuration error: {0}")]
    AcmeConfig(String),
}
```

**Scope boundary (ADR-088 §6).** `TlsError` is the
**config-construction** error type — what `new` and `for_quinn` can
fail on. Handshake-time errors (the unknown-raw-key fail-closed; a
`rustls::Error::InvalidCertificate` from a rejected cert) are
**handshake outcomes**, not config-construction errors — they flow
through the transport's connector (`quinn::Endpoint::connect_with`,
`TlsConnector::connect`), not through `TlsError`. ACME state-machine
errors (`EventError`, `OrderError`, `CertParseError`) are stream events,
logged in the spawned task, not `TlsError` variants — `new` spawns the
state machine and returns immediately; the state machine's failures
arrive asynchronously and are logged (ADR-082 §"Behavior-preservation
invariants").

**Ownership.** `TlsError` lives in `alknet-tls`, owned by the crate
that produces it. It is not re-exported from `alknet-core`; core has
no endpoint error type, so core does not need to know about
`TlsError`. The assembly layer (hub/worker) depends on `alknet-tls`
directly and gets `TlsError` from that dependency.

## Crate dependencies (in the dep graph)

```
alknet-tls
└── alknet-core (TlsIdentity, Ed25519SecretKey, fingerprint)

alknet-core (lightweight — types + auth + config + fingerprint + credentials)
└── (rustls / rustls-pki-types — only for fingerprint.rs types)

alknet-call (pure protocol crate — no TLS/transport deps per ADR-089 §5)
└── alknet-core (ProtocolHandler, Connection, types; ConnectionCredentials/
    RemoteIdentity from core per ADR-091)

alknet-hub (multi-transport endpoint)
├── alknet-tls (TlsServerConfig — shared across quinn + TCP)
├── alknet-endpoint (AlknetEndpoint with quinn + iroh + tcp features, HandlerRegistry)
├── alknet-client (AlknetClient — outbound worker dials, ADR-089)
├── alknet-channels-call (ChannelClient)
├── alknet-call (CallAdapter, Dispatcher)
├── alknet-http (HttpAdapter)
└── alknet-core (Connection, ProtocolHandler, AuthContext, IdentityProvider)
```

`alknet-tls` depends on `alknet-core` only. No handler crate depends on
`alknet-tls` — they depend on `alknet-core` for types and on
`alknet-tls` only indirectly through the assembly layer. The assembly
layer (the deployment binary) builds the `TlsServerConfig`(s), builds
the transport endpoints (quinn/iroh/TCP+TLS), and hands them to
`AlknetEndpoint` via `with_quinn` / `with_iroh` / `with_tcp_tls`
(ADR-083).

## Design Decisions

All design decisions are documented as ADRs in
[decisions/](../../decisions/).

| ADR | Decision | Summary |
|-----|----------|---------|
| [082](../../decisions/082-alknet-tls-extraction.md) | alknet-tls crate extraction | Extract TLS setup from alknet-core/endpoint.rs; `TlsServerConfig` shareable across quinn + TCP+TLS + iroh; one ACME state machine |
| [083](../../decisions/083-endpoint-as-accept-loop-runner.md) | Endpoint as multi-transport accept-loop runner | `AlknetEndpoint` takes no TLS config; TCP+TLS is an owned transport (`with_tcp_tls`); `dispatch` public for SSH/WT; `acme-tls/1` guard is in shared `dispatch` |
| [084](../../decisions/084-aws-lc-rs-crypto-provider.md) | aws-lc-rs crypto provider | `rustls::crypto::aws_lc_rs::default_provider()` on all server + client config paths; matches iroh; FIPS-capable; do not switch to `ring` or process-default without a new ADR |
| [086](../../decisions/086-endpoint-types-and-entry-points.md) | Endpoint types and entry points | Three endpoint types (web/native/iroh); split ALPN lists per endpoint type (resolves OQ-62); entry-point vs. endpoint ALPN distinction |
| [087](../../decisions/087-tlsclientconfig-not-blocked-on-dial.md) | `TlsClientConfig` not blocked on dial seam | `alknet-tls` provides `TlsClientConfig` (client-side); not deferred behind OQ-55; breaks the circular hedge; hub-as-client is a first-class use case |
| [088](../../decisions/088-tlserror-shape.md) | `TlsError` shape — single enum, owned by `alknet-tls` | Single `#[non_exhaustive]` enum, one variant per failure category (`CertLoad`, `SelfSigned`, `Rustls`, `VerifierBuild`, `QuinnWrap`, `AcmeConfig`); not a thin wrapper (the `for_quinn` failure is `NoInitialCipherSuite`, not `rustls::Error`); owned by `alknet-tls`, not re-exported from core |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-59** (resolved): `fingerprint.rs` stays in `alknet-core`. The
  client-side `FingerprintPinVerifier` (now in `alknet-tls` per
  ADR-089 §5 — the original `alknet-call` → `alknet-tls` dep-edge
  concern that motivated keeping `fingerprint.rs` in core is
  dissolved). `fingerprint.rs` stays in core because `alknet-core`'s
  own `Identity`/fingerprint code uses it; `alknet-tls` re-exports.
  The `rustls` dep in core is narrow — production fingerprint code uses
  only `sha2` + manual DER; the `rustls::sign` usage is a test helper.
- **OQ-60** (resolved): Where does transport construction live? The
  TCP+TLS accept loop lives in `alknet-endpoint` behind a `tcp` feature
  as an owned endpoint transport (`with_tcp_tls`). Builder functions are
  inlined by the assembly layer. See ADR-083.
- **OQ-61** (dissolved): Multi-owner shutdown coordination. The
  problem does not arise — the endpoint owns all its accept loops
  (quinn, iroh, TCP+TLS); `shutdown()` stops them all. See ADR-083.
- **OQ-62** (resolved): Does a hub pass the same ALPN list to both
  `TlsServerConfig`s? **Split list, by endpoint type** (ADR-086 §3).
  Each config advertises only the ALPNs its endpoint type's client
  class can negotiate — native ALPNs on the raw-key config, entry-point
  ALPNs + `alknet/channels` on the X.509/ACME config, native ALPNs on
  the iroh builder. The assembly layer filters
  `registry.alpn_strings()` per config.
- **OQ-63** (resolved): `TlsError` shape — a single
  `#[non_exhaustive]` enum with one variant per failure category, owned
  by `alknet-tls` (not re-exported from core). Six variants: `CertLoad`,
  `SelfSigned`, `Rustls`, `VerifierBuild`, `QuinnWrap` (quinn-gated),
  `AcmeConfig`. The decision is grounded in the actual error-producing
  call sites and the dependency-crate sources; three findings drove
  the single-enum choice over a thin wrapper (the `for_quinn()` failure
  is `NoInitialCipherSuite`, not `rustls::Error`;
  `rustls_pemfile::Error` is not a `std::error::Error`;
  `WebPkiServerVerifier::build()` returns `VerifierBuilderError`). See
  [ADR-088](../../decisions/088-tlserror-shape.md) for the full rationale
  and the "what is NOT a variant" list. The `TlsError` sketch is in the
  [TlsError](#tlserror) section below.
- **OQ-64** (resolved): `alknet-tls` provides `TlsClientConfig`
  (ADR-087). Not blocked on the dial-seam extraction — the TLS
  config is a prerequisite for the dial, not a consequence of it.
  Centralizes ADR-034 verifier selection + ADR-084 provider. The dial
  seam is `alknet-client` (ADR-089, OQ-55 resolved); `TlsClientConfig`
  is consumed by `AlknetClient`'s QUIC and TCP+TLS dials.

- **OQ-55** (resolved by ADR-089): `AlknetClient::dial()` — the
  transport-polymorphic dial seam. `alknet-client` has three dial
  methods (`dial_quic` / `dial_tcp_tls` / `dial_iroh`).
  `TlsClientConfig` (OQ-64, resolved) is the prerequisite the dial
  consumes. See [`crates/client/README.md`](../client/README.md) and
  [ADR-089](../../decisions/089-alknetclient-native-dial-seam.md).

### Client shape (in `alknet-client`)

[`crates/client/README.md`](../client/README.md) defines `AlknetClient`
— the native client dial seam (ADR-089, resolves OQ-55). There are
exactly two clients in the alknet client surface as far as
`TlsClientConfig` and `AlknetClient` are concerned: **call**
(`CallClient`) and **channels** (`ChannelClient`, a proxy over many
ALPNs via channel 0). Both consume `TlsClientConfig` through the dial
(`for_quinn` for QUIC, `into_rustls_config` wrapped in a `TlsConnector`
for TCP+TLS); iroh is the exception (shares the key, not the config).
`AlknetClient` is the dial that feeds them — it produces a `Connection`
and the protocol take-overs (`spawn_dispatch`, `from_connection`)
consume it. The dial is centralized in `AlknetClient` (ADR-089); the
protocol crates have no TLS/transport deps. The `alknet/register` ALPN
(named by ADR-089; wire protocol deferred, OQ-66) is the native
registration entry point, parallel to HTTP registration in OQ-58.

## References

- `docs/architecture/decisions/027-tls-identity-redesign-acme-rawkey-decoupling.md`
  — `TlsIdentity` (RawKey / X509 / Acme), RFC 7250, browser limitation
- `docs/architecture/decisions/030-peerentry-and-identity-id-decoupling.md` §6
  — fingerprint normalization (`ed25519:<hex>` across quinn/iroh)
- `docs/architecture/decisions/034-outgoing-only-x509-and-three-peer-roles.md`
  — client-side verifier selection (CA vs fingerprint pin)
- `docs/architecture/decisions/065-connection-from-stream-generic-single-stream.md`
  — `Connection::from_stream`/`from_bidi` (TCP+TLS path)
- `docs/architecture/decisions/010-alpn-router-and-endpoint.md`
  Amendment 2 — TCP+TLS is a first-class owned transport
  (`with_tcp_tls`); supersedes Amendment 1's sibling-loop framing
- `docs/architecture/decisions/086-endpoint-types-and-entry-points.md`
  — three endpoint types (web/native/iroh); split ALPN lists per
  endpoint type (resolves OQ-62); entry-point vs. endpoint distinction
- `docs/architecture/decisions/087-tlsclientconfig-not-blocked-on-dial.md`
  — `TlsClientConfig` (client-side); not blocked on the dial seam;
  breaks the circular hedge; hub-as-client requirement
- `docs/architecture/crates/endpoint/README.md` — `AlknetEndpoint`
  (the endpoint spec; TLS config is built by `alknet-tls`, not the
  endpoint — per ADR-083)
- `docs/architecture/crates/core/config.md` — `TlsIdentity`, `StaticConfig`
- `crates/alknet-tls/src/server.rs` — `TlsServerConfig`,
  `RawKeyCertResolver`, `AcceptAnyCertVerifier`,
  `generate_self_signed_cert`, `build_rustls_server_config`
- `crates/alknet-tls/src/client.rs` — `TlsClientConfig`,
  `FingerprintPinVerifier`, `RawKeyClientCertResolver`,
  `NoClientCertResolver`, `select_server_verifier`, `build_client_auth`,
  `load_platform_root_cert_store`
- `crates/alknet-tls/src/pem.rs` — `load_cert_chain`, `load_private_key`
- `crates/alknet-tls/src/signing.rs` — `Ed25519SigningKey`
- `crates/alknet-core/src/fingerprint.rs` — fingerprint extraction
  (shared by server endpoint and client verifier)