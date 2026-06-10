---
id: irpc-secret-protocol-integration
name: Wire SecretProtocol to irpc with local SecretServiceHandle and remote dispatch
status: pending
depends_on: [spec-update-secret-service, key-caching-ttl]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

The `SecretProtocol` enum in `protocol.rs` currently has a placeholder `SecretMessage = SecretProtocol` type alias. The spec (after update) defines two dispatch paths:

1. **Local dispatch (in-process)**: `SecretServiceHandle` — async methods that directly call into `SecretServiceInner`. No serialization overhead.
2. **Remote dispatch (in-cluster)**: `SecretProtocol` irpc client — sends `SecretMessage` via mpsc (local node) or QUIC stream (remote worker). The service runs on a head node; workers request derived keys via irpc.

Per ADR-027, irpc is always-on in alknet-secret (not feature-gated). Per ADR-033, irpc is one dispatch backend for OperationEnv.

### irpc Crate Details

The `irpc` crate (version 0.16.0 on crates.io) provides the `#[rpc_requests]` derive macro that generates message enums with channel types. This is the same pattern used by n0/iroh projects.

**Key irpc concepts:**
- `#[rpc_requests(message = SecretMessage)]` on the `SecretProtocol` enum generates a `SecretMessage` enum where each variant wraps the inner type in `WithChannels<Inner, SecretProtocol>`
- Each variant annotated with `#[rpc(tx=oneshot::Sender<T>)]` gets a `oneshot::Sender<T>` channel for responses
- `Client<SecretProtocol>` is the client type that can send messages locally (via mpsc) or remotely (via noq/QUIC)
- The `rpc` feature in irpc is enabled by default and includes the remote transport (postcard + noq)
- The `derive` feature in irpc enables the `#[rpc_requests]` macro

**Current pattern in alknet-core for reference:**
- `AuthProtocol` in alknet-core (`crates/alknet-core/src/auth/auth_protocol.rs`) is currently a plain enum with synchronous methods on `AuthServiceImpl` — it does NOT use `#[rpc_requests]` yet. The alknet-core irpc feature flag exists but is empty. This is because alknet-core's irpc integration hasn't been implemented yet.
- alknet-secret should use the actual `irpc` crate with `#[rpc_requests]` since it's the first crate to do the irpc integration properly.

**Workspace configuration:**
- `irpc = "0.16"` needs to be added to the workspace `Cargo.toml` `[workspace.dependencies]` section
- `alknet-secret/Cargo.toml` needs `irpc = { workspace = true }` and `irpc-derive = { workspace = true }` (the derive macro is in a separate crate)

**irpc dependency requirements:**
- `irpc` with default features brings in `noq` (QUIC transport), `postcard` (serialization), and `tokio`. These are acceptable for alknet-secret.
- The `derive` feature is needed for `#[rpc_requests]`.

### What needs to happen

1. **Add irpc as a workspace dependency**: Add `irpc = "0.16"` and `irpc-derive = "0.16"` to the workspace `Cargo.toml` `[workspace.dependencies]` section. Add `irpc = { workspace = true }` and `irpc-derive = { workspace = true }` to `alknet-secret/Cargo.toml`.

2. **Replace `SecretMessage` type alias with irpc-generated type**: Apply `#[rpc_requests(message = SecretMessage)]` to `SecretProtocol` with appropriate `#[rpc(tx=oneshot::Sender<T>)]` attributes on each variant. This generates:
   - `SecretMessage` enum with `WithChannels` wrappers
   - `Channels<SecretProtocol>` impl for each variant type
   - `From<VariantType> for SecretProtocol` impls
   - `Service` and `RemoteService` impls for `SecretProtocol`

3. **Update SecretProtocol enum for irpc**: The current enum has plain variants like `DeriveEd25519 { path: String }`. With irpc's `#[wrap]` attribute, each variant gets a wrapper struct:
   ```rust
   #[rpc_requests(message = SecretMessage)]
   #[derive(Debug, Serialize, Deserialize)]
   pub enum SecretProtocol {
       #[rpc(tx=oneshot::Sender<DerivedKey>)]
       #[wrap(DeriveEd25519)]
       DeriveEd25519 { path: String },
       
       #[rpc(tx=oneshot::Sender<DerivedKey>)]
       #[wrap(DeriveEncryptionKey)]
       DeriveEncryptionKey { path: String },
       
       #[rpc(tx=oneshot::Sender<DerivedKey>)]
       #[wrap(DeriveEthereumKey)]
       DeriveEthereumKey { path: String },
       
       #[rpc(tx=oneshot::Sender<Vec<u8>>)]
       #[wrap(DerivePassword)]
       DerivePassword { path: String, length: usize },
       
       #[rpc(tx=oneshot::Sender<EncryptedData>)]
       #[wrap(Encrypt)]
       Encrypt { plaintext: String, key_version: u32 },
       
       #[rpc(tx=oneshot::Sender<String>)]
       #[wrap(Decrypt)]
       Decrypt { encrypted: EncryptedData },
       
       #[rpc(tx=oneshot::Sender<()>)]
       #[wrap(Lock)]
       Lock,
       
       #[rpc(tx=oneshot::Sender<()>)]
       #[wrap(Unlock)]
       Unlock { passphrase: String },
   }
   ```

