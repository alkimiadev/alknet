---
id: vault/cache-zeroization-test
name: Verify and test that HashMap::clear() drops CachedKey values triggering zeroization
status: completed
depends_on: []
scope: single
risk: low
impact: isolated
level: implementation
---

## Description

Fix drift item #6: `KeyCache::clear()` removes entries and relies on
`CachedKey`'s `Drop` impl for zeroization. The spec says to verify that
`HashMap::clear()` actually drops the values (it does, but this is worth a
test). This task adds a test that proves zeroization happens on cache eviction
and clear.

### Background

`CachedKey` derives `Zeroize` and `ZeroizeOnDrop` (via the `DerivedKey` it
holds, which is `#[zeroize(drop)]`). When the cache evicts an entry (LRU or TTL)
or `clear()` is called, the `CachedKey` is dropped, which triggers
`ZeroizeOnDrop` — the private key bytes are zeroized before deallocation.

`HashMap::clear()` drops all values, which triggers their `Drop` impls. This
is standard Rust behavior, but the security-critical nature of key material
warrants an explicit test.

### What to add

A test in `cache.rs` (or `tests/`) that:

1. Inserts a `CachedKey` with a known private key into the cache
2. Verifies the key is present
3. Calls `clear()` (or evicts via LRU/TTL)
4. Verifies the `CachedKey` was dropped and zeroized

Testing zeroization directly is tricky because the memory is freed — you can't
easily inspect it after drop. A practical approach:

- **Option A**: Use a custom type with a `Drop` impl that sets a flag (e.g., an
  `Arc<AtomicBool>`) when zeroized. Insert it into the cache, clear, verify the
  flag is set. This tests the drop path, not the zeroize path directly, but
  confirms `clear()` drops values.
- **Option B**: Test the LRU eviction path — fill the cache to `max_entries`,
  insert one more, verify the LRU entry was evicted (dropped).
- **Option C**: Test that `lock()` calls `cache.clear()` and the cache is empty
  afterward (integration test via `VaultServiceHandle`).

At minimum, implement Option B and C. Option A is a bonus if feasible without
over-engineering the test type.

### Scope

This task touches `cache.rs` (test additions) and possibly `tests/`. It does
not depend on the irpc removal task (drift #4) because `cache.rs` is a separate
file. It can run in parallel with drift #4.

## Acceptance Criteria

- [ ] Test: LRU eviction drops the evicted `CachedKey` (cache exceeds `max_entries`, oldest evicted)
- [ ] Test: `lock()` clears the cache (verify cache is empty after lock)
- [ ] Test: TTL expiry evicts entries (set short TTL, wait, verify entry gone)
- [ ] Test: `clear()` removes all entries (verify empty after clear)
- [ ] `cargo test` succeeds
- [ ] `cargo clippy` succeeds with no warnings

## References

- docs/architecture/crates/vault/README.md — Known Source Drift table item #6
- docs/architecture/crates/vault/service.md — Cache section, Security Constraints
- docs/architecture/crates/vault/encryption.md — Security Constraints

## Notes

> `HashMap::clear()` does drop values, triggering their `Drop` impls. This is
> standard Rust behavior, but key material is security-critical enough to
> warrant an explicit test. This task touches only `cache.rs` and can run in
> parallel with the irpc removal task (drift #4).

## Summary

Added a `drop_tracker` test module proving `HashMap::clear()`/`remove()`/`insert`
(replace) drop values triggering their `Drop` impls, plus explicit tests for LRU
eviction (`test_lru_eviction_drops_evicted_cached_key`), TTL expiry
(`test_ttl_expiry_evicts_entry_on_access`), and `clear()`
(`test_clear_removes_all_entries_and_empties_cache`). The lock()-clears-cache
criterion is covered by existing `test_lock_clears_all_cache_entries` in
service.rs. All lib + integration tests pass; clippy clean. Merged to develop.