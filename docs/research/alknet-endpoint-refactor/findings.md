# alknet-endpoint-refactor — Endpoint as Pure ALPN Dispatcher

**Status:** Findings complete. The endpoint refactor and the TLS
extraction are interlocked; this document captures the full picture so
neither gets done in isolation and drops half the context.
**Date:** 2026-07-13
**Scope:** `AlknetEndpoint` becomes a pure accept-loop runner (no
transport construction); `alknet-tls` extraction (`TlsServerConfig`)
provides the shared cert config the assembly layer needs to build
transports. Both changes are enabled by the pre-implementation window
— nobody is using this code yet, and the issues surfaced from trying to
write the first real consumers (hub, workers, HTTP).

---

## TL;DR

1. **`AlknetEndpoint` conflates two concerns:** transport construction
   (building `quinn::Endpoint`, `iroh::Endpoint`, wiring TLS) and
   accept-loop orchestration (running accept loops, dispatching by
   ALPN, managing shutdown). The TLS extraction forces a separation
   because the TLS config has to be built once and shared across
   transports — the endpoint can't build it internally anymore.

2. **The endpoint becomes a pure accept-loop runner.** It receives
   pre-built transport endpoints (`quinn::Endpoint`, `iroh::Endpoint`)
   and runs their accept loops, dispatching by ALPN through the shared
   `HandlerRegistry`. No `StaticConfig` parameter, no `tls_identity`
   reading, no transport construction. The assembly layer builds the
   transports, wires them to share a `HandlerRegistry`, and hands the
   quinn/iroh endpoints to `AlknetEndpoint`. TCP+TLS accept loops run as
   siblings (already the case per ADR-010 Amendment 1 / ADR-065).

3. **A hub holds one or two `TlsServerConfig`s, not one.** A hub
   serving both browsers and native workers holds a raw-key config
   (QUIC + TCP+TLS fallback for native clients) and an X.509/ACME
   config (TCP+TLS for HTTPS, QUIC for WebTransport when it revives).
   The cert-reuse value is *within* each identity: one config per
   identity, shared across that identity's transports. Iroh and SSH
   share the *key* with the raw-key TLS path, not the `TlsServerConfig`.

4. **The TCP fallback for native clients doesn't need X.509.** The
   raw-key `TlsServerConfig` serves both QUIC and TCP+TLS. X.509 is
   only for the browser/HTTPS path. Raw-key and X.509 clients can
   connect to either server type — the server cert type and client cert
   type are independent in TLS. `AcceptAnyCertVerifier` already accepts
   both.

5. **This is a one-way door (endpoint struct shape) done during the
   pre-implementation window.** The cost of doing it now is the same
   refactor that has to happen anyway. The cost of not doing it is a
   hybrid model (quinn TLS injected, iroh key read internally, TCP+TLS
   outside) that will confuse every future reader and block the hub
   crate (the first multi-transport consumer).

---

## 1. The transport landscape

### 1.1 The full transport matrix

| Transport | TLS identity | Serves | Who connects | Status |
|-----------|-------------|--------|---------------|--------|
| QUIC (raw key) | RFC 7250 Ed25519 | `alknet/channels`, `alknet/call` | Native clients (workers, spokes) | Day 1 |
| TCP+TLS (raw key fallback) | RFC 7250 Ed25519 | Same as above | Native clients (UDP blocked) | Day 1 |
| TCP+TLS (X.509 / ACME) | X.509 | `h2`/`http/1.1` (HTTPS) | Browsers, HTTP clients | Day 1 |
| QUIC (X.509 / ACME) | X.509 | WebTransport | Browsers | Deferred (ADR-044; revives with channels) |
| iroh | Ed25519 (own TLS) | `alknet/channels`, `alknet/call` | P2P native clients | Day 1 |
| TCP (SSH) | Ed25519 key (SSH) | `alknet/ssh` channels | Legacy SSH clients | Future |

### 1.2 Two cert identities, shared within each

A hub serving both browsers and native workers holds **two**
`TlsServerConfig`s:

- **Raw key config** — built from `TlsIdentity::RawKey(Ed25519SecretKey)`.
  Shared across:
  - QUIC listener (`for_quinn()`) — native clients with UDP
  - TCP+TLS listener (`for_tcp_tls()`) — native clients when UDP blocked
  - The same `Ed25519SecretKey` also feeds iroh and (future) SSH

