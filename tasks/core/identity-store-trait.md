---
id: core/identity-store-trait
name: Add IdentityStore async write trait extending IdentityProvider (ADR-035)
status: pending
depends_on: [core/peer-entry-model]
scope: single
risk: low
impact: component
level: implementation
---

## Description

Add the `IdentityStore` async write trait for peer management, extending the
read-only `IdentityProvider` trait. Per ADR-035 §2.

`IdentityProvider` is read-only today and stays read-only — it is the hot-path
trait called on every incoming connection (sync, no `.await`). Peer mutations
(add/update/remove a `PeerEntry`) go through this separate async trait.

### IdentityStore trait

```rust
/// Write trait — management path, async (ADR-035). ConfigIdentityProvider
/// does NOT implement this (config reload is its write path — see below).
/// SqliteIdentityProvider does: writes hit SQLite, emit honker NOTIFY,
/// and the local LISTEN refreshes the in-memory read index.
#[async_trait]
pub trait IdentityStore: IdentityProvider {
    async fn put_peer(&self, peer: &PeerEntry) -> Result<(), StoreError>;
    async fn update_peer(&self, peer_id: &str, peer: &PeerEntry) -> Result<(), StoreError>;
    async fn remove_peer(&self, peer_id: &str) -> Result<(), StoreError>;
}
```

- `put_peer` — insert or replace a `PeerEntry` (upsert by `peer_id`).
- `update_peer` — update an existing `PeerEntry` (error if `peer_id` not found;
  for upsert semantics use `put_peer`).
- `remove_peer` — delete a `PeerEntry` by `peer_id`.

### Why a separate trait, not async methods on IdentityProvider

- The hot-path read trait is consumed by the accept loop and every handler —
  those call sites are sync and must not gain `.await`. If `put_peer` were on
  `IdentityProvider`, every consumer would see the async method even though
  only the management path calls it. A separate `IdentityStore: IdentityProvider`
  supertrait keeps the read surface lean and makes the write surface opt-in.
- `ConfigIdentityProvider` does **not** implement `IdentityStore`. Its write
  path is config reload (`ConfigReloadHandle::reload`), not a method call. This
  preserves the config-is-source-of-truth model. Implementing `IdentityStore`
> for `ConfigIdentityProvider` "for symmetry" would violate that model — the
> constraint is the absence of a backend, not a type-system constraint.

### ConfigIdentityProvider posture

`ConfigIdentityProvider` deliberately does NOT implement `IdentityStore`. This
task does not change `ConfigIdentityProvider` — it only adds the trait. The
trait is defined for future adapters (`SqliteIdentityProvider` in
`alknet-store-sqlite`) to implement. `StoreError` is already defined by
`core/credential-store-trait`.

### Module placement

Add `IdentityStore` alongside `IdentityProvider` in `alknet-core/src/auth.rs`
(or a new `store` module if `CredentialStore` landed there). Re-export from
`lib.rs`.

## Acceptance Criteria

- [ ] `IdentityStore` trait with `put_peer`, `update_peer`, `remove_peer` (all async)
- [ ] `IdentityStore: IdentityProvider` (supertrait)
- [ ] `StoreError` used as the error type (from `core/credential-store-trait`)
- [ ] `ConfigIdentityProvider` does NOT implement `IdentityStore`
- [ ] `#[async_trait]` on the trait
- [ ] No changes to `IdentityProvider` trait (stays read-only, sync)
- [ ] Unit test: a mock/test impl of `IdentityStore` compiles and works (verify the trait is implementable)
- [ ] Unit test: `ConfigIdentityProvider` does not implement `IdentityStore` (compile-time or trait-bound assertion)
- [ ] `cargo test -p alknet-core` succeeds
- [ ] `cargo clippy -p alknet-core` succeeds with no warnings

## References

- docs/architecture/crates/core/auth.md — IdentityStore write trait, ConfigIdentityProvider posture
- docs/architecture/decisions/035-concrete-persistence-adapter-shapes.md — ADR-035 §2 (the trait, read/write split rationale)
- docs/architecture/decisions/033-storage-boundary-and-repo-adapter-pattern.md — ADR-033 (the pattern)

## Notes

> Small task but locks the trait shape — a one-way door. The read/write split
> keeps the hot path sync (no `.await` in the accept loop). `ConfigIdentityProvider`
> not implementing `IdentityStore` is a design posture, not a type-system
> constraint: it holds no backend, and its write path is config reload. A
> deployment that wants method-call peer management wires the SQLite adapter
> (a separate crate, not built in this sync).

## Summary

> To be filled on completion