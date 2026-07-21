//! Data access layer: primitive read/write functions for all 17 TypeDef
//! kinds with endianness support, bounds checking, and zero-copy access.
//!
//! These are the building blocks used by the layout types ([`crate::offset_map`],
//! [`crate::layout_builder`], [`crate::sequential_reader`]) and the
//! [`crate::engine::TypedefEngine`]. Each function operates on a raw byte
//! buffer at a caller-provided offset and returns a [`TypedefError::Access`]
//! carrying the field path on bounds or encoding failures.
//!
//! # Conventions
//!
//! - All multi-byte types respect the [`Endian`] parameter passed by the caller.
//! - Bounds checks ensure `buffer.len() >= offset + size`; failures produce
//!   [`TypedefError::Access`] with a descriptive `reason`.
//! - Read functions for variable-length types return slices borrowing from
//!   the input buffer — no allocation.
//! - No `unwrap()` / `expect()` on fallible operations.

use crate::error::TypedefError;
use crate::schema::Endian;

const U32_SIZE: usize = 4;

fn check_bounds(
    buffer_len: usize,
    start: usize,
    end: usize,
    field_path: &str,
) -> Result<(), TypedefError> {
    if end < start || buffer_len < end {
        return Err(TypedefError::Access {
            field_path: field_path.to_string(),
            reason: format!(
                "buffer bounds check failed: need bytes [{start}..{end}), buffer has {buffer_len}"
            ),
        });
    }
    Ok(())
}

fn access_err(field_path: &str, reason: impl Into<String>) -> TypedefError {
    TypedefError::Access {
        field_path: field_path.to_string(),
        reason: reason.into(),
    }
}

pub(crate) fn read_array<const N: usize>(
    buffer: &[u8],
    offset: usize,
    field_path: &str,
) -> Result<[u8; N], TypedefError> {
    let end = offset.checked_add(N).ok_or_else(|| {
        access_err(
            field_path,
            format!("offset {offset} + size {N} overflows usize"),
        )
    })?;
    check_bounds(buffer.len(), offset, end, field_path)?;
    let slice = buffer.get(offset..end).ok_or_else(|| {
        access_err(
            field_path,
            format!(
                "slice [{offset}..{end}) unavailable in buffer of length {}",
                buffer.len()
            ),
        )
    })?;
    slice.try_into().map_err(|_| {
        access_err(
            field_path,
            format!("internal: try_into failed for {N}-byte slice"),
        )
    })
}

pub(crate) fn write_array<const N: usize>(
    buffer: &mut [u8],
    offset: usize,
    bytes: [u8; N],
    field_path: &str,
) -> Result<(), TypedefError> {
    let end = offset.checked_add(N).ok_or_else(|| {
        access_err(
            field_path,
            format!("offset {offset} + size {N} overflows usize"),
        )
    })?;
    check_bounds(buffer.len(), offset, end, field_path)?;
    let dest = buffer.get_mut(offset..end).ok_or_else(|| {
        access_err(
            field_path,
            format!("mutable slice [{offset}..{end}) unavailable"),
        )
    })?;
    dest.copy_from_slice(&bytes);
    Ok(())
}

fn u32_from(bytes: [u8; U32_SIZE], endian: Endian) -> u32 {
    match endian {
        Endian::Little => u32::from_le_bytes(bytes),
        Endian::Big => u32::from_be_bytes(bytes),
    }
}

fn u32_to(value: u32, endian: Endian) -> [u8; U32_SIZE] {
    match endian {
        Endian::Little => value.to_le_bytes(),
        Endian::Big => value.to_be_bytes(),
    }
}

// ---------------------------------------------------------------------------
// Fixed-size read functions
// ---------------------------------------------------------------------------

define_read_write_ne!(i8, read_i8, write_i8, 1, |bytes: [u8; 1]| bytes[0] as i8);
define_read_write_endian!(i16, read_i16, write_i16, 2);
define_read_write_endian!(i32, read_i32, write_i32, 4);
define_read_write_ne!(u8, read_u8, write_u8, 1, |bytes: [u8; 1]| bytes[0]);
define_read_write_endian!(u16, read_u16, write_u16, 2);
define_read_write_endian!(u32, read_u32, write_u32, 4);
define_read_write_endian!(u64, read_u64, write_u64, 8);
define_read_write_endian!(f32, read_f32, write_f32, 4);
define_read_write_endian!(f64, read_f64, write_f64, 8);

/// Read a `bool` at `offset` from `buffer`.
///
/// `0x00` decodes to `false`, `0x01` decodes to `true`. Any other byte value
/// produces [`TypedefError::Access`] with a reason of the form
/// `"invalid boolean byte 0x02 at offset {offset}"`.
pub fn read_bool(buffer: &[u8], offset: usize, field_path: &str) -> Result<bool, TypedefError> {
    let bytes: [u8; 1] = read_array(buffer, offset, field_path)?;
    match bytes[0] {
        0x00 => Ok(false),
        0x01 => Ok(true),
        other => Err(access_err(
            field_path,
            format!("invalid boolean byte 0x{other:02X} at offset {offset}"),
        )),
    }
}

/// Write a `bool` `value` at `offset` into `buffer`.
///
/// `false` is encoded as `0x00`, `true` as `0x01`.
pub fn write_bool(
    buffer: &mut [u8],
    offset: usize,
    value: bool,
    field_path: &str,
) -> Result<(), TypedefError> {
    write_array(
        buffer,
        offset,
        [if value { 0x01 } else { 0x00 }],
        field_path,
    )
}

/// Read a `TEnum` index (`u32`) at `offset` from `buffer`, applying `endian`.
///
/// The caller maps the returned index to the schema's `"enum"` array entry.
pub fn read_enum(
    buffer: &[u8],
    offset: usize,
    field_path: &str,
    endian: Endian,
) -> Result<u32, TypedefError> {
    read_u32(buffer, offset, field_path, endian)
}

/// Write a `TEnum` index (`u32`) `value` at `offset` into `buffer`, applying `endian`.
pub fn write_enum(
    buffer: &mut [u8],
    offset: usize,
    value: u32,
    field_path: &str,
    endian: Endian,
) -> Result<(), TypedefError> {
    write_u32(buffer, offset, value, field_path, endian)
}

// ---------------------------------------------------------------------------
// Variable-length read/write (inline length-prefixing)
// ---------------------------------------------------------------------------

/// Read a length-prefixed UTF-8 string borrowing from `buffer`.
///
/// Wire format: `[length: u32][UTF-8 bytes]`. The length prefix respects
/// `endian`. Returns a `&'a str` that borrows from the input buffer — no
/// allocation. Invalid UTF-8 produces [`TypedefError::Access`].
pub fn read_string<'a>(
    buffer: &'a [u8],
    offset: usize,
    field_path: &str,
    endian: Endian,
) -> Result<&'a str, TypedefError> {
    let bytes = read_bytes(buffer, offset, field_path, endian)?;
    std::str::from_utf8(bytes).map_err(|e| {
        access_err(
            field_path,
            format!("invalid UTF-8 in string at offset {offset}: {e}"),
        )
    })
}

/// Read length-prefixed raw bytes borrowing from `buffer`.
///
/// Wire format: `[length: u32][raw bytes]`. The length prefix respects
/// `endian`. Returns a `&'a [u8]` slice that borrows from the input buffer.
pub fn read_bytes<'a>(
    buffer: &'a [u8],
    offset: usize,
    field_path: &str,
    endian: Endian,
) -> Result<&'a [u8], TypedefError> {
    let len_bytes: [u8; U32_SIZE] = read_array(buffer, offset, field_path)?;
    let len = u32_from(len_bytes, endian) as usize;
    let data_start = offset.checked_add(U32_SIZE).ok_or_else(|| {
        access_err(
            field_path,
            format!("offset {offset} + {U32_SIZE} overflows usize"),
        )
    })?;
    let data_end = data_start.checked_add(len).ok_or_else(|| {
        access_err(
            field_path,
            format!("data_start {data_start} + length {len} overflows usize"),
        )
    })?;
    check_bounds(buffer.len(), data_start, data_end, field_path)?;
    Ok(&buffer[data_start..data_end])
}

/// Write a length-prefixed UTF-8 string into `buffer` at `offset`.
///
/// Wire format: `[length: u32][UTF-8 bytes]`. The length prefix respects
/// `endian`. Returns the total number of bytes written
/// (`4 + value.len()`) so the caller can advance the cursor.
pub fn write_string(
    buffer: &mut [u8],
    offset: usize,
    value: &str,
    field_path: &str,
    endian: Endian,
) -> Result<usize, TypedefError> {
    write_bytes(buffer, offset, value.as_bytes(), field_path, endian)
}

/// Write length-prefixed raw bytes into `buffer` at `offset`.
///
/// Wire format: `[length: u32][raw bytes]`. The length prefix respects
/// `endian`. Returns the total number of bytes written (`4 + value.len()`).
pub fn write_bytes(
    buffer: &mut [u8],
    offset: usize,
    value: &[u8],
    field_path: &str,
    endian: Endian,
) -> Result<usize, TypedefError> {
    let data_len = value.len();
    let total = U32_SIZE.checked_add(data_len).ok_or_else(|| {
        access_err(
            field_path,
            format!("prefix {U32_SIZE} + data length {data_len} overflows usize"),
        )
    })?;
    let end = offset.checked_add(total).ok_or_else(|| {
        access_err(
            field_path,
            format!("offset {offset} + total {total} overflows usize"),
        )
    })?;
    check_bounds(buffer.len(), offset, end, field_path)?;
    write_array(buffer, offset, u32_to(data_len as u32, endian), field_path)?;
    let data_start = offset + U32_SIZE;
    let dest = buffer.get_mut(data_start..end).ok_or_else(|| {
        access_err(
            field_path,
            format!("mutable data slice [{data_start}..{end}) unavailable"),
        )
    })?;
    dest.copy_from_slice(value);
    Ok(total)
}

// ---------------------------------------------------------------------------
// Variable-length read (offset indirection)
// ---------------------------------------------------------------------------

/// Read an offset-indirect string.
///
/// The 8-byte struct at `buffer[offset..offset+8]` is
/// `{ data_offset: u32, data_length: u32 }` (endian-aware). The actual UTF-8
/// bytes live in `data_region[data_offset..data_offset+data_length]`. Returns
/// a `&'a str` borrowing from `data_region`. Invalid UTF-8 produces
/// [`TypedefError::Access`].
pub fn read_string_indirect<'a>(
    buffer: &'a [u8],
    offset: usize,
    data_region: &'a [u8],
    field_path: &str,
    endian: Endian,
) -> Result<&'a str, TypedefError> {
    let bytes = read_bytes_indirect(buffer, offset, data_region, field_path, endian)?;
    std::str::from_utf8(bytes).map_err(|e| {
        access_err(
            field_path,
            format!("invalid UTF-8 in offset-indirect string: {e}"),
        )
    })
}

/// Read offset-indirect raw bytes.
///
/// The 8-byte struct at `buffer[offset..offset+8]` is
/// `{ data_offset: u32, data_length: u32 }` (endian-aware). Returns a
/// `&'a [u8]` slice of `data_region[data_offset..data_offset+data_length]`.
pub fn read_bytes_indirect<'a>(
    buffer: &'a [u8],
    offset: usize,
    data_region: &'a [u8],
    field_path: &str,
    endian: Endian,
) -> Result<&'a [u8], TypedefError> {
    let struct_end = offset
        .checked_add(8)
        .ok_or_else(|| access_err(field_path, format!("offset {offset} + 8 overflows usize")))?;
    check_bounds(buffer.len(), offset, struct_end, field_path)?;
    let off_bytes: [u8; U32_SIZE] = buffer[offset..offset + U32_SIZE]
        .try_into()
        .map_err(|_| access_err(field_path, "internal: try_into failed for data_offset"))?;
    let len_bytes: [u8; U32_SIZE] = buffer[offset + U32_SIZE..offset + 8]
        .try_into()
        .map_err(|_| access_err(field_path, "internal: try_into failed for data_length"))?;
    let data_offset = u32_from(off_bytes, endian) as usize;
    let data_length = u32_from(len_bytes, endian) as usize;
    let data_end = data_offset.checked_add(data_length).ok_or_else(|| {
        access_err(
            field_path,
            format!("data_offset {data_offset} + data_length {data_length} overflows usize"),
        )
    })?;
    check_bounds(data_region.len(), data_offset, data_end, field_path)?;
    Ok(&data_region[data_offset..data_end])
}

#[cfg(test)]
mod tests {
    use super::*;

    const LE: Endian = Endian::Little;
    const BE: Endian = Endian::Big;

    #[test]
    fn read_write_u8_round_trip() {
        let mut buf = [0u8; 1];
        write_u8(&mut buf, 0, 0xAB, "f").unwrap();
        assert_eq!(read_u8(&buf, 0, "f").unwrap(), 0xAB);
    }

    #[test]
    fn read_write_i8_round_trip() {
        let mut buf = [0u8; 1];
        write_i8(&mut buf, 0, -42, "f").unwrap();
        assert_eq!(read_i8(&buf, 0, "f").unwrap(), -42);
    }

    #[test]
    fn read_write_u16_endianness() {
        let mut buf = [0u8; 2];
        write_u16(&mut buf, 0, 0x1234, "f", LE).unwrap();
        assert_eq!(buf, [0x34, 0x12]);
        assert_eq!(read_u16(&buf, 0, "f", LE).unwrap(), 0x1234);

        write_u16(&mut buf, 0, 0x1234, "f", BE).unwrap();
        assert_eq!(buf, [0x12, 0x34]);
        assert_eq!(read_u16(&buf, 0, "f", BE).unwrap(), 0x1234);
    }

    #[test]
    fn read_write_i16_endianness() {
        let mut buf = [0u8; 2];
        write_i16(&mut buf, 0, -1, "f", LE).unwrap();
        assert_eq!(buf, [0xFF, 0xFF]);
        assert_eq!(read_i16(&buf, 0, "f", LE).unwrap(), -1);
    }

    #[test]
    fn read_write_u32_endianness() {
        let mut buf = [0u8; 4];
        write_u32(&mut buf, 0, 0x01020304, "f", LE).unwrap();
        assert_eq!(buf, [0x04, 0x03, 0x02, 0x01]);
        assert_eq!(read_u32(&buf, 0, "f", LE).unwrap(), 0x01020304);

        write_u32(&mut buf, 0, 0x01020304, "f", BE).unwrap();
        assert_eq!(buf, [0x01, 0x02, 0x03, 0x04]);
        assert_eq!(read_u32(&buf, 0, "f", BE).unwrap(), 0x01020304);
    }

    #[test]
    fn read_write_i32_endianness() {
        let mut buf = [0u8; 4];
        write_i32(&mut buf, 0, i32::MIN, "f", BE).unwrap();
        assert_eq!(read_i32(&buf, 0, "f", BE).unwrap(), i32::MIN);
    }

    #[test]
    fn read_write_u64_endianness() {
        let mut buf = [0u8; 8];
        write_u64(&mut buf, 0, 0x0102030405060708, "f", LE).unwrap();
        assert_eq!(buf, [0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01]);
        assert_eq!(read_u64(&buf, 0, "f", LE).unwrap(), 0x0102030405060708);

        write_u64(&mut buf, 0, 0x0102030405060708, "f", BE).unwrap();
        assert_eq!(buf, [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]);
        assert_eq!(read_u64(&buf, 0, "f", BE).unwrap(), 0x0102030405060708);
    }

    #[test]
    fn read_write_f32_round_trip() {
        let mut buf = [0u8; 4];
        let value: f32 = std::f32::consts::PI;
        write_f32(&mut buf, 0, value, "f", LE).unwrap();
        let read = read_f32(&buf, 0, "f", LE).unwrap();
        assert!(
            (read - value).abs() < 1e-6,
            "le mismatch: {read} vs {value}"
        );

        write_f32(&mut buf, 0, value, "f", BE).unwrap();
        let read = read_f32(&buf, 0, "f", BE).unwrap();
        assert!(
            (read - value).abs() < 1e-6,
            "be mismatch: {read} vs {value}"
        );
    }

    #[test]
    fn read_write_f64_round_trip() {
        let mut buf = [0u8; 8];
        let value: f64 = std::f64::consts::PI;
        write_f64(&mut buf, 0, value, "f", LE).unwrap();
        assert_eq!(read_f64(&buf, 0, "f", LE).unwrap(), value);

        write_f64(&mut buf, 0, value, "f", BE).unwrap();
        assert_eq!(read_f64(&buf, 0, "f", BE).unwrap(), value);
    }

    #[test]
    fn read_write_bool_round_trip() {
        let mut buf = [0u8; 1];
        write_bool(&mut buf, 0, false, "f").unwrap();
        assert_eq!(buf[0], 0x00);
        assert!(!read_bool(&buf, 0, "f").unwrap());

        write_bool(&mut buf, 0, true, "f").unwrap();
        assert_eq!(buf[0], 0x01);
        assert!(read_bool(&buf, 0, "f").unwrap());
    }

    #[test]
    fn read_bool_rejects_invalid_byte() {
        let buf = [0x02u8];
        let err = read_bool(&buf, 0, "f").unwrap_err();
        match err {
            TypedefError::Access { field_path, reason } => {
                assert_eq!(field_path, "f");
                assert!(reason.contains("0x02"), "reason: {reason}");
                assert!(reason.contains("offset 0"), "reason: {reason}");
            }
            other => panic!("expected Access, got {other:?}"),
        }
    }

    #[test]
    fn read_write_enum_round_trip() {
        let mut buf = [0u8; 4];
        write_enum(&mut buf, 0, 7, "f", LE).unwrap();
        assert_eq!(read_enum(&buf, 0, "f", LE).unwrap(), 7);

        write_enum(&mut buf, 0, 7, "f", BE).unwrap();
        assert_eq!(read_enum(&buf, 0, "f", BE).unwrap(), 7);
    }

    #[test]
    fn bounds_failure_returns_access_error() {
        let buf = [0u8; 2];
        let err = read_u32(&buf, 0, "header.id", LE).unwrap_err();
        match err {
            TypedefError::Access { field_path, reason } => {
                assert_eq!(field_path, "header.id");
                assert!(reason.contains("bounds"), "reason: {reason}");
            }
            other => panic!("expected Access, got {other:?}"),
        }
    }

    #[test]
    fn write_bounds_failure_returns_access_error() {
        let mut buf = [0u8; 2];
        let err = write_u32(&mut buf, 0, 1, "header.id", LE).unwrap_err();
        assert!(matches!(err, TypedefError::Access { .. }));
    }

    #[test]
    fn read_string_round_trip_and_zero_copy() {
        let mut buf = vec![0u8; 32];
        let written = write_string(&mut buf, 0, "hello", "name", LE).unwrap();
        assert_eq!(written, 4 + 5);
        let s = read_string(&buf, 0, "name", LE).unwrap();
        assert_eq!(s, "hello");
        assert!(std::ptr::eq(s.as_ptr(), buf.as_ptr().wrapping_add(4)));
    }

    #[test]
    fn read_string_be_length_prefix() {
        let mut buf = vec![0u8; 16];
        write_string(&mut buf, 0, "abc", "name", BE).unwrap();
        assert_eq!(buf[0..4], [0x00, 0x00, 0x00, 0x03]);
        assert_eq!(read_string(&buf, 0, "name", BE).unwrap(), "abc");
    }

    #[test]
    fn read_bytes_round_trip_and_zero_copy() {
        let mut buf = vec![0u8; 32];
        let payload = [0xAA, 0xBB, 0xCC, 0xDD];
        let written = write_bytes(&mut buf, 0, &payload, "data", LE).unwrap();
        assert_eq!(written, 4 + 4);
        let bytes = read_bytes(&buf, 0, "data", LE).unwrap();
        assert_eq!(bytes, &payload[..]);
    }

    #[test]
    fn read_string_invalid_utf8() {
        let mut buf = vec![0u8; 16];
        write_bytes(&mut buf, 0, &[0xFF, 0xFE, 0xFD], "name", LE).unwrap();
        let err = read_string(&buf, 0, "name", LE).unwrap_err();
        assert!(matches!(err, TypedefError::Access { .. }));
    }

    #[test]
    fn read_string_bounds_failure_on_prefix() {
        let buf = [0u8; 2];
        let err = read_string(&buf, 0, "name", LE).unwrap_err();
        assert!(matches!(err, TypedefError::Access { .. }));
    }

    #[test]
    fn read_string_bounds_failure_on_data() {
        let mut buf = vec![0u8; 6];
        let _ = write_bytes(&mut buf, 0, &[0x00; 32], "name", LE);
        let len_bytes = (100u32).to_le_bytes();
        buf[0..4].copy_from_slice(&len_bytes);
        let err = read_string(&buf, 0, "name", LE).unwrap_err();
        assert!(matches!(err, TypedefError::Access { .. }));
    }

    #[test]
    fn write_string_bounds_failure() {
        let mut buf = vec![0u8; 4];
        let err = write_string(&mut buf, 0, "hello", "name", LE).unwrap_err();
        assert!(matches!(err, TypedefError::Access { .. }));
    }

    #[test]
    fn read_string_indirect_round_trip() {
        let data_region = b"the quick brown fox";
        let mut index = [0u8; 8];
        write_u32(&mut index, 0, 4, "idx.off", LE).unwrap();
        write_u32(&mut index, 4, 11, "idx.len", LE).unwrap();
        let s = read_string_indirect(&index, 0, data_region, "msg", LE).unwrap();
        assert_eq!(s, "quick brown");
    }

    #[test]
    fn read_bytes_indirect_round_trip() {
        let data_region: &[u8] = b"HEADERbody-payloadTAIL";
        let mut index = [0u8; 8];
        write_u32(&mut index, 0, 6, "idx.off", BE).unwrap();
        write_u32(&mut index, 4, 12, "idx.len", BE).unwrap();
        let bytes = read_bytes_indirect(&index, 0, data_region, "blob", BE).unwrap();
        assert_eq!(bytes, b"body-payload");
    }

    #[test]
    fn read_bytes_indirect_bounds_failure_on_index() {
        let buf = [0u8; 4];
        let data_region = b"anything";
        let err = read_bytes_indirect(&buf, 0, data_region, "blob", LE).unwrap_err();
        assert!(matches!(err, TypedefError::Access { .. }));
    }

    #[test]
    fn read_bytes_indirect_bounds_failure_on_data_region() {
        let mut buf = [0u8; 8];
        write_u32(&mut buf, 0, 100, "idx.off", LE).unwrap();
        write_u32(&mut buf, 4, 10, "idx.len", LE).unwrap();
        let data_region = b"too short";
        let err = read_bytes_indirect(&buf, 0, data_region, "blob", LE).unwrap_err();
        assert!(matches!(err, TypedefError::Access { .. }));
    }

    #[test]
    fn read_at_nonzero_offset() {
        let mut buf = vec![0u8; 16];
        write_u32(&mut buf, 8, 0xDEADBEEF, "header.id", BE).unwrap();
        assert_eq!(read_u32(&buf, 8, "header.id", BE).unwrap(), 0xDEADBEEF);
    }

    #[test]
    fn write_bytes_zero_length() {
        let mut buf = vec![0u8; 8];
        let written = write_bytes(&mut buf, 0, &[], "data", LE).unwrap();
        assert_eq!(written, 4);
        assert_eq!(buf[0..4], [0, 0, 0, 0]);
        let bytes = read_bytes(&buf, 0, "data", LE).unwrap();
        assert!(bytes.is_empty());
    }
}