- **X.509 config** — built from `TlsIdentity::X509` or `TlsIdentity::Acme`.
  Shared across:
  - TCP+TLS listener (`for_tcp_tls()`) — HTTPS for browsers
  - QUIC listener (`for_quinn()`) — WebTransport when it revives

The cert-reuse value is *within* each identity, not across them. A hub
doesn't share one cert for everything — it shares one cert per identity
across that identity's transports. This is why `AlknetEndpoint::new`
taking a single `Arc<TlsServerConfig>` (as ADR-082 currently specifies)
is wrong for the hub case: the hub has two configs. The endpoint
shouldn't take any TLS config — the assembly layer builds the transports
and hands the endpoint pre-built quinn/iroh endpoints.

### 1.3 Iroh and SSH share the key, not the TLS config

Iroh has its own TLS built into the `Endpoint` (RFC 7250 raw keys). It
does not consume a `rustls::ServerConfig`. SSH uses the Ed25519 key for
its own handshake. Both share the `Ed25519SecretKey` with the raw-key
`TlsServerConfig`, but neither receives a `TlsServerConfig`.

The assembly layer reads `Ed25519SecretKey` from `StaticConfig` and
hands it to:
- `TlsServerConfig::new(TlsIdentity::RawKey(key), ...)` — for quinn/TCP+TLS
- `iroh::SecretKey::from_bytes(key.as_bytes())` — for iroh
- (future) the SSH handler's key configuration

### 1.4 Raw-key / X.509 mixing in the TLS handshake

A raw-key client can connect to an X.509 server and vice versa. In
TLS, the server cert type and client cert type are independent. A
server presenting an X.509 cert can accept a client presenting a raw
key (RFC 7250), as long as both sides have RFC 7250 support in their
rustls config. `AcceptAnyCertVerifier` already accepts both, and its
`supported_verify_schemes()` includes ED25519.

The mixing cases:
- **Raw-key client → X.509 server**: client verifies server via CA
  (ADR-034 public X.509 case) or fingerprint pin (known hub); server
  extracts client's `ed25519:<hex>` fingerprint.
- **X.509 client → raw-key server**: client verifies server via
  fingerprint pin (raw-key remotes are always known peers); server
  extracts client's `SHA256:<hex>` fingerprint.

This means the TCP fallback for native clients doesn't need X.509 — the
raw-key config serves both QUIC and TCP+TLS. X.509 is only for the
browser/HTTPS path.

### 1.5 What's required vs. what's built now

All transports in the matrix above are **requirements** — not "nice to
have." But we do not have to build all of them right now. The
abstractions (the endpoint refactor, the `TlsServerConfig` extraction,
`Connection::from_stream`/`from_bidi`) are what make the full matrix
tractable without blocking any of the future transports. The point is
to get the abstractions right so that each transport can be added
without changing the core.

By volume, the most common use case is raw-key QUIC (native P2P,
workers). But we have to support the HTTP crate and X.509 from day one.
The TCP fallback for native clients (raw key) is important too since
UDP is blocked on some networks. A worker has to register with the hub
via HTTP POST in many cases, then connect via QUIC if available and
fall back to TCP if not.

---

## 2. The endpoint refactor

### 2.1 Current state: the endpoint conflates construction and orchestration

`AlknetEndpoint::new` currently does three things:

1. **Builds the quinn endpoint** — reads `static_config.tls_identity`,
   constructs TLS internally (`TlsSetup::new`,
   `build_rustls_server_config`), wraps it into
   `quinn::ServerConfig`, binds `quinn::Endpoint::server()`.
2. **Builds the iroh endpoint** — reads `static_config.tls_identity`
   again (for the Ed25519 key), calls `build_iroh_endpoint()`.
3. **Runs accept loops** for whichever of those it built, dispatching
   by ALPN through the `HandlerRegistry`.

