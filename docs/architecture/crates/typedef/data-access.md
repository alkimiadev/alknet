---
status: draft
last_updated: 2026-07-22
---

# alknet-typedef — Data Access

The data access layer: read/write functions, TUnion dispatch, field paths,
zero-copy access for fixed-size types, and length-prefix reading for
variable-length types. This is the consumer-facing API — given a compiled
`TypedefEngine` and a byte buffer, read and write fields at
schema-computed offsets.

This document covers two layers:

- **Primitive read/write functions** in the `data_access` module —
  typed reads/writes at a caller-provided offset. These are the building
  blocks used by the layout types (`OffsetMap`, `LayoutBuilder`,
  `SequentialReader`) and the `TypedefEngine`. Each operates on a raw
  byte buffer at a known offset and returns a `TypedefError::Access`
  carrying the field path on bounds or encoding failures.
- **The `FieldValue` enum and the higher-level APIs** —
  `TypedefEngine::read_field`/`write_field` (aligned mode) and
  `SequentialReader::read_next`/`read_field` (packed mode) — which look
  up a field's offset via the layout and dispatch to the primitive
  functions, returning a unified `FieldValue<'a>`.

## The `FieldValue` enum

The higher-level read APIs return a single unified type — `FieldValue<'a>`
— so one method can read any field kind without the caller dispatching on
schema kind first. The variant carries the typed value; the lifetime
borrows from the input buffer for variable-length kinds (zero-copy).

```rust
pub enum FieldValue<'a> {
    I8(i8), I16(i16), I32(i32), I64(i64),
    U8(u8), U16(u16), U32(u32), U64(u64),
    F32(f32), F64(f64),
    Bool(bool),
    Enum(u32),                              // u32 index into the schema's "enum" array
    String(&'a str),                        // borrows from the buffer
    Bytes(&'a [u8]),                        // borrows from the buffer
    Struct { start: usize, end: usize },    // consumer recurses with a fresh reader
    Union { discriminator: String, variant_start: usize },
    Array { count: u32, element_start: usize, element_stride: usize },
}
```

For composite kinds (`Struct`, `Union`, `Array`), `FieldValue` returns a
layout descriptor, not the decoded contents — the consumer recurses with
a fresh `SequentialReader` (or a sub-range read) scoped to the reported
byte range. `Array`'s `element_stride` is `0` for variable-length element
types, signalling the consumer must walk each element sequentially.

## Read/Write Model

The typedef engine operates on raw byte buffers (`&[u8]` for reading,
`&mut [u8]` for writing). There is no intermediate `Value` tree, no
reflection, no dynamic dispatch per field. The engine uses the offset map
(or `LayoutBuilder`/`SequentialReader`) to locate fields, then performs
typed access at the computed positions.

### Higher-level read/write

The `TypedefEngine` and `SequentialReader` provide the primary
consumer-facing read/write APIs. They look up a field's offset via the
layout and dispatch to the primitive `data_access` functions, returning
`FieldValue` (read) or accepting `&FieldValue` (write).

```rust
impl TypedefEngine {
    // Aligned mode: looks up the field's ByteRange in the OffsetMap,
    // dispatches to the right data_access function by TypeDefKind.
    // Returns TypedefError::Access if compiled in packed mode
    // (use sequential_reader() for packed mode).
    pub fn read_field<'a>(&self, buffer: &'a [u8], field_path: &str)
        -> Result<FieldValue<'a>, TypedefError>;
    pub fn write_field(&self, buffer: &mut [u8], field_path: &str,
        value: &FieldValue<'_>) -> Result<(), TypedefError>;

    // Packed mode: returns an owned fresh SequentialReader (ADR-101).
    // Each call returns a new reader with the cursor at position 0.
    // The consumer owns the reader and drives read_next/read_field/reset.
    pub fn sequential_reader(&self) -> Option<SequentialReader>;
}

impl SequentialReader {
    // Packed mode: walks the buffer field-by-field, reading length
    // prefixes to find each field's position. read_field walks all
    // preceding fields to reach the target.
    pub fn read_next<'a>(&mut self, buffer: &'a [u8])
        -> Result<Option<(String, FieldValue<'a>)>, TypedefError>;
    pub fn read_field<'a>(&mut self, buffer: &'a [u8], field_path: &str)
        -> Result<FieldValue<'a>, TypedefError>;
    pub fn reset(&mut self);
    pub fn position(&self) -> usize;
    pub fn endian(&self) -> Endian;
}
```

