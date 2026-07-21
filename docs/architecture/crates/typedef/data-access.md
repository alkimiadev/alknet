---
status: draft
last_updated: 2026-07-20
---

# alknet-typedef — Data Access

The data access layer: read/write functions, TUnion dispatch, field paths,
zero-copy access for fixed-size types, and length-prefix reading for
variable-length types. This is the consumer-facing API — given a compiled
`TypedefEngine` and a byte buffer, read and write fields at
schema-computed offsets.

## Read/Write Model

The typedef engine operates on raw byte buffers (`&[u8]` for reading,
`&mut [u8]` for writing). There is no intermediate `Value` tree, no
reflection, no dynamic dispatch per field. The engine uses the offset map
(or `LayoutBuilder`/`SequentialReader`) to locate fields, then performs
typed access at the computed positions.

### Fixed-size types

Fixed-size types (`TFloat32`, `TInt32`, `TUint8`, `TEnum`, etc.) are accessed via
zero-copy pointer casts:

```rust
// Read a u32 at a known offset
fn read_u32(buffer: &[u8], offset: usize, endian: Endian) -> u32 {
    let bytes: [u8; 4] = buffer[offset..offset+4].try_into().unwrap();
    match endian {
        Endian::Little => u32::from_le_bytes(bytes),
        Endian::Big => u32::from_be_bytes(bytes),
    }
}

// Write a u32 at a known offset
fn write_u32(buffer: &mut [u8], offset: usize, value: u32, endian: Endian) {
    let bytes = match endian {
        Endian::Little => value.to_le_bytes(),
        Endian::Big => value.to_be_bytes(),
    };
    buffer[offset..offset+4].copy_from_slice(&bytes);
}
```

The engine applies endianness at access time based on the schema's
`"endian"` annotation (ADR-097). The offset computation is
endian-agnostic.

### TEnum access

`TEnum` is a fixed-size type (4 bytes, `u32` index). Read/write follows
the same pattern as other fixed-size types — the engine reads/writes a
`u32` at the field's computed offset, applying the schema's endianness:

```rust
fn read_enum(buffer: &[u8], offset: usize, endian: Endian) -> u32 {
    let bytes: [u8; 4] = buffer[offset..offset+4].try_into().unwrap();
    match endian {
        Endian::Little => u32::from_le_bytes(bytes),
        Endian::Big => u32::from_be_bytes(bytes),
    }
}
```

The consumer maps the `u32` index back to the enum's string values using
the schema's `"enum"` array (index 0 → first value, index 1 → second
value, etc.). The engine does not perform this mapping — it operates on
the raw `u32` index. The jsonschema validator checks that the index
corresponds to a valid enum value at the JSON level.

### Variable-length types (inline length-prefixing)

For variable-length types with inline length-prefixing (the default):

```rust
// Read a length-prefixed string
fn read_string<'a>(buffer: &'a [u8], offset: usize, endian: Endian) -> Result<&'a str, TypedefError> {
    let len_bytes: [u8; 4] = buffer[offset..offset+4].try_into()
        .map_err(|_| TypedefError::Access { /* ... */ })?;
    let len = match endian {
        Endian::Little => u32::from_le_bytes(len_bytes),
        Endian::Big => u32::from_be_bytes(len_bytes),
    } as usize;
    let data = buffer.get(offset+4..offset+4+len)
        .ok_or_else(|| TypedefError::Access { /* ... */ })?;
    std::str::from_utf8(data)
        .map_err(|e| TypedefError::Access { /* ... */ })
}

// Write a length-prefixed string
fn write_string(buffer: &mut [u8], offset: usize, value: &str, endian: Endian) -> Result<(), TypedefError> {
    let data = value.as_bytes();
    let len_bytes = match endian {
        Endian::Little => (data.len() as u32).to_le_bytes(),
        Endian::Big => (data.len() as u32).to_be_bytes(),
    };
    buffer.get_mut(offset..offset+4)
        .ok_or_else(|| TypedefError::Access { /* ... */ })?
        .copy_from_slice(&len_bytes);
    buffer.get_mut(offset+4..offset+4+data.len())
        .ok_or_else(|| TypedefError::Access { /* ... */ })?
        .copy_from_slice(data);
    Ok(())
}
```

