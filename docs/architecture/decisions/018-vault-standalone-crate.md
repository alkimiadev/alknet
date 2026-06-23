# ADR-018: Vault as Standalone Crate

## Status

Accepted

## Context

alknet-vault provides BIP39 mnemonic generation, SLIP-0010 Ed25519 HD key
derivation, BIP-0032 secp256k1 derivation (feature-gated), and AES-256-GCM
encryption. It holds the master seed — the root of trust for all derived keys
and encrypted credentials in the alknet system.

The question is: what does alknet-vault depend on? The candidates:

1. **Depend on alknet-core** for shared types (errors, maybe Identity). This
   pulls QUIC, quinn, iroh, rustls, and tokio runtime dependencies into the
   vault's dependency tree.
2. **Stand alone** — zero alknet crate dependencies. The vault defines its own
   types, its own error enum. Other crates depend on
   the vault; the vault depends on nothing in alknet.

This is a one-way door. Once the vault depends on alknet-core, reversing it
requires removing that dependency from every type, error conversion, and
test — and the longer it stays, the more entangled it becomes.

### Why standalone matters

The vault is used in contexts where QUIC networking does not exist:

- **CLI tools**: a key-derivation utility that derives an identity key from a
  mnemonic without starting a network endpoint.
- **Test harnesses**: integration tests in other crates derive test keys
  without spinning up a QUIC endpoint.
