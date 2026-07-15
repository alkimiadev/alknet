# ADR-088: `TlsError` Shape — Single Enum, Owned by `alknet-tls`

## Status

Accepted (resolves OQ-63)

## Context

OQ-63 flagged that `TlsError` is referenced in the public signatures of
`TlsServerConfig::new`, `TlsClientConfig::new`, and `for_quinn()` (ADR-082,
ADR-087) but was never sketched. An implementer cannot write the crate
without deciding the shape: a thin wrapper around `rustls::Error` /
`io::Error` (less code, no matchable context) or a single enum with variants
per failure category (more code, the assembly layer can distinguish
"cert file missing" from "quinn rejected the rustls config" from
"verifier build failed because the root store is empty"). This ADR makes
that decision, grounded in the actual error-producing call sites and the
actual error types the dependency crates return.

### What was verified (the grounding)

The decision is grounded in two sources, not memory:

1. **The current code** — `crates/alknet-core/src/endpoint.rs`
   (server-side: `build_rustls_server_config`, `TlsSetup::new`,
   `TlsSetup::new_acme`, `build_quinn_server_config_from_rustls`,
   `load_cert_chain`, `load_private_key`, `generate_self_signed_cert`)
   and `crates/alknet-call/src/client/call_client.rs` (client-side:
   `build_quinn_client_config`, `build_client_auth`,
   `select_server_verifier`, `load_platform_root_cert_store`). Both
   currently funnel every error into `io::Error` (server) or `String`
   (client), losing all context.
2. **The dependency-crate sources** (rustls 0.23.41, rustls-pemfile 2.2.0,
   rcgen 0.13.2, quinn-proto 0.11.15, rustls-acme 0.12.1, rustls-pki-types
   1.14.1) — read from the cargo cache to confirm the exact error type
   each call site returns, whether it implements `std::error::Error`, and
   whether it is `Send + Sync + 'static`. Also confirmed: `rustls::Error`'s
   `std::error::Error` impl is empty (no `source()` — the detail is in
   `Display`, not a source chain); `rustls_pemfile::Error` has no
   `Display` and no `Error` impl (only `From<Error> for io::Error`); the
   `quinn` crate re-exports `quinn_proto::crypto` as `quinn::crypto`, so
   `quinn::crypto::rustls::NoInitialCipherSuite` is the public path. The
   full mapping is below.

### The actual error-producing call sites

**Server side (`TlsServerConfig::new` + `for_quinn`):**

| Call site | Error type | Notes |
|-----------|-----------|-------|
| `std::fs::read(cert_path)` / `read(key_path)` | `io::Error` | file read; cert/key load |
| `rustls_pemfile::certs()` / `private_key()` | `io::Error` | pemfile funnels its own `pemfile::Error` into `io::Error` (it does not impl `std::error::Error` — see "Gotchas") |
| "no private key found in file" | `io::Error(InvalidData)` | the `Ok(None)` case from `private_key()` |
| `rcgen::KeyPair::generate()` / `CertificateParams::self_signed()` | `rcgen::Error` | self-signed cert generation |
| `ServerConfig::builder_with_provider(...).with_safe_default_protocol_versions()` | `rustls::Error` | protocol-version / cipher-suite selection |
| `.with_single_cert(cert_chain, key)` | `rustls::Error` | key load via provider + `keys_match` |
| `RawKeyCertResolver::new(...)` | **infallible** | builds `CertifiedKey` in-memory; `ResolvesServerCert` has no construction error |
| `QuicServerConfig::try_from(rustls::ServerConfig)` (in `for_quinn`) | **`NoInitialCipherSuite`** | NOT `rustls::Error` — a distinct struct (re-exported at `quinn::crypto::rustls::NoInitialCipherSuite`, defined in `quinn_proto::crypto::rustls`); the one path where `for_quinn()` fails |
| "ACME feature not enabled but `TlsIdentity::Acme` configured" | `io::Error(Unsupported)` | the `#[cfg(not(feature = "acme"))]` guard |
| `AcmeConfig::new()` / `AcmeConfig::state()` | **infallible** | ACME state-machine construction does not fail; errors are stream events (`EventError`), logged not returned (see "ACME") |