The engine reads the 4-byte length prefix at the field's offset, then
slices the data that follows. For writing, the engine writes the length
prefix + data.

In packed sequential mode, the `SequentialReader` uses the length prefix
to determine the position of the next field. In aligned static mode, the
`OffsetMap` records the position of the length prefix; the variable data
is accessed separately.

### Variable-length types (offset indirection)

For variable-length types with offset indirection (opt-in):

```rust
// Read an offset-indirect string
fn read_string_indirect<'a>(data_region: &'a [u8], offset: usize, endian: Endian) -> Result<&'a str, TypedefError> {
    let ptr_offset = match endian {
        Endian::Little => u32::from_le_bytes(data_region[offset..offset+4].try_into().unwrap()),
        Endian::Big => u32::from_be_bytes(data_region[offset..offset+4].try_into().unwrap()),
    } as usize;
    let ptr_length = match endian {
        Endian::Little => u32::from_le_bytes(data_region[offset+4..offset+8].try_into().unwrap()),
        Endian::Big => u32::from_be_bytes(data_region[offset+4..offset+8].try_into().unwrap()),
    } as usize;
    let data = data_region.get(ptr_offset..ptr_offset+ptr_length)
        .ok_or_else(|| TypedefError::Access { /* ... */ })?;
    std::str::from_utf8(data)
        .map_err(|e| TypedefError::Access { /* ... */ })
}
```

The field is a struct `{offset: u32, length: u32}` at a known position
in the `OffsetMap`. The consumer provides the data region separately; the
engine reads the offset and length, then slices the data region.

## TUnion Dispatch

TUnion dispatch reads the discriminator value, looks up the variant
schema, and then reads the variant's fields. The dispatch mechanism
differs by discriminator kind (ADR-097).

### Byte-offset discriminator

```rust
/// Read the discriminator value from a byte-offset TUnion.
/// Returns the mapping key (as a string) so the consumer can look up
/// the variant schema and read the variant's fields.
fn read_union_discriminator(
    buffer: &[u8],
    schema: &Value,
    endian: Endian,
) -> Result<String, TypedefError> {
    let disc = schema["discriminator"].as_object()
        .ok_or_else(|| TypedefError::Schema("missing discriminator".into()))?;
    let offset = disc["offset"].as_u64().unwrap_or(0) as usize;
    let disc_type = disc["type"].as_str().unwrap_or("TypeDef:Uint8");

    let (disc_value, disc_size) = match disc_type {
        "TypeDef:Uint8" => {
            let b = *buffer.get(offset)
                .ok_or_else(|| TypedefError::Access { /* ... */ })?;
            (b as u32, 1)
        }
        "TypeDef:Uint16" => {
            let bytes: [u8; 2] = buffer[offset..offset+2].try_into().unwrap();
            let v = match endian {
                Endian::Little => u16::from_le_bytes(bytes),
                Endian::Big => u16::from_be_bytes(bytes),
            };
            (v as u32, 2)
        }
        "TypeDef:Uint32" => {
            let bytes: [u8; 4] = buffer[offset..offset+4].try_into().unwrap();
            let v = match endian {
                Endian::Little => u32::from_le_bytes(bytes),
                Endian::Big => u32::from_be_bytes(bytes),
            };
            (v, 4)
        }
        _ => return Err(TypedefError::Schema(format!("unsupported discriminator type: {disc_type}"))),
    };

    let key = disc_value.to_string();
    if schema["mapping"].as_object().map_or(false, |m| m.contains_key(&key)) {
        Ok(key)
    } else {
        Err(TypedefError::Access {
            field_path: "__discriminator".into(),
            reason: format!("unknown discriminator value: {disc_value}"),
        })
    }
}
```

