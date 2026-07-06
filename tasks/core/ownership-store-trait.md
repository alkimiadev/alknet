---
id: core/ownership-store-trait
name: Add OwnershipProvider (sync read) + OwnershipStore (async write) traits and InMemoryOwnershipStore (ADR-050)
status: completed
depends_on: []
scope: moderate
risk: low
impact: component
level: implementation
---

## Description

Add the `OwnershipProvider` (sync read trait) and `OwnershipStore` (async
write trait) for runtime-spawned resource ownership, plus an
`InMemoryOwnershipStore` default adapter. This is the fourth instance of
the repo/adapter pattern (ADR-033), alongside `IdentityProvider` (ADR-004),
`IdentityStore` (ADR-035), and `CredentialStore` (ADR-031). Per ADR-050.

### Why this exists

Runtime-spawned resources (containers, TTYs, workspace processes) have
derived ownership: whoever spawned the resource owns it. The static
`Identity.resources` model can't represent this — the resource didn't exist
when the identity was resolved. `AccessControl::check` (in alknet-call) will
consult `OwnershipProvider` at check time to answer "does this identity own
this specific resource?"

### Module placement

Create a new `ownership.rs` module in `alknet-core/src/`, re-export from
`lib.rs`. Follow the same pattern as `store.rs` (which holds
`CredentialStore` + `InMemoryCredentialStore`).

### OwnershipProvider trait (read side, sync)

```rust
/// Read side: consulted by AccessControl::check on the dispatch hot path.
/// Sync — called in the dispatch loop, no .await.
pub trait OwnershipProvider: Send + Sync + 'static {
    /// Does `identity` own `resource_type/resource_id` with `action`?
    /// Called when AccessControl has resource_type + resource_action set
    /// and the dispatcher has extracted resource_id from the input via
    /// OperationSpec.resource_id_path (ADR-050 §2a).
    fn owns(
        &self,
        identity: &Identity,
        resource_type: &str,
        resource_id: &str,
        action: &str,
    ) -> bool;

    /// What resources of `resource_type` does `identity` own?
    /// Called for the `list` case (resource_type set, resource_id_path
    /// absent) — the result-filter path (ADR-050 §4a). Returns the set of
    /// resource IDs the caller owns, for the handler to filter against.
    fn owned_resources(
        &self,
        identity: &Identity,
        resource_type: &str,
    ) -> Vec<String>;

    /// Does `identity` own *any* resource of `resource_type`?
    /// Called for the `list` case — the scope-gate path (ADR-050 §4a).
    fn owns_any(
        &self,
        identity: &Identity,
        resource_type: &str,
    ) -> bool;
}
```

### OwnershipStore trait (write side, async)

```rust
/// Write side: called by the handler that manages the resource lifecycle.
/// Async — not on the dispatch hot path. The handler calls `record` on
/// spawn and `revoke` on teardown (ADR-050 §4b — handler-driven, not a
/// reaper).
#[async_trait]
pub trait OwnershipStore: Send + Sync + 'static {
    /// Record that `identity` spawned `resource_type/resource_id`.
    async fn record(
        &self,
        identity: &Identity,
        resource_type: &str,
        resource_id: &str,
    ) -> Result<(), OwnershipError>;

    /// Revoke ownership of `resource_type/resource_id`.
    /// Called by the handler on resource teardown (ADR-050 §4b).
    async fn revoke(
        &self,
        resource_type: &str,
        resource_id: &str,
    ) -> Result<(), OwnershipError>;
}
```

Note: the ADR-050 sketch shows `record`/`revoke` taking `&mut self`. The
in-memory implementation should use interior mutability (`RwLock` or
`Mutex`, same as `InMemoryCredentialStore` which takes `&self` for its
async methods). The trait takes `&self` so it can be shared as
`Arc<dyn OwnershipStore>`.

### OwnershipError

```rust
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum OwnershipError {
    #[error("backend error: {message}")]
    Backend { message: String },
    #[error("not found: {entity}")]
    NotFound { entity: String },
}
```

Follow the same shape as `StoreError` in `store.rs`. `#[non_exhaustive]`
so future variants (e.g., concurrency conflict) can be added without
breaking consumers.

### InMemoryOwnershipStore

