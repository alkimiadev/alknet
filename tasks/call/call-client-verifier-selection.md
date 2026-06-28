---
id: call/call-client-verifier-selection
name: Wire CallClient TLS client-auth and server cert verifier selection by PeerEntry presence (OQ-29, ADR-034)
status: pending
depends_on: [call/peer-composite-env]
scope: moderate
risk: high
impact: component
level: implementation
---

## Description

Wire the `CallClient` TLS client-auth (present Ed25519 key as RFC 7250 raw
public key client cert) and the server cert verifier selection by `PeerEntry`
presence. Per OQ-29 (resolved) and ADR-034 §2-3. This is the most
security-critical call-side change — TLS wiring and verifier selection.

### Current state

```rust
// crates/alknet-call/src/client/call_client.rs
fn build_quinn_client_config(_credentials: &CallCredentials, alpn: &[u8])
    -> Result<quinn::ClientConfig, String>
{
    let config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyServerCertVerifier))  // ← accepts ANY
        .with_no_client_auth();  // ← doesn't present client cert
    // ...
}
```

`AcceptAnyServerCertVerifier` is a security hole for X.509 (accepts any cert
without CA verification). `with_no_client_auth()` doesn't present the client's
Ed25519 key, so the server has no client cert to extract a fingerprint from —
the ADR-030 `PeerEntry` fingerprint → `peer_id` resolution path is not
activated for quinn connections.

### Target state (OQ-29 + ADR-034)

#### 1. TLS client-auth: present Ed25519 key as raw public key client cert

Replace `with_no_client_auth()` with presenting the client's Ed25519 key as an
RFC 7250 raw public key client cert. This is the client-side equivalent of the
server's `RawKeyCertResolver`. The `CallCredentials.tls_identity` carries the
`TlsIdentity::RawKey(Ed25519SecretKey)` (or X.509 cert pair).

```rust
fn build_quinn_client_config(credentials: &CallCredentials, alpn: &[u8])
    -> Result<quinn::ClientConfig, String>
{
    // 1. Client cert: present Ed25519 raw key (if configured)
    let client_auth = build_client_auth(&credentials.tls_identity)?;

    // 2. Server cert verifier: by PeerEntry presence (ADR-034 §3)
    let verifier = select_server_verifier(&credentials.remote_identity)?;

    let config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth(client_auth);  // ← present the key, not no_client_auth
    // ...
}
```

#### 2. Server cert verifier selection by PeerEntry presence (ADR-034 §3)

| Local has `PeerEntry` for remote? | Remote cert type | Client verifier |
|----------------------------------|------------------|-----------------|
| No (public X.509 endpoint) | X.509 | `WebPkiServerVerifier` (CA verification) |
| No | Ed25519 raw key | fails closed (no CA to fall back to) |
| Yes (hub, Ed25519 path) | Ed25519 raw key | fingerprint match (`ed25519:<hex>`) |
| Yes (hub, X.509 path) | X.509 | fingerprint match (`SHA256:<hex>`) |

`CallCredentials.remote_identity: Option<RemoteIdentity>` is load-bearing:
- `Some(fingerprint)` → known peer → fingerprint pin (the fingerprint IS the
  trust anchor).
- `None` → no `PeerEntry` for the remote → CA verification for X.509, fail
  closed for Ed25519 raw key. `None` is the public-X.509-endpoint state, not a
  missing field. An implementer must not default `remote_identity` to a
  placeholder, and must not treat `None` as "skip verification."

```rust
fn select_server_verifier(remote_identity: &Option<RemoteIdentity>)
    -> Result<Arc<dyn ServerCertVerifier>, String>
{
    match remote_identity {
        Some(ri) => {
            // Known peer → fingerprint pin
            Ok(Arc::new(FingerprintPinVerifier::new(ri.fingerprint.clone())))
        }
        None => {
            // Unknown remote → CA verification (WebPkiServerVerifier)
            // For Ed25519 raw-key remotes, this fails closed (no CA).
            // This is the public-X.509-endpoint path (ADR-034 §2).
            let roots = rustls::crypto::aws_lc_rs::default_provider().root_certificates;
            Ok(Arc::new(rustls::client::WebPkiServerVerifier::builder(roots.into()).build()?))
        }
    }
}
```

