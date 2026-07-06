# OQ-08: Vault Integration Point

- **Origin**: [overview.md](overview.md)
- **Status**: resolved
- **Door type**: One-way
- **Priority**: medium
- **Resolution**: CLI-embedded, assembly-layer only. The CLI binary instantiates `VaultServiceHandle` locally at startup, derives and decrypts the credentials each handler needs, and injects them into handler capabilities. alknet-vault has no ALPN, no alknet-core dependency, and no operations registered in the call protocol. The master seed and derived private keys never cross the network. The vault is a capability source, not a network service. See ADR-008 and ADR-014.
- **Cross-references**: ADR-003, ADR-005, ADR-008, ADR-014
