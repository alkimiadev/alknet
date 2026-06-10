---
id: unlock-passphrase-gap
name: Fix Unlock protocol variant to carry both mnemonic and BIP39 passphrase
status: pending
depends_on: [irpc-secret-protocol-integration]
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

The `Unlock` variant in `SecretProtocol` currently accepts only a single
`passphrase: String` field, which is used as the mnemonic phrase. The
`SecretServiceHandle::unlock()` method takes two parameters —
`phrase: &str` (the mnemonic) and `passphrase: Option<&str>` (the optional
BIP39 password extension) — but the irpc `Unlock` message cannot convey the
BIP39 passphrase.

The `SecretServiceActor::handle_message()` works around this by passing
`passphrase` as the mnemonic and `None` for the BIP39 passphrase:

```rust
SecretMessage::Unlock(msg) => {
    let Unlock { passphrase } = inner;
    let result = self.handle.unlock(&passphrase, None);
```

This means users who protect their mnemonic with a BIP39 passphrase (the
"25th word") cannot unlock the service via irpc. This is a protocol gap
that should be fixed before Phase A integration, since the wire format
becomes harder to change once consumers depend on it.

### What needs to happen

1. **Update `Unlock` variant in `protocol.rs`** to carry both fields:

   ```rust
   #[rpc(tx = oneshot::Sender<Result<(), SecretServiceError>>)]
   #[wrap(Unlock)]
   Unlock {
       /// The BIP39 mnemonic phrase (space-separated word list).
       mnemonic: String,
       /// Optional BIP39 passphrase (the "25th word" password extension).
       passphrase: Option<String>,
   },
   ```

2. **Update `SecretServiceActor::handle_message()`** to pass both fields:

   ```rust
   SecretMessage::Unlock(msg) => {
       let Unlock { mnemonic, passphrase } = inner;
       let result = self.handle.unlock(&mnemonic, passphrase.as_deref());
   ```

3. **Update the `Unlock` wrap struct** that `#[wrap]` generates — this is
   automatic via the `#[wrap(Unlock)]` attribute, so no manual change needed
   beyond updating the enum variant.

4. **Update the `unlock_new()` method's irpc path** (if any) — currently
   `unlock_new()` generates a random mnemonic and returns the phrase. There is
   no irpc variant for this; it's a local-only operation. No change needed.

5. **Add a test**: Verify that `Unlock { mnemonic, passphrase: Some("TREZOR") }`
   produces a different seed than `Unlock { mnemonic, passphrase: None }`.

6. **Verify backward compatibility**: Since `Unlock` is a new variant field,
   postcard deserialization of old messages (with only `passphrase`) will fail.
   This is acceptable because the secret service has not been deployed yet —
   there are no existing wire consumers. The protocol is still in development.

## Acceptance Criteria

- [ ] `Unlock` variant in `protocol.rs` has `mnemonic: String` and
      `passphrase: Option<String>` fields
- [ ] `SecretServiceActor::handle_message()` passes both fields to
      `SecretServiceHandle::unlock()`
- [ ] Unit test: Unlock with mnemonic + passphrase produces different seed
      than Unlock with same mnemonic + no passphrase
- [ ] Unit test: Unlock with mnemonic + None passphrase works (backward compat
      for the common case)
- [ ] Unit test: Unlock via `SecretMessage` (irpc actor path) with both fields
      works correctly
- [ ] `cargo test -p alknet-secret --all-features` passes
- [ ] `cargo clippy -p alknet-secret --all-features -- -D warnings` passes
- [ ] `cargo fmt -p alknet-secret -- --check` passes

## References

- docs/architecture/secret-service.md — SecretProtocol section (updated spec)
- crates/alknet-secret/src/protocol.rs — `Unlock` variant (current: single field)
- crates/alknet-secret/src/service.rs — `SecretServiceHandle::unlock()` and
  `SecretServiceActor::handle_message()`

## Notes

> This is a wire format change. Since alknet-secret is not yet deployed and has
> no existing consumers, this is safe to change now. After Phase A integration,
> wire format changes would require versioning or migration.

> The spec already reflects the target state (`Unlock { mnemonic, passphrase }`).
> This task brings the implementation in line with the spec.

> The `Unlock { passphrase: String }` field name was misleading — "passphrase"
> sounds like the BIP39 password, but it was actually the mnemonic phrase. The
> new field names are unambiguous: `mnemonic` is the word list, `passphrase` is
> the optional BIP39 password extension.