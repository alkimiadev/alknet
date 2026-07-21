---
id: typedef/tunion
name: Implement TUnion discriminator dispatch for byte-offset and field-name discriminators
status: pending
depends_on: [typedef/schema-types, typedef/error-type, typedef/data-access]
scope: narrow
risk: medium
impact: component
level: implementation
---

## Description

Implement TUnion discriminator dispatch in `crates/alknet-typedef/src/tunion.rs`.
TUnion supports two discriminator kinds (ADR-097 §4): byte-offset (protocol dispatch,
e.g., SFTP type bytes) and field-name (typedef.ts string pattern).

Per [data-access.md](../../docs/architecture/crates/typedef/data-access.md) §"TUnion Dispatch"
and [schema-layer.md](../../docs/architecture/crates/typedef/schema-layer.md) §"TUnion discriminators".

### Target shape

```rust
/// The result of reading a TUnion discriminator.
#[derive(Debug, Clone)]
pub struct UnionDispatch {
    /// The mapping key (stringified discriminator value).
    pub key: String,
    /// The byte offset where the variant struct starts.
    pub variant_offset: usize,
    /// The size of the discriminator in bytes.
    pub discriminator_size: usize,
}

/// Read the discriminator value from a byte-offset TUnion.
/// The discriminator is a fixed-size integer at a known byte offset.
/// Returns the mapping key (as a string) and the variant struct offset.
///
/// This is the SFTP `Packet` enum pattern — byte 0 is the type byte,
/// bytes 1..N are the variant struct. The call protocol's event type
/// dispatch uses the same pattern.
pub fn read_byte_discriminator(
    buffer: &[u8],
    union_schema: &serde_json::Value,
    endian: Endian,
) -> Result<UnionDispatch, TypedefError>;

/// Read the discriminator value from a field-name TUnion.
/// The discriminator is a named field within the struct — its offset
/// is computed like any other field. The consumer provides the
/// discriminator field's offset (from the OffsetMap or LayoutBuilder).
///
/// This is the typedef.ts `TUnion` pattern — the discriminator is a
/// field like any other, and the mapping keys are string values.
pub fn read_field_discriminator(
    buffer: &[u8],
    union_schema: &serde_json::Value,
    disc_field_offset: usize,
    endian: Endian,
) -> Result<UnionDispatch, TypedefError>;

/// Look up a variant schema from the union's mapping.
/// Returns the variant schema (resolving `$ref` if needed).
pub fn resolve_variant<'a>(
    union_schema: &'a serde_json::Value,
    key: &str,
) -> Result<&'a serde_json::Value, TypedefError>;

/// Get the discriminator size in bytes for a byte-offset discriminator.
pub fn discriminator_size(union_schema: &serde_json::Value) -> Result<usize, TypedefError>;
```

### Byte-offset discriminator

The discriminator is a fixed-size integer at a known byte offset. The mapping keys
are stringified integers (`"5"`, `"6"`, `"101"`).

Supported discriminator types: `TypeDef:Uint8` (1 byte), `TypeDef:Uint16` (2 bytes),
`TypeDef:Uint32` (4 bytes). The discriminator value is read using the appropriate
endian-aware read function from `data_access`.

The variant struct starts at `offset + discriminator_size`.

### Field-name discriminator

The discriminator is a named field within the struct. Its offset is computed like any
other field (by the `OffsetMap` or `LayoutBuilder`). The mapping keys are string
values matching the discriminator field's value.

The discriminator field's `TypeDef:*` kind determines how to read it:
- `TypeDef:String` → read a length-prefixed string
- `TypeDef:Uint8` → read a u8, stringify
- `TypeDef:Enum` → read a u32 index, map to string value from `"enum"` array

### Variant resolution

`resolve_variant()` looks up the mapping key in the union's `"mapping"` object.
Mapping values may be inline schemas or `$ref` pointers. `$ref` pointers are resolved
against the schema's `$defs` (the `$ref` normalization in `schema.rs` ensures they
are full JSON Pointer paths).

### What this does NOT include

- The offset computation for TUnion (that's in `offset_map.rs` and `layout_builder.rs`)
- The `TypedefEngine` struct (that's `engine.rs`)

## Acceptance Criteria

- [ ] `read_byte_discriminator()` reads discriminator at known byte offset
- [ ] Supports `TypeDef:Uint8`, `TypeDef:Uint16`, `TypeDef:Uint32` discriminator types
- [ ] Discriminator value read with correct endianness
- [ ] Returns mapping key as string (e.g., `"5"` for SFTP Read packet)
- [ ] Returns correct `variant_offset` (offset + discriminator_size)
- [ ] `read_field_discriminator()` reads discriminator from named field at given offset
- [ ] Supports `TypeDef:String`, `TypeDef:Uint8`, `TypeDef:Enum` discriminator field types
- [ ] `resolve_variant()` looks up mapping key and returns variant schema
- [ ] `resolve_variant()` resolves `$ref` pointers to `$defs`
- [ ] `resolve_variant()` returns `TypedefError::Schema` for unknown mapping keys
- [ ] `discriminator_size()` returns correct size for each discriminator type
- [ ] Returns `TypedefError::Access` for buffer-too-short
- [ ] Returns `TypedefError::Schema` for malformed discriminator annotations
- [ ] No `unwrap()` or `expect()` on error paths
- [ ] All public functions have doc comments
- [ ] `cargo check -p alknet-typedef` succeeds
- [ ] `cargo clippy -p alknet-typedef` succeeds with no warnings
- [ ] `cargo build --workspace` still succeeds

## References

- docs/architecture/crates/typedef/data-access.md — TUnion dispatch section
- docs/architecture/crates/typedef/schema-layer.md — TUnion discriminators section
- docs/architecture/decisions/097-schema-annotations.md — ADR-097 §4 (TUnion discriminators)
- /workspace/alknet-typedef-poc/src/lib.rs — POC reference (parse_union_discriminator, read_union_discriminator)

## Notes

> This is a focused module for TUnion discriminator dispatch. The two discriminator
> kinds cover both protocol dispatch (SFTP type bytes, call protocol event types) and
> the typedef.ts string pattern. The mapping keys are always strings — integer
> discriminator values are stringified. The POC 2's `parse_union_discriminator` and
> `read_union_discriminator` functions are good references.

## Summary

> To be filled on completion
