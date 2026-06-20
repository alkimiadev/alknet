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

## Remote Capability

The `VaultProtocol` is a remote-capable irpc service **by construction**.
The `#[rpc_requests]` macro generates both `Service` (local) and
`RemoteService` (remote) trait implementations. The `VaultServiceActor`
processes `VaultMessage` variants identically regardless of transport —
the only difference between local and remote use is the `Client<VaultProtocol>`
construction and the server-side listener setup.

This was a purposeful design decision: irpc's "zero-overhead local,
transparent remote" architecture means the same protocol definition and
actor code work for both in-process and cross-network dispatch. Enabling
remote vault access is a server-setup change, not a protocol change.

### What's already in place

- **Protocol**: `VaultProtocol` is already a `RemoteService`. No code
  changes needed in the protocol definition.
- **Serialization**: `DerivedKey`'s dual serialization (JSON redacts private
  key for safety; postcard preserves bytes for remote dispatch) was
  designed for this use case.
- **Actor**: `VaultServiceActor` already processes all message types. The
  actor is transport-agnostic — it doesn't know whether a message arrived
  via a local mpsc channel or a remote QUIC stream.
- **Auth transport**: irpc over iroh uses iroh's QUIC connections, which
  authenticate via NodeId (Ed25519, RFC 7250 raw keys) — the same identity
  model as the rest of alknet (ADR-010). The connection-level identity
  ("which NodeId is calling") is available before any vault operation is
  dispatched.

### What's not in place (the gap)

The `IrohProtocol` handler that irpc provides forwards **all** message
types to the actor without auth checks. For local use this is correct
(the assembly layer is trusted). For remote use, the listener needs:

1. **NodeId allowlist**: only known worker NodeIds may connect.
2. **Message filtering**: reject `Unlock` and `Lock` from remote callers
   (see "Operation access policy" below).
3. **Then** forward to the actor.

This auth-wrapping handler cannot live in the vault crate — the vault is
standalone (ADR-018) and depends on no alknet crate. The auth model
(`IdentityProvider`, `Identity`, scopes) lives in alknet-core. The
auth-wrapping listener lives in the **assembly layer** (the CLI binary)
or a dedicated vault-server crate that depends on both alknet-core and
alknet-vault. This is the same pattern as ADR-019: the vault is a
library, the assembly layer is the integrator.

```
alknet-vault (standalone, no deps)
  - VaultProtocol (RemoteService by construction)
  - VaultServiceActor (processes all message types, no auth)
  - VaultServiceHandle (direct API)

assembly layer / vault-server (depends on alknet-core + alknet-vault)
  - AuthWrappingHandler: checks NodeId, filters message types, forwards
  - IrohProtocol::new(auth_wrapping_handler)
  - Router::builder(endpoint).accept(b"alknet/vault", protocol).spawn()
```

### Operation access policy

Not all `VaultProtocol` operations are safe to expose remotely. The vault
spec defines the policy; the assembly-layer listener enforces it.

| Operation | Local (assembly layer) | Remote (workers) | Why |
|-----------|----------------------|-------------------|-----|
| `Unlock` | ✅ | ❌ | Sends the mnemonic (root of trust) over the wire. Even with NodeId auth, the mnemonic in transit is a different threat model — it's in memory on the receiving end, potentially in logs/traces. Local-only. |
| `Lock` | ✅ | ❌ | Locking the vault bricks the machine node for all workers. A compromised or buggy worker could DoS the entire machine node. Local-only. |
| `DeriveEd25519` | ✅ | ✅ | Workers need derived keys for signing, identity. The derivation path is the access control — the worker can only derive at paths the assembly layer declares. |
| `DeriveEncryptionKey` | ✅ | ✅ | Workers need encryption keys for credential encryption. Same path-based access control. |
| `DeriveEthereumKey` | ✅ | ✅ | Same as DeriveEd25519, for Ethereum signing. |
| `DerivePassword` | ✅ | ✅ | Workers need deterministic passwords for service credentials. |
| `Encrypt` | ✅ | ✅ | Workers encrypt external credentials (API keys) for storage. |
| `Decrypt` | ✅ | ✅ | Workers decrypt stored credentials at call time. |

