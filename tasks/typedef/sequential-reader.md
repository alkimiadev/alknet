---
id: typedef/sequential-reader
name: Implement packed sequential SequentialReader for protocol read-side
status: completed
depends_on: [typedef/schema-types, typedef/error-type, typedef/data-access]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Implement the packed sequential `SequentialReader` in `crates/alknet-typedef/src/sequential_reader.rs`.
This is the read-side of Mode 1 (ADR-096): walks a buffer field-by-field according to
the schema, reading length prefixes to determine variable-length data positions. Used
at read time when the consumer is parsing an incoming frame.

Per [layout-engine.md](../../docs/architecture/crates/typedef/layout-engine.md) §"Mode 1: Packed sequential".

### Target shape

```rust
/// Walks a buffer field-by-field according to a schema, reading length
/// prefixes to determine variable-length data positions. Used at read time
/// when parsing incoming protocol frames.
///
/// The reader is sequential — it cannot jump to field N without reading
/// fields 0..N-1 first. This is inherent to packed layouts where
/// variable-length fields shift subsequent fields.
#[derive(Debug)]
pub struct SequentialReader {
    /// The schema being read.
    schema: serde_json::Value,
    /// The endianness for the layout.
    endian: Endian,
}

/// A value read from a field during sequential traversal.
#[derive(Debug)]
pub enum FieldValue<'a> {
    I8(i8),
    I16(i16),
    I32(i32),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    F32(f32),
    F64(f64),
    Bool(bool),
    Enum(u32),
    String(&'a str),
    Bytes(&'a [u8]),
    /// A nested struct — the consumer recurses with a new SequentialReader
    /// scoped to the struct's byte range.
    Struct { start: usize, end: usize },
    /// A union — the consumer reads the discriminator, then recurses
    /// with the variant schema.
    Union { discriminator: String, variant_start: usize },
    /// An array — the consumer iterates elements.
    Array { count: u32, element_start: usize, element_stride: usize },
}

impl SequentialReader {
    /// Create a new SequentialReader from a schema.
    pub fn new(schema: &serde_json::Value) -> Result<Self, TypedefError>;

    /// Read the next field from the buffer at the current position.
    /// Returns the field name, the value, and advances the internal position.
    /// Returns `None` when all fields have been read.
    pub fn read_next<'a>(
        &mut self,
        buffer: &'a [u8],
    ) -> Result<Option<(String, FieldValue<'a>)>, TypedefError>;

    /// Read a specific field by path. This walks through all preceding fields
    /// to reach the target (sequential access is inherent to packed layouts).
    pub fn read_field<'a>(
        &mut self,
        buffer: &'a [u8],
        field_path: &str,
    ) -> Result<FieldValue<'a>, TypedefError>;

    /// Reset the reader to the beginning of the buffer.
    pub fn reset(&mut self);

    /// The current byte position in the buffer.
    pub fn position(&self) -> usize;
}
```

### How it works

For a struct with fields `[u8, u32, string]`:

```
SequentialReader:
  read_next() → ("field_0", FieldValue::U8(42)), position = 1
  read_next() → ("field_1", FieldValue::U32(1234)), position = 5
  read_next() → reads u32 length prefix at offset 5 → data_len = 10
                ("field_2", FieldValue::String("hello worl")), position = 19
  read_next() → None (no more fields)
```

The reader walks the buffer sequentially. It reads each field's type from the schema,
reads the appropriate number of bytes at the current position, and advances. For
variable-length fields, it reads the 4-byte length prefix to determine the data
extent, then advances past the data.

### Nested structs

When the reader encounters a `TStruct` field, it returns `FieldValue::Struct { start, end }`.
The consumer creates a new `SequentialReader` scoped to that byte range and reads the
inner fields.

### TUnion

When the reader encounters a `TUnion` field:
1. For byte-offset discriminators: reads the discriminator value at the known offset,
   looks up the variant schema, returns `FieldValue::Union { discriminator, variant_start }`.
2. For field-name discriminators: reads the discriminator field like any other field,
   then returns the union value.

### TArray

When the reader encounters a `TArray` field:
1. Reads the count (from `minItems`/`maxItems` if fixed, or from a 4-byte count prefix
   if variable).
2. Returns `FieldValue::Array { count, element_start, element_stride }`.
3. The consumer iterates elements using the stride.

### What this does NOT include

- The write-side of packed mode (that's `layout_builder.rs`)
- Aligned static layout (that's `offset_map.rs`)
- TUnion discriminator dispatch (that's `tunion.rs`)
- The `TypedefEngine` struct (that's `engine.rs`)

## Acceptance Criteria

- [ ] `SequentialReader` struct with `new(schema)` constructor
- [ ] `read_next(buffer)` reads the next field and advances position
- [ ] Fixed-size fields read correct number of bytes at current position
- [ ] Variable-length fields: reads 4-byte length prefix, then skips data
- [ ] Position advances correctly through all fields
- [ ] `read_next()` returns `None` when all fields have been read
- [ ] `read_field(field_path)` walks through preceding fields to reach target
- [ ] `reset()` resets position to beginning
- [ ] `position()` returns current byte offset
- [ ] Nested structs: returns `FieldValue::Struct { start, end }` for consumer recursion
- [ ] `TUnion`: reads discriminator, returns `FieldValue::Union { ... }`
- [ ] `TArray`: reads count, returns `FieldValue::Array { count, element_start, element_stride }`
- [ ] Endianness respected for all multi-byte reads
- [ ] Returns `TypedefError::Access` for buffer-too-short
- [ ] Returns `TypedefError::Schema` for malformed schemas
- [ ] No `unwrap()` or `expect()` on error paths
- [ ] All public types and functions have doc comments
- [ ] `cargo check -p alknet-typedef` succeeds
- [ ] `cargo clippy -p alknet-typedef` succeeds with no warnings
- [ ] `cargo build --workspace` still succeeds

## References

- docs/architecture/crates/typedef/layout-engine.md — Mode 1: Packed sequential, SequentialReader
- docs/architecture/crates/typedef/data-access.md — read/write functions used by the reader
- docs/architecture/decisions/096-two-layout-modes-packed-vs-aligned.md — ADR-096
- docs/architecture/decisions/097-schema-annotations.md — ADR-097 (encoding, discriminators)
- /workspace/alknet-typedef-poc/src/offset.rs — POC reference (SequentialReader in POC 2)

## Notes

> This is the read-side of the packed sequential mode. The reader walks the buffer
> sequentially — it cannot jump to field N without reading fields 0..N-1 first.
> This is inherent to packed layouts where variable-length fields shift subsequent
> fields. The POC 2's `SequentialReader` is a good reference. The reader uses the
> `data_access` module's read functions internally.

## Summary

Implemented the packed sequential `SequentialReader` and the `FieldValue`
enum in `crates/alknet-typedef/src/sequential_reader.rs`. The reader walks
a buffer field-by-field using `crate::data_access` reads, applying the
schema's endianness to every multi-byte value and reading the 4-byte
length prefix on variable-length fields (`TypeDef:String`/`Bytes`/
`Timestamp`) to advance correctly. Composite kinds return layout
descriptors: `TypeDef:Struct` returns `{start, end}` (the end is
computed by a `walk_struct_size` helper that recursively walks the
struct's fields, including variable-length ones), `TypeDef:Union`
returns `{discriminator, variant_start}` (resolving `$ref` variants
against either the union's own `$defs` or the root schema's `$defs`),
and `TypeDef:Array` returns `{count, element_start, element_stride}`
(fixed-count via `minItems == maxItems`, else a 4-byte count prefix;
stride is `0` for variable-length elements, signalling sequential
walks). The implementation handles byte-offset and field-name union
discriminators and `TypeDef:Record` count-prefixed entries, exposes
`new`/`read_next`/`read_field`/`reset`/`position` (plus `endian` and
`schema` accessors), and is covered by 24 unit tests. All verification
commands (`cargo check`, `cargo clippy -D warnings`, `cargo test
sequential_reader`, `cargo build --workspace`) pass.
