# OQ-21: Remote Vault Administration

- **Origin**: [service.md](crates/vault/service.md), [protocol.md](crates/vault/protocol.md), ADR-019
- **Status**: resolved
- **Door type**: One-way (vault crate is local-only by construction)
- **Priority**: medium
- **Resolution**: Remote vault access is **not a feature of the vault crate**. ADR-025 dropped irpc from the vault, making the vault local-only by construction — no `RemoteService` trait, no wire format for vault messages, no default-insecure remote handler. The vault's API is `VaultServiceHandle` (direct method calls), nothing else.

  If remote vault access is ever needed (e.g., the machine→worker pattern), it requires a **separate vault-server crate** that depends on both alknet-core (for `IdentityProvider`, scopes, auth-wrapping) and alknet-vault (for `VaultServiceHandle`). That crate would define its own threat model, access policy, operation filtering (Unlock/Lock local-only), and wire format — and requires its own ADR. This is a deliberate addition, not a flag flip on a default that was already loaded.

  The pre-ADR-025 deferral framed remote access as "non-breaking" (the wire format was additive). That framing was misleading: once workers build dependencies on the remote vault API, disabling it breaks them — the door is operationally one-way even if the wire format is additive. ADR-025 inverts the default: the vault is local-only by construction, and remote access requires building something new, not removing a default.

  Per-node vaults are the recommended pattern for multi-node deployments: each node has its own vault and mnemonic; credentials are encrypted *for* the receiving node's public key, not decrypted centrally. This is end-to-end encryption between nodes, matching ADR-008's "capability source" model.
- **Cross-references**: ADR-005, ADR-008, ADR-014, ADR-018, ADR-019, ADR-025, [protocol.md](crates/vault/protocol.md), [service.md](crates/vault/service.md)
