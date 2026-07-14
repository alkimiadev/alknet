# ADR-082: alknet-tls Crate Extraction

## Status

Proposed (amended 2026-07-14: the `AlknetEndpoint::new` signature
referenced here was superseded by ADR-083 — the endpoint takes no TLS
config; the assembly layer builds transports from `TlsServerConfig`s)

## Context

The TLS setup in `alknet-core` is welded to quinn. The current flow:

1. `StaticConfig.tls_identity: Option<TlsIdentity>` carries the
   identity (X509 / RawKey / SelfSigned / Acme).
2. `build_rustls_server_config(tls_identity, alpns)` produces a
   `rustls::ServerConfig`. This function is transport-agnostic in
   principle — it returns a `rustls::ServerConfig`, which is what both
   quinn and `tokio-rustls` consume.
3. `build_quinn_server_config_from_rustls(rustls_config)` **consumes**
   the `rustls::ServerConfig` into a `quinn::ServerConfig`. The rustls
   config is moved — it cannot be reused for a TCP+TLS listener.
4. `TlsSetup` (which owns the ACME state machine handle) is
   `#[cfg(feature = "quinn")]` — it only exists when quinn is enabled.
   ACME is structurally a quinn-only path today.
5. The `AcmeState` task is spawned inside `TlsSetup::new_acme`, and its
   `JoinHandle` is stored on `AlknetEndpoint`. If you wanted to run ACME
   for a TCP+TLS listener, you'd have to duplicate the ACME setup or
   restructure.

### The cert-reuse problem

A hub that serves HTTP (TCP+TLS on 443) and channels (QUIC on 4433)
with the same X.509 cert cannot do it with the current code. The
`rustls::ServerConfig` is moved into `quinn::ServerConfig` and
consumed. A TCP+TLS listener would have to build its own
`rustls::ServerConfig` from the same `TlsIdentity` — re-loading the cert
file, or re-deriving the raw key cert, or running a second ACME state
machine for the same domains.

ACME is the worst case: two ACME state machines for the same domain means
two cert-order attempts (race condition on Let's Encrypt's rate limiter),
two cert caches, two `AcmeState` tasks spawning duplicate `resolver()`
instances. One ACME state machine with one resolver, shared across
transports, is the only correct design.

### The `rustls::ServerConfig` is already shareable

`rustls::ServerConfig` is `Clone` — it holds `Arc`s to the cert resolver
and verifier, not the raw key material. So the fix is structural, not
algorithmic: build the config once, clone it for each transport. The
existing `build_rustls_server_config` function already produces the
right type; the welding is in the layer above it (the `#[cfg(feature =
"quinn")]` gates, the `TlsSetup` struct, the consumption into
`quinn::ServerConfig`).

### The three use cases

| Use case | Identity | Transports | Browsers? |
|----------|----------|-----------|-----------|
| P2P / native clients | RFC 7250 raw key (Ed25519) | QUIC + TCP (fallback when UDP blocked) | No |
| Domain-hosted / public service | X.509 (manual or ACME) | QUIC + TCP+TLS (same cert) | Yes |
| Development | Self-signed | Any | No |

In all cases, TLS + ALPNs "just works" — the TLS handshake negotiates the
ALPN, the `HandlerRegistry` dispatches by ALPN. The TLS crate's job is
to make the cert available to whichever transports the deployment runs,
without duplicating the cert or the ACME state machine.

### Iroh is different

Iroh has its own TLS built into the `Endpoint`, using RFC 7250 raw keys.
It does not consume a `rustls::ServerConfig` — it takes an
`iroh::SecretKey` and handles TLS internally. So `alknet-tls` does not
have a `for_iroh()` method. The assembly layer reads the
`Ed25519SecretKey` from `StaticConfig` and passes it to iroh directly.
`alknet-tls` is involved only when iroh is not the sole transport — in
that case, the same `Ed25519SecretKey` feeds both `TlsServerConfig::new`
(for quinn/TCP) and `iroh::SecretKey::from_bytes` (for iroh).

## Decision

### Extract `alknet-tls` as a new crate

A new crate `alknet-tls` holds the TLS setup code extracted from
`alknet-core/endpoint.rs`. The central type is `TlsServerConfig`, built
once from a `TlsIdentity` + ALPN list, shared across transports via
`Arc<TlsServerConfig>`. `TlsServerConfig` is not `Clone` (it holds a
`JoinHandle`); each transport accessor clones the inner
`rustls::ServerConfig`, which is cheap (Arc-shared cert resolver).

```rust
pub struct TlsServerConfig {
    config: rustls::ServerConfig,
    acme_handle: Option<tokio::task::JoinHandle<()>>,
}

impl TlsServerConfig {
    pub async fn new(identity: &TlsIdentity, alpns: &[Vec<u8>])
        -> Result<Self, TlsError>;

    #[cfg(feature = "quinn")]
    pub fn for_quinn(&self) -> Result<quinn::ServerConfig, TlsError>;

    #[cfg(feature = "tcp")]
    pub fn for_tcp_tls(&self) -> tokio_rustls::TlsAcceptor;

    pub fn rustls_config(&self) -> &rustls::ServerConfig;
}
```

### What moves

| Component | From | To |
|-----------|------|-----|
| `build_rustls_server_config()` | `alknet-core/endpoint.rs` | `alknet-tls` |
| `TlsSetup` / ACME state machine | `alknet-core/endpoint.rs` | `alknet-tls` (`TlsServerConfig::new` ACME path) |
| `RawKeyCertResolver` | `alknet-core/endpoint.rs` | `alknet-tls` |
| `Ed25519SigningKey` | `alknet-core/endpoint.rs` | `alknet-tls` |
| `AcceptAnyCertVerifier` | `alknet-core/endpoint.rs` | `alknet-tls` |
| `SelfSignedCert` / `generate_self_signed_cert()` | `alknet-core/endpoint.rs` | `alknet-tls` |
| `load_cert_chain()` / `load_private_key()` | `alknet-core/endpoint.rs` | `alknet-tls` |
| `build_quinn_server_config_from_rustls()` | `alknet-core/endpoint.rs` | `alknet-tls` (`for_quinn()`) |

### What stays

| Component | Location | Why |
|-----------|----------|-----|
| `TlsIdentity` enum | `alknet-core/config.rs` | Config type — `StaticConfig` holds it |
| `Ed25519SecretKey` | `alknet-core/config.rs` | Config type — iroh reads it directly |
| `AcmeDirectory` | `alknet-core/config.rs` | Config type |
| `fingerprint.rs` | `alknet-core` | Shared by server (endpoint) and client (`alknet-call`'s `FingerprintPinVerifier`) — moving it would create a dep edge from `alknet-call` to `alknet-tls`. Production code uses `sha2` + manual DER only; `rustls` is test-only. See OQ-59. |
| `AlknetEndpoint` | `alknet-core` | The endpoint struct stays; it takes no TLS config (see ADR-083) |

### Feature gates

```toml
[features]
default = []
quinn = ["dep:quinn"]       # for_quinn()
tcp = ["dep:tokio-rustls"]  # for_tcp_tls()
acme = ["dep:rustls-acme"]  # ACME state machine
```

A deployment enables the features for the transports it runs. The
`rustls` dep is always present (core TLS library). `tokio-rustls` is
only pulled in when `tcp` is enabled. `quinn` is only pulled in when
`quinn` is enabled. `rustls-acme` is only pulled in when `acme` is
enabled. `futures` (for `StreamExt` in the ACME event loop) is present
when `acme` is enabled. `rustls-pki-types` is available via `rustls`'s
re-export (core lists it directly; the new crate can rely on the
re-export or list it directly — implementation detail).

### `AlknetEndpoint` takes no TLS config (see ADR-083)

ADR-082's original proposal was that `AlknetEndpoint::new` would take
`Arc<TlsServerConfig>`. That does not hold: a hub serving both native
clients (raw key) and browsers (X.509/ACME) holds **two**
`TlsServerConfig`s, and the endpoint has no single "the TLS config" to
take. The endpoint takes no TLS config at all — it is a pure
accept-loop runner with a public `dispatch` method. The assembly layer
builds the `TlsServerConfig`s and the transports, and hands the
pre-built quinn/iroh endpoints to `AlknetEndpoint` via builder methods.
See [ADR-083](083-endpoint-as-accept-loop-runner.md) for the endpoint's
new shape and signature.

`alknet-tls`'s job is to make the cert available to whichever transports
the deployment runs. The endpoint's job is to dispatch. The two are
decoupled — `alknet-tls` provides `TlsServerConfig` and its accessors
(`for_quinn`, `for_tcp_tls`, `rustls_config`); the assembly layer wires
them to transports; the endpoint dispatches connections from those
transports.

### The TCP+TLS accept loop lives outside `alknet-tls`

`alknet-tls` provides `for_tcp_tls() -> TlsAcceptor`. The actual TCP
accept loop (`TcpListener::accept` → `TlsAcceptor::accept` →
`Connection::from_bidi` → `HandlerRegistry::dispatch`) lives elsewhere —
in `alknet-hub` (the primary multi-transport consumer) or in a future
`alknet-core` module. `alknet-tls` is the cert provider, not the
accept loop. This keeps `alknet-tls` focused on TLS setup and cert
sharing, not transport accept logic.

### One ACME state machine, shared

For ACME, `TlsServerConfig::new` spawns the `AcmeState` task once and
wires its `resolver()` into the `rustls::ServerConfig`. Both
`for_quinn()` and `for_tcp_tls()` clone the same `rustls::ServerConfig`,
which shares the same `Arc<dyn ResolvesServerCert>` (the ACME resolver).
One ACME order, one cert cache, one resolver, two transports.

The `acme-tls/1` ALPN is appended to the server's `alpn_protocols` only
when ACME is active (ADR-027 §7); this behavior moves to
`TlsServerConfig::new`'s ACME branch unchanged. The caller does not
append `acme-tls/1` — the TLS crate does.

### Behavior-preservation invariants

The extraction must preserve these load-bearing TLS behaviors from the
current code. An implementer who omits any of these produces a crate
that compiles and passes type-checks but silently changes TLS behavior:

- **`max_early_data_size = u32::MAX`** on all server config paths.
  Enables 0-RTT / early data. Omitting it silently breaks 0-RTT clients.
- **`rustls::crypto::aws_lc_rs::default_provider()`** as the crypto
  provider on all paths. Matches iroh's `tls-aws-lc-rs` feature. Do not
  switch to `ring` or the process-default provider without an ADR.
- **`AcceptAnyCertVerifier`'s `supported_verify_schemes()`** returns
  ED25519 + ECDSA P-256/P-384 + RSA PSS/PKCS1 (SHA256/384/512). This
  list determines which client cert signature algorithms the server
  accepts. Must be preserved verbatim.
- **`acme-tls/1` ALPN append** for the ACME path only (above).

## Consequences

**Positive:**
- One certificate identity serves QUIC and TCP+TLS simultaneously. A
  hub can serve HTTP on 443 (TCP+TLS) and channels on 4433 (QUIC) with
  the same X.509 cert or the same ACME-issued cert.
- One ACME state machine per domain, shared across transports. No
  duplicate orders, no cert cache divergence, no Let's Encrypt
  rate-limit risk from duplicate orders.
- `alknet-core` loses the quinn-specific TLS setup code and the
  `#[cfg(feature = "quinn")]` gates on `TlsSetup`, `RawKeyCertResolver`,
  `AcceptAnyCertVerifier`, etc. Core becomes leaner — it holds config
  types and the endpoint struct, not TLS setup machinery.
- `tokio-rustls` is an opt-in dep (behind `alknet-tls`'s `tcp` feature),
  not forced on every `alknet-core` consumer.
- The raw-key path (RFC 7250) works for both QUIC and TCP+TLS — native
  clients can use raw keys over either transport. The browser case
  (requires X.509) is the exception, not the constraint.

**Negative:**
- `AlknetEndpoint::new` no longer takes a `tls_config` parameter, which
  is a breaking change for existing callers. The assembly layer builds
  the `TlsServerConfig`s and the transports and hands pre-built
  endpoints to `AlknetEndpoint` via builder methods (see
  [ADR-083](083-endpoint-as-accept-loop-runner.md)). This is expected —
  it is the point of the extraction.
- `alknet-core` may keep a narrow `rustls` dep if `fingerprint.rs` stays
  (OQ-59). If `fingerprint.rs` moves to `alknet-tls`, `alknet-call`'s
  client-side `FingerprintPinVerifier` gains a dep on `alknet-tls`. The
  trade-off is documented in OQ-59.
- A new crate in the dep graph. Small, focused, one clear
  responsibility.

## Door type

**One-way.** Extracting TLS setup into a shareable config is a
structural change: the assembly layer must build `TlsServerConfig`
separately and the cert-sharing contract (one config, N transports)
becomes the architecture. Reversing would mean re-welding TLS to the
endpoint and losing the multi-transport cert-reuse capability — the
exact capability the hub needs. The `AlknetEndpoint::new` signature
change is documented in ADR-083, not here.

The `TlsServerConfig` API surface (`new`, `for_quinn`, `for_tcp_tls`,
`rustls_config`) is one-way — changing it after consumers exist is a
rewrite. The internal implementation (how the ACME state machine is
spawned, how the cert resolver works) is two-way — implementation
details that can change without breaking the contract.

## References

- ADR-010 Amendment 1 — TCP+TLS dispatch via `from_stream` (the accept
  loop that consumes `TlsServerConfig::for_tcp_tls()`)
- ADR-083 — endpoint as pure accept-loop runner with public dispatch
  (the endpoint takes no TLS config; the assembly layer builds
  transports from `TlsServerConfig`s)
- ADR-027 — `TlsIdentity` (RawKey / X509 / Acme), RFC 7250, browser
  limitation
- ADR-030 §6 — fingerprint normalization (`ed25519:<hex>` across
  quinn/iroh)
- ADR-034 — client-side verifier selection (CA vs fingerprint pin)
- ADR-065 — `Connection::from_stream`/`from_bidi` (TCP+TLS path)
- ADR-080 — `ChannelClient::from_connection` (transport-agnostic
  client; the pattern this ADR mirrors on the TLS side)
- OQ-59 — should `fingerprint.rs` stay in core or move to `alknet-tls`?
- `crates/alknet-core/src/endpoint.rs` — the code being extracted
- `crates/alknet-core/src/config.rs` — `TlsIdentity`, `Ed25519SecretKey`
  (staying in core)
- `crates/alknet-core/src/fingerprint.rs` — fingerprint extraction
  (OQ-59)
- `docs/architecture/crates/tls/README.md` — the crate spec