The discriminator is a fixed-size integer at a known byte offset. The
mapping keys are stringified integers. The variant struct starts at
`offset + discriminator_size`. After reading the discriminator, the
consumer looks up the variant schema and reads the variant's fields
using the normal typed read functions (e.g., `read_u32`, `read_string`)
at `offset + discriminator_size`.

This is the SFTP `Packet` enum pattern — byte 0 is the type byte, bytes
1..N are the variant struct. The call protocol's 5 event types
(`call.requested` → 0x01, etc.) use the same pattern.

### Field-name discriminator

```rust
/// Read the discriminator value from a field-name TUnion.
/// The discriminator is a named field within the struct — its offset
/// is computed like any other field. The consumer reads the field's
/// value, looks up the variant schema, then reads the variant's fields.
fn read_union_field_discriminator(
    buffer: &[u8],
    schema: &Value,
    offset_map: &OffsetMap,
    endian: Endian,
) -> Result<String, TypedefError> {
    let disc = schema["discriminator"].as_object()
        .ok_or_else(|| TypedefError::Schema("missing discriminator".into()))?;
    let field_name = disc["name"].as_str()
        .ok_or_else(|| TypedefError::Schema("discriminator has no 'name'".into()))?;

    // Read the discriminator field at its computed offset.
    // The field's TypeDef kind determines how to read it (typically a string).
    let field_schema = schema["properties"].get(field_name)
        .ok_or_else(|| TypedefError::Schema(format!("discriminator field '{field_name}' not found")))?;
    let kind = get_typedef_kind(field_schema)
        .ok_or_else(|| TypedefError::Schema("discriminator field has no TypeDef kind".into()))?;

    match kind {
        "TypeDef:String" => {
            let range = offset_map.get(field_name)
                .ok_or_else(|| TypedefError::Offset { /* ... */ })?;
            read_string(buffer, range.start, endian)
                .map(|s| s.to_string())
        }
        "TypeDef:Uint8" => {
            let range = offset_map.get(field_name)
                .ok_or_else(|| TypedefError::Offset { /* ... */ })?;
            Ok(buffer[range.start].to_string())
        }
        _ => Err(TypedefError::Schema(format!(
            "unsupported discriminator field type: {kind}"
        ))),
    }
}
```

The discriminator is a named field within the struct. Its offset is
computed like any other field. The mapping keys are string values.
After reading the discriminator, the consumer looks up the variant
schema and reads the variant's fields starting at the end of the
discriminator field (or at the start of the union buffer if the
discriminator is the first field).

## Field Paths

Fields are addressed by dotted paths: `"header.version"`, `"payload.data"`.
The `OffsetMap` stores fully-qualified paths. The read/write functions
accept a field path and look up the byte range:

```rust
/// Read an f32 field by path. This is the aligned-mode path (uses OffsetMap).
/// In packed mode, the consumer uses SequentialReader instead.
fn read_f32(&self, buffer: &[u8], field_path: &str) -> Result<f32, TypedefError> {
    let range = self.offset_map.get(field_path)
        .ok_or_else(|| TypedefError::Offset {
            field_path: field_path.to_string(),
            reason: "field not found in offset map".to_string(),
        })?;
    if buffer.len() < range.end {
        return Err(TypedefError::Access {
            field_path: field_path.to_string(),
            reason: format!("buffer too short: need {} bytes, have {}", range.end, buffer.len()),
        });
    }
    let bytes: [u8; 4] = buffer[range.start..range.end].try_into().unwrap();
    Ok(match self.endian {
        Endian::Little => f32::from_le_bytes(bytes),
        Endian::Big => f32::from_be_bytes(bytes),
    })
}
```

Nested structs produce nested field paths. The offset computation
propagates the field path prefix during recursion, so the `OffsetMap`
contains entries like `"header.version"` and `"header.magic"`.

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
