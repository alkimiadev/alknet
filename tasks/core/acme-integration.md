---
id: core/acme-integration
name: Add ACME auto-provisioning via rustls-acme (ADR-027)
status: pending
depends_on: [core/rawkey-decouple-from-iroh]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Implement ACME auto-provisioning (Let's Encrypt) for alknet endpoints,
following ADR-027. Adds `TlsIdentity::Acme`, a new `acme` feature gate,
a two-phase server-config construction (`TlsSetup`), and a
`dispatch_quinn` guard for `acme-tls/1` challenge connections.

The reverse-proxy project (`/workspace/@alkdev/reverse-proxy/src/tls/`)
demonstrates the proven pattern: `AcmeConfig`, `AcmeState` event loop,
`ResolvesServerCertAcme`, TLS-ALPN-01 challenge handling, DirCache for
cert persistence. This task adapts that pattern to alknet's quinn-based
endpoint.

### Implementation steps

1. **Add `acme` feature to alknet-core `Cargo.toml`:**

   ```toml
   [features]
   acme = ["dep:rustls-acme"]

   [dependencies]
   rustls-acme = { version = "0.12", optional = true, features = ["aws-lc-rs"] }
   ```

   Use the same version as reverse-proxy (`=0.12.1` or compatible).
   Confirm the exact version against the latest available and the
   reverse-proxy's `Cargo.toml`.

2. **Add `TlsIdentity::Acme` variant and supporting types** in
   `config.rs`:

   ```rust
   pub enum TlsIdentity {
       X509 { cert: PathBuf, key: PathBuf },
       RawKey(Ed25519SecretKey),
       SelfSigned,
       Acme {
           domains: Vec<String>,
           cache_dir: PathBuf,
           directory: AcmeDirectory,
           contact: Vec<String>,
       },
   }

   pub enum AcmeDirectory {
       Production,
       Staging,
       Custom(String),
   }
   ```

   `Acme` holds only static, `Clone`/`Debug`-safe data. No `AcmeState`.

3. **Introduce `TlsSetup`** in `endpoint.rs` — the two-phase
   construction (ADR-027 Decision 2):

   ```rust
   struct TlsSetup {
       server_config: rustls::ServerConfig,
       acme_state_handle: Option<tokio::task::JoinHandle<()>>,
   }

   impl TlsSetup {
       async fn new(
           tls_identity: &TlsIdentity,
           alpns: &[Vec<u8>],
       ) -> Result<Self, EndpointError> {
           match tls_identity {
               TlsIdentity::X509 { .. } | TlsIdentity::SelfSigned | TlsIdentity::RawKey(_) => {
                   // synchronous path (current build_rustls_server_config)
                   let config = build_rustls_server_config(tls_identity, alpns)?;
                   Ok(Self { server_config: config, acme_state_handle: None })
               }
               TlsIdentity::Acme { domains, cache_dir, directory, contact } => {
                   #[cfg(feature = "acme")]
                   { Self::new_acme(domains, cache_dir, directory, contact, alpns).await }
                   #[cfg(not(feature = "acme"))]
                   { Err(EndpointError::TlsConfig(io::Error::other("ACME feature not enabled"))) }
               }
           }
       }
   }
   ```

4. **Implement `TlsSetup::new_acme`** (`#[cfg(feature = "acme")]`):
   - Build `AcmeConfig::new(domains)` with `DirCache::new(cache_dir)`,
         directory URL (from `AcmeDirectory`), and contact.
   - Get `state = acme_config.state()` and `resolver = state.resolver()`.
   - Build `rustls::ServerConfig` with
         `with_cert_resolver(resolver)` (NOT `with_single_cert`).
   - Append `b"acme-tls/1"` to `alpn_protocols` alongside handler ALPNs.
   - Spawn the `AcmeState` event loop as a tokio task (pattern from
         `reverse-proxy/src/tls/acme.rs:spawn_acme_state`). Log
         `DeployedCachedCert`, `DeployedNewCert`, and error events.
   - Return `TlsSetup { server_config, acme_state_handle: Some(handle) }`.

5. **Wire `TlsSetup` into the endpoint construction**: replace the
   direct `build_quinn_server_config` call in the accept loop setup with
   `TlsSetup::new(...).await?`. The `acme_state_handle` is stored on
   `AlknetEndpoint` (or the accept loop context) so it can be aborted on
   shutdown.

6. **Add `acme-tls/1` guard in `dispatch_quinn`** (ADR-027 Decision 5):

   ```rust
   if alpn == b"acme-tls/1" {
       debug!("acme-tls/1 challenge connection completed at TLS layer; closing");
       connection.close(0u32.into(), b"acme done");
       return;
   }
   ```

   Place this before the `handlers.get(&alpn)` lookup. This is
   `#[cfg(feature = "acme")]` — without the feature, the guard is
   absent and `acme-tls/1` is never in the ALPN list.

7. **Shutdown**: abort the `acme_state_handle` JoinHandle in
   `AlknetEndpoint::shutdown()` alongside the existing shutdown logic.

### ACME challenge handling (from research)

The `ResolvesServerCertAcme` resolver intercepts TLS-ALPN-01 challenges
at the cert resolution step — during the TLS handshake, before the
connection surfaces to the application. The challenge cert (with the
SHA-256 key authorization in its SAN) is served by the resolver; the CA
validates it during the handshake. By the time `dispatch_quinn` runs,
the challenge already succeeded. The `acme-tls/1` guard just closes the
connection gracefully instead of logging a misleading "no handler"
warning.

Key constraint: ACME requires `with_cert_resolver`, not
`with_single_cert`. The `acme-tls/1` ALPN must be in
`alpn_protocols` or the challenge handshake aborts with
`no_application_protocol`.

### What NOT to change

- `TlsIdentity::X509`, `RawKey`, `SelfSigned` construction paths —
  unchanged (the RawKey decoupling is done by the predecessor task).
- iroh endpoint — ACME is quinn-only (iroh uses its own TLS).
- `endpoint-request-client-cert` — independent task, can proceed in
  parallel.

## Acceptance Criteria

- [ ] `acme` feature added to alknet-core with `rustls-acme` as optional dep
- [ ] `TlsIdentity::Acme` variant exists with `domains`, `cache_dir`, `directory`, `contact`
- [ ] `AcmeDirectory` enum exists (Production, Staging, Custom)
- [ ] `TlsSetup` two-phase construction: synchronous for X509/RawKey/SelfSigned, async for Acme
- [ ] ACME path uses `with_cert_resolver(ResolvesServerCertAcme)`, not `with_single_cert`
- [ ] `acme-tls/1` added to `alpn_protocols` when ACME is configured
- [ ] `dispatch_quinn` has `acme-tls/1` guard (closes silently, no "no handler" warning)
- [ ] ACME state machine spawned as tokio task, aborted on endpoint shutdown
- [ ] `TlsIdentity::Acme` without `acme` feature returns a clear error at endpoint construction
- [ ] Unit test: `AcmeDirectory` resolves to correct Let's Encrypt URLs (staging vs production)
- [ ] Unit test: `TlsSetup::new` with `X509`/`RawKey`/`SelfSigned` returns `acme_state_handle: None`
- [ ] `cargo build -p alknet-core --features quinn` (no acme) succeeds — no rustls-acme compiled
- [ ] `cargo build -p alknet-core --features "quinn acme"` succeeds
- [ ] `cargo test -p alknet-core --all-features` succeeds
- [ ] `cargo clippy -p alknet-core --all-features --all-targets` clean
- [ ] `cargo clippy -p alknet-core --features quinn --all-targets` clean (no acme, no warnings)

## References

- ADR-027 — full design (two-phase construction, challenge handling, feature gate)
- /workspace/@alkdev/reverse-proxy/src/tls/acme.rs — `AcmeTlsConfig`, `spawn_acme_state` (proven pattern)
- /workspace/@alkdev/reverse-proxy/src/tls/acceptor.rs — `build_acme_server_config`, `acme-tls/1` ALPN
- crates/alknet-core/src/endpoint.rs:286-314 — `dispatch_quinn` (guard insertion site)
- crates/alknet-core/src/endpoint.rs:464-509 — `build_rustls_server_config` (TlsSetup replaces this for Acme)
- crates/alknet-core/src/config.rs:33-41 — `TlsIdentity` enum (new Acme variant)

## Notes

> Depends on `core/rawkey-decouple-from-iroh` because both modify
> `TlsIdentity` and `build_rustls_server_config`. The decoupling task
> cleans up the enum shape first; this task adds the Acme variant on top.
> The `acme` feature gate is critical — it keeps `rustls-acme` and its
> deps out of non-ACME builds. The reverse-proxy project is the reference
> implementation; adapt its event loop logging and cache patterns.