The TLS extraction pulls step 1's TLS construction out to the assembly
layer. But step 2 (iroh) still reads `static_config.tls_identity`
internally. And TCP+TLS (step 3's sibling) is already outside the
endpoint entirely (ADR-010 Amendment 1). The result is a hybrid: quinn
TLS is *injected*, iroh's key is *read from config internally*, TCP+TLS
is *outside*. That asymmetry is a structural inconsistency.

### 2.2 The trajectory: transport construction moves to the assembly layer

The pattern is already emerging:

| Concern | Current | Direction |
|---------|---------|-----------|
| Quinn TLS config | Built internally | → Assembly layer builds `TlsServerConfig` |
| Quinn endpoint binding | Built internally | → Assembly layer builds `quinn::Endpoint` |
| Iroh endpoint binding | Built internally | → Assembly layer builds `iroh::Endpoint` |
| TCP+TLS accept loop | Outside the endpoint | ✓ Already assembly layer (ADR-010 Am. 1) |
| HandlerRegistry | Passed in | ✓ Already assembly layer |
| IdentityProvider | Passed in | ✓ Already assembly layer |
| Shutdown | Internal watch channel | Stays internal (endpoint owns the loop) |

The endpoint's role unambiguously becomes: **take established
connections, dispatch by ALPN.** It doesn't build transports, doesn't
read `tls_identity`, doesn't know what cert type is in use.

### 2.3 The new endpoint shape

```rust
pub struct AlknetEndpoint {
    quinn: Option<quinn::Endpoint>,
    iroh: Option<iroh::Endpoint>,
    handlers: Arc<HandlerRegistry>,
    dynamic: Arc<ArcSwap<DynamicConfig>>,
    identity_provider: Arc<dyn IdentityProvider>,
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
    drain_timeout: Duration,
}

impl AlknetEndpoint {
    pub fn new(
        handlers: HandlerRegistry,
        dynamic: Arc<ArcSwap<DynamicConfig>>,
        identity_provider: Arc<dyn IdentityProvider>,
        drain_timeout: Duration,
    ) -> Self;

    pub fn with_quinn(mut self, endpoint: quinn::Endpoint) -> Self;
    pub fn with_iroh(mut self, endpoint: iroh::Endpoint) -> Self;
}
```

The `StaticConfig` parameter disappears from `new()`. The assembly
layer reads `StaticConfig`, builds the transports, and hands the
quinn/iroh endpoints to `AlknetEndpoint` via builder methods.
`build_iroh_endpoint` and `build_quinn_server_config_from_rustls` move
out of core — the assembly layer (or `alknet-tls` as a convenience)
builds the transport endpoints.

TCP+TLS accept loops continue to run as siblings (ADR-010 Amendment 1),
sharing the `HandlerRegistry` but not owned by `AlknetEndpoint`. The
hub's assembly layer constructs all transport sources and wires them
to share one registry.

### 2.4 What the assembly layer does

For a hub serving native clients (raw key) and browsers (X.509):

1. Read `StaticConfig`.
2. Build the raw-key `TlsServerConfig` from `TlsIdentity::RawKey(key)`.
3. Build the X.509 `TlsServerConfig` from `TlsIdentity::Acme { ... }`
   (or `X509`).
4. Build the quinn endpoint for native clients:
   `raw_key_tls.for_quinn()` → `quinn::Endpoint::server()`.
5. Build the TCP+TLS accept loop for native fallback:
   `raw_key_tls.for_tcp_tls()` → `TcpListener` + `TlsAcceptor`.
6. Build the TCP+TLS accept loop for HTTPS:
   `x509_tls.for_tcp_tls()` → `TcpListener` + `TlsAcceptor`.
7. (Future) Build the quinn endpoint for WebTransport:
   `x509_tls.for_quinn()` → `quinn::Endpoint::server()`.
8. Build the iroh endpoint: `iroh::Endpoint::builder().secret_key(...)`.
9. Build the `HandlerRegistry`, register all handlers.
10. Construct `AlknetEndpoint::new(registry, ...)` with `.with_quinn()`
    (and `.with_iroh()` if iroh is used).
11. Run the TCP+TLS accept loops as siblings that share the registry.
12. Run `AlknetEndpoint::run()` — runs the quinn/iroh accept loops.

For a bare-bones P2P node (no public IP, no ACME, no browsers): just
step 1, step 8, step 9, step 10 (with only `.with_iroh()`). No
`TlsServerConfig` needed at all.

### 2.5 What this resolves

- **C5 (iroh path ownership):** the endpoint doesn't build iroh; the
  assembly layer does. No ambiguity.
- **The single-`Arc<TlsServerConfig>` problem:** ADR-082 currently
  specifies `AlknetEndpoint::new` taking `Arc<TlsServerConfig>`, but a
  hub has two configs (raw key + X.509). With the endpoint taking no
  TLS config, this disappears.
- **The `static_config.tls_identity` reading:** the endpoint no longer
  reads it. The assembly layer reads it and builds the transports.
- **The `acme_state_handle` on the endpoint:** the ACME task handle
  lives on `TlsServerConfig` (the assembly layer owns it), not on the
  endpoint.

---

## 3. Interaction with `alknet-tls` extraction (ADR-082)

### 3.1 The TLS extraction is the trigger, not the whole refactor

The TLS extraction (`alknet-tls` crate with `TlsServerConfig`) is what
surfaced the endpoint's conflation. But the endpoint refactor is a
separate, larger change that ADR-082 should reference but not contain.

### 3.2 ADR-082 changes

With the endpoint refactor, ADR-082's `AlknetEndpoint::new` signature
changes. Instead of:

```rust
pub async fn new(
    static_config: &StaticConfig,
    tls_config: Arc<TlsServerConfig>,  // ← was the new param
    handlers: HandlerRegistry,
    dynamic: Arc<ArcSwap<DynamicConfig>>,
    identity_provider: Arc<dyn IdentityProvider>,
) -> Result<Self, EndpointError>;
```

The endpoint takes no TLS config at all (see §2.3). ADR-082 should
reference the endpoint refactor ADR (to be written) for the new
signature, and focus on what `alknet-tls` provides:
`TlsServerConfig` and its accessors.

### 3.3 What moves out of `alknet-core/endpoint.rs`

| Code | Destination |
|------|-------------|
| `build_rustls_server_config()` | `alknet-tls` |
| `TlsSetup` / ACME state machine | `alknet-tls` (`TlsServerConfig::new` ACME path) |
| `RawKeyCertResolver` | `alknet-tls` |
| `Ed25519SigningKey` | `alknet-tls` |
| `AcceptAnyCertVerifier` | `alknet-tls` |
| `SelfSignedCert` / `generate_self_signed_cert()` | `alknet-tls` |
| `load_cert_chain()` / `load_private_key()` | `alknet-tls` |
| `build_quinn_server_config_from_rustls()` | `alknet-tls` (`for_quinn()`) |
| `build_iroh_endpoint()` | Assembly layer (or a helper; construction decision is assembly-layer) |

What stays in `alknet-core/endpoint.rs`:
- `AlknetEndpoint` struct (accept-loop runner only)
- `HandlerRegistry`
- `dispatch_quinn` / `dispatch_iroh` / `build_auth_context`
- `run_quinn_accept_loop` / `run_iroh_accept_loop`
- `extract_quinn_alpn` / `extract_quinn_client_fingerprint` /
  `extract_iroh_client_fingerprint`
- The `acme-tls/1` early-return guard in `dispatch_quinn`

What stays in `alknet-core/config.rs`:
- `TlsIdentity` enum (config type)
- `Ed25519SecretKey` (config type)
- `AcmeDirectory` (config type)
- `StaticConfig` (config type)

What stays in `alknet-core/fingerprint.rs` (pending OQ-59):
- `fingerprint_from_cert_der`, `extract_ed25519_raw_key_from_spki`

### 3.4 Ordering question: tls first or core refactor first?

This is deferred to the task-decomposition phase. Both are
interlocked: the TLS extraction removes code from `endpoint.rs`, and the
endpoint refactor changes what stays. The natural ordering might be:

1. **Endpoint refactor first** — change `AlknetEndpoint` to a pure
   accept-loop runner, move `build_iroh_endpoint` and
   `build_quinn_server_config_from_rustls` out of core. At this point,
   the TLS setup code still exists in core but is called by the
   assembly layer instead of the endpoint.
2. **TLS extraction second** — move the TLS setup code into
   `alknet-tls`, build `TlsServerConfig`, replace the assembly layer's
   direct calls with `TlsServerConfig::new` / `for_quinn()` /
   `for_tcp_tls()`.

But this is a task-decomposition decision, not an architecture
decision. The architecture is: both changes happen, the endpoint
becomes a pure accept-loop runner, and `alknet-tls` provides the
shareable cert config.

---

## 4. The remaining review items (from the tls spec review)

These are unaffected by the endpoint refactor. They're about the TLS
crate's API surface and the OQ/ADR status.

### 4.1 C2: Define `TlsError` type

The `TlsError` type is used in every public signature but never
defined. Needs a variant list. The current code uses
`EndpointError::TlsConfig(io::Error)`. The new type should cover:
- IO errors (file read failures for cert/key loading)
- rustls config errors (protocol version, cipher suite setup)
- Quinn config errors (`QuicServerConfig::try_from` failure)
- ACME feature-not-enabled (when `TlsIdentity::Acme` is configured
  without the `acme` feature)

### 4.2 C3 + C6: ACME task lifecycle and cert renewal

The ACME state machine task is spawned by `TlsServerConfig::new` (ACME
path) and its `JoinHandle` is held on `TlsServerConfig`. The spec
currently says "dropping this handle does NOT stop the ACME state
machine" but never says what *does* stop it.

Cert renewal behavior (confirmed from the reverse-proxy reference):

| Identity | Renewal behavior | Drain needed? |
|----------|-----------------|---------------|
| ACME | Live — `ResolvesServerCertAcme` swaps cert in place; all listeners sharing the `Arc<rustls::ServerConfig>` pick up the renewed cert on the next TLS handshake | No |
| X.509 (manual) | Loaded once at startup via `with_single_cert`; no live renewal | Yes (restart to load new cert) |
| RawKey | No expiry — Ed25519 keys don't expire | N/A |
| SelfSigned | Dev only — restart to regenerate | N/A |

The ACME renewal mechanism: `TlsServerConfig::new` (ACME path) calls
`state.resolver()` to get an `Arc<ResolvesServerCertAcme>`, wires it
into the `rustls::ServerConfig` via `with_cert_resolver(resolver)`, and
spawns the `AcmeState` event loop. When the event loop processes a
renewed cert (`EventOk::DeployedNewCert`), it updates the resolver's
internal cert in place. Since `for_quinn()` and `for_tcp_tls()` clone
the `rustls::ServerConfig` (which shares the `Arc<ResolvesServerCertAcme>`),
both the quinn endpoint and the TCP+TLS acceptor see the renewed cert
on the next handshake. No drain, no restart, no listener replacement.

The shutdown path: the ACME task's lifetime is the renewal lifetime.
Shutting it down stops renewal (but existing listeners keep serving
the last-known cert until it expires). Restarting the process
re-spawns the state machine, which deploys the cached cert
(`DeployedCachedCert`) and resumes renewal. The `TlsServerConfig`
needs a `shutdown()` method (or a `Drop` impl that aborts the handle)
so the assembly layer can cleanly stop the ACME task on process
shutdown.

### 4.3 C4: OQ-59 (fingerprint.rs location)

OQ-59 is open but the spec treats `fingerprint.rs` staying in core as
decided. The "Likely resolution" in OQ-59 is Option A (stay in core),
with solid rationale: `alknet-call`'s client-side
`FingerprintPinVerifier` uses fingerprint functions and shouldn't
have to depend on `alknet-tls` (which would pull `rustls` + `tokio` +
optionally `quinn`/`tokio-rustls`/`rustls-acme` into every
client-only deployment). Recommendation: resolve OQ-59 to Option A
(stay in core) — it's a two-way door (moving a module later is a
refactor, not a wire-format change), and the rationale is clear.

