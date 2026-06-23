---
status: stable
last_updated: 2026-06-23
---

# Protocol

The `DerivedKey` type, `KeyType` enum, and serialization behavior. The
vault's "protocol" is the `VaultServiceHandle` method API (ADR-025) — there
is no message enum, no irpc dispatch, and no wire format.

## What

The vault's dispatch is direct method calls on `VaultServiceHandle`
(ADR-025). The types defined here — `DerivedKey`, `KeyType` — are the
return types from those methods. There is no `VaultProtocol` enum, no
`VaultMessage`, no `VaultServiceActor`, and no remote dispatch capability.

The vault is **local-only by construction**. If remote vault access is ever
needed, it requires a separate crate that wraps the vault and adds remote
transport + auth (ADR-025, OQ-021).

## DerivedKey

The result of key derivation. Holds the key type, private key, and public
key.

```rust
#[derive(Zeroize)]
#[zeroize(drop)]
pub struct DerivedKey {
    #[zeroize(skip)]
    pub key_type: KeyType,          // not secret — tag only
    #[zeroize]
    pub private_key: Vec<u8>,       // zeroized on drop
    #[zeroize(skip)]
    pub public_key: Vec<u8>,        // not secret — public by definition
}
```

`DerivedKey` does **not** derive `Deserialize` via `#[derive]`. It has a **custom
`Deserialize` impl** that rejects redacted payloads — see
[Serialization Redaction](#serialization-redaction) below. (A derived
`Deserialize` would generate a default impl that conflicts with the manual one,
and would not produce the explicit redaction-rejection error the spec requires.)

The `#[zeroize(skip)]` attributes on `key_type` and `public_key` mean only
the `private_key` is zeroized when the `DerivedKey` is dropped. The public
key and key type are not secret material — zeroizing them is unnecessary
and would require them to derive `Zeroize` (which `KeyType` does not).

### Move-only, not Clone

`DerivedKey` does **not** derive `Clone`. It is move-only. Consumers
receive it by value and zeroize it when done (handled automatically by
`#[zeroize(drop)]`). This prevents accidental duplication of secret
material — there is exactly one copy of the private key, and it is
zeroized when the `DerivedKey` is dropped.

The assembly layer (CLI binary) extracts the bytes it needs (private key
for signing, public key for TLS identity) and constructs the alknet-core
types at the assembly boundary (ADR-018). The `DerivedKey` is then dropped
and zeroized.

### Serialization redaction

`DerivedKey` has a custom `Serialize` impl that **always** redacts the
private key, regardless of format:

- **JSON** (and all human-readable formats): `private_key` serializes as
  `"[REDACTED]"`. This is defense-in-depth — if a `DerivedKey` accidentally
  ends up in a log, a JSON config, or debug output, the private key is not
  exposed.
- **Deserialization**: a custom `Deserialize` impl rejects
  `private_key == "[REDACTED]"` with a deserialization error (not a corrupted
  key). This resolves review #002 W8 (silent corruption on JSON-deserialized
  `DerivedKey`). The custom impl is required because `#[derive(Deserialize)]`
  would generate a default impl that conflicts and would only fail incidentally
  (serde type mismatch: string vs sequence), not with the explicit
  redaction-rejection error the spec requires.
- **No binary-format preservation path.** ADR-025 dropped the postcard/remote
  dispatch path that previously preserved private key bytes in binary
  formats. `DerivedKey` is always used in-process (ADR-014: never appears
  in call protocol payloads). If a future remote-vault crate needs to send
  `DerivedKey` over the wire, it defines its own serialization for that
  context — the vault's `DerivedKey` stays redact-always.

```rust
// Custom Serialize — always redacts private_key
impl serde::Serialize for DerivedKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where S: serde::Serializer {
        use serde::SerializeStruct;
        let mut s = serializer.serialize_struct("DerivedKey", 3)?;
        s.serialize_field("key_type", &self.key_type)?;
        s.serialize_field("private_key", "[REDACTED]")?;  // never the real bytes
        s.serialize_field("public_key", &self.public_key)?;
        s.end()
    }
}

// Custom Deserialize — rejects "[REDACTED]" with an error
impl<'de> serde::Deserialize<'de> for DerivedKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where D: serde::Deserializer<'de> {
        #[derive(serde::Deserialize)]
        struct DerivedKeyHelper {
            key_type: KeyType,
            private_key: Vec<u8>,
            public_key: Vec<u8>,
        }
        let helper = DerivedKeyHelper::deserialize(deserializer)?;
        // Reject redacted payloads — a JSON-deserialized DerivedKey with a
        // redacted private key is invalid, not a corrupted key.
        if helper.private_key == b"[REDACTED]" {
            return Err(serde::de::Error::custom(
                "DerivedKey.private_key is \"[REDACTED]\" — redacted payloads \
                 cannot be deserialized. JSON round-tripping a DerivedKey is \
                 not supported (the private key is gone)."
            ));
        }
        Ok(DerivedKey {
            key_type: helper.key_type,
            private_key: helper.private_key,
            public_key: helper.public_key,
        })
    }
}
```