```rust
pub struct InMemoryOwnershipStore {
    // Map: (resource_type, resource_id) → owner Identity
    inner: RwLock<HashMap<(String, String), Identity>>,
}
```

- `owns`: look up `(resource_type, resource_id)`, compare owner's `id`
  to `identity.id`, check that the action is... actually, the ownership
  store records *who owns* a resource, not *what actions they can perform*.
  The `action` parameter in `owns` is the `resource_action` from
  `AccessControl` — it's passed for future use (per-action ownership
  grants), but the base model is "owner can do anything they own." For
  now, `owns` returns `true` if the identity owns the resource, regardless
  of action. The action parameter is accepted but not gated on — this
  preserves the door for per-action grants without building them now.
- `owned_resources`: iterate the map, return IDs where the owner's `id`
  matches.
- `owns_any`: iterate the map, return `true` if any entry matches the
  identity + resource_type.
- `record`: insert `(resource_type, resource_id) → identity`.
- `revoke`: remove `(resource_type, resource_id)`.

### What this task does NOT do

- Does NOT change `AccessControl::check` — that's
  `call/registry/access-control-ownership-check`, which depends on this
  task.
- Does NOT add `resource_id_path` to `OperationSpec` — that's
  `call/registry/operation-spec-resource-id-path`, which is independent.
- Does NOT build a persistence adapter (SQLite/honker-backed) — that's
  additive, built when a concrete use case forces it (ADR-050 §1).
- Does NOT wire the ownership provider into the dispatch path — that's
  `call/registry/dispatch-resource-id-extraction`, which depends on this
  task and the two call tasks.

## Acceptance Criteria

- [ ] `OwnershipProvider` trait with `owns`, `owned_resources`, `owns_any` (all sync)
- [ ] `OwnershipStore` trait with `record`, `revoke` (both async, `#[async_trait]`)
- [ ] `OwnershipError` enum (`#[non_exhaustive]`, `thiserror::Error`) with `Backend` + `NotFound` variants
- [ ] `InMemoryOwnershipStore` implements both `OwnershipProvider` + `OwnershipStore`
- [ ] `InMemoryOwnershipStore` uses interior mutability (`RwLock`, same as `InMemoryCredentialStore`)
- [ ] `owns` returns true if identity owns the resource (action accepted but not gated — base model is owner-can-do-anything)
- [ ] `owned_resources` returns the set of resource IDs the identity owns for a given type
- [ ] `owns_any` returns true if identity owns at least one resource of the type
- [ ] `record` inserts ownership; `revoke` removes it
- [ ] New `ownership.rs` module in `alknet-core/src/`
- [ ] Re-exported from `lib.rs`: `pub use ownership::{OwnershipProvider, OwnershipStore, InMemoryOwnershipStore, OwnershipError};`
- [ ] Unit tests: record → owns → revoke → not owns round-trip
- [ ] Unit test: owned_resources returns correct set for an owner with multiple resources
- [ ] Unit test: owns_any returns false for an owner with no resources of that type
- [ ] Unit test: revoke on non-existent resource is a no-op (or NotFound — pick one and document)
- [ ] `cargo test -p alknet-core` succeeds
- [ ] `cargo clippy -p alknet-core` succeeds with no warnings

## References

- docs/architecture/crates/core/auth.md — "Ownership Provider and Store (ADR-050)" section
- docs/architecture/decisions/050-dynamic-resource-ownership-for-runtime-spawned-resources.md — ADR-050 (the full decision)
- docs/architecture/decisions/033-storage-boundary-and-repo-adapter-pattern.md — ADR-033 (the pattern)
- crates/alknet-core/src/store.rs — `CredentialStore` + `InMemoryCredentialStore` (the pattern to follow)

## Notes

> The read/write split mirrors ADR-035: `OwnershipProvider` (read, sync) is
> the trait the dispatch path depends on; `OwnershipStore` (write, async)
> is the trait the handler lifecycle calls. The in-memory default
> implements both. The `owns` method accepts an `action` parameter but
> doesn't gate on it — the base model is "owner can do anything they own."
> Per-action grants are a future extension (additive, not needed now). The
> `action` parameter preserves the door without building the mechanism. A
> persistence adapter (SQLite/honker-backed) is not built in this sync —
> ownership is runtime state, meaningless across restarts; the in-memory
> default is sufficient for the docker/runner cases.