4. **Create SecretServiceActor**: Wrap `SecretServiceHandle` in an actor that processes `SecretMessage` variants and sends responses through the oneshot channels. The actor runs as a `tokio::task`:
   ```rust
   pub struct SecretServiceActor {
       handle: SecretServiceHandle,
   }
   
   impl SecretServiceActor {
       pub async fn run(mut self, mut rx: tokio::sync::mpsc::Receiver<SecretMessage>) { ... }
       pub fn handle(&self) -> &SecretServiceHandle { &self.handle }
   }
   ```

5. **Keep SecretServiceHandle as primary local API**: The `RwLock<SecretServiceInner>` pattern stays for direct in-process use. The actor wraps it for irpc dispatch.

6. **Update public API**: Re-export `SecretMessage`, `SecretServiceActor`, and `Client<SecretProtocol>` from `lib.rs`.

7. **Handle DerivedKey serialization for irpc**: Since `DerivedKey` will become non-Clone (per `derivedkey-zeroize-security` task) and needs custom serialization that redacts `private_key`, ensure the irpc wire format works correctly. The `#[wrap]` structs and `SecretMessage` need to serialize/deserialize `DerivedKey` — since irpc uses `postcard` for remote transport, the `Serialize`/`Deserialize` impls must handle the redacted `private_key` field appropriately. For local (mpsc) transport, `DerivedKey` is sent through oneshot channels without serialization, so the redacted serialization only matters for remote transport.

## Acceptance Criteria

- [ ] `irpc` and `irpc-derive` added as workspace dependencies in root `Cargo.toml`
- [ ] `irpc` and `irpc-derive` added to `alknet-secret/Cargo.toml` as workspace dependencies
- [ ] `SecretProtocol` enum annotated with `#[rpc_requests(message = SecretMessage)]` and `#[rpc(tx=...)]` attributes
- [ ] `SecretMessage` is no longer a type alias — it's the irpc-generated message type
- [ ] `SecretServiceActor` struct that wraps `SecretServiceHandle` and processes `SecretMessage` variants
- [ ] `SecretServiceActor::run()` method that spawns a message loop as a `tokio::task`
- [ ] `SecretServiceActor::spawn()` method that returns a `Client<SecretProtocol>` for sending messages
- [ ] Each `SecretMessage` variant dispatches to the corresponding `SecretServiceHandle` method and sends response through oneshot channel
- [ ] `SecretServiceHandle` remains the primary local API (RwLock-based, unchanged for direct use)
- [ ] Unit test: `SecretServiceActor` processes `SecretMessage::Unlock` and responds successfully
- [ ] Unit test: `SecretMessage::DeriveEd25519` dispatched through actor returns `DerivedKey`
- [ ] Unit test: `SecretMessage::Lock` clears state and subsequent derive calls fail
- [ ] `protocol.rs` updated: `SecretMessage` is the irpc-generated message type, not a type alias
- [ ] `lib.rs` re-exports updated to include `SecretServiceActor` and `Client<SecretProtocol>`
- [ ] `cargo test -p alknet-secret` passes with all existing tests
- [ ] `cargo clippy -p alknet-secret -- -D warnings` passes
- [ ] `cargo fmt -p alknet-secret -- --check` passes

## References

- docs/architecture/secret-service.md — irpc service section (after spec update)
- docs/architecture/decisions/027-crate-decomposition.md — ADR-027 (irpc always-on in alknet-secret)
- docs/architecture/decisions/033-operationenv-irpc-call-protocol.md — ADR-033 (irpc as dispatch backend)
- crates/alknet-secret/src/protocol.rs — Current SecretProtocol with placeholder SecretMessage
- crates/alknet-secret/src/service.rs — SecretServiceHandle and SecretService
- irpc crate (crates.io v0.16) — `#[rpc_requests]` derive macro, `Client<S>` type, `WithChannels`, `Channels` trait
- crates/alknet-core/src/auth/auth_protocol.rs — AuthProtocol pattern (reference, but note: NOT using irpc yet)

## Notes

> The irpc crate is on crates.io at version 0.16.0. Use `irpc = "0.16"` and `irpc-derive = "0.16"` as workspace dependencies. Do NOT use a local path dependency.

> The `#[rpc_requests]` macro generates: (1) a `SecretMessage` enum with `WithChannels` wrappers for each variant, (2) `Channels<SecretProtocol>` impls, (3) `From` impls, (4) `Service` and `RemoteService` impls. See the irpc crate docs and examples for the exact generated code structure.

> The `SecretServiceHandle` with `RwLock` should remain as the primary local API. It's simpler, faster, and works well for single-process use. The `SecretServiceActor` wraps it for irpc dispatch. This two-API pattern matches the spec's "minimal deployment (local handle) vs production deployment (irpc service)" distinction.

> Since `DerivedKey` is becoming non-Clone with redacted serialization (per `derivedkey-zeroize-security`), the irpc integration needs to handle this. For local (mpsc) transport, `DerivedKey` moves through oneshot channels without serialization — no issue. For remote (postcard) transport, `DerivedKey` needs proper Serialize/Deserialize. The custom serialization should serialize `private_key` as bytes (not redacted) for postcard since it's a binary format used for in-cluster Rust-to-Rust communication — the redaction is for JSON/debug output only.

## Summary

> To be filled on completion