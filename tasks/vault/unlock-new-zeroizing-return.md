---
id: vault/unlock-new-zeroizing-return
name: Change unlock_new return type from String to Zeroizing<String>
status: completed
depends_on: [vault/irpc-removal]
scope: single
risk: low
impact: isolated
level: implementation
---

## Description

Fix drift item #8: `unlock_new` currently returns `String`, which is not
zeroized on drop. The mnemonic phrase is the root of trust — it must not linger
in freed heap memory. Change the return type to `Zeroizing<String>` (from the
`zeroize` crate, already a dependency).

### Current state

```rust
pub fn unlock_new(&self, word_count: usize) -> Result<String, VaultServiceError>;
```

### Target state

```rust
pub fn unlock_new(&self, word_count: usize) -> Result<Zeroizing<String>, VaultServiceError>;
```

Per `docs/architecture/crates/vault/service.md` → unlock_new:

> The returned phrase is the root of trust — it is heap-allocated and zeroized
> on drop, so it does not linger in freed memory. The caller should extract the
> phrase for secure storage (write down, display to user) and let the
> `Zeroizing<String>` drop when done. Do not clone the returned value or store
> it in a non-zeroizing container.

### Caller adaptation

The assembly layer (CLI binary, not yet implemented) will call `unlock_new` and
extract the phrase. The `Zeroizing<String>` wrapper derefs to `String`, so
`&*result` or `result.as_str()` works for reading. The caller must not clone the
inner `String` into a non-zeroizing container.

Existing tests that call `unlock_new` need updating to handle the new return
type — use `&*phrase` or `phrase.as_str()` to read the string.

### Scope

This task touches `service.rs` (the method signature and body) and test files.
It depends on the irpc removal task (drift #4) because both modify `service.rs`.

## Acceptance Criteria

- [ ] `unlock_new` return type changed from `Result<String, ...>` to `Result<Zeroizing<String>, ...>`
- [ ] Method body constructs `Zeroizing<String>` from the generated phrase
- [ ] Existing tests updated to handle `Zeroizing<String>` return type
- [ ] No `clone()` of the returned value in non-test code
- [ ] `cargo check` succeeds
- [ ] `cargo test` succeeds
- [ ] `cargo clippy` succeeds with no warnings

## References

- docs/architecture/crates/vault/README.md — Known Source Drift table item #8
- docs/architecture/crates/vault/service.md — unlock_new section
- docs/architecture/decisions/025-vault-local-only-dispatch.md — ADR-025 (resolves W7)

## Notes

> The mnemonic is the root of trust. Returning a plain `String` means the phrase
> lingers in freed heap memory after the caller drops it. `Zeroizing<String>`
> zeroizes the bytes on drop. This resolves review #002 W7. Depends on irpc
> removal because both modify `service.rs`.

## Summary

Changed `unlock_new` return type from `Result<String, ...>` to
`Result<Zeroizing<String>, ...>`. The generated mnemonic phrase is wrapped in
`Zeroizing::new()` so it is zeroized on drop. Existing tests work via Deref
coercion. All tests pass; clippy clean. Merged to develop.