`read_field`/`write_field` on `TypedefEngine` work for the fixed-size
primitive kinds and the length-prefixed `String`/`Bytes`/`Timestamp`
fields. Composite kinds (`Struct`, `Union`, `Array`, `Record`) return a
`FieldValue` carrying a layout descriptor (byte range, variant start,
or array stride) for the consumer to recurse on — see §"FieldValue" above.

For writing in packed mode, the consumer uses `LayoutBuilder::build` to
compute positions, then calls the primitive `data_access::write_*`
functions at the computed offsets. There is no packed-mode
`engine.write_field` — the layout depends on the actual data sizes,
which the builder consumes at `build` time.

### Primitive read/write functions

The `data_access` module exposes typed read/write functions for each
primitive kind. Each takes `field_path: &str` for error attribution
(produces a `TypedefError::Access` carrying the path on bounds or
encoding failures) and, for multi-byte types, an `Endian` parameter.

### Fixed-size types

Fixed-size types (`TFloat32`, `TInt32`, `TUint8`, `TEnum`, etc.) are
accessed via zero-copy reads of N bytes at the offset:

```rust
// Read a u32 at a known offset, applying endianness. Bounds-checked.
fn read_u32(buffer: &[u8], offset: usize, field_path: &str, endian: Endian)
    -> Result<u32, TypedefError> {
    let bytes: [u8; 4] = read_array(buffer, offset, field_path)?;
    Ok(match endian {
        Endian::Little => u32::from_le_bytes(bytes),
        Endian::Big => u32::from_be_bytes(bytes),
    })
}

// Write a u32 at a known offset, applying endianness. Bounds-checked.
fn write_u32(buffer: &mut [u8], offset: usize, value: u32,
              field_path: &str, endian: Endian) -> Result<(), TypedefError> {
    let bytes = match endian {
        Endian::Little => value.to_le_bytes(),
        Endian::Big => value.to_be_bytes(),
    };
    write_array(buffer, offset, bytes, field_path)
}
```

The engine applies endianness at access time based on the schema's
`"endian"` annotation (ADR-097). The offset computation is
endian-agnostic. The `read_array`/`write_array` helpers perform the
bounds check and produce `TypedefError::Access` with the field path on
failure.

### TEnum access

`TEnum` is a fixed-size type (4 bytes, `u32` index). Read/write delegates
to the `u32` primitives, applying the schema's endianness:

```rust
pub fn read_enum(buffer: &[u8], offset: usize, field_path: &str, endian: Endian)
    -> Result<u32, TypedefError> {
    read_u32(buffer, offset, field_path, endian)
}
```

The consumer maps the `u32` index back to the enum's string values using
the schema's `"enum"` array (index 0 → first value, index 1 → second
value, etc.). The engine does not perform this mapping — it operates on
the raw `u32` index. The jsonschema validator checks that the index
corresponds to a valid enum value at the JSON level.

### Variable-length types (inline length-prefixing)

For variable-length types with inline length-prefixing (the default),
the `data_access` module provides `read_string`/`write_string`/
`read_bytes`/`write_bytes`. Each takes `field_path: &str` for error
attribution and `endian` for the length prefix:

```rust
// Read a length-prefixed string, borrowing from the buffer.
fn read_string<'a>(buffer: &'a [u8], offset: usize,
                   field_path: &str, endian: Endian) -> Result<&'a str, TypedefError>;

// Write a length-prefixed string. Returns total bytes written (4 + data.len()).
fn write_string(buffer: &mut [u8], offset: usize, value: &str,
                field_path: &str, endian: Endian) -> Result<usize, TypedefError>;

// read_bytes / write_bytes have the same shape — raw bytes, no UTF-8 check.
```

