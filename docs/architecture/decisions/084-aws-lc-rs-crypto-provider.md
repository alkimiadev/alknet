# ADR-084: aws-lc-rs as the TLS Crypto Provider

## Status

Accepted

## Context

The TLS stack in alknet uses `rustls`, which requires a crypto provider.
Rustls 0.23 made the crypto provider explicit — it is no longer a
process-global default that gets set once and forgotten. Each
`ServerConfig` / `ClientConfig` is built with a specific provider via
`builder_with_provider(Arc<dyn CryptoProvider>)`.

The current code (`alknet-core/endpoint.rs`, ADR-027, and the
`alknet-tls` extraction in ADR-082) uses
`rustls::crypto::aws_lc_rs::default_provider()` on all server config
paths (X509, RawKey, SelfSigned, ACME). This choice was made during
ADR-027's implementation to match iroh's `tls-aws-lc-rs` feature, but
was never recorded as a decision. ADR-082's behavior-preservation
invariants list says "do not switch to `ring` or the process-default
provider without an ADR" — this ADR is that record.

### Why this is load-bearing

The crypto provider determines:

- **FIPS status**: `aws-lc-rs` has a FIPS-certified build mode (via
  AWS-LC). `ring` does not. If alknet ever needs FIPS compliance (e.g.,
  for regulated deployments), `aws-lc-rs` is the path; `ring` would be a
  dead end.
- **Platform support**: `aws-lc-rs` supports a broad set of platforms
  via its C/C++ build. `ring` has a different (historically narrower)
  platform matrix. Switching providers changes which platforms compile.
- **Cipher suite defaults**: `default_provider()` returns a provider
  with a specific set of cipher suites and signature algorithms.
  `AcceptAnyCertVerifier`'s `supported_verify_schemes()` (ED25519 +
  ECDSA P-256/P-384 + RSA PSS/PKCS1) must be supported by the provider —
  `aws_lc_rs` supports all of these; a provider that dropped one would
  silently change which client cert signature algorithms the server
  accepts.
- **iroh compatibility**: iroh uses `aws-lc-rs` via its
  `tls-aws-lc-rs` feature. If alknet's quinn/TCP+TLS path used a
  different provider, the same Ed25519 key would produce TLS handshakes
  with different crypto internals on the quinn path vs the iroh path —
  a consistency risk for the fingerprint normalization (ADR-030 §6).

### The options

- **`aws_lc_rs::default_provider()`** (current): FIPS-capable, broad
  platform support, matches iroh. The choice already in the code.
- **`ring::default_provider()`**: simpler pure-Rust crate, no FIPS,
  different platform matrix. Would break iroh provider consistency.
- **Process-default provider** (`rustls::crypto::CryptoProvider::get_default()`):
  relies on whoever set the process global first. Non-deterministic in a
  library context — alknet crates shouldn't depend on the binary having
  set the right global. Explicit per-config is the library-correct
  pattern (and is what the current code does).

## Decision

**`rustls::crypto::aws_lc_rs::default_provider()` is alknet's TLS crypto
provider on all server and client config paths.** This applies to
`alknet-tls` (the `TlsServerConfig` construction paths: X509, RawKey,
SelfSigned, ACME) and to `alknet-call`'s client-side verifier
construction (which builds a `rustls::ClientConfig` with the same
provider for consistency).

The provider is constructed explicitly per config
(`Arc::new(rustls::crypto::aws_lc_rs::default_provider())` passed to
`builder_with_provider`), not via the process-default global. This
keeps alknet crates correct as libraries — they don't depend on the
binary having set a global.

### What this means for `alknet-tls`

The `TlsServerConfig::new` paths (X509, RawKey, SelfSigned, ACME) all
construct their `rustls::ServerConfig` with
`builder_with_provider(Arc::new(aws_lc_rs::default_provider()))`. This
is already the case in the current code (ADR-082's
behavior-preservation invariants record this); this ADR makes the
*decision* explicit so that a future switch requires a new ADR.

### What this means for `alknet-call`

The client-side `rustls::ClientConfig` (used by `CallClient` for
outgoing connections, with either `FingerprintPinVerifier` or
`WebPkiServerVerifier`) must use the same provider. This ensures the
client and server agree on cipher suites and signature algorithms — a
mismatch could cause handshake failures or silent signature-algorithm
changes.

## Consequences

**Positive:**
- One crypto provider across all TLS paths (quinn, TCP+TLS, iroh's
  built-in TLS, client-side). Consistent cipher suites, signature
  algorithms, and FIPS status.
- FIPS-capable: if a regulated deployment needs FIPS, `aws-lc-rs`'s
  FIPS build mode is available without an ADR (it's a build-flag, not a
  provider change).
- Matches iroh's `tls-aws-lc-rs` feature — no provider mismatch between
  alknet's quinn/TCP+TLS path and iroh's built-in TLS path.

**Negative:**
- `aws-lc-rs` is a C/C++ crate (built via `aws-lc`), not pure Rust. This
  adds a C build dependency. `ring` is also C-backed, so this is not a
  regression vs the alternative — but a pure-Rust provider (e.g., a
  future `rustls` provider) would be lighter. Not switching now; the
  FIPS and iroh-consistency benefits outweigh the build complexity.
- A future switch to a different provider requires a new ADR (this is
  already stated in ADR-082's invariants; this ADR makes the *current*
  choice a recorded decision, not just an invariant).

## Door type

**One-way.** The provider choice is baked into every TLS config path.
Switching providers after consumers exist requires updating every config
construction site and verifying cipher-suite + signature-algorithm
compatibility. The FIPS and iroh-consistency constraints make `ring`
and the process-default provider non-viable — the decision is
`aws_lc_rs` unless a future ADR records a different choice with
rationale.

## References

- ADR-027: TLS Identity Redesign (where `aws_lc_rs` was first used, to
  match iroh's `tls-aws-lc-rs` feature)
- ADR-082: alknet-tls extraction (behavior-preservation invariants:
  `aws_lc_rs::default_provider()` on all paths; "do not switch to `ring`
  or the process-default provider without an ADR")
- ADR-030 §6: fingerprint normalization (requires provider consistency
  across quinn/iroh/TCP+TLS — the same Ed25519 key must produce the same
  fingerprint regardless of transport)
- `crates/alknet-core/src/endpoint.rs` — the current code using
  `aws_lc_rs::default_provider()` on all server config paths
- `crates/alknet-call/src/client/call_client.rs` — the client-side
  `rustls::ClientConfig` construction (must use the same provider)