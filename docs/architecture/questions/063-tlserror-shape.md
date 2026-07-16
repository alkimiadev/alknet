# OQ-63: `TlsError` Shape

- **Origin**: `docs/architecture/crates/tls/README.md`
  (`TlsError` is referenced as the `Result` error type in
  `TlsServerConfig::new`, `TlsClientConfig::new`, `for_quinn()`, and the
  crate's public signatures, but is never sketched or defined); ADR-082
  (same — `TlsError` in signatures, no shape); ADR-087 (extends the
  surface to `TlsClientConfig::new` — the client-side error variants).
- **Status**: resolved (ADR-088)
- **Door type**: one-way (the error type is the public API surface of
  `alknet-tls`; changing it after consumers exist is a breaking change
  to every assembly-layer call site)
- **Priority**: high (an implementer cannot write the crate without
  deciding this; guessing produces divergent shapes — one thin
  `rustls::Error` wrapper vs a 10-variant enum with per-path context)
- **Impacts**: Blocks `alknet-tls` implementation — both
  `TlsServerConfig::new` and `TlsClientConfig::new` reference `TlsError`
  in their signatures. Blocks the hub's assembly-layer wiring (which
  calls both). This is the next decision needed before the TLS crate
  can be implemented.
- **Resolution**: **Decided (ADR-088).** `TlsError` is a single
  `#[non_exhaustive]` enum with one variant per failure category, owned
  by `alknet-tls` (not re-exported from core). The variants, grounded in
  the actual error-producing call sites and the dependency-crate sources
  (rustls 0.23.41, rustls-pemfile 2.2.0, rcgen 0.13.2, quinn-proto
  0.11.15, rustls-acme 0.12.1):

  - **Cert/key loading** (`X509`: file read + PEM parse;
    `SelfSigned`: rcgen generation) — currently `io::Error`-wrapped in
    the core code.
  - **rustls config construction** (`builder_with_provider` /
    `with_safe_default_protocol_versions` /
    `with_single_cert` / `with_cert_resolver`) — currently
    `rustls::Error`-wrapped.
  - **quinn wrap** (`QuicServerConfig::try_from(rustls::ServerConfig)`)
    — the one path where `for_quinn()` can fail; currently
    `io::Error::other(e)`-wrapped.
  - **ACME** — `rustls-acme` has its own error types; the ACME task
    runs in the background and surfaces errors via events (logged, not
    returned from `new`), so `new`'s ACME path may only need to cover
    "ACME feature not enabled but `TlsIdentity::Acme` configured"
    (currently an `io::ErrorKind::Unsupported`).
  - **Client verifier construction** (ADR-087) — `TlsClientConfig::new`
    builds a `rustls::ClientConfig` with ADR-034's verifier selection.
    Failure modes: verifier construction error (bad fingerprint format,
    CA store init failure), unknown-remote fail-closed (not an error to
    return — it's a `Result::Err` the caller gets for trying to connect
    to an unknown raw-key remote), provider init failure. These
    overlap with the server-side rustls-build errors but have
    client-specific context (verifier selection inputs).

  The open question was the granularity: a single `TlsError` enum with
  variants per failure category (cert-load, rustls-build, quinn-wrap,
  acme-disabled) vs a thin wrapper around `rustls::Error` /
  `io::Error`. **Resolved: the single-enum shape.** The thin wrapper is
  actively wrong here — three findings from the dependency-crate source
  drive the decision: (1) `for_quinn()` fails with
  `NoInitialCipherSuite`, a distinct type from `rustls::Error` — a thin
  `rustls::Error` wrapper cannot represent the `for_quinn()` failure;
  (2) `rustls_pemfile::Error` is not a `std::error::Error` (no
  `Display`, no `Error` impl), so pemfile's BufRead APIs return
  `io::Error` — a `#[from] rustls_pemfile::Error` would not compile;
  (3) `WebPkiServerVerifier::build()` returns `VerifierBuilderError`,
  not `rustls::Error` — a thin wrapper cannot represent "empty CA root
  store" as a first-class failure. The six variants: `CertLoad(io::Error)`,
  `SelfSigned(rcgen::Error)`, `Rustls(rustls::Error)`,
  `VerifierBuild(VerifierBuilderError)`, `QuinnWrap(NoInitialCipherSuite)`
  (quinn-gated), `AcmeConfig(String)`. ACME `EventError`/`OrderError`
  are deliberately absent — they are stream events, logged not returned
  from `new`. The unknown-raw-key fail-closed is deliberately absent —
  it is a handshake-time rejection (dial time), not a
  config-construction error (`new` time). See ADR-088 for the full
  rationale, the variant definitions, and the "what is NOT a variant"
  list.

  Subsidiary question: does `TlsError` live in `alknet-tls` (owned by
  the crate that produces it) or is it re-exported from `alknet-core`?
  **Resolved: `alknet-tls`, owned by the crate that produces it.**
  `EndpointError` is removed entirely after ADR-083 (both variants
  vestigial); core has no endpoint error type and does not need to know
  about `TlsError`. Re-exporting from core would invert the ownership
  (core re-exporting a type from a crate that depends on it).
- **Cross-references**: ADR-088 (the resolution — single enum, owned by
  `alknet-tls`, six variants), ADR-082 (the extraction that introduces
  `TlsError`), ADR-083 (the endpoint refactor that removes
  `EndpointError` entirely, making `TlsError` the sole TLS error
  surface), ADR-087 (extends the surface to client-side variants),
  `crates/alknet-core/src/endpoint.rs` (the current
  `io::Error`-wrapping pattern the new type replaces),
  `crates/alknet-call/src/client/call_client.rs` (the current
  `String`-wrapping pattern on the client side)