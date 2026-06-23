---
id: vault/derivedkey-serialization
name: Implement always-redact DerivedKey serialization and reject redacted payloads on deserialize
status: pending
depends_on: [vault/irpc-removal]
scope: narrow
risk: medium
impact: component
level: implementation
---

## Description

Fix drift item #5: `DerivedKey` currently has dual serialization behavior — JSON
redacts the private key, but postcard (the binary format used by irpc) preserves
the raw bytes. ADR-025 dropped the postcard/remote path, so `DerivedKey` should
**always** redact on serialize and reject `"[REDACTED]"` on deserialize with an
explicit error.

### Current state

`protocol.rs` has `DerivedKey` with `#[derive(Serialize, Deserialize)]` (or
similar) that produces JSON redaction for JSON but preserves bytes in postcard.
The postcard tests in the test suite verify the binary round-trip.

### Target state

Per `docs/architecture/crates/vault/protocol.md` → Serialization Redaction:

`DerivedKey` must **not** derive `Deserialize` via `#[derive]`. It needs custom
`Serialize` and `Deserialize` impls:

**Custom Serialize** — always redacts `private_key`:
```rust
impl serde::Serialize for DerivedKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where S: serde::Serializer {
        use serde::SerializeStruct;
        let mut s = serializer.serialize_struct("DerivedKey", 3)?;
        s.serialize_field("key_type", &self.key_type)?;
        s.serialize_field("private_key", "[REDACTED]")?;
        s.serialize_field("public_key", &self.public_key)?;
        s.end()
    }
}
```

**Custom Deserialize** — rejects `"[REDACTED]"` with an explicit error:
```rust
impl<'de> serde::Deserialize<'de> for DerivedKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where D: serde::Deserializer<'de> {
        #[derive(serde::Deserialize)]
        struct DerivedKeyHelper {
            key_type: KeyType,
            private_key: Vec<u8>,
            public_key: Vec<u8>,
        }
        let helper = DerivedKeyHelper::deserialize(deserializer)?;
        if helper.private_key == b"[REDACTED]" {
            return Err(serde::de::Error::custom(
                "DerivedKey.private_key is \"[REDACTED]\" — redacted payloads \
                 cannot be deserialized. JSON round-tripping a DerivedKey is \
                 not supported (the private key is gone)."
            ));
        }
        Ok(DerivedKey {
            key_type: helper.key_type,
            private_key: helper.private_key,
            public_key: helper.public_key,
        })
    }
}
```

**Debug impl** — also redacts:
```rust
impl fmt::Debug for DerivedKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DerivedKey")
            .field("key_type", &self.key_type)
            .field("private_key", &"[REDACTED]")
            .field("public_key", &self.public_key)
            .finish()
    }
}
```

### Remove postcard tests

The postcard round-trip tests (which verified binary format preserved private
key bytes) are removed — ADR-025 dropped that path. The `postcard`
dev-dependency was removed in the irpc removal task (drift #4).

### Why custom impls instead of derives

A derived `Deserialize` would generate a default impl that conflicts with the
manual one, and would only fail incidentally (serde type mismatch: string vs
sequence), not with the explicit redaction-rejection error the spec requires.
The custom impl is required for the explicit error message.

### Scope

This task touches `protocol.rs` (the `DerivedKey` type, its serde impls, Debug
impl) and test files (remove postcard tests, add redaction tests). It depends on
the irpc removal task (drift #4) because both modify `protocol.rs`.

## Acceptance Criteria

- [ ] `DerivedKey` does not derive `Serialize` or `Deserialize` via `#[derive]`
- [ ] Custom `Serialize` impl always redacts `private_key` as `"[REDACTED]"`
- [ ] Custom `Deserialize` impl rejects `private_key == b"[REDACTED]"` with explicit error
- [ ] Custom `Debug` impl redacts `private_key` as `"[REDACTED]"`
- [ ] Postcard round-trip tests removed
- [ ] Unit test: JSON serialize produces `"[REDACTED]"` for `private_key`
- [ ] Unit test: JSON deserialize of a redacted payload returns an error (not a corrupted key)
- [ ] Unit test: `{:?}` on `DerivedKey` does not contain private key bytes
- [ ] `cargo test` succeeds
- [ ] `cargo clippy` succeeds with no warnings

## References

- docs/architecture/crates/vault/README.md — Known Source Drift table item #5
- docs/architecture/crates/vault/protocol.md — Serialization Redaction, Debug redaction
- docs/architecture/decisions/025-vault-local-only-dispatch.md — ADR-025 (resolves W8)
- docs/architecture/decisions/014-secret-material-flow-and-capability-injection.md — ADR-014

## Notes

> The redaction is defense-in-depth for logging safety, not the primary control
> — the primary control is that `DerivedKey` never crosses the call protocol
> wire (ADR-014). ADR-025 dropped the postcard/remote path that previously
> preserved bytes in binary formats. The custom Deserialize impl is required
> because `#[derive(Deserialize)]` would conflict and not produce the explicit
> redaction-rejection error. Depends on irpc removal because both modify
> `protocol.rs`.

## Summary

> To be filled on completion