---
id: auth/client-auth-handler
name: Implement client-side SSH authentication with Ed25519 key pairs
status: pending
depends_on:
  - auth/key-loading
  - auth/error-types
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Implement the client-side SSH authentication. The client presents an Ed25519 private key during SSH handshake. This creates the `russh::client::Handler` implementation and the `russh::client::ConnectStreamConfig` that uses the loaded key.

No password auth. The client handler is simpler than the server — it just needs to provide the private key and handle the auth callback from russh.

## Acceptance Criteria

- [ ] `crates/alknet-core/src/auth/client_auth.rs` exports `ClientAuthConfig` and client handler
- [ ] `ClientAuthConfig` holds: `private_key: KeyPair`, optional `public_key: PublicKey`
- [ ] `ClientAuthConfig::from_key_source(source: KeySource) -> Result<Self>` — loads key via key-loading module
- [ ] Implements `russh::client::Handler` with `auth_publickey()` returning the public key
- [ ] Client handler returns `russh::client::AuthResult::Accept` or appropriate auth state
- [ ] Unit tests: valid key creates handler, auth flow succeeds with mock SSH session

## References

- docs/architecture/client.md — "Authentication is Ed25519 public key or OpenSSH certificate (ADR-012)"
- docs/architecture/decisions/012-auth-ed25519-and-cert-authority.md — key-based auth only

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion