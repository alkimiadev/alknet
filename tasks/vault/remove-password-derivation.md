---
id: vault/remove-password-derivation
name: Remove derive_password and site_password_path methods (password-manager pattern not relevant)
status: completed
depends_on: [vault/irpc-removal]
scope: single
risk: trivial
impact: isolated
level: implementation
---

## Description

Fix drift item #7: the vault currently has `derive_password`,
`derive_password_string`, and `site_password_path` methods. These implement a
password-manager pattern (deriving site-specific passwords from the seed) that
is not relevant to an RPC system's vault. Remove them entirely per ADR-025
(resolves review #002 C9).

### What to remove

- `derive_password` method from `VaultServiceHandle` (in `service.rs`)
- `derive_password_string` method from `VaultServiceHandle` (in `service.rs`)
- `site_password_path` function (in `mnemonic-derivation.rs` or `derivation.rs`,
  wherever it's defined)
- Any associated path constants for password derivation
- Any tests for these methods
- Any references in `lib.rs` re-exports

### Why

The vault's purpose in alknet is to derive cryptographic keys (Ed25519 for
identity, AES-256-GCM for encryption) and encrypt/decrypt external credentials.
Site-specific password derivation is a password-manager feature that doesn't
belong in a networking toolkit's vault. Keeping it expands the attack surface
and API surface for no benefit.

### Scope

This task touches `service.rs` and possibly `derivation.rs` /
`mnemonic-derivation.rs`. It depends on the irpc removal task (drift #4) because
both modify `service.rs`.

## Acceptance Criteria

- [ ] `derive_password` method removed from `VaultServiceHandle`
- [ ] `derive_password_string` method removed from `VaultServiceHandle`
- [ ] `site_password_path` function removed
- [ ] Any password-derivation path constants removed
- [ ] Tests for password derivation removed
- [ ] No references to password derivation remain in `lib.rs` re-exports
- [ ] `cargo check` succeeds (no dangling references)
- [ ] `cargo test` succeeds
- [ ] `cargo clippy` succeeds with no warnings

## References

- docs/architecture/crates/vault/README.md — Known Source Drift table item #7
- docs/architecture/decisions/025-vault-local-only-dispatch.md — ADR-025 (resolves C9)

## Notes

> Straightforward removal. The password-manager pattern was inherited from the
> POC and is not relevant to alknet's vault use case. Depends on irpc removal
> because both modify `service.rs`.

## Summary

Removed `derive_password`, `derive_password_string` from `VaultServiceHandle`
(service.rs), `site_password_path` from derivation.rs, the doc-table row, all 5
password-derivation tests, and the now-unused `base64` URL_SAFE_NO_PAD import.
109 lines deleted. All tests pass; clippy clean. Merged to develop.