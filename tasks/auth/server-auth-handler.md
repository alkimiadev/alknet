---
id: auth/server-auth-handler
name: Implement server-side authentication (Ed25519 keys + OpenSSH cert-authority)
status: completed
depends_on:
  - auth/key-loading
  - auth/error-types
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Implement the server-side SSH authentication logic per ADR-012:

1. **Ed25519 public key**: `auth_publickey()` checks presented key against the authorized set using constant-time comparison
2. **OpenSSH certificate authority**: validates presented certificate — checks CA signature, expiry, and principal restrictions (`permit-port-forwarding`, `no-pty`, `source-address`)

No password authentication over SSH. This is the `russh::server::Handler::auth_publickey()` implementation that the server handler will call.

## Acceptance Criteria

- [ ] `crates/alknet-core/src/auth/server_auth.rs` exports `ServerAuthConfig` and auth logic
- [ ] `ServerAuthConfig` holds: `authorized_keys: HashSet<PublicKey>`, `cert_authorities: Vec<CertAuthorityEntry>`
- [ ] `ServerAuthConfig::from_keys_and_ca()` constructor: loads authorized keys and cert-authority entries from provided key sources
- [ ] Auth check function: given a presented key/certificate, return `Accept` or `Reject`
- [ ] Ed25519 key matching uses constant-time comparison (via `russh`/`ssh-key` crate builtins)
- [ ] Certificate validation checks: CA signature valid, cert not expired, principal restrictions enforced
- [ ] Certificate options respected: `permit-port-forwarding`, `no-pty`, `source-address`
- [ ] Returns `AuthError::KeyRejected` or `AuthError::CertInvalid`/`CertExpired`/`CertPrincipalMismatch` on failure
- [ ] Unit tests: valid key accepted, invalid key rejected, cert-authority signed cert accepted, expired cert rejected, wrong principal rejected

## References

- docs/architecture/server.md — Authentication section
- docs/architecture/decisions/012-auth-ed25519-and-cert-authority.md — ADR for key + cert-authority
- docs/architecture/client.md — "Authentication is Ed25519 public key or OpenSSH certificate"

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion