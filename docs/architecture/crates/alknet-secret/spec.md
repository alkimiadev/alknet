---
status: planned
last_updated: 2026-06-15
---

# alknet-secret

> **Status: Planned** — This spec has not been written yet. The crate is already implemented and stable.

## Purpose

Standalone crate for BIP39 mnemonic generation, SLIP-0010 Ed25519 HD key derivation, AES-256-GCM encryption, and the `SecretProtocol` irpc service. Does not depend on alknet-core.

## Key Questions

- **OQ-08**: Secret service integration point — irpc service, ALPN handler, or embedded library?

## References

- [overview.md](../../overview.md)
- ADR-003: Crate decomposition (alknet-secret is standalone)