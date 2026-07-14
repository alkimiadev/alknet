---
status: draft
last_updated: 2026-07-14
---

# alknet-tls

Shared TLS configuration and certificate management. Builds a
`rustls::ServerConfig` (or an ACME state machine + cert resolver) once,
and shares it across multiple transports — quinn, `tokio-rustls`
(TCP+TLS), and iroh — so one certificate identity serves QUIC and TCP
endpoints simultaneously. One ACME state machine, one cert, N
transports.

## What

`alknet-tls` extracts the TLS setup that was welded to the quinn endpoint
in `alknet-core`. The existing code (`endpoint.rs`) builds a
`rustls::ServerConfig` from a `TlsIdentity`, then **consumes** it into a
`quinn::ServerConfig` — making it impossible to reuse the same cert for a
TCP+TLS listener. ACME is worse: the `AcmeState` task is spawned inside
the quinn endpoint, so a TCP+TLS listener would need its own ACME state
machine (two orders for the same domain, two cert caches, potential
Let's Encrypt rate-limiting).

`alknet-tls` fixes this by making the TLS config **shareable**:

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

`alknet-core` builds the `rustls::ServerConfig` once, then consumes it
into a `quinn::ServerConfig` — making the cert unreusable for a TCP+TLS
listener. For ACME the problem is worse: the `AcmeState` task is spawned
inside the quinn endpoint, so a TCP+TLS listener would need a second ACME
state machine for the same domain (duplicate orders, divergent cert
caches, Let's Encrypt rate-limit risk). The full rationale, including
the cert-reuse problem, the ACME worst case, and the three reasons a
separate crate is the right shape (dependency isolation, ACME weight,
quinn/iroh having their own TLS), is in
[ADR-082](../../decisions/082-alknet-tls-extraction.md).

### The three use cases

| Use case | Identity | Transports | Browsers? |
|----------|----------|-----------|-----------|
| P2P / native clients | RFC 7250 raw key (Ed25519) | QUIC + TCP (fallback when UDP blocked) | No (browsers can't do raw keys) |
| Domain-hosted / public service | X.509 (manual or ACME) | QUIC + TCP+TLS (same cert) | Yes (via HTTPS / WebSocket; WebTransport when revived) |
| Development | Self-signed | Any | No (untrusted) |

In all cases, TLS + ALPNs "just works" — the TLS handshake negotiates the
ALPN, the `HandlerRegistry` dispatches by ALPN, the transport is a
parameter. The TLS crate's job is to make the cert available to whichever
transports the deployment runs.

## Architecture

### What moves from `alknet-core` to `alknet-tls`

| Component | Current location | New location |
|-----------|-----------------|-------------|
| `TlsIdentity` enum | `alknet-core/config.rs` | **stays in core** (it's a config type) |
| `Ed25519SecretKey` | `alknet-core/config.rs` | **stays in core** (config type) |
| `build_rustls_server_config()` | `alknet-core/endpoint.rs` (`#[cfg(feature = "quinn")]`) | `alknet-tls` (unconditional) |
| `build_quinn_server_config_from_rustls()` | `alknet-core/endpoint.rs` (`#[cfg(feature = "quinn")]`) | `alknet-tls` (`for_quinn()` — wraps rustls config in `QuicServerConfig`) |
| `TlsSetup` (ACME state machine) | `alknet-core/endpoint.rs` (`#[cfg(feature = "quinn")]`) | `alknet-tls` (the `TlsServerConfig::new` ACME path) |
| `RawKeyCertResolver` | `alknet-core/endpoint.rs` (`#[cfg(feature = "quinn")]`) | `alknet-tls` |
| `Ed25519SigningKey` | `alknet-core/endpoint.rs` (`#[cfg(feature = "quinn")]`) | `alknet-tls` |
| `AcceptAnyCertVerifier` | `alknet-core/endpoint.rs` (`#[cfg(feature = "quinn")]`) | `alknet-tls` |
| `SelfSignedCert` / `generate_self_signed_cert()` | `alknet-core/endpoint.rs` (`#[cfg(feature = "quinn")]`) | `alknet-tls` |
| `load_cert_chain()` / `load_private_key()` | `alknet-core/endpoint.rs` | `alknet-tls` |
| `fingerprint.rs` | `alknet-core/fingerprint.rs` | **stays in core** (shared by server + client; client is in `alknet-call`; production code uses `sha2` + manual DER only — `rustls` is test-only. See OQ-59.) |

`TlsIdentity` and `Ed25519SecretKey` stay in core because they're config
types — `StaticConfig` holds a `TlsIdentity`, and config types belong in
core. `alknet-tls` re-exports them for convenience. `fingerprint.rs` stays
in core because it's shared by both the server path (endpoint extracts
fingerprint from the client cert) and the client path (`FingerprintPinVerifier`
in `alknet-call` matches the server's cert against a pinned fingerprint).
The production code in `fingerprint.rs` uses only `sha2` and manual DER
parsing — the `rustls::sign` usage is in the test helper only. See OQ-59
for the full trade-off.

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

The ALPN list is the set of ALPNs the deployment wants to advertise
(`alknet/call`, `alknet/channels`, `h2`, `http/1.1`, etc.). For ACME, the
`acme-tls/1` ALPN is appended automatically (for the TLS-ALPN-01
challenge, ADR-027 §7).

### Behavior-preservation invariants

The extraction must preserve these load-bearing TLS behaviors. They
originate from [ADR-027](../../decisions/027-tls-identity-redesign-acme-rawkey-decoupling.md),
which established the `TlsIdentity` model, the `Acme` variant, and the
`acme-tls/1` ALPN challenge handling. An implementer who omits any of
these produces a crate that compiles and passes type-checks but silently
changes TLS behavior:

- **`max_early_data_size = u32::MAX`** on all server config paths (X509,
  RawKey, SelfSigned, ACME). Enables 0-RTT / early data. Omitting it
  disables 0-RTT, silently breaking clients that use it.
- **`rustls::crypto::aws_lc_rs::default_provider()`** as the crypto
  provider on all paths. Matches iroh's `tls-aws-lc-rs` feature. Do not
  switch to `ring` or the process-default provider without an ADR —
  different FIPS status, different platform support.
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
    /// build their own transport-specific wrapper (e.g. iroh, which
    /// has its own TLS built in but shares the Ed25519 key).
    pub fn rustls_config(&self) -> &rustls::ServerConfig;
}
```

### Iroh: shares the key, not the rustls config

Iroh is different from quinn and TCP+TLS: it has its own TLS built into
the `Endpoint`, using RFC 7250 raw keys. It does not consume a
`rustls::ServerConfig` — it takes an `iroh::SecretKey` and handles TLS
internally. So `alknet-tls` does not have a `for_iroh()` method. Instead,
the assembly layer reads the `Ed25519SecretKey` from `StaticConfig`
(stays in core) and passes it to iroh's `Endpoint::builder().secret_key()`
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
├── rustls            (ServerConfig, cert types — always present)
├── rustls-pki-types  (CertificateDer, PrivateKeyDer, etc. — via rustls re-export
│                     or direct dep; core lists it directly)
├── rustls-pemfile    (cert/key file loading — always present)
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

`alknet-core` loses `rustls-pemfile`, `rcgen`, and `rustls-acme` from
its dependencies — the cert-loading, self-signed generation, and ACME
machinery move to `alknet-tls`. Core keeps `quinn` and `iroh` (the
endpoint struct and accept loops remain in core), `ed25519-dalek`
(`Ed25519SecretKey` stays in `config.rs`), and `rustls` /
`rustls-pki-types` (`fingerprint.rs` uses `rustls::pki_types` in
production and `rustls::sign` in the test helper `build_ed25519_spki_der`
— see OQ-59).

### What `AlknetEndpoint` does after the refactor

`AlknetEndpoint::new()` currently builds `TlsSetup` internally. After
the refactor (see [ADR-083](../../decisions/083-endpoint-as-accept-loop-runner.md)),
the endpoint takes **no TLS config at all** — it is a multi-transport
accept-loop runner. TCP+TLS is an owned transport (via `with_tcp_tls`),
not an external loop:

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
    pub async fn shutdown(&self) -> Result<(), EndpointError>;
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
(quinn, iroh, TCP+TLS); one `shutdown()` stops them all.

### The TCP+TLS accept loop (out of scope for this crate)

`alknet-tls` provides `for_tcp_tls() -> TlsAcceptor`. The actual TCP
accept loop (`TcpListener::accept` → `TlsAcceptor::accept` →
`Connection::from_bidi` → `endpoint.dispatch()`) lives in `alknet-core`
behind a `tcp` feature, as an owned transport on `AlknetEndpoint` (via
`with_tcp_tls(listener, acceptor)` — see ADR-083). `alknet-tls` is the
cert provider, not the accept loop. This keeps `alknet-tls` focused on
TLS setup and cert sharing, not transport accept logic.

## Crate dependencies (in the dep graph)

```
alknet-tls
├── alknet-core (TlsIdentity, Ed25519SecretKey, fingerprint)

alknet-core (loses TLS setup code)
├── (rustls — only for fingerprint.rs types, if kept)

alknet-call (client-side verifier — unchanged)
├── alknet-core (fingerprint.rs)

alknet-hub (multi-transport endpoint)
├── alknet-tls (TlsServerConfig — shared across quinn + TCP)
├── alknet-channels-call (ChannelClient)
├── alknet-call (CallAdapter, Dispatcher)
├── alknet-http (HttpAdapter)
├── alknet-core (AlknetEndpoint with quinn + iroh + tcp features, HandlerRegistry, Connection)
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
| [083](../../decisions/083-endpoint-as-accept-loop-runner.md) | Endpoint as multi-transport accept-loop runner | `AlknetEndpoint` takes no TLS config; TCP+TLS is an owned transport (`with_tcp_tls`); `dispatch` public for SSH/WT; `acme-tls/1` guard moves to shared `dispatch` |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-59** (open): Should `fingerprint.rs` stay in `alknet-core` or move
  to `alknet-tls`? It uses `rustls::pki_types` and `rustls::sign` types,
  which creates a `rustls` dep in core. If it moves to `alknet-tls`, the
  client-side `FingerprintPinVerifier` (in `alknet-call`) would depend
  on `alknet-tls` — a new dep edge. If it stays, core keeps a narrow
  `rustls` dep. Decision-ready — the answer depends on whether we want
  core to be `rustls`-free.
- **OQ-60** (resolved): Where does transport construction live? The
  TCP+TLS accept loop lives in `alknet-core` behind a `tcp` feature as
  an owned endpoint transport (`with_tcp_tls`). Builder functions are
  inlined by the assembly layer. See ADR-083.
- **OQ-61** (dissolved): Multi-owner shutdown coordination. The
  problem does not arise — the endpoint owns all its accept loops
  (quinn, iroh, TCP+TLS); `shutdown()` stops them all. See ADR-083.

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
- `docs/architecture/crates/core/endpoint.md` — current endpoint design
  (TLS section will be amended to point to `alknet-tls`)
- `docs/architecture/crates/core/config.md` — `TlsIdentity`, `StaticConfig`
- `crates/alknet-core/src/endpoint.rs` — the code being extracted
  (`build_rustls_server_config`, `TlsSetup`, `RawKeyCertResolver`,
  `Ed25519SigningKey`, `AcceptAnyCertVerifier`, `generate_self_signed_cert`,
  `load_cert_chain`, `load_private_key`)
- `crates/alknet-core/src/fingerprint.rs` — fingerprint extraction
  (shared by server endpoint and client verifier)