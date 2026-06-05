# ADR-023: Unified Authentication with Shared Key Material

## Status
Accepted

## Context

Alknet currently authenticates connections exclusively through SSH public key
auth in the SSH handshake. This works for SSH-over-any-transport (TCP, TLS,
iroh) because SSH carries its own auth protocol. But WebTransport and other
HTTP-level transports cannot perform SSH key exchange — browsers speak HTTP/3,
not SSH.

Without unification, non-SSH transports would need a completely separate
identity system (API keys, JWTs, session tokens). This creates two problems:
(1) operators manage two key sets with two rotation mechanisms, and (2) the
same person connecting via SSH and WebTransport appears as two different
identities.

The `IdentityProvider` trait is needed to decouple alknet-core from any
specific identity storage (config file vs. database). Without it, alknet-core
would either hardcode config-file-based auth or take a database dependency —
neither is acceptable for a library crate.

## Decision

**Unified authentication**: The same Ed25519 key material (`authorized_keys`
and `cert_authorities`) is shared across both SSH auth and token auth. The
presentation differs per transport, but the verification result (an
`Identity` with scopes) is the same.

**Token auth for non-SSH transports**: WebTransport clients present a signed
timestamp token in the CONNECT request URL:

```
AuthToken = base64url(key_id || timestamp || signature)
  key_id    = SHA-256 fingerprint of the Ed25519 public key (32 bytes)
  timestamp = Unix seconds, big-endian u64 (8 bytes)
  signature = Ed25519 sign(key_id || timestamp_bytes, private_key)
```

Server extracts the fingerprint, looks it up in the same `authorized_keys`
set, verifies the signature, and checks the timestamp window (default ±300s).

**`IdentityProvider` trait**: Decouples alknet-core from identity storage. The
trait resolves a fingerprint or token to an `Identity`. Default implementation
loads from `DynamicConfig.auth` (no database). Hub implementation can back it
with `@alkdev/storage`.

**`TokenKeySource::Shared`**: The token auth uses the same authorized keys set
as SSH auth by default. Deployments that want separate access control can use
`TokenKeySource::Separate` with a distinct key set.

**Replay protection via timestamps**: V1 uses timestamp-only (no server state).
Zero-replay can be added later via a nonce challenge-response without changing
the key material.

## Consequences

- **Positive**: One key set, one rotation, one `reloadAuth()` call. Adding a
  key to `authorized_keys` immediately grants access via both SSH and
  WebTransport.
- **Positive**: `IdentityProvider` trait makes alknet-core independent of any
  specific database. Default: config file. Hub: `@alkdev/storage`.
- **Positive**: Browser clients can authenticate using Ed25519 keys via
  SubtleCrypto (Chrome 105+, Firefox 130+, Safari 17+). Deno supports it
  natively.
- **Positive**: No JWT library dependency. The token is a simple Ed25519
  signature over a fixed structure — same primitives SSH already uses.
- **Negative**: V1 has a replay window (±300s). An attacker who intercepts a
  QUIC packet can replay the token within the window. Acceptable because QUIC
  interception is the same threat level as connection hijacking.
- **Negative**: Certificate authority tokens are not supported in v1. CA
  verification requires the full OpenSSH certificate structure, which doesn't
  fit in a signed timestamp.
- **Negative**: Browser-side key management is less ergonomic than SSH key
  files. The private key must be imported into SubtleCrypto. This is a UI/UX
  concern, not a protocol concern.

## References

- [auth.md](../auth.md) — Full auth architecture spec
- [ADR-012](012-auth-ed25519-and-cert-authority.md) — Ed25519 + cert-authority auth
- [OQ-17](../open-questions.md) — Transport-aware auth (resolved by this ADR)
- [configuration.md](../../research/configuration.md) — OQ-CFG-04, OQ-CFG-06 (resolved)