### 4.4 W3: Feature-gate edge cases

- `TlsIdentity::Acme` configured without the `acme` feature — should
  return a runtime error (matching the current code's behavior).
- `default = []` with no features — `new()` and `rustls_config()` work
  but `for_quinn`/`for_tcp_tls` don't exist. This is a supported config
  for iroh-only deployments (no `TlsServerConfig` needed at all — the
  assembly layer just uses the `Ed25519SecretKey` directly).
- `acme` without `quinn` or `tcp` — ACME task spawns, `rustls_config()`
  available, but no transport accessor. Valid for a pure cert-provider
  use case (not currently expected, but not blocked).

### 4.5 W4: aws_lc_rs crypto provider ADR

The `aws_lc_rs::default_provider()` choice is a load-bearing decision
(FIPS status, platform support) with no ADR. Either add a clause to
ADR-082 (or ADR-027) recording the choice, or write a small standalone
ADR. The invariant "do not switch to `ring` or the process-default
provider without an ADR" is already in the spec; the decision record
should exist.

### 4.6 W5: ADR-082 status

ADR-082 is "Proposed." If implementation is about to begin (and the
endpoint refactor is part of the same work), the ADR should move to
"Accepted" — with OQ-59 noted as a non-blocking open question that
can be resolved alongside implementation. "Proposed" for an
actively-implemented extraction is a stale status.

---

## 5. The reverse-proxy as reference

The reverse-proxy (`/workspace/@alkdev/reverse-proxy/`) is a useful
reference for the TLS setup, with the caveat that it is X.509-only
(manual + ACME). The relevant code:

- `src/tls/acme.rs` — `AcmeTlsConfig::setup()` constructs the
  `AcmeState` and gets the `resolver()`. `spawn_acme_state()` runs the
  event loop with a `tokio::select!` against a shutdown signal (so the
  ACME task can be cleanly stopped, not just aborted).
- `src/tls/acceptor.rs` — `TlsMode` enum (`Manual` vs `Acme`) holds
  the `Arc<ServerConfig>` and the `Arc<ResolvesServerCertAcme>`.
  `setup_tls()` builds the `TlsAcceptor` from the server config.
- `src/tls/config.rs` — `crypto_provider()` constructs the
  `aws_lc_rs` provider with restricted cipher suites. Manual cert
  loading via `rustls-pemfile`.

What the reverse-proxy does that alknet-tls should learn from:
- The ACME task is spawned with a shutdown signal (`tokio::select!`
  against `shutdown_rx.changed()`), not just a bare `JoinHandle`. This
  is cleaner than abort — the state machine gets to finish its current
  operation and exit gracefully. (Relevant to C3.)
- The `TlsMode` enum separates the static config from the runtime
  objects — `Manual(Arc<ServerConfig>)` vs
  `Acme { default_config, resolver }`. This mirrors the
  `TlsServerConfig` / `TlsIdentity` split.
- The `ResolvesServerCertAcme` is the live renewal mechanism — the
  `TlsAcceptor` built from `default_config` (which holds an `Arc` to
  the resolver) picks up renewed certs on the next handshake. No
  drain, no restart. (Relevant to C6.)

What the reverse-proxy doesn't cover (because it's X.509-only):
- RFC 7250 raw keys (the `RawKeyCertResolver` / `Ed25519SigningKey`
  path)
