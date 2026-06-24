---
id: vault/mnemonic-debug-redaction
name: Replace Mnemonic derive(Debug) with redacting impl to prevent seed phrase leak
status: pending
depends_on: []
scope: single
risk: low
impact: isolated
level: implementation
---

## Description

`Mnemonic` (crates/alknet-vault/src/mnemonic.rs:35) currently derives
`Debug`:

```rust
#[derive(Debug)]
pub struct Mnemonic {
    inner: Bip39Mnemonic,
    phrase: String,
}
```

The derived `Debug` prints both fields, including `phrase: "abandon
abandon abandon ..."`. The crate's own module doc (mnemonic.rs:8) says
"Seed material is protected with `Zeroize`" and the `Mnemonic::phrase()`
accessor (line 82) warns "Handle with care — this is the root of trust
for all derived keys." The `Zeroize + Drop` impls (lines 88–99) wipe
the phrase from memory on drop — but `Debug` will happily hand the
phrase to any `tracing::debug!`, `format!("{:?}")`, panic backtrace,
or error-context printer that touches a `Mnemonic` value.

This is inconsistent with the rest of the crate's secret handling:
- `DerivedKey` has a custom redacting `Debug` (protocol.rs:48–56)
  printing `private_key: "[REDACTED]"`.
- `EncryptionKey` has a custom redacting `Debug` (encryption.rs:132–139).
- `Capabilities` has a redacting `Debug` (types.rs:112–118).
- `Secret<T>` has a redacting `Debug` (types.rs:51–55).
- `Mnemonic` ... derives `Debug` and prints the phrase.

The root of trust should never have a `Debug` impl that can print it.
A future debugging session that adds `tracing::debug!(?mnemonic, ...)`
would land the seed phrase in a log file.

### Resolution

Replace `#[derive(Debug)]` with a manual impl that redacts, matching
the pattern used for `DerivedKey`:

```rust
impl std::fmt::Debug for Mnemonic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mnemonic")
            .field("phrase", &"[REDACTED]")
            .finish()
    }
}
```

Also check `Seed` (mnemonic.rs:107–129) — it derives
`#[derive(Clone, Zeroize)]` with `#[zeroize(drop)]` but does not derive
`Debug`. Verify it has no `Debug` impl that prints `bytes`; if it does
(or if a future `derive(Debug)` would), add a redacting impl there too.

### Test

Add a test asserting `format!("{:?}", mnemonic)` does not contain any
word from the generated phrase. Pattern after
`test_derived_key_debug_redacts_private_key` (protocol.rs:106–118):

```rust
#[test]
fn test_mnemonic_debug_redacts_phrase() {
    let mnemonic = Mnemonic::generate(24).unwrap();
    let debug_output = format!("{:?}", mnemonic);
    assert!(
        debug_output.contains("[REDACTED]"),
        "Debug must show [REDACTED] for phrase, got: {debug_output}"
    );
    for word in mnemonic.phrase().split_whitespace() {
        assert!(
            !debug_output.contains(word),
            "Debug must not leak phrase word '{word}', got: {debug_output}"
        );
    }
}
```

## Acceptance Criteria

- [ ] `Mnemonic` no longer derives `Debug`; has a manual redacting `Debug` impl
- [ ] `format!("{:?}", mnemonic)` contains `"[REDACTED]"` and no phrase word
- [ ] `Seed` checked — either has no `Debug` impl, or has a redacting one
- [ ] Unit test: `test_mnemonic_debug_redacts_phrase` passes
- [ ] Existing tests still pass (`cargo test -p alknet-vault`)
- [ ] `cargo clippy -p alknet-vault --all-targets` succeeds with no warnings

## References

- docs/reviews/004-post-implementation-sanity-check.md — W3 (full finding)
- crates/alknet-vault/src/protocol.rs:48–56 — `DerivedKey` redacting `Debug` (pattern to follow)
- crates/alknet-vault/src/encryption.rs:132–139 — `EncryptionKey` redacting `Debug`
- crates/alknet-core/src/types.rs:51–55 — `Secret<T>` redacting `Debug`

## Notes

> Small fix, but eliminates a latent root-of-trust leak. The same
> pattern (custom redacting `Debug`) is already established in three
> other places in this codebase — this task brings `Mnemonic` in line.