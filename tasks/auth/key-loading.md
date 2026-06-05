---
id: auth/key-loading
name: Implement SSH key material loading (file paths and in-memory data)
status: pending
depends_on:
  - auth/error-types
  - setup/project-init
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Implement key material loading that accepts both file paths and in-memory data per the programmatic-first API (ADR-011). Key inputs (`--identity`, `--authorized-keys`, `--cert-authority`, `--key`) accept either:
- **File path**: load from filesystem
- **In-memory data**: raw key bytes provided programmatically

All keys must be in **OpenSSH key format** (not PEM/PKCS#1/PKCS#8). This module handles:
- Loading private keys (OpenSSH format: `-----BEGIN OPENSSH PRIVATE KEY-----`)
- Loading public keys (OpenSSH format: `ssh-ed25519 AAAA... user@host`)
- Loading authorized_keys files (standard OpenSSH format)
- Parsing `cert-authority` entries in authorized_keys

## Acceptance Criteria

- [ ] `crates/alknet-core/src/auth/keys.rs` exports key loading functions
- [ ] `KeySource` enum: `File(PathBuf)` and `Memory(Vec<u8>)` for unified key input handling
- [ ] `load_private_key(source: KeySource) -> Result<russh::key::KeyPair>` — loads OpenSSH private key from file or memory
- [ ] `load_public_keys(source: KeySource) -> Result<Vec<russh::key::PublicKey>>` — loads one or more public keys from authorized_keys format
- [ ] Parses standard `authorized_keys` format including options (e.g., `cert-authority,permit-port-forwarding ssh-ed25519 AAAA...`)
- [ ] `CertAuthorityEntry` struct: `public_key: PublicKey, options: Vec<String>` parsed from authorized_keys cert-authority lines
- [ ] Returns `ConfigError::KeyFileNotFound` for missing file paths
- [ ] Returns `ConfigError::InvalidFlag` with clear message for PEM-encoded (non-OpenSSH) keys
- [ ] Unit tests: load Ed25519 key from file, load from memory, parse authorized_keys with multiple entries, reject PEM format

## References

- docs/architecture/client.md — Key Material Format section
- docs/architecture/server.md — Key Material Format section
- docs/architecture/decisions/012-auth-ed25519-and-cert-authority.md — authorized_keys format with cert-authority
- docs/architecture/decisions/011-no-ssh-config-programmatic-api.md — programmatic-first, file paths or in-memory

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion