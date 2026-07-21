---
id: typedef/data-access
name: Implement primitive read/write functions for all 17 TypeDef kinds with endianness support
status: pending
depends_on: [typedef/schema-types, typedef/error-type]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Implement the data access layer in `crates/alknet-typedef/src/data_access.rs`. This
module provides the primitive typed read/write functions that operate on raw byte
buffers at given offsets. These are the building blocks used by the layout types
(`OffsetMap`, `SequentialReader`) and the `TypedefEngine`.

Per [data-access.md](../../docs/architecture/crates/typedef/data-access.md).

### Fixed-size type read/write

All fixed-size read/write functions take a buffer, an offset, and an `Endian`, and
return `Result<T, TypedefError>` (or `Result<(), TypedefError>` for writes). They
perform bounds checking and return `TypedefError::Access` with the field path on
failure.

```rust
// Signed integers
pub fn read_i8(buffer: &[u8], offset: usize, field_path: &str) -> Result<i8, TypedefError>;
pub fn read_i16(buffer: &[u8], offset: usize, field_path: &str, endian: Endian) -> Result<i16, TypedefError>;
pub fn read_i32(buffer: &[u8], offset: usize, field_path: &str, endian: Endian) -> Result<i32, TypedefError>;

// Unsigned integers
pub fn read_u8(buffer: &[u8], offset: usize, field_path: &str) -> Result<u8, TypedefError>;
pub fn read_u16(buffer: &[u8], offset: usize, field_path: &str, endian: Endian) -> Result<u16, TypedefError>;
pub fn read_u32(buffer: &[u8], offset: usize, field_path: &str, endian: Endian) -> Result<u32, TypedefError>;
pub fn read_u64(buffer: &[u8], offset: usize, field_path: &str, endian: Endian) -> Result<u64, TypedefError>;

// Floats
pub fn read_f32(buffer: &[u8], offset: usize, field_path: &str, endian: Endian) -> Result<f32, TypedefError>;
pub fn read_f64(buffer: &[u8], offset: usize, field_path: &str, endian: Endian) -> Result<f64, TypedefError>;

// Boolean (0x00 = false, 0x01 = true; other values are errors)
pub fn read_bool(buffer: &[u8], offset: usize, field_path: &str) -> Result<bool, TypedefError>;

// TEnum (u32 index, endian-aware)
pub fn read_enum(buffer: &[u8], offset: usize, field_path: &str, endian: Endian) -> Result<u32, TypedefError>;
```

Write counterparts:

```rust
pub fn write_i8(buffer: &mut [u8], offset: usize, value: i8, field_path: &str) -> Result<(), TypedefError>;
pub fn write_i16(buffer: &mut [u8], offset: usize, value: i16, field_path: &str, endian: Endian) -> Result<(), TypedefError>;
pub fn write_i32(buffer: &mut [u8], offset: usize, value: i32, field_path: &str, endian: Endian) -> Result<(), TypedefError>;
pub fn write_u8(buffer: &mut [u8], offset: usize, value: u8, field_path: &str) -> Result<(), TypedefError>;
pub fn write_u16(buffer: &mut [u8], offset: usize, value: u16, field_path: &str, endian: Endian) -> Result<(), TypedefError>;
pub fn write_u32(buffer: &mut [u8], offset: usize, value: u32, field_path: &str, endian: Endian) -> Result<(), TypedefError>;
pub fn write_u64(buffer: &mut [u8], offset: usize, value: u64, field_path: &str, endian: Endian) -> Result<(), TypedefError>;
pub fn write_f32(buffer: &mut [u8], offset: usize, value: f32, field_path: &str, endian: Endian) -> Result<(), TypedefError>;
pub fn write_f64(buffer: &mut [u8], offset: usize, value: f64, field_path: &str, endian: Endian) -> Result<(), TypedefError>;
pub fn write_bool(buffer: &mut [u8], offset: usize, value: bool, field_path: &str) -> Result<(), TypedefError>;
pub fn write_enum(buffer: &mut [u8], offset: usize, value: u32, field_path: &str, endian: Endian) -> Result<(), TypedefError>;
```

### Variable-length type read/write (inline length-prefixing)

For variable-length types with inline length-prefixing (the default):

```rust
/// Read a length-prefixed UTF-8 string. Returns a slice borrowing from the buffer.
/// Format: [length: u32][UTF-8 bytes]
pub fn read_string<'a>(buffer: &'a [u8], offset: usize, field_path: &str, endian: Endian) -> Result<&'a str, TypedefError>;

/// Read length-prefixed raw bytes. Returns a slice borrowing from the buffer.
pub fn read_bytes<'a>(buffer: &'a [u8], offset: usize, field_path: &str, endian: Endian) -> Result<&'a [u8], TypedefError>;

/// Write a length-prefixed UTF-8 string.
pub fn write_string(buffer: &mut [u8], offset: usize, value: &str, field_path: &str, endian: Endian) -> Result<usize, TypedefError>;
// Returns the number of bytes written (4 + value.len()) so the caller can advance.

/// Write length-prefixed raw bytes.
pub fn write_bytes(buffer: &mut [u8], offset: usize, value: &[u8], field_path: &str, endian: Endian) -> Result<usize, TypedefError>;
```