**Client side (`TlsClientConfig::new`):**

| Call site | Error type | Notes |
|-----------|-----------|-------|
| `ClientConfig::builder_with_provider(...).with_safe_default_protocol_versions()` | `rustls::Error` | same as server-side |
| `FingerprintPinVerifier::new(...)` | **infallible** | stores the fingerprint + supported algorithms; the known-peer path |
| `WebPkiServerVerifier::builder_with_provider(...).build()` | **`VerifierBuilderError`** | `rustls::webpki::VerifierBuilderError` — `NoRootAnchors`, `InvalidCrl`; the unknown-X.509-remote path |
| `rustls::RootCertStore::add(cert)` | `rustls::Error` | adding a native root cert (maps `webpki::Error` → `InvalidCertificate(...)`) |
| `rustls-native-certs::load_native_certs()` errors | logged, not returned | the current client code logs and continues; an empty store falls back to built-in webpki-roots |
| Client-auth cert resolver — RawKey | **infallible** | builds `CertifiedKey` in-memory |
| Client-auth cert resolver — X.509 | `io::Error` (load) + `rustls::Error` (`CertifiedKey::from_der`) | cert/key file load + key parse |
| "ACME TLS identity is server-only; cannot be used for client auth" | `io::Error`-shape | the `TlsIdentity::Acme` as client-identity guard |
| `aws_lc_rs::default_provider()` | **infallible** | returns `CryptoProvider` by value; provider init cannot fail |

### Gotchas that drive the enum-not-wrapper decision

Three findings from the dependency-crate source make a thin wrapper
**actively wrong** for this crate:

1. **`for_quinn()` fails with `NoInitialCipherSuite`, not `rustls::Error`.**
   These are two distinct, non-overlapping types from two different crates.
   A thin wrapper around `rustls::Error` cannot represent the `for_quinn()`
   failure at all; a thin wrapper around `io::Error` (the current
   server-side pattern) erases it into "other I/O error." The assembly
   layer that calls `for_quinn()` needs to distinguish "the rustls config
   was built fine but quinn rejected it for lacking a TLS 1.3 AES-128-GCM
   initial cipher suite" from "the rustls config build failed" — those
   have different remediations (the first is a provider/cipher-suite
   config issue; the second is a cert/key issue).

2. **`rustls_pemfile::Error` is not a `std::error::Error`.** It has no
   `Display` impl and no `Error` trait impl — it only has
   `From<Error> for io::Error`. This is why every BufRead-based pemfile
   API returns `io::Error`, not `pemfile::Error`. A `TlsError` that tried
   to `#[from] rustls_pemfile::Error` would not compile. The crate's
   `io::Error`-returning APIs are the correct surface to wrap.

3. **`WebPkiServerVerifier::build()` returns `VerifierBuilderError`,
   not `rustls::Error`.** This is the client-side unknown-X.509-remote
   path (ADR-034 §3). Its variants (`NoRootAnchors`, `InvalidCrl`) are
   distinct from `rustls::Error::InvalidCertificate(...)`. A thin
   `rustls::Error` wrapper cannot represent "the CA root store was
   empty" as a first-class failure; the enum can.

A single enum with one variant per failure category gives the assembly
layer matchable context — the one thing the current `io::Error` /
`String` funneling destroys — and correctly represents the four
distinct error types (`rustls::Error`, `NoInitialCipherSuite`,
`VerifierBuilderError`, `rcgen::Error`, plus `io::Error` for file I/O
and the ACME-disabled guard) as separate variants instead of erasing
them into one.

### What the thin wrapper would lose

The current server-side code (`endpoint.rs`) wraps everything in
`EndpointError::TlsConfig(io::Error::other(e))`. The current client-side
code (`call_client.rs`) converts everything to `String`. Both lose the
matchable context. Concretely, an operator who sees
"tls config error: No such file or directory" cannot tell from the
error alone whether the cert path was wrong, the key path was wrong, or
the ACME cache dir was wrong — and an operator who sees
"tls config error: invalid cipher suite specified" cannot tell whether
that came from `with_safe_default_protocol_versions()` (a rustls
config issue) or from `QuicServerConfig::try_from` (a quinn-wrap issue).
The enum names the category; the wrapped source keeps the detail.

