---
id: vault/poisoned-lock-recovery
name: Replace unwrap() on RwLock acquisition with poisoned-lock recovery via unwrap_or_else
status: completed
depends_on: [vault/irpc-removal]
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Fix drift item #2: `VaultServiceHandle` methods use `unwrap()` on every
`RwLock` acquisition (read and write locks). A poisoned lock (caused by a panic
while the lock was held) would brick the vault for all subsequent operations.
Replace with `unwrap_or_else(|e| e.into_inner())` to recover the inner data from
a poisoned lock, or explicit error propagation where appropriate.

### Current state

`service.rs` uses `.unwrap()` on `RwLock` read and write acquisitions at
approximately lines 142, 161, 182, 191, 196, 227, 264, 307, 340, 367 (line
numbers may shift after the irpc removal task — match by pattern: every
`.read().unwrap()` and `.write().unwrap()` call in `VaultServiceHandle` method
bodies).

### Target state

For read locks:
```rust
let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
```

For write locks:
```rust
let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
```

The rationale: a poisoned lock means a panic occurred while the lock was held.
The data may be in an inconsistent state, but bricking the vault (panicking on
every subsequent call) is worse than attempting to continue. The vault's
operations are idempotent reads (derive) and state transitions (lock/unlock) —
recovering the inner data and continuing is the pragmatic choice. If the data
is truly corrupted, the next operation will fail with a normal error, not a
panic.

### No unwrap() or expect() outside tests

This is a general constraint for the vault: no `unwrap()` or `expect()` outside
test code. After fixing the RwLock acquisitions, audit the rest of `service.rs`
for any remaining `unwrap()`/`expect()` calls and replace them with proper error
propagation (`?` operator, explicit `Result` returns, or
`unwrap_or_else(|e| e.into_inner())` for lock recovery).

### Scope

This task touches `service.rs` only. It depends on the irpc removal task (drift
#4) because that task restructures `service.rs` — doing this first would cause
merge conflicts.

## Acceptance Criteria

- [ ] All `.read().unwrap()` calls in `VaultServiceHandle` methods replaced with `.read().unwrap_or_else(|e| e.into_inner())`
- [ ] All `.write().unwrap()` calls in `VaultServiceHandle` methods replaced with `.write().unwrap_or_else(|e| e.into_inner())`
- [ ] No `unwrap()` or `expect()` calls remain in `service.rs` outside of test code
- [ ] Unit test: vault remains usable after a simulated panic (poison the lock, verify next call recovers)
- [ ] `cargo test` succeeds
- [ ] `cargo clippy` succeeds with no warnings

## References

- docs/architecture/crates/vault/README.md — Known Source Drift table item #2
- docs/architecture/crates/vault/service.md — Security Constraints: No unwrap() outside tests
- docs/architecture/decisions/025-vault-local-only-dispatch.md — ADR-025

## Notes

> A panic in one vault operation must not brick the vault for all other
> operations. The poisoned-lock recovery via `unwrap_or_else(|e| e.into_inner())`
> is the standard Rust pattern for this. This task depends on the irpc removal
> task because both modify `service.rs` heavily.

## Summary

Replaced all 10 `.read().unwrap()`/`.write().unwrap()` calls in
`VaultServiceHandle` methods with `.unwrap_or_else(|e| e.into_inner())` for
poisoned-lock recovery. Added `test_poisoned_lock_recovery` that poisons the
lock via a panicking thread and verifies the vault remains usable. No
`unwrap()`/`expect()` remain outside test code. Merged to develop (resolved
conflicts with remove-password-derivation and unlock-new-zeroizing-return).