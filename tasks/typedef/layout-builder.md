---
id: typedef/layout-builder
name: Implement packed sequential LayoutBuilder for protocol write-side
status: completed
depends_on: [typedef/schema-types, typedef/error-type, typedef/data-access]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Implement the packed sequential `LayoutBuilder` in `crates/alknet-typedef/src/layout_builder.rs`.
This is the write-side of Mode 1 (ADR-096): fields are packed with no alignment padding.
Variable-length fields shift all subsequent fields. The consumer provides actual data
sizes for variable-length fields; the builder computes byte positions for each field.

Per [layout-engine.md](../../docs/architecture/crates/typedef/layout-engine.md) §"Mode 1: Packed sequential".

### Target shape

```rust
/// Builds a packed sequential layout for protocol wire formats.
/// Fields are packed with no alignment padding. Variable-length fields
/// shift all subsequent fields. The consumer provides actual data sizes
/// for variable-length fields to compute correct positions.
///
/// Used at write time when the consumer knows the data sizes upfront.
#[derive(Debug)]
pub struct LayoutBuilder {
    /// The schema being laid out.
    schema: serde_json::Value,
    /// The endianness for the layout.
    endian: Endian,
}

/// A field position computed by the LayoutBuilder.
#[derive(Debug, Clone)]
pub struct FieldPosition {
    /// Byte offset of the field within the buffer.
    pub offset: usize,
    /// Byte size of the field (4 for length prefix of variable-length fields,
    /// actual size for fixed-size fields).
    pub size: usize,
    /// The TypeDef kind of the field.
    pub kind: String,
}

/// The result of building a layout: a map of field_path → FieldPosition
/// and the total buffer size needed.
#[derive(Debug)]
pub struct PackedLayout {
    fields: Vec<(String, FieldPosition)>,
    total_size: usize,
}

impl LayoutBuilder {
    /// Create a new LayoutBuilder from a schema.
    pub fn new(schema: &serde_json::Value) -> Result<Self, TypedefError>;

    /// Build the packed layout given actual data sizes for variable-length fields.
    /// `var_sizes` maps field paths to their actual byte sizes (not including
    /// the 4-byte length prefix — the builder adds that).
    ///
    /// For fixed-size fields, the size is known from the schema.
    /// For variable-length fields, the size comes from `var_sizes`.
    /// For TUnion, the consumer provides the discriminator value to select
    /// the variant, and the variant's field sizes.
    pub fn build(
        &self,
        var_sizes: &HashMap<String, usize>,
    ) -> Result<PackedLayout, TypedefError>;
}

impl PackedLayout {
    /// Look up a field's position by dotted path.
    pub fn get(&self, field_path: &str) -> Option<&FieldPosition>;

    /// The total buffer size needed to hold all fields.
    pub fn total_size(&self) -> usize;

    /// Iterate over all (field_path, position) pairs in layout order.
    pub fn iter(&self) -> impl Iterator<Item = &(String, FieldPosition)>;
}
```

### How it works

For a struct with fields `[u8, u32, string]` where the string is 10 bytes:

```
LayoutBuilder::build(var_sizes: {"payload": 10}):
  field[0] u8:     offset 0, size 1
  field[1] u32:    offset 1, size 4
  field[2] string: offset 5, size 4 (length prefix) + 10 (data) = 14
  total: 19
```

There is no alignment padding. The `u32` at offset 1 is unaligned — this is correct
for protocol wire formats, which pack fields tightly.

### Variable-length fields in packed mode

The `LayoutBuilder` takes actual data sizes for variable-length fields to compute
correct positions for subsequent fields. The consumer must know the data sizes before
writing — this is inherent to packed layouts.

For each variable-length field:
1. The builder records the position of the 4-byte length prefix.
2. The builder adds `4 + data_size` to the current offset.
3. Subsequent fields start after the variable data.

### TUnion in packed mode

For `TUnion`, the consumer provides the discriminator value and the variant's field
sizes. The builder:
1. Computes the discriminator's position and size.
2. Looks up the variant schema from the mapping.
3. Computes the variant's field positions starting at `offset + discriminator_size`.
4. The union's total size is `discriminator_size + variant_size`.

### What this does NOT include

- The read-side of packed mode (that's `sequential_reader.rs`)
- Aligned static layout (that's `offset_map.rs`)
- TUnion discriminator dispatch (that's `tunion.rs`)
- The `TypedefEngine` struct (that's `engine.rs`)

## Acceptance Criteria

- [ ] `LayoutBuilder` struct with `new(schema)` constructor
- [ ] `LayoutBuilder::build(var_sizes)` computes packed field positions
- [ ] Fixed-size fields get correct offsets with no alignment padding
- [ ] `u8` at offset 0, `u32` at offset 1 (no padding) — packed sequential
- [ ] Variable-length fields: 4-byte length prefix at computed offset, data follows
- [ ] Variable-length field sizes come from `var_sizes` map
- [ ] Subsequent fields shift based on actual variable-length data sizes
- [ ] Nested structs produce dotted field paths
- [ ] `TArray` of fixed-size elements: correct stride (element size, no padding)
- [ ] `TArray` with variable count: 4-byte count prefix at computed offset
- [ ] `TUnion` with byte-offset discriminator: discriminator at `offset`, variant at `offset + disc_size`
- [ ] `TUnion` with field-name discriminator: discriminator is a regular field
- [ ] `PackedLayout::get("field_name")` returns correct `FieldPosition`
- [ ] `PackedLayout::total_size()` returns correct total buffer size
- [ ] `PackedLayout::iter()` iterates fields in layout order
- [ ] Returns `TypedefError::Schema` for malformed schemas
- [ ] Returns `TypedefError::Offset` for missing variable-length field sizes
- [ ] No `unwrap()` or `expect()` on error paths
- [ ] All public types and functions have doc comments
- [ ] `cargo check -p alknet-typedef` succeeds
- [ ] `cargo clippy -p alknet-typedef` succeeds with no warnings
- [ ] `cargo build --workspace` still succeeds

## References

- docs/architecture/crates/typedef/layout-engine.md — Mode 1: Packed sequential, LayoutBuilder
- docs/architecture/crates/typedef/schema-layer.md — the 17 TypeDef kinds and their byte sizes
- docs/architecture/decisions/096-two-layout-modes-packed-vs-aligned.md — ADR-096
- docs/architecture/decisions/097-schema-annotations.md — ADR-097 (encoding)
- /workspace/alknet-typedef-poc/src/offset.rs — POC reference (LayoutBuilder in POC 2)

## Notes

> This is the write-side of the packed sequential mode. The consumer knows the data
> sizes upfront (e.g., when constructing an SFTP response packet) and uses the
> `LayoutBuilder` to compute where each field goes. The builder does not write data —
> it only computes positions. The consumer uses the `data_access` module's write
> functions at the computed positions. The POC 2's `LayoutBuilder` is a good reference.

## Summary

Implemented the packed sequential `LayoutBuilder` and `PackedLayout` types in `crates/alknet-typedef/src/layout_builder.rs`. The builder walks the schema recursively via a `BuildCtx` threaded through the computation, recording `(field_path, FieldPosition)` pairs in layout order with no alignment padding. Fixed-size fields use the schema's known byte sizes; variable-length fields (`String`/`Bytes`/`Timestamp`/`Record`) always use inline length-prefixing in packed mode, pulling actual data sizes from the consumer-provided `var_sizes` map. Nested structs produce dotted field paths; `TArray` of fixed-size elements records each element as `"<path>[i]"` (or a 4-byte count prefix for variable-count arrays); `TUnion` with byte-offset discriminators uses the SFTP pattern (`<union>.__discriminator` plus a variant struct laid out at `offset + disc_size`), and field-name discriminators use a 0-based variant index under `<union>.__variant`. The implementation includes 37 unit tests covering the spec example (u8/u32/string → total 19), nested structs, arrays, both TUnion discriminator kinds, error paths, and `$ref` resolution, all passing with `cargo check`, `cargo clippy -D warnings`, and `cargo build --workspace`.
