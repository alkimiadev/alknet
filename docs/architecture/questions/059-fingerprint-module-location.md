# OQ-59: Should `fingerprint.rs` Stay in `alknet-core` or Move to `alknet-tls`?

- **Origin**: `docs/architecture/crates/tls/README.md` (the crate spec),
  `docs/architecture/decisions/082-alknet-tls-extraction.md` (the ADR).
- **Status**: open
- **Door type**: two-way (moving a module between crates is a refactor,
  not a wire-format change; the function signatures and types are
  unchanged)
- **Priority**: medium
- **Blocked on**: nothing structural — the question is a dependency-edge
  trade-off, not a missing capability. The existing code works in either
  location. The decision is about which dep edge is cleaner.
- **Resolution**: Not yet decided. The trade-off:

  **Option A: `fingerprint.rs` stays in `alknet-core`.**
  - `alknet-core` keeps a narrow `rustls` dep (uses
    `rustls::pki_types::CertificateDer` and `rustls::sign::public_key_to_spki`
    in tests; the production code uses only `sha2` and manual DER parsing,
    but the test helper `build_ed25519_spki_der` uses `rustls::sign`).
  - `alknet-call`'s client-side `FingerprintPinVerifier` depends on
    `alknet-core` (existing edge, no new dep).
  - `alknet-tls` depends on `alknet-core` and re-exports the fingerprint
    functions for convenience.
  - Core is not `rustls`-free, but the dep is narrow (pki-types + sign
    types, not the full TLS stack).
  - This is the lower-friction option — no dep edges change.

  **Option B: `fingerprint.rs` moves to `alknet-tls`.**
  - `alknet-core` becomes `rustls`-free (loses the `rustls`,
    `rustls-pki-types` deps for fingerprint).
  - `alknet-call`'s `FingerprintPinVerifier` gains a dep on `alknet-tls`
    — a new dep edge from a handler crate to a transport-infra crate.
    This may or may not violate ADR-003's "no handler-depends-on-handler"
    rule (alknet-tls is not a handler, but it is transport infra that
    handler crates didn't previously depend on).
  - The dep edge is the main concern: `alknet-call` depending on
    `alknet-tls` means every call-protocol consumer transitively pulls
    `rustls` + `tokio` + (optionally) `quinn`/`tokio-rustls`/`rustls-acme`.
    That's heavy for a client-only deployment that just needs
    fingerprint matching, not TLS setup.

  **Likely resolution**: Option A (stay in core). The `rustls` dep in
  core is narrow (type usage, not the TLS stack), and moving
  `fingerprint.rs` to `alknet-tls` creates a heavy dep edge from
  `alknet-call` to `alknet-tls` that doesn't serve the client-only case.
  The fingerprint functions are used by both the server path (endpoint,
  which does depend on `alknet-tls`) and the client path (call-client,
  which does not want to depend on `alknet-tls`). Keeping them in core
  serves both paths without forcing the client path to pull TLS infra.
- **What does NOT block on this**: the `alknet-tls` extraction itself
  (ADR-082). The crate can be created with `fingerprint.rs` staying in
  core, and the question can be resolved later without blocking the
  extraction.
- **Cross-references**: ADR-082 (alknet-tls extraction), ADR-003 (crate
  decomposition — handler dep rules), ADR-030 §6 (fingerprint
  normalization), `crates/alknet-core/src/fingerprint.rs` (the code).