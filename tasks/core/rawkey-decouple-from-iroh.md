---
id: core/rawkey-decouple-from-iroh
name: Decouple TlsIdentity::RawKey from the iroh feature (ADR-027)
status: completed
depends_on: []
scope: narrow
risk: medium
impact: component
level: implementation
---

## Description

`TlsIdentity::RawKey(iroh::SecretKey)` is gated `#[cfg(feature = "iroh")]`
and the `RawKeyCertResolver` / `Ed25519SigningKey` rustls impls are gated
`#[cfg(all(feature = "quinn", feature = "iroh"))]`. This means quinn-only
builds (the default feature set) cannot use RFC 7250 raw-key identity —
the mode described as "default for most alknet nodes" (OQ-12, ADR-027).

The coupling is artificial: `iroh::SecretKey` is a thin newtype over
`ed25519_dalek::SigningKey`. The alknet code uses only `.public().as_bytes()`,
`.sign(msg)`, and `.clone()`. This task replaces `iroh::SecretKey` with an
alknet-core-owned `Ed25519SecretKey` wrapper, un-gates the raw-key TLS
path from the `iroh` feature, and updates the iroh transport to convert.

See ADR-027 for the full design rationale.

### Implementation steps

1. **Add `ed25519-dalek` as a direct dependency** of alknet-core in
   `Cargo.toml`. It's already in the lockfile (transitive via iroh).
   Version: `2.2` (match what's in `Cargo.lock`).

2. **Introduce `Ed25519SecretKey`** in `config.rs` (or a new
   `tls.rs` module if config.rs is getting large):

   ```rust
   #[derive(Clone)]
   pub struct Ed25519SecretKey(ed25519_dalek::SigningKey);

   impl Ed25519SecretKey {
       pub fn generate() -> Self { ... }
       pub fn from_bytes(bytes: &[u8; 32]) -> Self { ... }
       pub fn as_bytes(&self) -> &[u8; 32] { ... }
       pub fn public(&self) -> ed25519_dalek::VerifyingKey { ... }
   }
   ```

   Add `ZeroizeOnDrop` (the key is secret material). Add a redacting
   `Debug` impl (like `Secret<T>` in types.rs). Do NOT derive `Debug` —
   the raw key bytes must not be printed.

3. **Change `TlsIdentity::RawKey`** from `RawKey(iroh::SecretKey)` to
   `RawKey(Ed25519SecretKey)`. Remove the `#[cfg(feature = "iroh")]` gate
   — `RawKey` is available in all builds.

4. **Rewire `Ed25519SigningKey`** in `endpoint.rs`:
   - Change the inner field from `iroh::SecretKey` to `Ed25519SecretKey`
     (or `ed25519_dalek::SigningKey`).
   - `spki_public_key()`: use `self.key.public().as_bytes()` (same logic,
     different key type — `ed25519_dalek::VerifyingKey` has `as_bytes()`).
   - `sign()`: use `self.key.sign(message)` → ed25519-dalek's
     `SigningKey::sign` returns `Signature` which has `to_bytes()`.
   - Change the cfg gate from `#[cfg(all(feature = "quinn", feature =
     "iroh"))]` to `#[cfg(feature = "quinn")]` on `RawKeyCertResolver`,
     `Ed25519SigningKey`, and all related impls.

5. **Update `build_iroh_endpoint`**: when `TlsIdentity::RawKey(key)` is
   present, convert to `iroh::SecretKey::from_bytes(key.as_bytes())`
   before passing to `iroh::Endpoint::builder().secret_key(...)`. This
   conversion is `#[cfg(feature = "iroh")]` only.

6. **Update `build_rustls_server_config`**: the `RawKey` arm changes
   from `#[cfg(feature = "iroh")]` to always-available (within the
   `#[cfg(feature = "quinn")]` function). The `RawKeyCertResolver::new`
   takes `&Ed25519SecretKey` instead of `&iroh::SecretKey`.

7. **Update all tests** that construct `TlsIdentity::RawKey`:
   - `endpoint.rs` tests: `iroh::SecretKey::generate(&mut csprng)` →
     `Ed25519SecretKey::generate()`.
   - Any test in `config.rs` that constructs `RawKey`.

### What NOT to change

- `TlsIdentity::X509` and `SelfSigned` — untouched by this task.
- The `endpoint-request-client-cert` task (server config client auth) —
  independent, can proceed in parallel or before/after this task.
- ACME — separate follow-up task (`core/acme-integration`).

## Acceptance Criteria

- [ ] `ed25519-dalek` is a direct dependency of alknet-core
- [ ] `Ed25519SecretKey` type exists with `generate`, `from_bytes`, `as_bytes`, `public`; redacting `Debug`; `ZeroizeOnDrop`
- [ ] `TlsIdentity::RawKey` uses `Ed25519SecretKey`, not `iroh::SecretKey`
- [ ] `TlsIdentity::RawKey` is not gated behind `#[cfg(feature = "iroh")]`
- [ ] `RawKeyCertResolver` and `Ed25519SigningKey` are gated `#[cfg(feature = "quinn")]` only (not `all(feature = "quinn", feature = "iroh")`)
- [ ] `build_iroh_endpoint` converts `Ed25519SecretKey` → `iroh::SecretKey::from_bytes`
- [ ] `cargo build -p alknet-core --features quinn` (no iroh) succeeds with `TlsIdentity::RawKey` usable
- [ ] `cargo build -p alknet-core --all-features` succeeds
- [ ] `cargo test -p alknet-core --all-features` succeeds
- [ ] `cargo test -p alknet-core --features quinn` succeeds (quinn-only, no iroh)
- [ ] `cargo clippy -p alknet-core --all-features --all-targets` clean
- [ ] `cargo clippy -p alknet-core --features quinn --all-targets` clean

## References

- ADR-027 — full design rationale
- crates/alknet-core/src/config.rs:33-41 — `TlsIdentity` enum
- crates/alknet-core/src/endpoint.rs:593-689 — `RawKeyCertResolver`, `Ed25519SigningKey`
- crates/alknet-core/src/endpoint.rs:511-538 — `build_iroh_endpoint` (conversion site)
- crates/alknet-core/src/endpoint.rs:484-495 — `build_rustls_server_config` RawKey arm
- /workspace/iroh/iroh-base/src/key.rs:261 — `iroh::SecretKey(SigningKey)` newtype

## Notes

> This is the foundation task for ADR-027. The ACME task
> (`core/acme-integration`) depends on this one because both modify
> `TlsIdentity` and `build_rustls_server_config`. Doing decoupling first
> means the ACME task builds on the cleaned-up enum without iroh coupling.