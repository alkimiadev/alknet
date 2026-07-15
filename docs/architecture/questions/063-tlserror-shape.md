# OQ-63: `TlsError` Shape

- **Origin**: `docs/architecture/crates/tls/README.md`
  (`TlsError` is referenced as the `Result` error type in
  `TlsServerConfig::new`, `TlsClientConfig::new`, `for_quinn()`, and the
  crate's public signatures, but is never sketched or defined); ADR-082
  (same — `TlsError` in signatures, no shape); ADR-087 (extends the
  surface to `TlsClientConfig::new` — the client-side error variants).
- **Status**: open
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
- **Resolution**: Not yet decided. The shape needs to cover the failure
  modes across all server identity paths **and** the client verifier
  paths (ADR-087):

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

  The open question is the granularity: a single `TlsError` enum with
  variants per failure category (cert-load, rustls-build, quinn-wrap,
  acme-disabled) vs a thin wrapper around `rustls::Error` /
  `io::Error`. The single-enum shape gives callers matchable context
  (the assembly layer can distinguish "cert file missing" from "quinn
  rejected the rustls config"); the thin wrapper is less code but
  loses the distinction. This is an API-surface decision — it needs an
  ADR or at minimum a sketch in the TLS README before implementation.

  Subsidiary question: does `TlsError` live in `alknet-tls` (owned by
  the crate that produces it) or is it re-exported from `alknet-core`?
  Likely `alknet-tls` (it's the crate's own error), but worth
  confirming so `alknet-core`'s `EndpointError` (which no longer has a
  `TlsConfig` variant after ADR-083) doesn't need to know about it.
- **Cross-references**: ADR-082 (the extraction that introduces
  `TlsError`), ADR-083 (the endpoint refactor that removes
  `EndpointError::TlsConfig`, making `TlsError` the sole TLS error
  surface), `crates/alknet-core/src/endpoint.rs` (the current
  `io::Error`-wrapping pattern the new type replaces)