The redaction is **not the primary control** for keeping private keys off
the wire. The primary control is architectural: `DerivedKey` never appears
in call protocol payloads (ADR-014). The redaction is a safety net for
logging accidents and debug output.

### Debug redaction

`DerivedKey`'s `Debug` impl also redacts the private key:

```rust
impl fmt::Debug for DerivedKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DerivedKey")
            .field("key_type", &self.key_type)
            .field("private_key", &"[REDACTED]")
            .field("public_key", &self.public_key)
            .finish()
    }
}
```

`{:?}` on a `DerivedKey` never exposes the private key. This makes it safe
to use in `tracing` spans and error messages.

## KeyType

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum KeyType {
    Ed25519,      // SLIP-0010 derivation (32-byte private + 32-byte public)
    Aes256Gcm,    // Symmetric key (32 bytes, used for encryption)
    Secp256k1,    // BIP-0032 derivation (32-byte private + 33-byte compressed public)
}
```

Tags `DerivedKey` and `CachedKey` so consumers know what they received.
`KeyType` is `Serialize`/`Deserialize` (retained for `EncryptedData` interop
and future use — ADR-025 removed the irpc dispatch path that previously
justified these derives, but the type remains serializable for structured
storage scenarios) and `Clone` (it's not secret material — it's a tag).

## Wire Format

The vault has no wire format (ADR-025). Dispatch is direct method calls on
`VaultServiceHandle` — no serialization, no channels, no network. The
`DerivedKey` custom `Serialize`/`Deserialize` impls exist solely for
logging safety (redaction) and defense-in-depth, not for wire transport.

`EncryptedData` has a stable wire format (shared with `alknet-storage` and
the TypeScript consumer by type-level agreement — see
[encryption.md](encryption.md) and ADR-018). That format is for *stored
encrypted data*, not for vault dispatch — the vault's `encrypt`/`decrypt`
methods operate on `EncryptedData` as a value type, not as a wire message.

## Local-Only by Construction

The vault is **local-only by construction** (ADR-025). There is no
`RemoteService` trait, no remote handler, no wire format for vault
messages. The vault's API is `VaultServiceHandle` — direct method calls,
nothing else.

If remote vault access is ever needed (e.g., the machine→worker pattern
where a long-lived node exposes a restricted vault API to ephemeral
workers), it requires a **separate vault-server crate** that:

1. Depends on both alknet-core (for `IdentityProvider`, scopes,
   auth-wrapping) and alknet-vault (for `VaultServiceHandle`).
2. Defines its own threat model, access policy, and operation filtering
   (`Unlock`/`Lock` must be local-only; other operations may be
   remote-capable depending on the policy).
3. Adds the remote transport (iroh/QUIC or similar) and an auth-wrapping
   handler that checks caller identity before forwarding to the vault.
4. Requires its own ADR (matching ADR-019's language: "requires its own
   ADR") defining the threat model and access policy.

This is a deliberate addition, not a flag flip on a default that was
already loaded. The pre-ADR-025 design made the vault remote-capable *by
construction* (irpc generated `RemoteService` by default), which was the
default-insecure anti-pattern. ADR-025 inverts the default: local-only is
the only mode, and remote access requires building something new.

**Per-node vaults are the recommended pattern for multi-node deployments.**
Each node has its own vault and mnemonic. Credentials are encrypted *for*
the receiving node's public key or derived at a shared path the receiving
node can derive locally. This is end-to-end encryption between nodes, not
a centralized decryption oracle. It matches ADR-008's "capability source"
model — credentials are injected at the assembly layer, not fetched over
the network at call time.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| Vault is standalone | [ADR-018](../../decisions/018-vault-standalone-crate.md) | Zero alknet crate dependencies |
| Vault is local-only | [ADR-025](../../decisions/025-vault-local-only-dispatch.md) | Direct method calls, no irpc, no remote dispatch capability |
| HD derivation (not stored keys) | — | One seed, many keys, no key storage |
| `DerivedKey` is move-only | [ADR-014](../../decisions/014-secret-material-flow-and-capability-injection.md) | Prevents accidental duplication of secret material |
| JSON redacts private key (always) | [ADR-014](../../decisions/014-secret-material-flow-and-capability-injection.md) | Defense-in-depth for logging accidents |
| No vault operations on call protocol | [ADR-008](../../decisions/008-secret-service-integration.md), [ADR-014](../../decisions/014-secret-material-flow-and-capability-injection.md) | Master seed never crosses the network |
| No remote dispatch in vault crate | [ADR-025](../../decisions/025-vault-local-only-dispatch.md) | Remote access requires a separate vault-server crate with its own ADR |

## Open Questions

None active for this document. OQ-21 (remote vault) is resolved — see
ADR-025 and [open-questions.md](../../open-questions.md).

## References

- Implementation: `crates/alknet-vault/src/protocol.rs`
- Tests: `crates/alknet-vault/src/protocol.rs` (unit tests for redaction
  and zeroize behavior)
- [service.md](service.md) — `VaultServiceHandle` runtime API
- [mnemonic-derivation.md](mnemonic-derivation.md) — what `KeyType` means