- Multiple transport sharing (it's TCP-only; no quinn, no iroh)
- The endpoint-as-accept-loop-runner pattern (it has its own
  `serve_https_listener` that's simpler than `AlknetEndpoint`)

---

## 6. Open questions for the next session

1. **C2 (`TlsError` variants):** what error types should `TlsError`
   wrap? The current `EndpointError::TlsConfig(io::Error)` is too
   narrow — it doesn't cover `rustls::Error` or ACME-specific errors.
   Need a variant list.

2. **C3 (ACME shutdown):** should `TlsServerConfig` have a
   `shutdown()` method (clean, explicit) or a `Drop` impl (automatic,
   but surprising)? The reverse-proxy uses a shutdown signal
   (`tokio::select!` against `shutdown_rx.changed()`) rather than
   `abort()`. Should alknet-tls do the same?

3. **C4 (OQ-59):** resolve to Option A (stay in core) or keep open?
   Recommendation: resolve to Option A.

4. **Endpoint refactor ADR:** draft ADR-083 (amending ADR-010) for the
   endpoint-as-accept-loop-runner shape, or fold it into ADR-082?
   Recommendation: standalone ADR — it changes ADR-010's endpoint
   design, which is a broader change than the TLS extraction.

5. **Ordering:** does the endpoint refactor land before the TLS
   extraction, or after? Deferred to task decomposition.