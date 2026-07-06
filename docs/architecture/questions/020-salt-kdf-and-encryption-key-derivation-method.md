# OQ-20: Salt/KDF and Encryption Key Derivation Method

- **Origin**: [encryption.md](crates/vault/encryption.md)
- **Status**: resolved
- **Door type**: One-way (key derivation method), two-way (salt field usage)
- **Priority**: high
- **Resolution**: The vault uses SLIP-0010 HD derivation from the BIP39 seed at path `m/74'/2'/0'/0'` to produce the AES-256-GCM encryption key — not PBKDF2. The `salt` field in `EncryptedData` is unused for key derivation (kept for wire-format compatibility with the TS predecessor). The TypeScript `@alkdev/storage` crypto module used PBKDF2 with a password + salt; data encrypted by that method (key_version=1) cannot be decrypted by the vault and must be migrated via one-time re-encryption to key_version=2. See ADR-020 for the full rationale and migration path.
- **Cross-references**: ADR-020, [encryption.md](crates/vault/encryption.md)