The engine reads the 4-byte length prefix at the field's offset, then
slices the data that follows. For writing, the engine writes the length
prefix + data. `read_string` validates UTF-8 and returns a `&str`
borrowing from the input buffer (zero-copy); `read_bytes` returns a
`&[u8]` slice with no encoding check.

In packed sequential mode, the `SequentialReader` uses the length prefix
to determine the position of the next field. In aligned static mode, the
`OffsetMap` records the position of the length prefix; the variable data
is accessed separately.

### Variable-length types (offset indirection)

For variable-length types with offset indirection (opt-in), the
`data_access` module provides `read_string_indirect`/`read_bytes_indirect`.
The 8-byte struct at `buffer[offset..offset+8]` is
`{ data_offset: u32, data_length: u32 }` (endian-aware); the actual
bytes live in a separate `data_region`:

```rust
fn read_string_indirect<'a>(buffer: &'a [u8], offset: usize,
                             data_region: &'a [u8], field_path: &str,
                             endian: Endian) -> Result<&'a str, TypedefError>;
fn read_bytes_indirect<'a>(buffer: &'a [u8], offset: usize,
                            data_region: &'a [u8], field_path: &str,
                            endian: Endian) -> Result<&'a [u8], TypedefError>;
```

The field is a struct `{offset: u32, length: u32}` at a known position
in the `OffsetMap`. The consumer provides the data region separately; the
engine reads the offset and length, then slices the data region.

## TUnion Dispatch

The `tunion` module provides TUnion discriminator dispatch — reading the
discriminator value from a byte buffer, looking up the variant schema in
the union's `mapping`, and reporting the offset where the variant struct
begins. All reads go through the `data_access` primitives so bounds checks
and endianness handling are uniform with the rest of the engine.

The result of dispatch is a `UnionDispatch` struct:

```rust
pub struct UnionDispatch {
    pub key: String,              // mapping key (stringified disc value)
    pub variant_offset: usize,    // byte offset where the variant struct starts
    pub discriminator_size: usize, // discriminator's byte size
}
```

After dispatch, the consumer calls `tunion::resolve_variant(union_schema, &dispatch.key)`
to get the variant schema, then reads the variant's fields at
`dispatch.variant_offset` using the normal `data_access` functions (or a
fresh `SequentialReader` scoped to the variant).

### Byte-offset discriminator

```rust
/// Read the discriminator value from a byte-offset TUnion. The discriminator
/// is a fixed-size integer (TypeDef:Uint8/Uint16/Uint32) at a known byte
/// offset. Returns the mapping key (stringified integer) and the variant
/// struct offset.
pub fn read_byte_discriminator(
    buffer: &[u8],
    union_schema: &Value,
    endian: Endian,
) -> Result<UnionDispatch, TypedefError>;
```

This is the SFTP `Packet` enum pattern — byte 0 is the type byte, bytes
1..N are the variant struct. The call protocol's 5 event types
(`call.requested` → 0x01, etc.) use the same pattern. The variant struct
starts at `offset + discriminator_size`.

### Field-name discriminator

```rust
/// Read the discriminator value from a field-name TUnion. The
/// discriminator is a named field within the struct — the consumer
/// provides the field's computed offset (from the OffsetMap or
/// LayoutBuilder). Supports TypeDef:String, Uint8, and Enum discriminator
/// fields.
pub fn read_field_discriminator(
    buffer: &[u8],
    union_schema: &Value,
    disc_field_offset: usize,
    endian: Endian,
) -> Result<UnionDispatch, TypedefError>;
```

The discriminator is a named field within the struct. Its offset is
computed like any other field (the consumer passes it in as
`disc_field_offset`). The mapping keys are string values. After reading
the discriminator, the consumer looks up the variant schema and reads
the variant's fields starting at the end of the discriminator field.

### Variant resolution

```rust
/// Look up a variant schema from the union's mapping. Inline schemas
/// are returned directly. $ref pointers of the form "#/$defs/<name>"
/// are resolved against the union schema's own $defs block.
pub fn resolve_variant<'a>(union_schema: &'a Value, key: &str)
    -> Result<&'a Value, TypedefError>;

/// Get the discriminator's byte size (1/2/4 for Uint8/16/32) for a
/// byte-offset TUnion. Field-name discriminators have no fixed size
/// and produce a TypedefError::Schema.
pub fn discriminator_size(union_schema: &Value) -> Result<usize, TypedefError>;
```

### TUnion in the layout engines

The `LayoutBuilder` and `SequentialReader` also handle TUnion fields
inline during traversal (the consumer does not need to call the `tunion`
functions for a union field reached during a sequential walk). For
`LayoutBuilder`, the consumer supplies the discriminator value (byte-offset)
or variant index (field-name) in `var_sizes` under the synthetic key
`"<union_path>.__discriminator"` or `"<union_path>.__variant"`. For
`SequentialReader`, a union field yields
`FieldValue::Union { discriminator, variant_start }`. The standalone
`tunion` functions are for dispatch outside the layout walk — e.g., a
consumer that receives a bare union buffer and needs to identify the
variant before recursing.

## Field Paths

Fields are addressed by dotted paths: `"header.version"`, `"payload.data"`.
Both `OffsetMap` and `PackedLayout` store fully-qualified paths (nested
struct fields appear under their parent's path prefix). The higher-level
APIs (`TypedefEngine::read_field`/`write_field`, `SequentialReader::read_field`)
accept a field path, look up the byte range/position in the layout, and
dispatch to the primitive `data_access` function for the field's kind.

For aligned-mode access, `TypedefEngine::read_field(&buffer, "header.version")`
returns `FieldValue` — it looks up the `ByteRange` in the `OffsetMap`, finds
the field's `TypeDef:*` kind in the schema, and calls the matching
`data_access::read_*` function. `write_field` is the mirror. Composite
kinds (`Struct`, `Union`, `Array`, `Record`) return a `FieldValue`
carrying a layout descriptor; the consumer recurses with a fresh reader
or sub-range read.

For packed-mode access, `SequentialReader::read_field(&buffer, "c")` walks
all preceding fields to reach the target (sequential access is inherent
to packed layouts). `read_next` walks fields in declaration order.

Nested structs produce nested field paths. The offset computation
propagates the field path prefix during recursion, so the `OffsetMap`
and `PackedLayout` contain entries like `"header.version"` and
`"header.magic"`.

## Zero-Copy Access

For fixed-size types, the engine provides zero-copy access — the consumer
gets a reference to the bytes in the buffer, not a copy. This is
important for performance-sensitive paths (metatensor tensor access,
high-throughput protocol parsing).

For variable-length types with inline length-prefixing, the engine
returns a slice of the buffer — the string or byte array data is not
copied. The consumer gets a `&str` or `&[u8]` that borrows from the
input buffer.

For offset-indirect types, the consumer provides the data region; the
engine returns a slice of that region.

## Error Handling

Read/write errors carry the field path for debugging. See
[ADR-098](../../decisions/098-error-handling-validation-strategy.md) and
[validation.md](validation.md) for the full error model.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| Two layout modes | [ADR-096](../../decisions/096-two-layout-modes-packed-vs-aligned.md) | Determines whether offsets are fixed (OffsetMap) or sequential (SequentialReader) |
| Schema annotations | [ADR-097](../../decisions/097-schema-annotations.md) | Endianness, encoding, and TUnion discriminator shapes that control data access |
| Error handling | [ADR-098](../../decisions/098-error-handling-validation-strategy.md) | Field-path-carrying errors for read/write operations |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-069** (deferred(scope)): Arrays of variable-length-element structs
  — affects the sequential walking logic for array access.

## References

- `docs/research/alknet-typedef/findings.md` §"POC Results" — POC 1
  (read/write round-trip) and POC 2 (SFTP byte-identical round-trip)
- [layout-engine.md](layout-engine.md) — offset computation that produces
  the positions this layer reads/writes at
- [validation.md](validation.md) — validation that runs on the same
  buffers
