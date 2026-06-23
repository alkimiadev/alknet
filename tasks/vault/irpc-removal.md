---
id: vault/irpc-removal
name: Remove irpc dependency and actor dispatch from vault, convert to direct method calls on VaultServiceHandle
status: completed
depends_on: []
scope: broad
risk: high
impact: component
level: implementation
---

## Description

Remove the irpc-based actor dispatch from the vault crate and convert to direct
method calls on `VaultServiceHandle`. This is drift item #4 from the vault README
drift table and the foundational ADR-025 refactor — it restructures `service.rs`
and `protocol.rs` fundamentally, which is why most other vault drift tasks depend
on this one.

### What to remove

- `VaultProtocol` enum with `#[rpc_requests]` derive in `protocol.rs`
- `VaultServiceActor` in `service.rs`
- `Client<VaultProtocol>` usage
- `irpc` and `irpc-derive` dependencies from `Cargo.toml`
- `postcard` from dev-dependencies (was only needed for the irpc binary path)
- `tokio` dependency from `Cargo.toml` (the vault uses `std::sync::RwLock`, not
  `tokio::sync::RwLock` — ADR-025)
- `VaultMessage` / `VaultProtocol` re-exports from `lib.rs`

### What to keep / change

- `VaultServiceHandle` stays — it becomes the sole API. It is already
  `Arc<std::sync::RwLock<VaultServiceInner>>` with synchronous methods. The actor
  path (`mpsc` channel + oneshot backchannels via irpc's `Service` trait) is
  removed entirely.
- `VaultServiceError` drops `Serialize`/`Deserialize` derives (were needed for
  irpc dispatch — ADR-025 removed that path). It becomes a plain `thiserror::Error`
  enum.
- `DerivedKey` and `KeyType` stay in `protocol.rs` — the file is renamed in
  spirit to "the types module" but the filename stays `protocol.rs` for
  continuity. The `VaultProtocol` enum is removed; `DerivedKey`/`KeyType` remain.
- `lib.rs` re-exports are updated to remove `VaultMessage`, `VaultProtocol`,
  `VaultServiceActor` and reflect the new public API per the vault README's
  Public API section.

### Public API after this task

Per `docs/architecture/crates/vault/README.md` → Public API:

```rust
pub use mnemonic::{Language, Mnemonic, Seed};
pub use derivation::{DerivationError, ExtendedPrivKey, PATHS};
pub use encryption::{EncryptedData, EncryptionError, EncryptionKey};
pub use encryption::CURRENT_KEY_VERSION;
pub use protocol::{DerivedKey, KeyType};
pub use service::{VaultServiceError, VaultServiceHandle};
pub use cache::CacheConfig;
```

### Cargo.toml changes

Remove from `[dependencies]`:
- `irpc = { workspace = true }`
- `irpc-derive = { workspace = true }`
- `tokio = { version = "1", features = ["sync", "rt", "macros"] }`

Remove from `[dev-dependencies]`:
- `postcard = { version = "1", features = ["alloc"] }`

The vault should have **zero** async runtime dependency after this task.

## Acceptance Criteria

- [ ] `VaultProtocol` enum and `#[rpc_requests]` derive removed from `protocol.rs`
- [ ] `VaultServiceActor` removed from `service.rs`
- [ ] `Client<VaultProtocol>` usage removed
- [ ] `irpc`, `irpc-derive`, `tokio` removed from `[dependencies]` in `Cargo.toml`
- [ ] `postcard` removed from `[dev-dependencies]` in `Cargo.toml`
- [ ] `VaultServiceError` no longer derives `Serialize`/`Deserialize`
- [ ] `lib.rs` re-exports match the Public API section of vault README (no `VaultMessage`, `VaultProtocol`, `VaultServiceActor`)
- [ ] `VaultServiceHandle` methods are all synchronous (no `async`, no `.await`)
- [ ] `cargo check` succeeds
- [ ] `cargo clippy` succeeds with no warnings
- [ ] `cargo test` succeeds (existing tests updated to remove irpc/postcard usage)
- [ ] No `tokio` dependency remains in the vault `Cargo.toml`

## References

- docs/architecture/crates/vault/README.md — Known Source Drift table item #4, Public API section
- docs/architecture/crates/vault/service.md — Dispatch section, VaultServiceHandle
- docs/architecture/crates/vault/protocol.md — Local-Only by Construction
- docs/architecture/decisions/025-vault-local-only-dispatch.md — ADR-025

## Notes

> This is the foundational vault refactor. It restructures `service.rs` and
> `protocol.rs` — most other vault drift tasks touch these same files and must
> follow this one to avoid merge conflicts. The `VaultServiceHandle` struct
> already uses `std::sync::RwLock` with synchronous methods; the actor path is
> the dead code to remove. After this task, the vault has no async runtime
> dependency and no RPC framework dependency — it is local-only by construction.

## Summary

Removed the irpc-based actor dispatch from the vault crate. `VaultServiceHandle`
(`Arc<std::sync::RwLock<VaultServiceInner>>`) is now the sole synchronous API.
Removed: `VaultProtocol` enum + `#[rpc_requests]` derive, `VaultServiceActor`,
`VaultService` wrapper, `Client<VaultProtocol>` usage, `irpc`/`irpc-derive`/
`tokio` deps, `postcard` dev-dep, `Serialize`/`Deserialize` on `VaultServiceError`.
`lib.rs` re-exports match the vault README Public API. The vault is now local-only
by construction with zero async runtime dependency. All 58 lib + 14 integration
tests pass; clippy clean. Merged to develop.