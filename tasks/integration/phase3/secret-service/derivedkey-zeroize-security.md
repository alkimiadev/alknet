---
id: derivedkey-zeroize-security
name: Make DerivedKey private_key Zeroize-derived and fix clone semantics for ADR-038 compliance
status: pending
depends_on: [spec-update-secret-service]
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

The `DerivedKey` struct in `protocol.rs` carries `private_key: Vec<u8>` which is sensitive key material, but it derives `Clone` and `Serialize`/`Deserialize` without any zeroize protection. Per ADR-038, all sensitive material must implement `Zeroize` and be zeroized on drop.

The current code:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DerivedKey {
    pub key_type: KeyType,
    pub private_key: Vec<u8>,  // SENSITIVE — must zeroize
    pub public_key: Vec<u8>,   // Not sensitive (public)
}
```

Problems:
1. `private_key` doesn't derive `Zeroize` — it stays in memory after `DerivedKey` is dropped
2. `Clone` copies `private_key` without zeroizing the source — if the clone is dropped, one copy may linger
3. The struct is `Serialize` — serializing `private_key` to JSON is a potential leak vector (logging, debug output)

**Fix approach:**

- Make `DerivedKey` implement `Zeroize` with `#[zeroize(drop)]`
- Replace `#[derive(Clone)]` with a manual `Clone` impl that zeroizes the source's `private_key` after copying (move semantics through clone — the source key is consumed, not left in memory)
- OR change the API to return `DerivedKey` by value only (no Clone) — consumers get one copy and must zeroize it when done. This is the more conventional crypto API pattern.
- Add `#[serde(skip_serializing)]` or a custom serializer that redacts `private_key` from JSON output (or use a dedicated display format that shows only the public key)
- `KeyType` and `public_key` are not sensitive and can remain as-is

**Important**: This change affects the `SecretServiceHandle` methods that return `DerivedKey`. If `DerivedKey` becomes non-Clone, those methods must return `DerivedKey` by value (which they already do — they construct a new `DerivedKey` each time). The key caching task (which adds a cache) will need to handle this carefully.

## Acceptance Criteria

- [ ] `DerivedKey` derives `Zeroize` with `#[zeroize(drop)]` on the `private_key` field
- [ ] `DerivedKey` does NOT derive `Clone` — it's a move-only type (consumers must zeroize when done)
- [ ] `DerivedKey` serialization redacts `private_key` — JSON output shows a placeholder (e.g., `"[REDACTED]"`) instead of key bytes
- [ ] `DerivedKey::zeroize()` overwrites `private_key` with zeros
- [ ] `Drop` for `DerivedKey` calls `zeroize()` on the `private_key` field
- [ ] Existing `SecretServiceHandle` methods compile without `Clone` (they already return `DerivedKey` by value)
- [ ] Unit test: `DerivedKey` zeroes `private_key` on drop
- [ ] Unit test: `DerivedKey` serialization does NOT contain `private_key` bytes
- [ ] ADR-038 compliance: all types holding private key material derive `Zeroize`

## References

- docs/architecture/secret-service.md — DerivedKey specification
- docs/architecture/decisions/038-seed-lifecycle-memory-security.md — ADR-038
- crates/alknet-secret/src/protocol.rs — Current DerivedKey definition
- crates/alknet-secret/src/derivation.rs — ExtendedPrivKey (already Zeroize-derived)

## Notes

> The `ExtendedPrivKey` in `derivation.rs` already correctly implements `Zeroize` with `#[zeroize(drop)]`. This task brings the same security discipline to `DerivedKey`.

> Making `DerivedKey` non-Clone is the safer choice. In crypto APIs, returning key material by value and requiring explicit zeroization is the standard pattern. The key cache (in the caching task) will hold derived keys in an internal cache type, not in `DerivedKey` directly.

> For serialization redaction: consider a custom `Serialize` impl that serializes `private_key` as `"[REDACTED]"` for JSON display but `Deserialize` still reads the full bytes for protocol use. Alternatively, `private_key` can be skipped entirely in serialization (since `DerivedKey` is intended for local use, not wire transfer — the irpc protocol sends `DerivedKey` through postcard, not JSON). The key cache task will need to handle this.

## Summary

> To be filled on completion