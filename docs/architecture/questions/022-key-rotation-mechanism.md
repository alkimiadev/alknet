# OQ-22: Key Rotation Mechanism

- **Origin**: [encryption.md](crates/vault/encryption.md)
- **Status**: resolved
- **Door type**: One-way (path scheme), two-way (rotation policy)
- **Priority**: medium
- **Resolution**: Key rotation uses version-indexed derivation paths. Each key version maps to a distinct SLIP-0010 path: `m/74'/2'/0'/{version-2}'`. v2 (current) is at `m/74'/2'/0'/0'`; v3 is at `m/74'/2'/0'/1'`; etc. The `decrypt` method derives the key at the path indicated by `encrypted.key_version` (not always at `PATHS::ENCRYPTION`). The `rotate` method decrypts with the old version's key and re-encrypts with the new version's key — no new mnemonic needed. The assembly layer or a migration tool iterates stored blobs and calls `rotate` on each; the vault does not self-rotate. Partial rotation is safe (old keys remain derivable). See ADR-021.
- **Cross-references**: ADR-020, ADR-021, [encryption.md](crates/vault/encryption.md), [service.md](crates/vault/service.md)
