---
status: draft
last_updated: 2026-07-13
---

# alknet-tls

Shared TLS configuration and certificate management. Builds a
`rustls::ServerConfig` (or an ACME state machine + cert resolver) once,
and hands clones to multiple transports — quinn, `tokio-rustls` (TCP+TLS),
and iroh — so one certificate identity serves QUIC and TCP endpoints
simultaneously. One ACME state machine, one cert, N transports.

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
    config: rustls::ServerConfig,      // Clone — Arc internally
    acme_handle: Option<JoinHandle<()>>,  // one ACME task, shared
}

impl TlsServerConfig {
    pub async fn new(identity: &TlsIdentity, alpns: &[Vec<u8>]) -> Result<Self, TlsError>;

    /// Clone for quinn. Feature-gated on `quinn`.
    #[cfg(feature = "quinn")]
    pub fn for_quinn(&self) -> Result<quinn::ServerConfig, TlsError>;

    /// Clone for tokio-rustls (TCP+TLS). Feature-gated on `tcp`.
    #[cfg(feature = "tcp")]
    pub fn for_tcp_tls(&self) -> tokio_rustls::TlsAcceptor;

    /// The underlying rustls config, for any other consumer.
    pub fn rustls_config(&self) -> &rustls::ServerConfig;
}
```

The key property: `rustls::ServerConfig` is `Clone` (it holds `Arc`s to
the cert resolver and verifier, not the raw key material). So one
`TlsServerConfig` can feed quinn and TCP+TLS simultaneously — one cert,
one ACME state machine, two transports.

## Why

### The cert-reuse problem is concrete

A hub that serves HTTP (TCP+TLS on 443) and channels (QUIC on 4433) with
the same X.509 cert cannot do it with the current code. The
`rustls::ServerConfig` is moved into `quinn::ServerConfig` and consumed.
A TCP+TLS listener would have to build its own `rustls::ServerConfig`
from the same `TlsIdentity` — re-loading the cert file, or re-deriving the
raw key cert, or running a second ACME state machine for the same
domains.

### ACME is the worst case

Two ACME state machines for the same domain:
- Two cert-order attempts (race condition on Let's Encrypt's rate
  limiter).
- Two cert caches (divergent cache dirs or a shared dir with no
  coordination).
- Two `AcmeState` tasks spawning duplicate `resolver()` instances.

One `TlsServerConfig` with one `AcmeState` task solves this: the ACME
state machine runs once, the `resolver()` is `Arc`-shared across
transports, and both quinn and TCP+TLS get the same cert from the same
order.

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

### Why a separate crate

1. **Dependency isolation.** `tokio-rustls` is a real dependency that
   shouldn't be forced on every `alknet-core` consumer. A node that only
   uses quinn doesn't need `tokio-rustls`. A node that uses TCP+TLS
   needs it. Feature-gating in core mixes concerns — core is types and
   traits, not transport-specific TLS setup.

2. **ACME is heavy.** `rustls-acme` spawns a long-running async task,
   manages a cert cache, talks to Let's Encrypt. That's transport infra,
   not core types. It belongs in a crate the assembly layer pulls in when
   it needs real TLS setup, not in core.

3. **Quinn and iroh have their own TLS.** Quinn wraps
   `rustls::ServerConfig` into `quinn::ServerConfig`. Iroh uses its own
   raw-key TLS built into the `Endpoint`. `alknet-tls` is the "I have a
   cert and I want to share it across transports" layer — it doesn't
   replace quinn's or iroh's TLS, it provides the shared cert source.

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

The extraction must preserve these load-bearing TLS behaviors from the
current code. An implementer who omits any of these produces a crate
that compiles and passes type-checks but silently changes TLS behavior:

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
- **`acme-tls/1` ALPN append** for the ACME path only. The TLS-ALPN-01
  challenge requires the server to advertise `acme-tls/1` in its ALPN
  list. Appended in `TlsServerConfig::new`'s ACME branch, not by the
  caller.

Transport-specific accessors:

```rust
impl TlsServerConfig {
    /// Produce a `quinn::ServerConfig` for a QUIC listener. Clones the
    /// rustls config (cheap — Arc-shared cert resolver), wraps it in
    /// `QuicServerConfig`. Feature-gated on `quinn`.
    #[cfg(feature = "quinn")]
    pub fn for_quinn(&self) -> Result<quinn::ServerConfig, TlsError>;