## Decision

### 1. `TlsError` is a single enum with variants per failure category

`TlsError` is a `#[non_exhaustive]` enum, one variant per failure
category, each variant wrapping the underlying error as its `#[source]`.
The decision is the **variant names and their wrapped types**; the
`#[from]` vs. explicit-field style and the `AcmeConfig(String)` vs.
structured-enum choice are two-way implementation details (see "Door
type"). The shape:

```rust
/// Errors produced by `TlsServerConfig::new`, `TlsClientConfig::new`,
/// and the transport accessors (`for_quinn`, `for_tcp_tls` is infallible).
///
/// One variant per failure category. The wrapped error is the
/// `#[source]` — match on the variant for the category, inspect the
/// source for the detail.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TlsError {
    /// Cert or key file could not be read or parsed.
    /// Source: `io::Error` (file read + pemfile parse, which pemfile
    /// funnels into `io::Error`; see ADR-088 §"Gotchas" #2).
    #[error("loading cert/key material: {0}")]
    CertLoad(#[from] std::io::Error),

    /// Self-signed certificate generation failed.
    /// Source: `rcgen::Error`.
    #[error("generating self-signed cert: {0}")]
    SelfSigned(#[from] rcgen::Error),

    /// rustls server or client config construction failed
    /// (`with_safe_default_protocol_versions`, `with_single_cert`,
    /// `CertifiedKey::from_der`, `RootCertStore::add`).
    /// Source: `rustls::Error`.
    #[error("building rustls config: {0}")]
    Rustls(#[from] rustls::Error),

    /// `WebPkiServerVerifier::build()` failed (the unknown-X.509-remote
    /// client path — empty root store, invalid CRL). Distinct from
    /// `Rustls` because the type is distinct and the remediation is
    /// different (root-store / CRL config, not cert/key config).
    /// Source: `rustls::webpki::VerifierBuilderError`.
    #[error("building webpki verifier: {0}")]
    VerifierBuild(#[from] rustls::webpki::VerifierBuilderError),

    /// `QuicServerConfig::try_from(rustls::ServerConfig)` failed — the
    /// one path where `for_quinn()` can fail. The rustls config was
    /// built fine; quinn rejected it (no TLS 1.3 AES-128-GCM initial
    /// cipher suite in the provider). Distinct from `Rustls` because
    /// the type is `NoInitialCipherSuite`, not `rustls::Error`, and the
    /// remediation is provider/cipher-suite config, not cert/key.
    /// Source: `quinn_proto::crypto::rustls::NoInitialCipherSuite`.
    #[cfg(feature = "quinn")]
    #[error("wrapping rustls config for quinn: {0}")]
    QuinnWrap(#[from] quinn::crypto::rustls::NoInitialCipherSuite),

    /// ACME feature not enabled but `TlsIdentity::Acme` configured, or
    /// ACME identity used where a client identity is required.
    /// Not a wrapped error — a configuration mismatch detected by the
    /// crate itself.
    #[error("ACME configuration error: {0}")]
    AcmeConfig(String),
}
```

**The variant set, in words:**

- `CertLoad(io::Error)` — file read + PEM parse (cert, key, and via the
  shared `load_cert_chain`/`load_private_key` helpers). Covers the
  server X.509 path, the client X.509 client-auth path, and the
  "no private key found" case. `#[from] io::Error` because `io::Error` is
  the type the pemfile BufRead APIs actually return.
- `SelfSigned(rcgen::Error)` — self-signed cert generation (server
  `SelfSigned` identity).
- `Rustls(rustls::Error)` — rustls server or client config construction
  (`with_safe_default_protocol_versions`, `with_single_cert`,
  `CertifiedKey::from_der`, `RootCertStore::add`). The largest failure
  surface; both server and client paths.
- `VerifierBuild(VerifierBuilderError)` —
  `WebPkiServerVerifier::build()` (the unknown-X.509-remote client path).
  Distinct from `Rustls` because the type is distinct and the
  remediation is root-store/CRL config, not cert/key config.
- `QuinnWrap(NoInitialCipherSuite)` — `for_quinn()`'s one failure path.
  Feature-gated on `quinn`. Distinct from `Rustls` because the type is
  distinct and the remediation is provider/cipher-suite config.
- `AcmeConfig(String)` — "ACME feature not enabled but
  `TlsIdentity::Acme` configured" (server) and "ACME identity is
  server-only; cannot be used for client auth" (client). A config
  mismatch detected by the crate, not a wrapped third-party error.

### 2. What is NOT a variant (and why)

These are deliberately absent because they cannot be produced by the
crate's public API:

- **`rustls_pemfile::Error`** — not a `std::error::Error` (no `Display`,
  no `Error` impl); pemfile's BufRead APIs return `io::Error`. Wrapped
  as `CertLoad(io::Error)`. See §"Gotchas" #2.
- **ACME `EventError` / `OrderError` / `CertParseError`** — these are
  stream events from the ACME state machine, not errors returned from
  `TlsServerConfig::new`. `new` spawns the state machine and returns
  immediately (ADR-082 §"`async fn new` — lifecycle semantics"); the
  state machine's errors surface via the event stream and are logged
  (the current code logs them in the spawned task, ADR-082 §"Behavior
  preservation"). They do not flow through `TlsError`. See §"ACME" below.
- **Provider init error** — `aws_lc_rs::default_provider()` is
  infallible (returns `CryptoProvider` by value). There is no
  `ProviderInit` variant because provider init cannot fail.
- **`rustls::server::ResolvesServerCert` / `client::ResolvesClientCert`
  construction** — infallible; the resolver traits have no construction
  error. `RawKeyCertResolver::new` and the client-side
  `RawKeyClientCertResolver::new` build `CertifiedKey` in-memory and
  cannot fail.
- **`FingerprintPinVerifier::new`** — infallible; stores the fingerprint
  + supported algorithms. The known-peer client path has no construction
  error.
- **The unknown-raw-key fail-closed** — this is NOT an error returned
  from `TlsClientConfig::new`. It is a `Result::Err` the *caller* gets
  when *trying to connect* to an unknown raw-key remote (the
  `WebPkiServerVerifier` fails the handshake at dial time). It is not in
  `TlsError`'s scope — `TlsError` is for config construction, not
  handshake outcomes. OQ-63's framing listed it as a "client verifier
  construction" failure mode; the research corrects this: the
  fail-closed is a handshake-time rejection, not a config-construction
  error. See §"The fail-closed distinction" below.

### 3. `TlsError` lives in `alknet-tls` (owned by the crate that produces it)

`TlsError` is defined in `alknet-tls` and is the sole TLS error surface
for the crate. It is not re-exported from `alknet-core`. Rationale:

- `alknet-core`'s `EndpointError` no longer has a `TlsConfig` variant
  after ADR-083 (the endpoint takes no TLS config; the assembly layer
  builds `TlsServerConfig` and hands pre-built transports to the
  endpoint). Core does not need to know about `TlsError`.
- `alknet-tls` is the crate that produces the errors; it owns the type.
  Re-exporting from core would invert the ownership (core re-exporting a
  type from a crate that depends on it).
- The assembly layer (hub/worker) depends on `alknet-tls` directly (it
  calls `TlsServerConfig::new` / `TlsClientConfig::new`); it gets
  `TlsError` from that dependency, not from core.

This resolves the subsidiary question in OQ-63 ("does `TlsError` live in
`alknet-tls` or is it re-exported from `alknet-core`?"): **`alknet-tls`,
owned by the crate that produces it.**

### 4. Feature gates

`QuinnWrap` is gated on the `quinn` feature (it wraps a quinn type).
`VerifierBuild` wraps a `rustls::webpki` type that is always present
when `rustls` is present — it is not feature-gated. The ACME
config-mismatch variant (`AcmeConfig`) is present regardless of the
`acme` feature, because the "ACME configured but feature not enabled"
guard must exist even when `acme` is off (the `#[cfg(not(feature =
"acme"))]` branch returns `AcmeConfig("ACME feature not enabled...")`).

The enum is `#[non_exhaustive]` so adding variants (e.g., a future
`TcpWrap` if `for_tcp_tls` ever gains a failure path — it does not today)
is not a breaking change.

### 5. ACME — errors are stream events, not `TlsError` variants

`AcmeConfig::new()` and `AcmeConfig::state()` are infallible (confirmed
in the rustls-acme 0.12.1 source). `TlsServerConfig::new`'s ACME branch
spawns the state machine and returns immediately; the state machine's
errors (`EventError` — `CertCacheLoad`, `AccountCacheLoad`, `Order`,
`NewCertParse`, etc.) arrive asynchronously through the event stream
and are logged in the spawned task (the current code's pattern, preserved
by ADR-082 §"Behavior-preservation invariants"). They do not flow through
`TlsError`.

The only ACME-related `TlsError` is `AcmeConfig(String)` — the
configuration-mismatch guard ("ACME feature not enabled but
`TlsIdentity::Acme` configured" on the server; "ACME identity is
server-only; cannot be used for client auth" on the client). This is a
synchronous config error detected at `new` time, not an ACME-protocol
error.

If a future requirement wants to surface ACME state-machine errors to
the caller (rather than log them), that is a new ADR — it changes the
`TlsServerConfig` API (the ACME handle would need an error channel) and
is out of scope for the error-shape decision.

### 6. The fail-closed distinction

OQ-63's framing listed "unknown-remote fail-closed (not an error to
return — it's a `Result::Err` the caller gets for trying to connect to
an unknown raw-key remote)" as a client verifier construction failure
mode. The research corrects this: the fail-closed is **not** a
`TlsClientConfig::new` error. It is a handshake-time rejection produced
by the verifier at dial time, not a config-construction error. The
distinction:

- `TlsClientConfig::new` builds the config. If the *inputs* are bad
  (empty root store → `VerifierBuilderError`, bad fingerprint format →
  not currently a construction error because `FingerprintPinVerifier`
  stores the string as-is and rejects at handshake), `new` returns
  `TlsError`.
- The *handshake* (dial time) can fail with `rustls::Error` (the
  verifier returns `InvalidCertificate(...)`). That error flows through
  the transport's connector (`quinn::Endpoint::connect_with` →
  `quinn::Connection` error; `TlsConnector::connect` → `io::Error`), not
  through `TlsError`.

`TlsError` is the **config-construction** error type. Handshake-time
errors are the transport's error type. This keeps `TlsError` scoped to
what `new` and `for_quinn` can actually fail on, and avoids pretending
handshake outcomes are config-construction errors. A future ADR that
introduces a dial helper (the OQ-55 dial seam) would define how
handshake errors are surfaced then; that is not this ADR.

## Consequences

**Positive:**

- The assembly layer can match on `TlsError` to distinguish failure
  categories — "cert file missing" (`CertLoad`) from "quinn rejected the
  rustls config" (`QuinnWrap`) from "empty CA root store"
  (`VerifierBuild`) from "self-signed generation failed" (`SelfSigned`).
  The current `io::Error` / `String` funneling destroys this context; the
  enum restores it.
- The four distinct underlying error types (`rustls::Error`,
  `NoInitialCipherSuite`, `VerifierBuilderError`, `rcgen::Error`) are
  represented as separate variants instead of erased into one —
  correctly reflecting that they come from different crates, have
  different remediations, and (in the `NoInitialCipherSuite` case) are
  not even the same type as `rustls::Error`.
- `#[from]` conversions make the implementation terse (`?` propagates
  `io::Error`, `rustls::Error`, `rcgen::Error`,
  `VerifierBuilderError`, `NoInitialCipherSuite` directly) while keeping
  the category explicit at the variant level.
- `TlsError` lives in `alknet-tls`, owned by the crate that produces it;
  core's `EndpointError` is unaffected (and no longer has a `TlsConfig`
  variant after ADR-083 anyway).
- `#[non_exhaustive]` lets future variants be added without breaking
  downstream matches.
- The ACME boundary is clear: `TlsError::AcmeConfig` is the
  config-mismatch guard; ACME state-machine errors are stream events,
  logged, not `TlsError` variants.

**Negative:**

- Six variants is more code than a thin `rustls::Error` wrapper. The
  trade is matchable context vs. minimal code; for a crate whose entire
  purpose is shared TLS config across multiple assembly-layer call sites,
  the context wins. The assembly layer is the consumer that benefits
  from the distinction.
- The client-side `TlsError` coverage is slightly asymmetrical with the
  server side: the server has `QuinnWrap` (quinn-specific) and
  `SelfSigned` (rcgen); the client has `VerifierBuild` (webpki). This
  reflects the actual asymmetry — the server wraps for quinn and
  generates self-signed certs; the client builds a CA verifier. The
  shared variants (`CertLoad`, `Rustls`, `AcmeConfig`) cover both. The
  asymmetry is the real shape, not a missing piece.
- `rustls::Error`'s `std::error::Error` impl is empty (no `source()`),
  so the source chain stops at the variant. The variant name carries the
  category; the `rustls::Error`'s `Display` carries the detail. This is
  the same as the current code's behavior (the detail is in the
  `io::Error::other(e)` string); the enum adds the category on top.
- `NoInitialCipherSuite` has a private field — it cannot be constructed
  externally (only received from quinn). This is fine for `#[from]`
  (we receive it, we don't construct it), but it means tests cannot
  synthesize a `QuinnWrap` variant without a real quinn rejection. The
  `for_quinn` failure path is tested via integration, not unit.

## Door type

**One-way.** `TlsError` is the public API surface of `alknet-tls`. Every
assembly-layer call site (`TlsServerConfig::new`, `TlsClientConfig::new`,
`for_quinn`) returns it. Changing the variant set after consumers exist
is a breaking change to every call site's `match`. `#[non_exhaustive]`
makes *adding* variants non-breaking, but *removing* or *renaming*
variants is a breaking change. The one-way-ness is why this ADR exists
before implementation, not after.

The internal implementation (whether `CertLoad` uses `#[from] io::Error`
or an explicit `io::Error` field; whether `AcmeConfig` is a `String` or
a structured enum) is two-way — those are implementation details that
can change without breaking the variant names.

## References

- OQ-63 (resolved by this ADR) — `TlsError` shape
- [ADR-082](082-alknet-tls-extraction.md) — `TlsServerConfig` extraction
  (introduces `TlsError` in signatures)
- [ADR-083](083-endpoint-as-accept-loop-runner.md) — endpoint takes no
  TLS config; `EndpointError::TlsConfig` is removed, making `TlsError`
  the sole TLS error surface
- [ADR-084](084-aws-lc-rs-crypto-provider.md) — `aws_lc_rs` provider
  (infallible; no `ProviderInit` variant)
- [ADR-087](087-tlsclientconfig-not-blocked-on-dial.md) —
  `TlsClientConfig` (extends `TlsError` to client-side variants)
- [ADR-034](034-outgoing-only-x509-and-three-peer-roles.md) §3 — verifier
  selection rule (known peer → fingerprint pin; unknown X.509 → CA
  verify; unknown raw-key → fail closed at handshake, not at `new`)
- `crates/alknet-core/src/endpoint.rs` — the current server-side
  `io::Error`-wrapping pattern (`TlsConfig(io::Error::other(e))`)
- `crates/alknet-call/src/client/call_client.rs` — the current
  client-side `String`-wrapping pattern (`e.to_string()`)
- Dependency-crate sources (read from the cargo cache):
  rustls 0.23.41 (`src/error.rs`, `src/builder.rs`, `src/server/builder.rs`,
  `src/client/builder.rs`, `src/webpki/server_verifier.rs`,
  `src/webpki/anchors.rs`), rustls-pemfile 2.2.0 (`src/lib.rs`,
  `src/pemfile.rs`), rcgen 0.13.2 (`src/error.rs`, `src/certificate.rs`,
  `src/key.rs`), quinn-proto 0.11.15 (`src/crypto/rustls.rs` —
  `NoInitialCipherSuite`, `TryFrom<rustls::ServerConfig>`), rustls-acme
  0.12.1 (`src/state.rs`, `src/acme.rs` — `EventError`, `OrderError`,
  infallible `AcmeConfig::new`/`state`), rustls-pki-types 1.14.1
  (`src/lib.rs`, `src/pem.rs`)