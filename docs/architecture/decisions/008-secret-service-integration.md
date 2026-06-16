# ADR-008: Secret Service Integration Point

## Status

Accepted

## Context

alknet-secret is a standalone crate with zero alknet crate dependencies. It provides BIP39 mnemonic generation, SLIP-0010 Ed25519 HD key derivation, AES-256-GCM encryption, and an irpc-based `SecretProtocol` service. It is already implemented and stable.

The question (OQ-08) is: how does the rest of the alknet system access alknet-secret's capabilities? The options are:

1. **irpc service over `alknet/call`**: Other services call SecretProtocol operations through the call protocol on `alknet/call`. The secret service is just another set of operations registered in the call protocol's operation registry.

2. **ALPN handler on `alknet/secret`**: alknet-secret implements `ProtocolHandler` and gets its own ALPN. Remote nodes call it over a dedicated QUIC connection.

3. **Direct library dependency**: alknet-core or handler crates depend on alknet-secret directly, breaking its independence.

4. **CLI-embedded with call protocol exposure**: The CLI binary instantiates SecretServiceHandle locally and registers secret operations in the call protocol's registry.

This is a one-way door because if alknet-secret gets pulled into alknet-core as a dependency, its independence is permanently lost. The standalone property is valuable — alknet-secret has no QUIC, no tokio runtime requirement (the handle works without it), and no alknet crate dependencies. It can be used in contexts where QUIC networking doesn't exist (CLI tools, test harnesses, WASM key derivation).

## Decision

**Option 4: CLI-embedded with call protocol exposure.**

The CLI binary (the `alknet` crate) is the integration point. It:

1. Instantiates `SecretServiceHandle` locally at startup (or on-demand with Unlock/Lock lifecycle).
2. Registers secret operations (DeriveEd25519, DeriveEncryptionKey, Encrypt, Decrypt, etc.) in the call protocol's operation registry.
3. Other handlers access secret capabilities by calling operations on `alknet/call` — they don't import alknet-secret directly.

alknet-secret remains standalone with no alknet crate dependencies. Its `SecretServiceHandle` is used directly (in-process, no serialization) by the CLI binary. Its `SecretProtocol` irpc service is available for remote access through the call protocol.

**alknet-secret does NOT get its own ALPN.** Here's why:

- `alknet/secret` as a separate ALPN would mean a remote node opens a QUIC connection to access key derivation — this is architecturally wrong. Key derivation is a local operation that should never cross the network in its raw form.
- If a remote node needs derived keys (e.g., for end-to-end encryption), the local node derives them and sends only the public component over `alknet/call` — never the seed or private key.
- The secret service's Unlock/Lock lifecycle (holding the master seed in RAM) is inherently local. There's no safe way to expose Unlock/Lock over the network.

**What if a handler needs a key at runtime?** The handler calls through the call protocol. The CLI registers secret operations in the call registry at startup. The call protocol routes the request to the locally-running SecretServiceHandle. No handler crate depends on alknet-secret.

## Consequences

**Positive:**
- alknet-secret remains fully standalone — no QUIC dependency, no tokio runtime requirement for the handle
- Key derivation and encryption are local-only by default — the master seed never leaves the node
- Remote access to public key material (not secrets) flows through the existing call protocol — no separate ALPN needed
- The CLI binary is the single integration point — clean dependency graph, no circular dependencies
- The `SecretServiceHandle` is used in-process with zero serialization overhead — direct method calls, not irpc messages
- Test harnesses can use `SecretServiceHandle` directly without any QUIC infrastructure

**Negative:**
- Handlers that need keys must go through the call protocol — this adds a hop even for local calls (mitigated: local call protocol calls can be short-circuited to direct method calls via irpc's local dispatch)
- The CLI binary has a larger dependency tree since it imports both alknet-call and alknet-secret (expected: the CLI assembles everything)
- If the call protocol is not yet running when a handler needs a key, the handler must wait for initialization (mitigated: the CLI starts SecretServiceHandle before accepting connections)

## References

- ADR-003: Crate decomposition (alknet-secret is standalone)
- ADR-005: irpc as call protocol foundation
- OQ-08: Secret service integration point (resolved by this ADR)
- alknet-secret implementation: `crates/alknet-secret/`