- **WASM key derivation**: a future WASM target that derives keys in a browser
  (the BiStream trait in ADR-007 preserves this door at the transport layer;
  the vault's independence preserves it at the secret layer).
- **Embedded assembly**: a binary that only needs the vault to decrypt a
  config file at startup, with no networking at all.

If the vault depends on alknet-core, all of these contexts pull in quinn,
iroh, rustls, and tokio — none of which they need. The vault's job is
cryptographic derivation and encryption. It has no networking concern.

### What the vault provides without alknet-core

The vault defines its own types and traits:

- `Mnemonic`, `Seed` — BIP39 root material
- `ExtendedPrivKey` (Ed25519), `Secp256k1ExtendedPrivKey` (Ethereum) —
  derived key material
- `DerivedKey`, `KeyType` — protocol-level key representation
- `EncryptedData`, `EncryptionKey` — AES-256-GCM blobs
- `VaultServiceHandle` — runtime API (direct method calls; no actor, no
  message enum — see ADR-025)
- `VaultServiceError` — its own error enum (string-wrapped sub-errors; the
  vault doesn't share an error type with alknet-core)

The vault uses direct method calls on `VaultServiceHandle`, not irpc
dispatch (ADR-025). The vault is local-only by construction — no remote
dispatch capability, no `RemoteService` trait, no wire format for vault
messages. If remote vault access is ever needed, it's a separate crate that
wraps the vault (see ADR-025, OQ-021).

## Decision

**alknet-vault has zero alknet crate dependencies.** It depends only on
external crates (`bip39`, `ed25519-bip32`, `aes-gcm`, `sha2`, `hmac`,
`secp256k1`, `tokio` for `RwLock` sync primitives, `serde`,
`zeroize`, `thiserror`, `base64`, `rand`). ADR-025 dropped `irpc`,
`irpc-derive`, and `postcard` — the vault no longer uses irpc dispatch.

The vault does not depend on:
- `alknet-core` — no shared types, no `Identity`, no `AuthContext`
- `alknet-call` — no `OperationSpec`, no `OperationContext`, no call protocol
- `alknet-vault` does not implement `ProtocolHandler` — it has no ALPN (see
  ADR-019)

Dependency flow is strictly one-directional:

```
alknet-vault (standalone)
      ↑
alknet (CLI binary) — the only crate that depends on alknet-vault
```

No handler crate depends on alknet-vault directly. Handlers receive derived
material through capabilities injected by the assembly layer (ADR-014). The
CLI binary is the sole integration point (ADR-008, ADR-019).

### Type independence

The vault defines its own types and does not share types with alknet-core:

- `VaultServiceError` is the vault's error enum. It is a plain
  `thiserror::Error` (ADR-025 dropped irpc, so vault errors no longer need
  `Serialize`/`Deserialize` for wire dispatch). It does not implement
  `From` for alknet-core error types — the CLI binary converts at the
  assembly boundary.
- `DerivedKey` is the vault's key representation. It is not shared with
  alknet-core's `Identity` type. The CLI binary extracts the bytes it needs
  (private key for signing, public key for TLS identity) and constructs the
  alknet-core types at the assembly layer.
- `EncryptedData` is the vault's encrypted blob format. It is shared with
  `alknet-storage` (a future crate) by type-level agreement, not by a crate
  dependency — both crates must agree on the serialization format (see
  [encryption.md](../crates/vault/encryption.md)). The format is **frozen**
  (see Decision below).

## `EncryptedData` Wire Format Lock

The `EncryptedData` struct is a **stable wire format** shared with
`alknet-storage` (a future crate) and the TypeScript consumer
(`@alkdev/storage`) by type-level agreement, not by a crate dependency.
Both crates and the TypeScript consumer must agree on the serialization
format. The format is now explicitly **frozen**:

```rust
pub struct EncryptedData {
    pub key_version: u32,  // rotation tracking
    pub salt: String,      // base64, 32 bytes — unused in v2 (wire-format compat)
    pub iv: String,        // base64, 12 bytes — AES-GCM nonce
    pub data: String,      // base64 — ciphertext + auth tag
}
```

The frozen compatibility surface:

- **Fields**: `key_version`, `salt`, `iv`, `data` — no fields may be
  removed or renamed. New fields may be added only if they are optional
  (default on deserialization) and do not change the meaning of existing
  fields.
- **Encoding**: all binary fields are base64-encoded as strings for JSON
  serialization. This is the cross-language wire format.
- **Field semantics**: `key_version` selects the derivation path
  (ADR-021). `salt` is unused in v2 but is part of the frozen format —
  it cannot be removed without a format-version migration (a future KDF
  in v3 would use the salt for *new* data, not retroactively for v2 data
  — see ADR-020, W6). `iv` is the 12-byte GCM nonce. `data` is the
  ciphertext with the GCM auth tag appended.

**Why this needs an explicit lock**: the "type-level agreement, not a
crate dependency" approach means there is no compiler enforcement of the
format across crates. The stability contract existed only in prose. An
implementer modifying `EncryptedData` (e.g., removing the unused `salt`
field) would find no ADR saying "this format is frozen." This decision
makes the freeze explicit and enforceable by review.

**This resolves review #002 W10.**

## Consequences

**Positive:**
- The vault compiles and runs without QUIC, quinn, iroh, rustls, or a tokio
  runtime (the `VaultServiceHandle` works with just `std::sync::RwLock`; the
  actor uses `tokio::sync::mpsc` but that's a lightweight dependency).
- CLI tools, test harnesses, and future WASM targets can use the vault for key
  derivation without pulling in networking crates.
- The vault's API surface is stable — changes to alknet-core types don't
  force a vault recompile, and changes to vault types don't force a
  handler recompile (the CLI is the only consumer).
- No circular dependency risk. The dependency graph is a strict DAG.
- The vault can be published and used independently of alknet — it's a
  general-purpose local key vault, not an alknet-specific component.

**Negative:**
- The vault cannot share types with alknet-core. If a type wants to be shared
  (e.g., a future `Fingerprint` type), it must live in alknet-core and the
  vault must define its own equivalent, or a new shared crate must be
  created. This is a feature, not a bug — it forces explicit boundaries.
- The CLI binary must convert between vault types and alknet-core types at
  the assembly boundary. This is a small amount of glue code (extract bytes
  from `DerivedKey`, construct alknet-core types). See ADR-019.
- The vault's `VaultServiceError` is separate from alknet-core's
  `HandlerError`. The CLI binary maps vault errors to handler errors or
  startup failures. This is expected — the vault is a library, not a
  handler.

## Assumptions

1. **The vault's API is consumed by one component (the CLI binary) in the
   alknet system.** If a future use case requires multiple crates to depend
   on the vault directly, the dependency flow still holds — they depend on
   the vault, the vault depends on nothing. The standalone property is
   preserved.

2. **Shared types between the vault and other crates are agreed by type-level
   compatibility, not by a crate dependency.** `EncryptedData` is the example:
   both the vault and `alknet-storage` (future) must agree on the
   serialization format. This is documented in the type's spec, not enforced
   by the type system across crates.

3. **The vault's error type does not need to integrate with alknet-core's
   error handling.** The vault returns `VaultServiceError`; the CLI binary
   handles it at the assembly boundary. If a future use case requires
   propagating vault errors through alknet-core's error types, the CLI
   converts at the boundary.

## References

- ADR-003: Crate decomposition (alknet-vault is standalone)
- ADR-005: irpc as call protocol foundation (irpc remains the foundation
  for alknet-*call*; the vault no longer uses irpc — see ADR-025)
- ADR-025: Vault local-only dispatch (dropped irpc from the vault; the
  vault uses direct method calls, no actor, no remote capability)
- ADR-008: Vault integration point (CLI-embedded, assembly-layer only)
- ADR-014: Secret material flow and capability injection
- ADR-019: Vault assembly-layer-only access
- [crates/vault/README.md](../crates/vault/README.md)
- Implementation: `crates/alknet-vault/`