### Variable-length type read (offset indirection)

For offset-indirect types (opt-in, metatensor blob tensor pattern):

```rust
/// Read an offset-indirect string. The field at `offset` is a struct
/// {data_offset: u32, data_length: u32}. The consumer provides the
/// separate data region.
pub fn read_string_indirect<'a>(
    buffer: &'a [u8],       // the index struct buffer
    offset: usize,           // position of {data_offset, data_length}
    data_region: &'a [u8],   // the separate data region
    field_path: &str,
    endian: Endian,
) -> Result<&'a str, TypedefError>;

/// Read offset-indirect raw bytes.
pub fn read_bytes_indirect<'a>(
    buffer: &'a [u8],
    offset: usize,
    data_region: &'a [u8],
    field_path: &str,
    endian: Endian,
) -> Result<&'a [u8], TypedefError>;
```

### Design notes

- **Zero-copy**: Read functions for variable-length types return slices borrowing
  from the input buffer — no allocation.
- **Bounds checking**: Every function checks that the buffer is large enough for
  the requested read/write at the given offset. Returns `TypedefError::Access`
  with the field path on failure.
- **Endianness**: Applied at access time based on the schema's `"endian"` annotation.
  The offset computation is endian-agnostic.
- **No `unwrap`**: All fallible operations use proper `Result` returns. The spec
  pseudocode uses `unwrap` for brevity; production code must not.
- **`TEnum`**: Reads/writes a `u32` index. The consumer maps the index to string
  values using the schema's `"enum"` array. The engine does not perform this mapping.
- **`TBoolean`**: `0x00` = false, `0x01` = true. Other values produce
  `TypedefError::Access`.

### What this does NOT include

- The offset computation (that's `offset_map.rs` and `layout_builder.rs`)
- TUnion discriminator dispatch (that's `tunion.rs`)
- The `TypedefEngine` struct (that's `engine.rs`)
- `TRecord` read/write (deferred — requires count-prefixed sequence walking; can be
  added when a consumer needs it, or implemented here if straightforward)

## Acceptance Criteria

- [ ] All fixed-size read functions implemented: `read_i8`, `read_i16`, `read_i32`, `read_u8`, `read_u16`, `read_u32`, `read_u64`, `read_f32`, `read_f64`, `read_bool`, `read_enum`
- [ ] All fixed-size write functions implemented: `write_i8`, `write_i16`, `write_i32`, `write_u8`, `write_u16`, `write_u32`, `write_u64`, `write_f32`, `write_f64`, `write_bool`, `write_enum`
- [ ] `read_string` and `read_bytes` (inline length-prefixed) implemented
- [ ] `write_string` and `write_bytes` (inline length-prefixed) implemented, returning bytes written
- [ ] `read_string_indirect` and `read_bytes_indirect` (offset-indirect) implemented
- [ ] All functions respect `Endian` parameter (little-endian vs big-endian byte order)
- [ ] All functions perform bounds checking and return `TypedefError::Access` with field path on failure
- [ ] `read_bool` rejects values other than `0x00` and `0x01`
- [ ] `read_string` validates UTF-8 and returns `TypedefError::Access` on invalid UTF-8
- [ ] Zero-copy: read functions for variable-length types return slices, not owned data
- [ ] No `unwrap()` or `expect()` on error paths — all fallible operations use `Result`
- [ ] All public functions have doc comments
- [ ] `cargo check -p alknet-typedef` succeeds
- [ ] `cargo clippy -p alknet-typedef` succeeds with no warnings
- [ ] `cargo build --workspace` still succeeds

## References

- docs/architecture/crates/typedef/data-access.md — read/write model, TEnum access, variable-length handling
- docs/architecture/crates/typedef/schema-layer.md — the 17 TypeDef kinds and their byte sizes
- docs/architecture/decisions/097-schema-annotations.md — ADR-097 (endianness, encoding)
- docs/architecture/decisions/098-error-handling-validation-strategy.md — ADR-098 (error handling)
- /workspace/alknet-typedef-poc/src/lib.rs — POC reference for read/write functions

## Notes

> This module provides the primitive read/write operations. The layout types
> (`OffsetMap`, `SequentialReader`) use these to access fields at computed positions.
> The functions are endian-aware — the caller passes the schema's `Endian` and the
> functions byte-swap accordingly. All functions use proper `Result` returns with
> field paths for debugging — no `unwrap()` in production code. The `TRecord`
> read/write is deferred unless it proves straightforward to implement here.

## Summary

> To be filled on completion
