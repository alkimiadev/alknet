---
id: architecture/spec-secret-service
name: Create secret-service.md architecture spec
status: completed
depends_on:
  - architecture/adr-027-crate-decomposition
  - architecture/adr-032-event-boundary-discipline
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Create `docs/architecture/secret-service.md` — a new architecture spec for the `alknet-secret` crate and its `SecretProtocol` irpc service.

This slides from the research in `docs/research/services.md` (SecretProtocol definition) and `docs/research/storage.md` (secrets section, key derivation paths). The secret service is well-bounded: BIP39 mnemonics, SLIP-0010 Ed25519 HD key derivation, AES-256-GCM encryption for external credentials, and a lock/unlock lifecycle.

**Scope**: alknet-secret crate definition, not alknet-core changes.

**Key content from research**:
- SecretProtocol enum: Unlock, Lock, DeriveEd25519, DeriveEncryptionKey, DeriveEthereumKey, DerivePassword, Encrypt, Decrypt
- DerivedKey, KeyType, EncryptedData types
- Security model: locked/unlocked states, seed in RAM only, never on disk
- Derivation path constants (SLIP-0044 coin type 74')
- Event boundary: SecretService domain events (honker streams for key derivation notifications) stay internal. External consumers use irpc calls or call protocol operations that project to integration events.

## Acceptance Criteria

- [ ] `docs/architecture/secret-service.md` exists with YAML frontmatter (`status: draft`)
- [ ] Follows spec format: What, Why, Architecture, Constraints, Open Questions, Design Decisions
- [ ] Documents BIP39 mnemonic generation and seed derivation
- [ ] Documents SLIP-0010 Ed25519 HD key derivation (SLIP-0044 coin type 74')
- [ ] Documents AES-256-GCM encryption/decryption for external credentials
- [ ] Documents SecretProtocol irpc service: Unlock, Lock, DeriveEd25519, DeriveEncryptionKey, Encrypt, Decrypt variants
- [ ] Documents EncryptedData type (key_version, salt, iv, ciphertext)
- [ ] Documents derivation path constants
- [ ] Documents security model: locked/unlocked states, seed lifecycle, never persisted
- [ ] States crate dependencies: bip39, ed25519-bip32, aes-gcm, sha2, irpc
- [ ] States crate does NOT depend on alknet-core or alknet-storage
- [ ] States interface back to core: EncryptedData format referenced by alknet-storage (wire format compatibility, not crate dependency)
- [ ] Event boundary per ADR-032: honker streams internal, irpc calls internal, no direct EventEnvelope emission
- [ ] References ADR-027, ADR-032
- [ ] `docs/architecture/README.md` updated to include secret-service.md

## References

- docs/research/services.md — SecretProtocol definition, DerivedKey, KeyType, EncryptedData
- docs/research/storage.md — secrets section, key derivation paths
- docs/research/integration-plan.md — Phase 2.1 (alknet-secret)

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion