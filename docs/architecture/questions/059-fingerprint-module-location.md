# OQ-59: Should `fingerprint.rs` Stay in `alknet-core` or Move to `alknet-tls`?

- **Origin**: `docs/architecture/crates/tls/README.md` (the crate spec),
  `docs/architecture/decisions/082-alknet-tls-extraction.md` (the ADR).
- **Status**: resolved
- **Door type**: two-way (moving a module between crates is a refactor,
  not a wire-format change; the function signatures and types are
  unchanged)
- **Priority**: medium
- **Resolution**: **Option A — `fingerprint.rs` stays in `alknet-core`.**

  The fingerprint functions are used by two paths with different dep
  profiles:

  - **Server path** (`alknet-core::endpoint` → `alknet-tls`): extracts the
    fingerprint from the presented client cert for `PeerEntry` resolution.
    This path already depends on `alknet-tls` — no new dep edge.
  - **Client path** (`alknet-call::FingerprintPinVerifier`): matches the
    server's presented cert against a pinned fingerprint. This path does
    NOT want to depend on `alknet-tls` — it's a client-only deployment
    that needs fingerprint matching, not TLS setup. Moving
    `fingerprint.rs` to `alknet-tls` would force `alknet-call` to depend
    on `alknet-tls`, pulling `rustls` + `tokio` + (optionally)
    `quinn`/`tokio-rustls`/`rustls-acme` into every client-only deployment.
    That's heavy and doesn't serve the use case.

  The `rustls` dep in core is narrow: the production code in
  `fingerprint.rs` uses only `sha2` + `hex` + manual DER parsing. The
  `rustls::sign::public_key_to_spki` and `rustls::pki_types::alg_id::ED25519`
  usage is in the **test helper only** (`build_ed25519_spki_der`), not
  production. Core's production dep tree doesn't need `rustls` for
  fingerprint — only the test does. The `rustls` and `rustls-pki-types`
  deps are listed directly on `alknet-core` for this test helper (and for
  `Connection`'s quinn/iroh integration types), but the fingerprint
  production path is `rustls`-free.

  Keeping `fingerprint.rs` in core serves both the server path (which
  depends on `alknet-tls` anyway) and the client path (which must not
  depend on `alknet-tls`) without forcing a heavy dep edge on the client.
  `alknet-tls` re-exports the fingerprint functions for convenience.

  This is a two-way door — moving the module later is a refactor, not a
  wire-format change. But the rationale is clear: the client path's
  fingerprint-matching need doesn't justify pulling TLS setup infra.
- **Cross-references**: ADR-082 (alknet-tls extraction), ADR-003 (crate
  decomposition — handler dep rules), ADR-030 §6 (fingerprint
  normalization), `crates/alknet-core/src/fingerprint.rs` (the code),
  `crates/alknet-call/src/client/call_client.rs` (`FingerprintPinVerifier`
  — the client-side consumer)