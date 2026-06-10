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

Per ADR-027, irpc is always a dependency in alknet-secret (not feature-gated). Per ADR-033, irpc is one dispatch backend for OperationEnv.

**What needs to happen:**

1. **irpc crate integration**: The `irpc` crate needs to be added as a dependency to `alknet-secret`. The `#[rpc_requests]` macro must be applied to `SecretProtocol` to generate `SecretMessage` with oneshot channels for the response types.

2. **SecretMessage definition**: Replace `pub type SecretMessage = SecretProtocol;` with the irpc-generated message type. Each variant gets a `oneshot::Sender<T>` for the response:
   - `DeriveEd25519 { path: String, tx: oneshot::Sender<DerivedKey> }` → `SecretMessage::DeriveEd25519`
   - `DeriveEncryptionKey { path: String, tx: oneshot::Sender<DerivedKey> }`
   - `DeriveEthereumKey { path: String, tx: oneshot::Sender<DerivedKey> }`
   - `DerivePassword { path: String, length: usize, tx: oneshot::Sender<Vec<u8>> }`
   - `Encrypt { plaintext: String, key_version: u32, tx: oneshot::Sender<EncryptedData> }`
   - `Decrypt { encrypted: EncryptedData, tx: oneshot::Sender<String> }`
   - `Lock { tx: oneshot::Sender<()> }`
   - `Unlock { passphrase: String, tx: oneshot::Sender<()> }`

   **Note**: If the `irpc` crate's `#[rpc_requests]` macro generates this automatically, use it. If irpc doesn't exist as a crate yet (it may still be in design), create the `SecretMessage` enum manually with the oneshot channels, following the pattern used by `AuthProtocol` in alknet-core.

3. **SecretServiceHandle dispatch**: The `SecretServiceHandle` methods should dispatch through the irpc channel. When assembled locally, the handle sends `SecretMessage` variants to a `tokio::sync::mpsc` channel; a task runs `SecretServiceInner` and processes messages. This replaces the current `RwLock<SecretServiceInner>` pattern with an actor-model pattern.

   **OR** keep the current `RwLock<SecretServiceInner>` for local use and add a separate `SecretServiceActor` that wraps the handle in an mpsc-based message loop. The `SecretServiceHandle` stays as the primary local API. The actor is the irpc entry point.

   **Prefer the simpler approach**: Keep `SecretServiceHandle` with `RwLock` for direct local use (current code). Add a `SecretServiceActor` that:
   - Holds a `SecretServiceHandle`
   - Runs a message loop: receives `SecretMessage`, dispatches to `SecretServiceHandle` methods, sends responses through oneshot channels
   - Can be spawned as a `tokio::task` for in-process irpc
   - Exposes a `tokio::sync::mpsc::Sender<SecretMessage>` as the client handle

4. **irpc feature**: Per ADR-027, irpc is always-on in alknet-secret. No feature flag needed. If the `irpc` crate exists, depend on it directly. If not, the `SecretMessage` type can be defined locally following the irpc pattern.

**Current state**: `irpc` is listed as `"0.x"` in the spec's dependencies but is not in `Cargo.toml`. The current code doesn't import irpc at all. Check whether the `irpc` crate exists in the workspace or if it needs to be defined locally.

**Critical dependency**: This task cannot proceed until we know the irpc crate's API. If it doesn't exist yet, we should define `SecretMessage` manually following the same pattern as `AuthProtocol` in alknet-core (which also uses irpc behind a feature flag).

## Acceptance Criteria

- [ ] `irpc` dependency added to `Cargo.toml` (or `SecretMessage` defined manually if irpc doesn't exist yet)
- [ ] `SecretMessage` enum defined with oneshot channels for each `SecretProtocol` variant's response type
- [ ] `SecretServiceActor` struct that wraps `SecretServiceHandle` and processes `SecretMessage` variants
- [ ] `SecretServiceActor::run()` method that spawns a message loop as a `tokio::task`
- [ ] `SecretServiceActor::handle(&self) -> mpsc::Sender<SecretMessage>` returns a client handle for sending messages
- [ ] Each `SecretMessage` variant dispatches to the corresponding `SecretServiceHandle` method
- [ ] `SecretServiceHandle` remains the primary local API (RwLock-based, unchanged for direct use)
- [ ] Unit test: `SecretServiceActor` processes `SecretMessage::Unlock` and responds successfully
- [ ] Unit test: `SecretMessage::DeriveEd25519` dispatched through actor returns `DerivedKey`
- [ ] Unit test: `SecretMessage::Lock` clears state and subsequent derive calls fail
- [ ] `protocol.rs` updated: `SecretMessage` is no longer a type alias, it's the irpc message type
- [ ] `lib.rs` re-exports updated to include `SecretServiceActor` and `SecretMessage`

## References

- docs/architecture/secret-service.md — irpc service section (after spec update)
- docs/architecture/decisions/027-crate-decomposition.md — ADR-027 (irpc always-on in alknet-secret)
- docs/architecture/decisions/033-operationenv-irpc-call-protocol.md — ADR-033 (irpc as dispatch backend)
- crates/alknet-secret/src/protocol.rs — Current SecretProtocol with placeholder SecretMessage
- crates/alknet-secret/src/service.rs — SecretServiceHandle and SecretService
- crates/alknet-core/src/auth/ — AuthProtocol pattern (reference for irpc integration)

## Notes

> This is the biggest gap identified by the architect. The spec says `#[rpc_requests]` but that macro doesn't exist in the codebase yet. Check whether `irpc` is a workspace crate or an external dependency.

> If `irpc` doesn't exist yet, create a local `SecretMessage` type following the same channel-based pattern that alknet-core uses for its irpc services. The key pattern is: each protocol variant has a corresponding message variant with a `oneshot::Sender<Response>` for the response. The service actor receives messages, processes them, and sends responses.

> The `SecretServiceHandle` with `RwLock` should remain as the primary local API. It's simpler, faster, and works well for single-process use. The `SecretServiceActor` wraps it for irpc dispatch. This two-API pattern matches the spec's "minimal deployment (local handle) vs production deployment (irpc service)" distinction.

## Summary

> To be filled on completion