    /// Produce a `tokio_rustls::TlsAcceptor` for a TCP+TLS listener.
    /// Clones the rustls config. Feature-gated on `tcp` (pulls
    /// `tokio-rustls`).
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
├── futures           (StreamExt for ACME event loop — present; only used
│                     in the ACME path, can be acme-gated or always present)
├── tracing           (logging)
├── quinn             (optional — for_quinn())
├── tokio-rustls      (optional — for_tcp_tls())
└── rustls-acme       (optional — ACME state machine)
```

`alknet-core` loses the `quinn`, `iroh`, `rustls-pemfile`, `rcgen`,
`ed25519-dalek`, and `rustls-acme` deps from its `[features]`
section — they move to `alknet-tls`. Core keeps a narrow `rustls` /
`rustls-pki-types` dep only if `fingerprint.rs` stays (OQ-59): the
production fingerprint code uses `sha2` + manual DER only, but the test
helper `build_ed25519_spki_der` uses `rustls::sign::public_key_to_spki`.
If `fingerprint.rs` moves to `alknet-tls`, core becomes `rustls`-free.

### What `AlknetEndpoint` does after the refactor

`AlknetEndpoint::new()` currently builds `TlsSetup` internally. After
the refactor, it takes an `Arc<TlsServerConfig>`:

```rust
impl AlknetEndpoint {
    pub async fn new(
        static_config: &StaticConfig,
        tls_config: Arc<TlsServerConfig>,  // ← new param
        handlers: HandlerRegistry,
        dynamic: Arc<ArcSwap<DynamicConfig>>,
        identity_provider: Arc<dyn IdentityProvider>,
    ) -> Result<Self, EndpointError>;
}
```

The endpoint calls `tls_config.for_quinn()` to get its quinn server
config. The ACME handle is on the `TlsServerConfig`, not the endpoint —
the endpoint doesn't own the ACME task. The assembly layer builds the
`TlsServerConfig` once, passes `Arc::clone()` to the endpoint, and
passes another `Arc::clone()` to the TCP+TLS accept loop (which lives in
the hub or a future `TcpTlsAcceptor`).

### The TCP+TLS accept loop (out of scope for this crate)

`alknet-tls` provides `for_tcp_tls() -> TlsAcceptor`. The actual TCP
accept loop (`TcpListener::accept` → `TlsAcceptor::accept` →
`Connection::from_bidi` → `HandlerRegistry::dispatch`) lives elsewhere —
in `alknet-hub` (the hub is the primary multi-transport consumer) or in
a future `alknet-core` `TcpTlsAcceptor` module. `alknet-tls` is the cert
provider, not the accept loop. This keeps `alknet-tls` focused on TLS
setup and cert sharing, not transport accept logic.

## Crate dependencies (in the dep graph)

```
alknet-tls
├── alknet-core (TlsIdentity, Ed25519SecretKey, fingerprint)

alknet-core (loses TLS setup code)
├── (rustls — only for fingerprint.rs types, if kept)

alknet-call (client-side verifier — unchanged)
├── alknet-core (fingerprint.rs)

alknet-http (future: TCP+TLS accept loop)
├── alknet-tls (TlsServerConfig::for_tcp_tls())
├── alknet-core (Connection::from_bidi, HandlerRegistry)

alknet-hub (multi-transport endpoint)
├── alknet-tls (TlsServerConfig — shared across quinn + TCP)
├── alknet-channels-call (ChannelClient)
├── alknet-call (CallAdapter, Dispatcher)
├── alknet-http (HttpAdapter)
├── alknet-core (AlknetEndpoint, HandlerRegistry, Connection)
```

`alknet-tls` depends on `alknet-core` only. No handler crate depends on
`alknet-tls` — they depend on `alknet-core` for types and on
`alknet-tls` only indirectly through the assembly layer. The assembly
layer (the deployment binary) builds the `TlsServerConfig` and passes it
to the endpoint and the TCP+TLS accept loop.

## Design Decisions

All design decisions are documented as ADRs in
[decisions/](../../decisions/).

| ADR | Decision | Summary |
|-----|----------|---------|
| [082](../../decisions/082-alknet-tls-extraction.md) | alknet-tls crate extraction | Extract TLS setup from alknet-core/endpoint.rs; `TlsServerConfig` shareable across quinn + TCP+TLS + iroh; one ACME state machine |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-59** (open): Should `fingerprint.rs` stay in `alknet-core` or move
  to `alknet-tls`? It uses `rustls::pki_types` and `rustls::sign` types,
  which creates a `rustls` dep in core. If it moves to `alknet-tls`, the
  client-side `FingerprintPinVerifier` (in `alknet-call`) would depend
  on `alknet-tls` — a new dep edge. If it stays, core keeps a narrow
  `rustls` dep. Decision-ready — the answer depends on whether we want
  core to be `rustls`-free.

## References

- `docs/architecture/decisions/027-tls-identity-redesign-acme-rawkey-decoupling.md`
  — `TlsIdentity` (RawKey / X509 / Acme), RFC 7250, browser limitation
- `docs/architecture/decisions/030-peerentry-and-identity-id-decoupling.md` §6
  — fingerprint normalization (`ed25519:<hex>` across quinn/iroh)
- `docs/architecture/decisions/034-outgoing-only-x509-and-three-peer-roles.md`
  — client-side verifier selection (CA vs fingerprint pin)
- `docs/architecture/decisions/065-connection-from-stream-generic-single-stream.md`
  — `Connection::from_stream`/`from_bidi` (TCP+TLS path)
- `docs/architecture/decisions/010-alpn-router-and-endpoint.md` Amendment 1
  — TCP+TLS dispatch via `from_stream` (accept loop outside the endpoint struct)
- `docs/architecture/crates/core/endpoint.md` — current endpoint design
  (TLS section will be amended to point to `alknet-tls`)
- `docs/architecture/crates/core/config.md` — `TlsIdentity`, `StaticConfig`
- `crates/alknet-core/src/endpoint.rs` — the code being extracted
  (`build_rustls_server_config`, `TlsSetup`, `RawKeyCertResolver`,
  `Ed25519SigningKey`, `AcceptAnyCertVerifier`, `generate_self_signed_cert`,
  `load_cert_chain`, `load_private_key`)
- `crates/alknet-core/src/fingerprint.rs` — fingerprint extraction
  (shared by server endpoint and client verifier)