#### 3. FingerprintPinVerifier

A new `ServerCertVerifier` that pins a specific fingerprint:
- For `ed25519:<hex>` remotes: extract the raw Ed25519 pub key from the
  presented cert and match against the pinned fingerprint.
- For `SHA256:<hex>` remotes: hash the cert DER and match against the pinned
  fingerprint.
- No match → verification failure (connection rejected).

#### 4. CallCredentials

`CallCredentials` (already defined) carries the three credential dimensions:

```rust
pub struct CallCredentials {
    pub tls_identity: Option<TlsIdentity>,      // RFC 7250 raw key or X.509
    pub auth_token: Option<AuthToken>,           // call-protocol-level token
    pub remote_identity: Option<RemoteIdentity>, // expected fingerprint (None = CA path)
}

pub struct RemoteIdentity { pub fingerprint: String }
```

`remote_identity: None` is load-bearing — the public-X.509-endpoint state
(ADR-034 §2). The implementation must not default it to a placeholder.

### What this task does NOT do

- Does NOT change the server-side endpoint (`AcceptAnyCertVerifier` in
  alknet-core is unchanged — it's "request-but-don't-require" for fingerprint
  extraction).
- Does NOT build `PeerCompositeEnv` (that's `call/peer-composite-env`, a
  dependency) — but a connection with no resolved identity (no `PeerEntry`) gets
  no `PeerId` and is not added to `PeerCompositeEnv` (that's handled in
  `call/peer-composite-env` / `call/dispatch-peer-identity`).

## Acceptance Criteria

- [ ] `build_quinn_client_config` presents Ed25519 key as RFC 7250 raw public key client cert (replaces `with_no_client_auth()`)
- [ ] `select_server_verifier` selects verifier by `remote_identity` presence
- [ ] `Some(fingerprint)` → `FingerprintPinVerifier` (fingerprint match)
- [ ] `None` + X.509 → `WebPkiServerVerifier` (CA verification)
- [ ] `None` + Ed25519 raw key → fails closed (no CA to fall back to)
- [ ] `FingerprintPinVerifier` matches `ed25519:<hex>` (raw key extraction) and `SHA256:<hex>` (DER hash)
- [ ] `AcceptAnyServerCertVerifier` removed (security hole for X.509)
- [ ] `CallCredentials.remote_identity: None` is load-bearing (not defaulted to placeholder)
- [ ] No-env-vars invariant preserved (credentials from Capabilities, not env vars)
- [ ] Unit test: `FingerprintPinVerifier` matches correct fingerprint
- [ ] Unit test: `FingerprintPinVerifier` rejects wrong fingerprint
- [ ] Unit test: `select_server_verifier` returns CA verifier for `None`
- [ ] Unit test: client auth presents Ed25519 key (config built without error)
- [ ] `cargo test -p alknet-call` succeeds
- [ ] `cargo clippy -p alknet-call` succeeds with no warnings

## References

- docs/architecture/crates/call/client-and-adapters.md — CallCredentials, verifier selection, TLS client-auth
- docs/architecture/crates/core/auth.md — Client-side verifier selection table
- docs/architecture/decisions/034-outgoing-only-x509-and-three-peer-roles.md — ADR-034 §2-3
- docs/architecture/decisions/030-peerentry-and-identity-id-decoupling.md — ADR-030 §6 (fingerprint normalization)

## Notes

> Most security-critical call-side change. `AcceptAnyServerCertVerifier` is a
> security hole for X.509 — replaced by verifier selection by `PeerEntry`
> presence. `None` + X.509 = CA verification (public X.509 endpoint); `None` +
> Ed25519 = fail closed (raw-key remotes are always known peers). `Some` =
> fingerprint pin (known peer). The client presents its Ed25519 key as a raw
> public key client cert so the server can extract the fingerprint — this
> activates the PeerEntry fingerprint → peer_id resolution path on quinn.

## Summary

> To be filled on completion