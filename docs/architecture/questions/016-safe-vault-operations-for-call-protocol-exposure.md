# OQ-16: Safe Vault Operations for Call Protocol Exposure

- **Origin**: [operation-registry.md](crates/call/operation-registry.md), ADR-008
- **Status**: resolved
- **Door type**: One-way
- **Priority**: high
- **Resolution**: No vault operations are exposed over the call protocol. The vault is accessed only at the assembly layer (CLI binary at startup). Handlers receive secret material through `OperationContext.capabilities`, not by calling vault operations over the wire. The `operation-registry.md` spec previously showed `vault/derive`, `vault/unlock`, and `vault/decrypt` registered as call protocol operations — that was a contradiction with ADR-008's "capability source" model and has been corrected. See ADR-014.
- **Cross-references**: ADR-008, ADR-014, [operation-registry.md](crates/call/operation-registry.md)
