---
id: core/fingerprint-normalization
name: Normalize quinn Ed25519 raw-key fingerprint to ed25519:hex format (ADR-030 §6)
status: pending
depends_on: [core/peer-entry-model]
scope: narrow
risk: medium
impact: component
level: implementation
---

## Description

Normalize the quinn Ed25519 raw-key fingerprint extraction to produce
`ed25519:<hex of 32-byte pub key>`, matching the iroh path. Currently
`fingerprint_from_cert_der` produces `SHA256:<hex of cert DER>` for ALL certs,
including RFC 7250 raw public keys. ADR-030 §6 requires that Ed25519 raw keys
produce `ed25519:<hex>` regardless of transport (quinn or iroh), so the same
key has the same fingerprint in `PeerEntry.fingerprints` — one entry, both
transports.

### Current state

```rust
// crates/alknet-core/src/endpoint.rs
fn extract_quinn_client_fingerprint(connection: &quinn::Connection) -> Option<String> {
    let identity = connection.peer_identity()?;
    let cert = identity.iter().next()?;
    fingerprint_from_cert_der(cert.as_ref())
}

fn fingerprint_from_cert_der(cert_der: &[u8]) -> Option<String> {
    // Always SHA256:<hex of DER> — wrong for Ed25519 raw keys
    let mut hasher = Sha256::new();
    hasher.update(cert_der);
    Some(format!("SHA256:{}", hex::encode(hasher.finalize())))
}

fn extract_iroh_client_fingerprint(connection: &iroh::endpoint::Connection) -> Option<String> {
    let node_id = connection.remote_node_id().ok()?;
    Some(format!("ed25519:{}", node_id))  // ← already correct
}
```

### Target state (ADR-030 §6)

`fingerprint_from_cert_der` (or a new `fingerprint_from_client_cert` function)
must distinguish:

1. **RFC 7250 raw public key cert** (SPKI with Ed25519 algorithm identifier):
   extract the raw 32-byte Ed25519 public key from the SPKI DER and format as
   `ed25519:<lowercase hex of 32 bytes>`. This matches the iroh path — the same
   key has the same fingerprint regardless of transport.

2. **X.509 cert**: keep `SHA256:<hex of cert DER>` (the DER hash — X.509 certs
   don't have a "raw public key" form).

The distinction is whether the presented cert is an RFC 7250 raw public key
(SPKI with Ed25519 algorithm identifier, no X.509 wrapper) or a full X.509
cert. The `RawKeyCertResolver` on the server side already has the raw key bytes
via `Ed25519SecretKey::public()`; the client-side extraction must parse the
SPKI DER to extract the raw key.

### Fingerprint format table (ADR-030 §6)

| Transport | Source | Format |
|-----------|--------|--------|
| iroh (direct or relay) | peer `NodeId` (Ed25519 public key) | `ed25519:<lowercase hex of 32-byte pub key>` |
| quinn (RFC 7250 raw key) | SPKI cert → extract raw Ed25519 pub key | `ed25519:<lowercase hex of 32-byte pub key>` (normalized) |
| quinn (X.509) | leaf client cert DER | `SHA256:<hex of SHA-256(cert_der)>` |

### Implementation approach

Parse the cert DER to detect whether it's a raw public key (SPKI) or an X.509
cert. If SPKI with Ed25519 algorithm identifier, extract the 32-byte public key
and format as `ed25519:<hex>`. Otherwise, hash the full DER as `SHA256:<hex>`.

The `rustls-pki-types` crate (already a dependency) provides
`CertificateDer`. The `rustls` crate's webpki or a manual DER parse of the
SPKI's `SubjectPublicKeyInfo` → `subjectPublicKey` field can extract the raw
key. A minimal DER parser for the SPKI structure (AlgorithmIdentifier +
subjectPublicKey) is sufficient — the structure is small and well-defined.

### Test migration

The existing endpoint.rs tests expect `SHA256:` for all fingerprints. Tests
with Ed25519 raw keys must migrate to expect `ed25519:`. Tests with X.509 certs
stay `SHA256:`. Add a test that the same Ed25519 key produces the same
fingerprint via both the quinn SPKI-extraction path and the iroh NodeId path
(if testable without a live iroh connection, test the format function directly).

## Acceptance Criteria

- [ ] `fingerprint_from_cert_der` (or replacement) distinguishes RFC 7250 raw key SPKI from X.509 cert
- [ ] Ed25519 raw key (SPKI) → `ed25519:<lowercase hex of 32-byte pub key>`
- [ ] X.509 cert → `SHA256:<hex of SHA-256(cert_der)>` (unchanged)
- [ ] iroh path already produces `ed25519:<hex>` (unchanged — verify)
- [ ] Same Ed25519 key produces same fingerprint via quinn and iroh paths
- [ ] No-client-cert case still produces `tls_client_fingerprint: None` (no regression)
- [ ] Unit test: Ed25519 raw key SPKI → `ed25519:<hex>` format
- [ ] Unit test: X.509 cert → `SHA256:<hex>` format (unchanged)
- [ ] Unit test: fingerprint is lowercase hex
- [ ] Unit test: 32-byte pub key extracted correctly (not the DER wrapper)
- [ ] Existing endpoint.rs fingerprint tests migrated (Ed25519 → `ed25519:`, X.509 → `SHA256:`)
- [ ] `cargo test -p alknet-core` succeeds
- [ ] `cargo clippy -p alknet-core` succeeds with no warnings

## References

- docs/architecture/crates/core/auth.md — Fingerprint string format table
- docs/architecture/decisions/030-peerentry-and-identity-id-decoupling.md — ADR-030 §6 (normalization rationale)
- docs/architecture/decisions/027-tls-identity-redesign-acme-rawkey-decoupling.md — ADR-027 (RawKey model)

## Notes

> The normalization is load-bearing for the peer graph: a peer that connects
> via quinn direct and via iroh must have the same fingerprint in
> `PeerEntry.fingerprints` — one entry, both transports. Without this, the same
> key produces `ed25519:abc...` on iroh and `SHA256:def...` on quinn, breaking
> the ADR-030 resolution path. The X.509 path stays `SHA256:<hex of DER>`
> because X.509 certs don't have a "raw public key" form. This also simplifies
> the coming WebTransport relay work (proxied Ed25519 identity is the same
> `ed25519:<hex>` whether direct or proxied).

## Summary

> To be filled on completion