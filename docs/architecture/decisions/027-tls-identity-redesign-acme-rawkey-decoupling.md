# ADR-027: TLS Identity Redesign — ACME Integration + RawKey Decoupling

## Status

Accepted

## Context

OQ-12 marked "resolved" identified two TLS identity use cases: RFC 7250
raw Ed25519 keys (default, P2P) and X.509 certs (domain-hosted, browsers).
ACME auto-provisioning was described as "additive — it will be adapted
when domain-hosted nodes need it." That deferral created two
architectural issues that surface now that ACME is a concrete target.

### Issue 1: `TlsIdentity` cannot represent ACME

`TlsIdentity` is `#[derive(Debug, Clone)]` and lives in `StaticConfig` —
a static, synchronous config value. ACME requires:

- A long-lived async state machine (`AcmeState` event loop, spawned for
  the endpoint's lifetime) that handles ordering, challenge response,
  cert renewal, and cache I/O.
- TLS-ALPN-01 challenge handling: `acme-tls/1` must be in the server's
  `alpn_protocols`, and a `ResolvesServerCertAcme` must serve challenge
  certs during the TLS handshake.
- Config fields: domains, cache directory, ACME directory URL, contact
  email.

`AcmeState` is not `Clone`. It cannot be a `TlsIdentity` variant. The
current `build_rustls_server_config(&TlsIdentity) -> ServerConfig` is
synchronous — there's no room for spawning an async state machine or
holding a runtime resolver handle. The reverse-proxy project solved this
with a two-phase construction: static config → `TlsMode` (runtime
objects) → `ServerConfig`. alknet needs the same split.

### Issue 2: `RawKey` is coupled to the `iroh` feature

`TlsIdentity::RawKey(iroh::SecretKey)` is gated `#[cfg(feature = "iroh")]`.
The `RawKeyCertResolver` and `Ed25519SigningKey` impls are gated
`#[cfg(all(feature = "quinn", feature = "iroh"))]`. This means a
quinn-only build (the default feature set) **cannot use RFC 7250 raw-key
identity** — the very mode described as "default for most alknet nodes."

The coupling is artificial. `iroh::SecretKey` is a thin newtype over
`ed25519_dalek::SigningKey` (`pub struct SecretKey(SigningKey)`). The
alknet code uses exactly three APIs: `.public().as_bytes()`, `.sign(msg)`,
and `.clone()`. None of these are iroh-specific. The raw-key TLS path
needs Ed25519 signing + SPKI encoding — both available from
`ed25519-dalek` + `rustls` without iroh.

The iroh *transport* (`build_iroh_endpoint`) does need `iroh::SecretKey`
for `iroh::Endpoint::builder().secret_key(...)`. If `TlsIdentity::RawKey`
no longer carries an `iroh::SecretKey`, the iroh transport must convert
from the new key type — trivial since `iroh::SecretKey::from_bytes(&[u8;
32])` accepts raw Ed25519 key bytes.

### ACME challenge handling with quinn (QUIC, not TCP)

Research confirmed how TLS-ALPN-01 works with quinn:

- The `ResolvesServerCertAcme` resolver intercepts the challenge at the
  **cert resolution step**, during the TLS handshake, before the
  handshake result is surfaced to the application.
- When an ACME CA connects with ALPN `[acme-tls/1]`, rustls calls the
  resolver, which returns the challenge cert. The handshake completes.
  The CA inspects the cert's SAN and validates the challenge — no
  application-layer data exchange needed.
- quinn's `connecting.await` then returns a completed `Connection` with
  ALPN `acme-tls/1`. alknet's `dispatch_quinn` would find no handler for
  that ALPN and close the connection. **The challenge already succeeded**
  — the close is cosmetic.
- Unlike the reverse-proxy (TCP + `LazyConfigAcceptor`), quinn gives no
  "peek at ClientHello" hook. The challenge is fully TLS-layer-handled;
  the application only needs to close challenge connections gracefully
  (silent close, not a "no handler" warning).

Key constraint: ACME requires `with_cert_resolver(ResolvesServerCertAcme)`,
not `with_single_cert`. You cannot just append `acme-tls/1` to an
`X509`/`SelfSigned` config — there'd be no resolver to serve the
challenge cert. ACME is a distinct `ServerConfig` construction path.

## Decision

### 1. Add `TlsIdentity::Acme` variant (static config data only)

```rust
pub enum TlsIdentity {
    X509 { cert: PathBuf, key: PathBuf },
    RawKey(Ed25519SecretKey),      // see Decision 3
    SelfSigned,
    Acme {                         // NEW
        domains: Vec<String>,
        cache_dir: PathBuf,
        directory: AcmeDirectory,  // enum: Production, Staging, Custom(String)
        contact: Vec<String>,      // e.g. ["mailto:admin@example.com"]
    },
}
```

`Acme` holds only static, `Clone`/`Debug`-safe config data. No
`AcmeState`, no resolver, no runtime objects. The async state machine is
constructed at endpoint setup time (Decision 2).

### 2. Split server-config construction into two phases

Replace the synchronous `build_rustls_server_config(&TlsIdentity) ->
ServerConfig` with a two-phase construction:

**Phase 1 — `TlsSetup` (async, at endpoint construction):**

```rust
struct TlsSetup {
    server_config: rustls::ServerConfig,
    acme_state: Option<AcmeStateHandle>,  // spawned task + handle for shutdown
}
```

For `X509`, `SelfSigned`, `RawKey`: construct `ServerConfig`
synchronously (current path, unchanged). `acme_state` is `None`.

For `Acme`: construct `AcmeConfig`, spawn the `AcmeState` event loop,
get `ResolvesServerCertAcme`, build `ServerConfig` with
`with_cert_resolver(resolver)`, add `acme-tls/1` to `alpn_protocols`.
`acme_state` is `Some(handle)` so the endpoint can abort the ACME task
on shutdown.

**Phase 2 — use `TlsSetup.server_config` to build `quinn::ServerConfig`:**

Same as today: `QuicServerConfig::try_from(rustls_config)` →
`quinn::ServerConfig::with_crypto(...)`.

The `TlsSetup` is constructed inside `AlknetEndpoint::new()` (or
`run_quinn_accept_loop`), not inside `TlsIdentity`. The `TlsIdentity`
enum stays a pure data structure.

### 3. Decouple `RawKey` from iroh — use `ed25519-dalek` directly

Replace `TlsIdentity::RawKey(iroh::SecretKey)` with
`TlsIdentity::RawKey(Ed25519SecretKey)`, where `Ed25519SecretKey` is a
thin alknet-core-owned wrapper over `ed25519_dalek::SigningKey`:

```rust
pub struct Ed25519SecretKey(ed25519_dalek::SigningKey);
```

This type is `Clone`, `Debug` (redacting), `Zeroize`, and not gated
behind any feature flag. `ed25519-dalek` becomes a direct dependency of
alknet-core (it's already in the dependency tree transitively via iroh).

The `RawKeyCertResolver` and `Ed25519SigningKey` rustls impls move from
`#[cfg(all(feature = "quinn", feature = "iroh"))]` to
`#[cfg(feature = "quinn")]` — raw-key TLS identity works in quinn-only
builds.

The `iroh` feature gate on `TlsIdentity::RawKey` is removed. The
variant is always available.

### 4. iroh transport converts from `Ed25519SecretKey`

`build_iroh_endpoint` currently reads `TlsIdentity::RawKey(iroh::SecretKey)`
and passes it to `iroh::Endpoint::builder().secret_key(...)`. After
decoupling, it converts:

```rust
if let Some(TlsIdentity::RawKey(key)) = static_config.tls_identity.as_ref() {
    let iroh_key = iroh::SecretKey::from_bytes(key.as_bytes());
    builder = builder.secret_key(iroh_key);
}
```

`iroh::SecretKey::from_bytes(&[u8; 32])` accepts raw Ed25519 key bytes —
no information loss. This conversion is `#[cfg(feature = "iroh")]` only.

### 5. ACME ALPN challenge handling in `dispatch_quinn`

Add an early-return guard in `dispatch_quinn` before the handler lookup:

```rust
if alpn == b"acme-tls/1" {
    debug!("acme-tls/1 challenge connection completed at TLS layer; closing");
    connection.close(0u32.into(), b"acme done");
    return;
}
```

This avoids the misleading "no handler for ALPN" warning. The challenge
is already answered at the TLS layer; the application just closes
gracefully. No `ProtocolHandler` registration for `acme-tls/1`.

### 6. Feature-gate ACME behind a new `acme` feature

Add a `acme` feature to alknet-core:

```toml
[features]
acme = ["dep:rustls-acme"]
```

`TlsIdentity::Acme` is available regardless of feature (it's just config
data), but constructing `TlsSetup` with an `Acme` variant requires the
`acme` feature. Without it, `TlsIdentity::Acme` at endpoint construction
returns an error ("ACME feature not enabled"). This keeps the
footprint down for nodes that don't need ACME — `rustls-acme` and its
dependencies are only compiled when the feature is on.

### 7. `acme-tls/1` in ALPN list only when ACME is active

When `TlsIdentity::Acme` is configured, `acme-tls/1` is appended to the
`alpn_protocols` list alongside the handler ALPNs. When ACME is not
configured, `acme-tls/1` is not advertised — no behavior change for
non-ACME nodes.

## Consequences

- **Breaking change to `TlsIdentity`**: `RawKey(iroh::SecretKey)` →
  `RawKey(Ed25519SecretKey)`. Pre-1.0 crate, in-repo consumers only.
  The assembly layer and tests that construct `TlsIdentity::RawKey` must
  update.
- **`ed25519-dalek` becomes a direct dependency** of alknet-core. It's
  already in the dependency tree (transitive via iroh), so no new
  compilation cost for `iroh` builds. Quinn-only builds that were not
  using `RawKey` before will now compile `ed25519-dalek` — it's a small,
  pure-Rust crate with no C dependencies.
- **`rustls-acme` is feature-gated** (`acme` feature). Nodes not using
  ACME don't compile it. The feature is compatible with `quinn` (ACME
  is quinn-only; iroh uses its own TLS).
- **`build_rustls_server_config` becomes async** (or is replaced by an
  async `TlsSetup::new`). The accept loop already runs in an async
  context, so this is a local change.
- **ACME state machine lifecycle**: the `AcmeState` task is spawned in
  `AlknetEndpoint::new()` and aborted on shutdown. The `TlsSetup` struct
  carries the `JoinHandle` so `AlknetEndpoint::shutdown()` can abort it.
- **No handler needed for `acme-tls/1`**: the `dispatch_quinn` guard
  handles it. `HandlerRegistry` is not involved.

## Alternatives Considered

### A. ACME as a `ResolvesServerCert` wrapper behind `X509`

OQ-12 suggested ACME "fits naturally as an additional `TlsIdentity`
variant or as a `rustls::ResolvesServerCert` implementation behind the
existing `X509` path." The second option — wrapping `X509` — was
rejected because ACME needs async state + config fields (domains, cache,
contact) that don't fit behind the static `X509 { cert, key }` variant.
A `ResolvesServerCert` that internally does ACME would need to be
constructed at config time with those fields, which means `X509` would
need to carry them — bloating the variant for non-ACME users. A
dedicated `Acme` variant is cleaner.

### B. Keep `RawKey` coupled to iroh, only add ACME

Rejected because the coupling is the root cause of quinn-only builds not
supporting the "default" identity mode. Fixing only ACME would leave the
artificial iroh dependency in place. Since both changes touch
`TlsIdentity` and `build_rustls_server_config`, doing them together
avoids two breaking changes to the same enum.

### C. Use `iroh::SecretKey` for both, re-export from alknet-core

Rejected because it would make `iroh` a non-optional dependency of
alknet-core, defeating the feature-gated transport design (ADR-010).
`ed25519-dalek` is a lightweight, pure-Rust crate; `iroh` is not.

### D. Register a no-op `ProtocolHandler` for `acme-tls/1`

Rejected because it would require the handler registry to know about
ACME (a TLS-layer concern), polluting the ALPN dispatch abstraction.
The `dispatch_quinn` guard is a one-line check that keeps ACME handling
in the endpoint layer where it belongs.

## Cross-References

- OQ-12 (TLS identity provisioning) — updated by this ADR
- [ADR-010](010-alpn-router-and-endpoint.md) — multi-connectivity endpoint, feature-gated transports
- [ADR-004](004-auth-as-shared-core.md) — auth as shared core
- `docs/architecture/crates/core/endpoint.md` — TLS identity use cases, updated
- `docs/architecture/crates/core/config.md` — `TlsIdentity` enum, updated
- `/workspace/@alkdev/reverse-proxy/src/tls/` — proven ACME implementation pattern
- `rustls-acme` crate — ACME state machine + cert resolver