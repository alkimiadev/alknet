# OQ-29: CallClient TLS Client-Auth and Remote-Identity Verification

- **Origin**: [client-and-adapters.md](crates/call/client-and-adapters.md), ADR-017 §7
- **Status**: **resolved** (2026-06-27 by ADR-030 §6 + this decision)
- **Door type**: One-way (identity model interaction), two-way (mechanism)
- **Priority**: ~~high~~ → resolved
- **Resolution**: **Three things are decided:**

  1. **Wire quinn client-auth.** The client presents its Ed25519 key as an
     RFC 7250 raw public key client cert (the client-side equivalent of
     the server's `RawKeyCertResolver`). The server's
     `AcceptAnyCertVerifier` already requests client certs and extracts
     the fingerprint — the gap was entirely on the client side
     (`with_no_client_auth()` → present the key). This activates the
     `PeerEntry` fingerprint → `peer_id` resolution path on quinn
     connections.

  2. **Key-type-aware server cert verification.** The client's
     `ServerCertVerifier` depends on the remote's identity type:
     - **Ed25519 raw key** (the common case): accept the cert, extract the
       fingerprint, match against `PeerEntry.fingerprints`. The fingerprint
       IS the trust anchor — no CA needed. (Same model as iroh.)
     - **X.509** (domain-facing endpoints, ACME): verify against a CA
       (rustls's `WebPkiServerVerifier` with the platform root store or a
       configured CA). `AcceptAnyServerCertVerifier` is a security hole for
       X.509 — it's only safe for raw keys.
     - The verifier choice is driven by `CallCredentials.remote_identity`,
       which carries the expected key type.

  3. **Fingerprint normalization** (ADR-030 §6): the quinn path extracts
     the raw Ed25519 public key from the SPKI cert and formats it as
     `ed25519:<hex>`, matching iroh. The same key has the same fingerprint
     regardless of transport. X.509 fingerprints stay as `SHA256:<hex of
     DER>`.

  **The iroh path already works** — iroh uses RFC 7250 raw keys, both
  sides automatically exchange Ed25519 public keys during the TLS
  handshake, and `extract_iroh_client_fingerprint` already gets the
  `NodeId`. No client-auth wiring needed for iroh (direct or relay). The
  gap was quinn-only.

  **What's genuinely additive** (not blocking the ADR-029 migration):
  remote-identity verification (the client verifying the server's
  fingerprint against an expected value) is additive — the server-side
  fingerprint extraction is what matters for `PeerId`, not the client-side
  verification. The verifier for raw keys can start as "accept any, extract
  fingerprint" and add fingerprint-pinning later.

  See ADR-030 §6 for the fingerprint normalization details.
- **Cross-references**: ADR-014, ADR-017, ADR-027, ADR-029, ADR-030,
  [client-and-adapters.md](crates/call/client-and-adapters.md),
  [endpoint.md](crates/core/endpoint.md), [auth.md](crates/core/auth.md)