The policy is: **`Unlock` and `Lock` are local-only; all other operations
are remote-capable.** The assembly-layer listener filters `Unlock` and
`Lock` messages from remote connections and returns an error.

### Use case: machine node → workers

The primary use case is a **machine node** (long-lived, holds the mnemonic,
manages container services) exposing a restricted vault API to its
**workers** (ephemeral, containerized, no mnemonic):

```
Machine Node (head, vault unlocked locally)
├── exposes alknet/vault ALPN to workers
├── NodeId allowlist: only known worker NodeIds may connect
├── message filter: rejects Unlock/Lock from remote callers
│
├── Worker A (no mnemonic)
│   └── calls DeriveEd25519, Encrypt, Decrypt on machine node's vault
│
└── Worker B (also a head for its own sub-workers)
    ├── gets its own credentials from machine node's vault
    └── can expose its own restricted vault API to sub-workers
```

Workers don't hold mnemonics. They get static credentials injected at
construction (the common case) and call the machine node's vault for
dynamic derivation or decryption when needed. This is the
defense-in-depth (Russian doll) model: the seed is the innermost layer,
the machine node's vault is the next, iroh's NodeId auth is the outer,
and workers are outside that — calling in through authenticated channels.

### Per-machine-node vaults, not shared

Each machine node has its own vault and mnemonic. Machine nodes do not
share vaults with each other. Compromising one machine node exposes only
that node's workers, not all nodes. This is compartmentalization — the
blast radius of a vault compromise is one machine node, not the entire
fleet.

The remote vault capability is for the **machine→worker** relationship,
not for cross-machine-node sharing. Machine nodes don't expose their
vaults to peer machine nodes — only to their own workers, authenticated
by NodeId.

### What's breaking vs. non-breaking

| Change | Breaking? | Why |
|--------|-----------|-----|
| Enabling remote vault access | **No** | Server-setup change — register `IrohProtocol` with an ALPN. The protocol is already a `RemoteService`. |
| Restricting which operations are remote-capable | **No** | Policy in the assembly-layer handler, not a protocol change. |
| Adding NodeId auth checks | **No** | Implementation in the assembly-layer handler. The vault crate doesn't change. |
| Adding new `VaultProtocol` variants | **Yes (wire break)** | Inherent to irpc — versioning is a non-goal. Would need ALPN versioning (`alknet/vault/v2`) if the protocol evolves. Same constraint as any irpc service. |
| Changing `DerivedKey` serialization | **No** | Dual serialization is already in place — postcard preserves bytes for remote, JSON redacts for safety. |

The only breaking change is evolving the `VaultProtocol` enum itself, and
that's manageable with ALPN versioning (`alknet/vault`, then
`alknet/vault/v2` if needed) — the same pattern alknet uses for all ALPN
protocols (ADR-006).

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| irpc for vault dispatch | [ADR-005](../../decisions/005-irpc-as-call-protocol-foundation.md) | In-process type-safe dispatch; remote-capable by construction |
| `DerivedKey` is move-only | [ADR-014](../../decisions/014-secret-material-flow-and-capability-injection.md) | Prevents accidental duplication of secret material |
| JSON redacts private key | [ADR-014](../../decisions/014-secret-material-flow-and-capability-injection.md) | Defense-in-depth for logging accidents |
| postcard preserves private key | — | Required for in-cluster irpc dispatch |
| No vault operations on call protocol | [ADR-008](../../decisions/008-secret-service-integration.md), [ADR-014](../../decisions/014-secret-material-flow-and-capability-injection.md) | Master seed never crosses the network |
| Unlock/Lock are local-only | OQ-21 (deferred) | Mnemonic and lock control must not be remotely accessible |
| Auth wrapping lives in assembly layer | [ADR-018](../../decisions/018-vault-standalone-crate.md), [ADR-019](../../decisions/019-vault-assembly-layer-only.md) | Vault is standalone; can't import alknet-core's auth model |

## Open Questions

None active for this document.

## References

- Implementation: `crates/alknet-vault/src/protocol.rs`
- Tests: `crates/alknet-vault/src/protocol.rs` (unit tests for redaction
  and zeroize behavior)
- [service.md](service.md) — how the actor dispatches `VaultMessage`
- [mnemonic-derivation.md](mnemonic-derivation.md) — what `KeyType` means