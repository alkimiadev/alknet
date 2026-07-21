---
id: typedef/offset-map
name: Implement aligned static OffsetMap for mmap-friendly formats
status: pending
depends_on: [typedef/schema-types, typedef/error-type, typedef/data-access]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Implement the aligned static `OffsetMap` in `crates/alknet-typedef/src/offset_map.rs`.
This is Mode 2 of the two layout modes (ADR-096): fields have fixed positions with
natural alignment padding. Variable-length fields get a 4-byte length prefix at a
known offset; the variable data is not included in the static layout.

Per [layout-engine.md](../../docs/architecture/crates/typedef/layout-engine.md) §"Mode 2: Aligned static".

### Target shape

```rust
/// A byte range within a buffer.
#[derive(Debug, Clone, Copy)]
pub struct ByteRange {
    pub start: usize,
    pub end: usize,
}

/// A flat table of (field_path, byte_range) pairs computed from a schema.
/// Fields have fixed positions with natural alignment padding.
/// Used for mmap-friendly formats (metatensor, safetensors).
#[derive(Debug)]
pub struct OffsetMap {
    fields: Vec<(String, ByteRange)>,
    total_size: usize,
}

impl OffsetMap {
    /// Compute the offset map from a schema JSON value.
    /// Walks the schema recursively, computing byte positions for each field
    /// based on type sizes, field order, and alignment.
    pub fn compute(schema: &serde_json::Value) -> Result<Self, TypedefError>;

    /// Look up a field's byte range by dotted path (e.g., "header.version").
    pub fn get(&self, field_path: &str) -> Option<&ByteRange>;

    /// The total size of the struct in bytes (including alignment padding).
    pub fn total_size(&self) -> usize;

    /// Iterate over all (field_path, byte_range) pairs.
    pub fn iter(&self) -> impl Iterator<Item = &(String, ByteRange)>;
}
```

### Offset computation algorithm

The algorithm walks the schema recursively:

1. **Fixed-size types**: Determine the type's byte size from the `TypeDef:*` kind.
   Insert alignment padding to satisfy the type's alignment (or the field's `align`
   annotation, or the struct's `align` default). Record the field's `(start, end)`
   range. Advance the current offset by the type's size.

2. **`TStruct`**: Recurse into the struct's `properties`. Inner fields are computed
   relative to the struct's start offset. The struct's total size is the sum of its
   fields' sizes plus alignment padding. The struct itself may have an `align`
   annotation that rounds up its total size.

3. **`TUnion`**: The discriminator occupies `offset..offset + discriminator_size`
   bytes. For byte-offset discriminators, the variant struct starts at
   `offset + discriminator_size`. For field-name discriminators, the discriminator
   is just another field. The union's total size is `discriminator_size +
   max(variant_sizes)`.

4. **`TArray` of fixed-size elements**: Element stride = element size plus alignment
   padding. Element `i` starts at `array_offset + i × stride`. The array's total
   size is `count × stride`. Count is determined from `minItems`/`maxItems` (when
   equal, fixed count; otherwise variable — uses length-prefixed encoding).

5. **Variable-length types (inline length-prefixing)**: Record the position of the
   4-byte length prefix. The variable data is not included in the static layout.
   The length prefix is aligned to 4 bytes.

6. **Variable-length types (fixed-size reservation, `maxLength`)**: Reserve
   `maxLength` bytes at a fixed offset. Data shorter than `maxLength` is zero-padded.
   Subsequent fields have known, unchanging offsets.

7. **Variable-length types (offset indirection)**: The field is a struct
   `{offset: u32, length: u32}` (8 bytes total). Record its position. The consumer
   provides the data region separately.

### Nested structs and field paths

Nested structs produce dotted field paths: `"header.version"`, `"header.magic"`.
The offset computation propagates the field path prefix during recursion. The
`OffsetMap` stores fully-qualified paths.

### Alignment rules

- Default alignment: 1 for u8/bool, 2 for u16/i16, 4 for u32/i32/f32/enum, 8 for
  u64/i64/f64, max field alignment for structs.
- Struct-level `"align"` sets the default for all fields in that struct.
- Field-level `"align"` overrides the struct default.
- The struct's total size is rounded up to its alignment.
- Alignment padding is inserted before each field to satisfy its alignment.

### What this does NOT include

- Packed sequential layout (that's `layout_builder.rs` and `sequential_reader.rs`)
- TUnion discriminator dispatch (that's `tunion.rs`)
- The `TypedefEngine` struct (that's `engine.rs`)
- Arrays of variable-length-element structs (deferred, OQ-069)

## Acceptance Criteria

- [ ] `OffsetMap` struct with `fields: Vec<(String, ByteRange)>` and `total_size: usize`
- [ ] `OffsetMap::compute(schema)` walks the schema and computes byte positions
- [ ] Fixed-size types get correct byte ranges with natural alignment padding
- [ ] `u8` at offset 0, `u32` at offset 4 (3 bytes padding) — natural alignment
- [ ] Nested structs produce dotted field paths (`"header.version"`)
- [ ] `TArray` of fixed-size elements: correct stride and element offsets
- [ ] `TArray` with `minItems == maxItems`: fixed count, known at schema time
- [ ] `TArray` with variable count: length-prefixed encoding (4-byte count prefix)
- [ ] Variable-length types (inline length-prefixing): 4-byte length prefix at known offset
- [ ] Variable-length types (`maxLength`): reserved `maxLength` bytes at fixed offset
- [ ] Variable-length types (offset-indirect): 8-byte `{offset, length}` struct at known offset
- [ ] `TUnion` with byte-offset discriminator: discriminator at `offset`, variant at `offset + disc_size`
- [ ] `TUnion` with field-name discriminator: discriminator is a regular field
- [ ] Struct-level `"align"` annotation: rounds up struct total size
- [ ] Field-level `"align"` annotation: overrides struct default for that field
- [ ] `OffsetMap::get("header.version")` returns the correct `ByteRange`
- [ ] `OffsetMap::total_size()` returns the correct total size
- [ ] `OffsetMap::iter()` iterates all field paths
- [ ] Returns `TypedefError::Schema` for malformed schemas
- [ ] Returns `TypedefError::Offset` for unsupported type combinations
- [ ] No `unwrap()` or `expect()` on error paths
- [ ] All public types and functions have doc comments
- [ ] `cargo check -p alknet-typedef` succeeds
- [ ] `cargo clippy -p alknet-typedef` succeeds with no warnings
- [ ] `cargo build --workspace` still succeeds

## References

- docs/architecture/crates/typedef/layout-engine.md — Mode 2: Aligned static, offset computation algorithm
- docs/architecture/crates/typedef/schema-layer.md — the 17 TypeDef kinds and their byte sizes
- docs/architecture/decisions/096-two-layout-modes-packed-vs-aligned.md — ADR-096
- docs/architecture/decisions/097-schema-annotations.md — ADR-097 (alignment, encoding)
- /workspace/alknet-typedef-poc/src/offset.rs — POC reference for offset computation

## Notes

> This is the aligned static layout mode — the simpler of the two modes. Fields have
> fixed positions; the consumer can read field N without reading fields 0..N-1 first.
> Used by metatensor and safetensors. The offset computation is a recursive walk of
> the schema JSON. Nested structs propagate field path prefixes. Alignment padding
> is inserted between fields based on type sizes and annotations. The POC's
> `offset.rs` is a good reference — the algorithm is correct and can be adapted.

## Summary

> To be filled on completion
