---
id: core/credential-store-trait
name: Add CredentialStore trait, InMemoryCredentialStore, EncryptedData mirror, and StoreError (ADR-031/035)
status: completed
depends_on: []
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Add the second repo trait to alknet-core: `CredentialStore` for encrypted-
credential persistence, alongside its in-memory default adapter and the shared
`StoreError` type. Per ADR-031 (the trait) and ADR-035 (refines `put`/`delete`
to async, renames the error to `StoreError`).

This task is standalone — it has no dependency on `core/peer-entry-model`. The
`CredentialStore` trait persists `EncryptedData` blobs (the vault's encrypted
output); it never decrypts (ADR-025 — the vault is the sole decryption
boundary).

### CredentialStore trait

```rust
pub trait CredentialStore: Send + Sync {
    fn get(&self, provider: &str) -> Option<EncryptedData>;
    async fn put(&self, provider: &str, data: &EncryptedData) -> Result<(), StoreError>;
    async fn delete(&self, provider: &str) -> Result<(), StoreError>;
}
```

- `get` is **sync** (cached read — the hot path; ADR-035 §1).
- `put`/`delete` are **async** (they hit the backend; ADR-035 §3 refines
  ADR-031's sync sketch to async within the one-way door).
- `get` returns `Option<EncryptedData>` (missing credential is the common case,
  not an error).
- No `list` method (ADR-031 §4 — additive if needed later).

### InMemoryCredentialStore

```rust
pub struct InMemoryCredentialStore {
    entries: RwLock<HashMap<String, EncryptedData>>,
}

impl InMemoryCredentialStore {
    pub fn new() -> Self;
    pub fn with_entries(entries: HashMap<String, EncryptedData>) -> Self;
}

impl CredentialStore for InMemoryCredentialStore { ... }
```

The default adapter covers tests and config-loaded deployments. `put`/`delete`
are async with no `.await` points (trivially satisfy an async trait — ADR-035
§3). Same posture as `ConfigIdentityProvider` — no persistence, no backend
dependency, no env vars.

### EncryptedData core mirror

A thin serializable struct mirroring the vault's `EncryptedData` (ADR-020),
so the trait can reference it without a vault dependency (ADR-018 — vault is
standalone with zero alknet-crate dependencies):

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EncryptedData {
    pub key_version: u32,
    pub salt: Vec<u8>,   // wire-format compat (OQ-20); unused in v2 but kept
    pub iv: Vec<u8>,     // AES-GCM IV (OsRng-generated)
    pub data: Vec<u8>,   // ciphertext
}
```

The `salt` field is kept for wire-format compatibility with the TS predecessor
(OQ-20) — a core mirror that omitted it could not round-trip the vault's
`EncryptedData`. v2 may write a zero-length salt but must not drop the field
(ADR-035 §6).

### StoreError

```rust
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("backend error: {message}")]
    Backend { message: String },
    #[error("not found: {entity}")]
    NotFound { entity: String },
    #[error("serialization error: {message}")]
    Serialization { message: String },
}
```

Shared by both `CredentialStore` and `IdentityStore` (ADR-035 §7 renames
ADR-031's `CredentialStoreError` to `StoreError`). `#[non_exhaustive]` so
adapter crates can extend without breaking match arms. Lives in alknet-core
(where the traits live).

### Module placement

Add a new `store` module (or `credential_store` module) in `alknet-core/src/`.
Re-export `CredentialStore`, `InMemoryCredentialStore`, `EncryptedData`, and
`StoreError` from `lib.rs`.

## Acceptance Criteria

- [ ] `CredentialStore` trait with sync `get`, async `put`/`delete`
- [ ] `InMemoryCredentialStore` with `new()` and `with_entries()`
- [ ] `InMemoryCredentialStore` implements `CredentialStore` (async put/delete with no .await points)
- [ ] `EncryptedData` core mirror with 4 fields (`key_version`, `salt`, `iv`, `data`), derives Serialize/Deserialize/Clone/Debug
- [ ] `StoreError` enum with 3 variants, `#[non_exhaustive]`, `thiserror::Error`
- [ ] No vault dependency added to alknet-core (EncryptedData is a core-owned mirror)
- [ ] No `list` method on the trait
- [ ] Unit test: InMemoryCredentialStore get/put/delete round-trip
- [ ] Unit test: get returns None for missing provider
- [ ] Unit test: EncryptedData serializes and deserializes (round-trip)
- [ ] Unit test: StoreError Display formatting
- [ ] `cargo test -p alknet-core` succeeds
- [ ] `cargo clippy -p alknet-core` succeeds with no warnings

## References

- docs/architecture/crates/core/auth.md — CredentialStore, StoreError, EncryptedData mirror
- docs/architecture/decisions/031-credentialstore-repo-trait.md — ADR-031 (the trait)
- docs/architecture/decisions/035-concrete-persistence-adapter-shapes.md — ADR-035 (async put/delete, StoreError rename, schema)
- docs/architecture/decisions/033-storage-boundary-and-repo-adapter-pattern.md — ADR-033 (the pattern)

## Notes

> Standalone task — no dependency on PeerEntry. The `CredentialStore` trait is
> the second repo trait in core (alongside `IdentityProvider`), establishing
> the repo/adapter pattern concretely (ADR-033). The trait is the one-way door;
> the in-memory default is the reference implementation; persistence adapters
> (alknet-store-sqlite, ADR-035) are separate crates, not built in this sync.
> `get` stays sync because the credential load happens at startup into
> `Capabilities` (ADR-031); `put`/`delete` are async because a SQLite-backed
> adapter cannot do a sync write without blocking (ADR-035 §3).

## Summary

Added `store` module to alknet-core with: `CredentialStore` trait (sync `get`, async `put`/`delete` via #[async_trait], no `list`), `InMemoryCredentialStore` default adapter (`new()`/`with_entries()`, async put/delete with no .await points, RwLock-backed), `EncryptedData` core mirror (4 fields: key_version/salt/iv/data, derives Serialize/Deserialize/Clone/Debug), and `StoreError` enum (3 variants, #[non_exhaustive], thiserror::Error). Re-exported all four from lib.rs. No vault dependency added (core-owned mirror per ADR-018). 9 unit tests covering get/put/delete round-trip, missing-provider None, put-replaces, with_entries seeding, EncryptedData serde round-trip (empty + non-empty salt), and StoreError Display for all variants. 119 total tests pass, clippy clean.