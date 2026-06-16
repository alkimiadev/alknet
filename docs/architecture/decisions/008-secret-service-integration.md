# ADR-008: Vault Integration Point

## Status

Accepted

## Context

alknet-vault (formerly alknet-secret) is a standalone crate with zero alknet crate dependencies. It provides BIP39 mnemonic generation, SLIP-0010 Ed25519 HD key derivation, AES-256-GCM encryption, and an irpc-based `VaultProtocol` for message dispatch. It is already implemented and stable.

The question (OQ-08) was: how does the rest of the alknet system access alknet-vault's capabilities? The options were:

1. **irpc service over `alknet/call`**: Other services call vault operations through the call protocol.
2. **ALPN handler on `alknet/secret`**: alknet-vault implements ProtocolHandler and gets its own ALPN.
3. **Direct library dependency**: alknet-core or handler crates depend on alknet-vault directly, breaking its independence.
4. **CLI-embedded with call protocol exposure**: The CLI binary instantiates VaultServiceHandle locally and registers vault operations in the call protocol's registry.

This is a one-way door because if alknet-vault gets pulled into alknet-core as a dependency, its independence is permanently lost. The standalone property is valuable — alknet-vault has no QUIC, no tokio runtime requirement (the handle works without it), and no alknet crate dependencies. It can be used in contexts where QUIC networking doesn't exist (CLI tools, test harnesses, WASM key derivation).

Beyond the integration point, there's a question of access patterns. The vault holds the master seed and can derive keys and encrypt/decrypt arbitrary data. This is used for:

- Identity key derivation (SSH host keys, node identity)
- Provider API key storage (Vast.ai, LLM providers) — encrypted at rest, decrypted on demand
- Credential encryption for storage
- Multi-tenant key derivation (different derivation paths per tenant)

The vault is a capability source, not a service endpoint. Operations that need provider keys don't hold a reference to the vault — they receive the derived/decrypted material through their operation context. The vault is unlocked at startup by the CLI, and the CLI injects material into operation contexts as needed.

## Decision

**Option 4: CLI-embedded with call protocol exposure.**

The CLI binary (the `alknet` crate) is the integration point. It:

1. Instantiates `VaultServiceHandle` locally at startup (or on-demand with Unlock/Lock lifecycle).
2. Registers vault operations (DeriveEd25519, DeriveEncryptionKey, Encrypt, Decrypt, etc.) in the call protocol's operation registry.
3. Other handlers access vault capabilities by calling operations on `alknet/call` — they don't import alknet-vault directly.

**alknet-vault does NOT get its own ALPN.** Key derivation is a local operation — the master seed never crosses the network. If a remote node needs derived public keys (e.g., for identity verification), they're shared through the call protocol, not through direct vault access.

**The vault is accessed at the assembly layer, not by individual handlers.** The CLI (or a configuration middleware it sets up) is the only component that talks to the vault directly. Derived keys and decrypted credentials are injected into operation contexts — handlers receive the material they need, not a vault reference.

This is analogous to the reverse-proxy admin key pattern (ADR-028 in the reverse-proxy project): the proxy reads the key file once at startup, hashes it, and individual handlers never see the file. Here, the CLI unlocks the vault once at startup, and individual handlers receive the results of vault operations through their contexts.

## Consequences

**Positive:**
- alknet-vault remains fully standalone — no QUIC dependency, no tokio runtime requirement for the handle
- Key derivation and encryption are local-only by default — the master seed never leaves the node
- Remote access to public key material (not secrets) flows through the existing call protocol — no separate ALPN needed
- The CLI binary is the single integration point — clean dependency graph, no circular dependencies
- The `VaultServiceHandle` is used in-process with zero serialization overhead — direct method calls, not irpc messages
- Test harnesses can use `VaultServiceHandle` directly without any QUIC infrastructure
- Access pattern is clear: vault → CLI → operation contexts, not vault ← handlers

**Negative:**
- Handlers that need keys must receive them through their operation context — this requires the CLI or call protocol to mediate
- The CLI binary has a larger dependency tree since it imports both alknet-call and alknet-vault (expected: the CLI assembles everything)
- If the call protocol is not yet running when a handler needs a key, the handler must wait for initialization (mitigated: the CLI starts VaultServiceHandle before accepting connections)

## References

- ADR-003: Crate decomposition (alknet-vault is standalone)
- ADR-005: irpc as call protocol foundation
- ADR-009: One-way door decision framework
- OQ-08: Secret service integration point (resolved by this ADR)
- alknet-vault implementation: `crates/alknet-vault/`
- Reverse-proxy ADR-028: Admin HTTP API (analogous key management pattern)