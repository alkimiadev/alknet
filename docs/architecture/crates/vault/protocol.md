---
status: draft
last_updated: 2026-06-19
---

# Protocol

The `VaultProtocol` irpc message enum, `DerivedKey` type, and serialization
behavior.

## What

The protocol layer defines the message enum that the irpc dispatch
infrastructure uses (ADR-005) and the `DerivedKey` type that derivation
methods return. This is the vault's internal dispatch protocol — not the
alknet call protocol (the vault has no ALPN, ADR-008).

## VaultProtocol

The irpc message enum. The `#[rpc_requests]` macro generates the
`VaultMessage` enum (with `WithChannels` wrappers), `Channels` impls,
`From` impls, and `Service`/`RemoteService` traits for remote dispatch.

```rust
#[rpc_requests(message = VaultMessage, no_spans)]
#[derive(Debug, Serialize, Deserialize)]
pub enum VaultProtocol {
    DeriveEd25519 { path: String },
    DeriveEncryptionKey { path: String },
    DeriveEthereumKey { path: String },
    DerivePassword { path: String, length: usize },
    Encrypt { plaintext: String, key_version: u32 },
    Decrypt { encrypted: EncryptedData },
    Lock,
    Unlock { mnemonic: String, passphrase: Option<String> },
}
```

Each variant is a vault operation. The `tx` channel type for each variant
is `oneshot::Sender<Result<T, VaultServiceError>>`, where `T` is the
operation's return type (`DerivedKey`, `Vec<u8>`, `EncryptedData`, `String`,
or `()`).

### State requirements

All operations except `Unlock` require the vault to be **unlocked**.
Calling derive/encrypt/decrypt on a locked vault returns
`VaultServiceError::VaultLocked` (not a panic, not a channel close).

### Dispatch

The `VaultServiceActor` (see [service.md](service.md)) processes
`VaultMessage` variants and dispatches to `VaultServiceHandle` methods.
For local in-process use, prefer `VaultServiceHandle` directly — no
channel overhead.

## DerivedKey

The result of key derivation. Holds the key type, private key, and public
key.

```rust
#[derive(Zeroize, Deserialize)]
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

`DerivedKey` has a custom `Serialize` impl that redacts the private key in
human-readable formats:

- **JSON** (human-readable): `private_key` serializes as `"[REDACTED]"`.
  This is defense-in-depth — if a `DerivedKey` accidentally ends up in a
  log or a JSON config, the private key is not exposed.
- **postcard** (binary, used by irpc): `private_key` serializes as the
  actual bytes. This is required for in-cluster irpc dispatch to work —
  the remote side needs the actual key bytes.
- **Deserialization**: always reads the full bytes, regardless of format.
  A JSON-deserialized `DerivedKey` will have `"[REDACTED]"` as its
  `private_key` string — this is expected; JSON round-tripping a
  `DerivedKey` is not a supported use case (the private key is gone).

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
`KeyType` is `Serialize`/`Deserialize` (it's part of the irpc protocol) and
`Clone` (it's not secret material — it's a tag).

## Wire Format

For local (in-process) calls, the protocol uses tokio channels directly —
no serialization. For remote (in-cluster) calls, the protocol is serialized
with postcard (binary, compact). For cross-node (call protocol) exposure,
the vault is wrapped in an operation that serializes to JSON — but **no
vault operations are exposed over the call protocol** (ADR-014). The JSON
serialization path exists only for the `DerivedKey` redaction safety net.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| irpc for vault dispatch | [ADR-005](../../decisions/005-irpc-as-call-protocol-foundation.md) | In-process type-safe dispatch |
| `DerivedKey` is move-only | [ADR-014](../../decisions/014-secret-material-flow-and-capability-injection.md) | Prevents accidental duplication of secret material |
| JSON redacts private key | [ADR-014](../../decisions/014-secret-material-flow-and-capability-injection.md) | Defense-in-depth for logging accidents |
| postcard preserves private key | — | Required for in-cluster irpc dispatch |
| No vault operations on call protocol | [ADR-008](../../decisions/008-secret-service-integration.md), [ADR-014](../../decisions/014-secret-material-flow-and-capability-injection.md) | Master seed never crosses the network |

## Open Questions

None active for this document.

## References

- Implementation: `crates/alknet-vault/src/protocol.rs`
- Tests: `crates/alknet-vault/src/protocol.rs` (unit tests for redaction
  and zeroize behavior)
- [service.md](service.md) — how the actor dispatches `VaultMessage`
- [mnemonic-derivation.md](mnemonic-derivation.md) — what `KeyType` means