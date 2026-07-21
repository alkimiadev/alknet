//! TUnion discriminator dispatch (ADR-097 §4).
//!
//! TUnion supports two discriminator kinds: byte-offset (protocol
//! dispatch, e.g., SFTP type bytes) and field-name (typedef.ts string
//! pattern). This module reads the discriminator value from a byte
//! buffer, looks up the variant schema in the union's `mapping`, and
//! reports the offset where the variant struct begins.
//!
//! All reads go through [`crate::data_access`] so bounds checks and
//! endianness handling are uniform with the rest of the engine.

use crate::data_access::{read_enum, read_string, read_u16, read_u32, read_u8};
use crate::error::TypedefError;
use crate::schema::{get_typedef_kind, parse_discriminator, DiscriminatorKind, Endian};
use serde_json::Value;

const U32_SIZE: usize = 4;
const STRING_PREFIX_SIZE: usize = 4;
const DISCRIMINATOR_PATH: &str = "__discriminator";

/// The result of reading a TUnion discriminator.
#[derive(Debug, Clone)]
pub struct UnionDispatch {
    /// The mapping key (stringified discriminator value for byte-offset,
    /// string value for field-name).
    pub key: String,
    /// The byte offset where the variant struct starts.
    pub variant_offset: usize,
    /// The size of the discriminator in bytes.
    pub discriminator_size: usize,
}

/// Read the discriminator value from a byte-offset TUnion.
///
/// The discriminator is a fixed-size integer at a known byte offset.
/// Returns the mapping key (as a string) and the variant struct offset.
///
/// This is the SFTP `Packet` enum pattern — byte 0 is the type byte,
/// bytes 1..N are the variant struct. The call protocol's event type
/// dispatch uses the same pattern.
///
/// # Errors
///
/// - [`TypedefError::Schema`] if the discriminator annotation is missing
///   or malformed, or if the discriminator `type` is not one of
///   `TypeDef:Uint8` / `TypeDef:Uint16` / `TypeDef:Uint32`.
/// - [`TypedefError::Access`] if the buffer is too short to contain the
///   discriminator, or if the read value is not present in the union's
///   `mapping`.
pub fn read_byte_discriminator(
    buffer: &[u8],
    union_schema: &Value,
    endian: Endian,
) -> Result<UnionDispatch, TypedefError> {
    let disc = parse_discriminator(union_schema)?;
    let (offset, disc_type) = match disc {
        DiscriminatorKind::Byte { offset, disc_type } => (offset, disc_type),
        DiscriminatorKind::Field { .. } => {
            return Err(TypedefError::Schema(
                "read_byte_discriminator requires a byte-offset discriminator".to_string(),
            ));
        }
    };

    let (disc_value, discriminator_size) = match disc_type.as_str() {
        "TypeDef:Uint8" => (u32::from(read_u8(buffer, offset, DISCRIMINATOR_PATH)?), 1),
        "TypeDef:Uint16" => (
            u32::from(read_u16(buffer, offset, DISCRIMINATOR_PATH, endian)?),
            2,
        ),
        "TypeDef:Uint32" => (read_u32(buffer, offset, DISCRIMINATOR_PATH, endian)?, 4),
        other => {
            return Err(TypedefError::Schema(format!(
                "unsupported byte discriminator type: {other}"
            )));
        }
    };

    let key = disc_value.to_string();
    verify_mapping_key(union_schema, &key, DISCRIMINATOR_PATH, &key)?;

    let variant_offset =
        offset
            .checked_add(discriminator_size)
            .ok_or_else(|| TypedefError::Access {
                field_path: DISCRIMINATOR_PATH.to_string(),
                reason: format!(
                    "offset {offset} + discriminator_size {discriminator_size} overflows usize"
                ),
            })?;

    Ok(UnionDispatch {
        key,
        variant_offset,
        discriminator_size,
    })
}

/// Read the discriminator value from a field-name TUnion.
///
/// The discriminator is a named field within the struct — its offset
/// is computed like any other field. The consumer provides the
/// discriminator field's offset (from the OffsetMap or LayoutBuilder).
///
/// This is the typedef.ts `TUnion` pattern — the discriminator is a
/// field like any other, and the mapping keys are string values.
///
/// # Errors
///
/// - [`TypedefError::Schema`] if the discriminator annotation is missing
///   or malformed, the discriminator field is not declared in
///   `properties`, the field has no `TypeDef:*` kind, or the field's
///   kind is not one of `TypeDef:String` / `TypeDef:Uint8` /
///   `TypeDef:Enum`.
/// - [`TypedefError::Access`] if the buffer is too short to contain the
///   discriminator field, or if the read value is not present in the
///   union's `mapping`.
pub fn read_field_discriminator(
    buffer: &[u8],
    union_schema: &Value,
    disc_field_offset: usize,
    endian: Endian,
) -> Result<UnionDispatch, TypedefError> {
    let disc = parse_discriminator(union_schema)?;
    let name = match disc {
        DiscriminatorKind::Field { name } => name,
        DiscriminatorKind::Byte { .. } => {
            return Err(TypedefError::Schema(
                "read_field_discriminator requires a field-name discriminator".to_string(),
            ));
        }
    };

    let field_schema = union_schema
        .get("properties")
        .and_then(Value::as_object)
        .and_then(|props| props.get(&name))
        .ok_or_else(|| {
            TypedefError::Schema(format!(
                "discriminator field '{name}' not found in union properties"
            ))
        })?;

    let kind = get_typedef_kind(field_schema).ok_or_else(|| {
        TypedefError::Schema(format!(
            "discriminator field '{name}' has no TypeDef:* kind"
        ))
    })?;

    let (key, discriminator_field_size) = match kind {
        "TypeDef:String" => {
            let s = read_string(buffer, disc_field_offset, &name, endian)?;
            let size =
                STRING_PREFIX_SIZE
                    .checked_add(s.len())
                    .ok_or_else(|| TypedefError::Access {
                        field_path: name.clone(),
                        reason: format!(
                            "string prefix {STRING_PREFIX_SIZE} + data length {} overflows usize",
                            s.len()
                        ),
                    })?;
            (s.to_string(), size)
        }
        "TypeDef:Uint8" => {
            let v = read_u8(buffer, disc_field_offset, &name)?;
            (v.to_string(), 1)
        }
        "TypeDef:Enum" => {
            let v = read_enum(buffer, disc_field_offset, &name, endian)?;
            (v.to_string(), U32_SIZE)
        }
        other => {
            return Err(TypedefError::Schema(format!(
                "unsupported discriminator field type: {other}"
            )));
        }
    };

    verify_mapping_key(union_schema, &key, &name, key.as_str())?;

    let variant_offset = disc_field_offset
        .checked_add(discriminator_field_size)
        .ok_or_else(|| TypedefError::Access {
            field_path: name.clone(),
            reason: format!(
                "disc_field_offset {disc_field_offset} + discriminator_field_size {discriminator_field_size} overflows usize"
            ),
        })?;

    Ok(UnionDispatch {
        key,
        variant_offset,
        discriminator_size: discriminator_field_size,
    })
}

/// Look up a variant schema from the union's mapping.
///
/// Returns the variant schema. Inline schemas are returned directly.
/// `$ref` pointers of the form `"#/$defs/<name>"` are resolved against
/// the `union_schema`'s own `$defs` block (when the union schema is the
/// schema root). For nested unions whose `$defs` live on an ancestor,
/// the caller (typically `TypedefEngine::compile`) is expected to
/// resolve refs before reaching this function, or to inline the
/// variant schemas into the mapping at load time.
///
/// # Errors
///
/// - [`TypedefError::Schema`] if the union has no `mapping` object, the
///   `key` is not present, a `$ref` is malformed, or a `$ref` cannot be
///   resolved against the union schema's own `$defs`.
pub fn resolve_variant<'a>(union_schema: &'a Value, key: &str) -> Result<&'a Value, TypedefError> {
    let mapping = union_schema
        .get("mapping")
        .and_then(Value::as_object)
        .ok_or_else(|| TypedefError::Schema("union is missing 'mapping' object".to_string()))?;

    let variant = mapping
        .get(key)
        .ok_or_else(|| TypedefError::Schema(format!("unknown mapping key: {key}")))?;

    let ref_str = match variant.get("$ref").and_then(Value::as_str) {
        Some(r) => r,
        None => return Ok(variant),
    };

    let pointer = ref_str
        .strip_prefix('#')
        .ok_or_else(|| TypedefError::Schema(format!("unsupported $ref form: {ref_str}")))?;

    let resolved = resolve_json_pointer(union_schema, pointer).ok_or_else(|| {
        TypedefError::Schema(format!(
            "cannot resolve $ref {ref_str} against union schema; ensure refs are inlined or the union schema contains $defs"
        ))
    })?;
    Ok(resolved)
}

/// Get the discriminator size in bytes for a byte-offset discriminator.
///
/// Returns 1 for `TypeDef:Uint8`, 2 for `TypeDef:Uint16`, and 4 for
/// `TypeDef:Uint32`. Field-name discriminators have no fixed size and
/// produce a [`TypedefError::Schema`].
///
/// # Errors
///
/// - [`TypedefError::Schema`] if the discriminator annotation is
///   missing/malformed, the discriminator `type` is unsupported, or the
///   discriminator is a field-name discriminator.
pub fn discriminator_size(union_schema: &Value) -> Result<usize, TypedefError> {
    let disc = parse_discriminator(union_schema)?;
    match disc {
        DiscriminatorKind::Byte { disc_type, .. } => match disc_type.as_str() {
            "TypeDef:Uint8" => Ok(1),
            "TypeDef:Uint16" => Ok(2),
            "TypeDef:Uint32" => Ok(4),
            other => Err(TypedefError::Schema(format!(
                "unsupported byte discriminator type: {other}"
            ))),
        },
        DiscriminatorKind::Field { .. } => Err(TypedefError::Schema(
            "field-name discriminator has no fixed size".to_string(),
        )),
    }
}

fn verify_mapping_key(
    union_schema: &Value,
    key: &str,
    field_path: &str,
    raw_value: &str,
) -> Result<(), TypedefError> {
    let in_mapping = union_schema
        .get("mapping")
        .and_then(Value::as_object)
        .map(|m| m.contains_key(key))
        .unwrap_or(false);
    if in_mapping {
        Ok(())
    } else {
        Err(TypedefError::Access {
            field_path: field_path.to_string(),
            reason: format!("unknown discriminator value: {raw_value}"),
        })
    }
}

fn resolve_json_pointer<'a>(root: &'a Value, pointer: &str) -> Option<&'a Value> {
    if pointer.is_empty() {
        return Some(root);
    }
    let trimmed = pointer.strip_prefix('/')?;
    let mut current = root;
    for unescaped in trimmed.split('/') {
        let segment = unescape_json_pointer_token(unescaped)?;
        current = current.get(&segment)?;
    }
    Some(current)
}

fn unescape_json_pointer_token(token: &str) -> Option<String> {
    let mut out = String::with_capacity(token.len());
    let mut chars = token.chars();
    while let Some(c) = chars.next() {
        match c {
            '~' => match chars.next() {
                Some('0') => out.push('~'),
                Some('1') => out.push('/'),
                _ => return None,
            },
            other => out.push(other),
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const LE: Endian = Endian::Little;
    const BE: Endian = Endian::Big;

    fn byte_union_schema(offset: usize, disc_type: &str) -> Value {
        json!({
            "TypeDef:Union": true,
            "discriminator": {"kind": "byte", "offset": offset, "type": disc_type},
            "mapping": {
                "5": {"TypeDef:Struct": true, "properties": {"id": {"TypeDef:Uint32": true}}},
                "6": {"TypeDef:Struct": true, "properties": {"len": {"TypeDef:Uint16": true}}}
            }
        })
    }

    fn field_union_schema(field_name: &str, field_kind: &str) -> Value {
        let field_schema = match field_kind {
            "TypeDef:Enum" => json!({
                "TypeDef:Enum": true,
                "enum": ["read", "write"]
            }),
            _ => json!({field_kind: true}),
        };
        let (key_a, key_b) = match field_kind {
            "TypeDef:String" => ("read", "write"),
            _ => ("0", "1"),
        };
        json!({
            "TypeDef:Union": true,
            "discriminator": {"kind": "field", "name": field_name},
            "properties": {
                field_name: field_schema
            },
            "mapping": {
                key_a: {"TypeDef:Struct": true, "properties": {"n": {"TypeDef:Uint32": true}}},
                key_b: {"TypeDef:Struct": true, "properties": {"m": {"TypeDef:Uint16": true}}}
            }
        })
    }

    #[test]
    fn read_byte_discriminator_uint8_default_offset() {
        let schema = byte_union_schema(0, "TypeDef:Uint8");
        let buf = [5u8, 0xAA, 0xBB, 0xCC];
        let d = read_byte_discriminator(&buf, &schema, LE).expect("read");
        assert_eq!(d.key, "5");
        assert_eq!(d.variant_offset, 1);
        assert_eq!(d.discriminator_size, 1);
    }

    #[test]
    fn read_byte_discriminator_uint8_big_endian() {
        let schema = byte_union_schema(0, "TypeDef:Uint8");
        let buf = [6u8];
        let d = read_byte_discriminator(&buf, &schema, BE).expect("read");
        assert_eq!(d.key, "6");
        assert_eq!(d.variant_offset, 1);
    }

    #[test]
    fn read_byte_discriminator_uint16_little_endian() {
        let schema = byte_union_schema(2, "TypeDef:Uint16");
        let mut buf = vec![0u8; 4];
        buf[2..4].copy_from_slice(&5u16.to_le_bytes());
        let d = read_byte_discriminator(&buf, &schema, LE).expect("read");
        assert_eq!(d.key, "5");
        assert_eq!(d.variant_offset, 4);
        assert_eq!(d.discriminator_size, 2);
    }

    #[test]
    fn read_byte_discriminator_uint16_big_endian() {
        let schema = byte_union_schema(0, "TypeDef:Uint16");
        let buf = [0x00, 0x06, 0xAA, 0xBB];
        let d = read_byte_discriminator(&buf, &schema, BE).expect("read");
        assert_eq!(d.key, "6");
        assert_eq!(d.variant_offset, 2);
    }

    #[test]
    fn read_byte_discriminator_uint32_little_endian() {
        let schema = byte_union_schema(0, "TypeDef:Uint32");
        let mut buf = vec![0u8; 8];
        buf[0..4].copy_from_slice(&5u32.to_le_bytes());
        let d = read_byte_discriminator(&buf, &schema, LE).expect("read");
        assert_eq!(d.key, "5");
        assert_eq!(d.variant_offset, 4);
        assert_eq!(d.discriminator_size, 4);
    }

    #[test]
    fn read_byte_discriminator_uint32_big_endian() {
        let schema = byte_union_schema(0, "TypeDef:Uint32");
        let mut buf = vec![0u8; 8];
        buf[0..4].copy_from_slice(&6u32.to_be_bytes());
        let d = read_byte_discriminator(&buf, &schema, BE).expect("read");
        assert_eq!(d.key, "6");
        assert_eq!(d.variant_offset, 4);
    }

    #[test]
    fn read_byte_discriminator_unknown_value_is_access_error() {
        let schema = byte_union_schema(0, "TypeDef:Uint8");
        let buf = [99u8];
        let err = read_byte_discriminator(&buf, &schema, LE).unwrap_err();
        match err {
            TypedefError::Access { field_path, reason } => {
                assert_eq!(field_path, DISCRIMINATOR_PATH);
                assert!(reason.contains("99"), "reason: {reason}");
            }
            other => panic!("expected Access, got {other:?}"),
        }
    }

    #[test]
    fn read_byte_discriminator_buffer_too_short_is_access_error() {
        let schema = byte_union_schema(4, "TypeDef:Uint32");
        let buf = [0u8; 2];
        let err = read_byte_discriminator(&buf, &schema, LE).unwrap_err();
        assert!(matches!(err, TypedefError::Access { .. }));
    }

    #[test]
    fn read_byte_discriminator_field_kind_is_schema_error() {
        let schema = field_union_schema("type", "TypeDef:String");
        let buf = [0u8; 16];
        let err = read_byte_discriminator(&buf, &schema, LE).unwrap_err();
        assert!(matches!(err, TypedefError::Schema(_)));
    }

    #[test]
    fn read_field_discriminator_string() {
        let schema = field_union_schema("type", "TypeDef:String");
        let mut buf = vec![0u8; 32];
        let value = "read";
        let len_bytes = (value.len() as u32).to_le_bytes();
        buf[0..4].copy_from_slice(&len_bytes);
        buf[4..4 + value.len()].copy_from_slice(value.as_bytes());
        let d = read_field_discriminator(&buf, &schema, 0, LE).expect("read");
        assert_eq!(d.key, "read");
        assert_eq!(d.variant_offset, 4 + value.len());
        assert_eq!(d.discriminator_size, 4 + value.len());
    }

    #[test]
    fn read_field_discriminator_uint8() {
        let schema = field_union_schema("type", "TypeDef:Uint8");
        let mut buf = vec![0u8; 8];
        buf[0] = 0;
        let d = read_field_discriminator(&buf, &schema, 0, LE).expect("read");
        assert_eq!(d.key, "0");
        assert_eq!(d.variant_offset, 1);
        assert_eq!(d.discriminator_size, 1);
    }

    #[test]
    fn read_field_discriminator_enum() {
        let schema = field_union_schema("type", "TypeDef:Enum");
        let mut buf = vec![0u8; 8];
        buf[0..4].copy_from_slice(&0u32.to_le_bytes());
        let d = read_field_discriminator(&buf, &schema, 0, LE).expect("read");
        assert_eq!(d.key, "0");
        assert_eq!(d.variant_offset, 4);
        assert_eq!(d.discriminator_size, 4);
    }

    #[test]
    fn read_field_discriminator_string_big_endian() {
        let schema = field_union_schema("type", "TypeDef:String");
        let mut buf = vec![0u8; 32];
        let value = "write";
        let len_bytes = (value.len() as u32).to_be_bytes();
        buf[0..4].copy_from_slice(&len_bytes);
        buf[4..4 + value.len()].copy_from_slice(value.as_bytes());
        let d = read_field_discriminator(&buf, &schema, 0, BE).expect("read");
        assert_eq!(d.key, "write");
        assert_eq!(d.variant_offset, 4 + value.len());
    }

    #[test]
    fn read_field_discriminator_unknown_value_is_access_error() {
        let schema = field_union_schema("type", "TypeDef:Uint8");
        let mut buf = vec![0u8; 8];
        buf[0] = 99;
        let err = read_field_discriminator(&buf, &schema, 0, LE).unwrap_err();
        match err {
            TypedefError::Access { field_path, reason } => {
                assert_eq!(field_path, "type");
                assert!(reason.contains("99"), "reason: {reason}");
            }
            other => panic!("expected Access, got {other:?}"),
        }
    }

    #[test]
    fn read_field_discriminator_field_not_found_is_schema_error() {
        let schema = json!({
            "TypeDef:Union": true,
            "discriminator": {"kind": "field", "name": "missing"},
            "properties": {"other": {"TypeDef:Uint8": true}},
            "mapping": {"5": {"TypeDef:Struct": true}}
        });
        let buf = [0u8; 4];
        let err = read_field_discriminator(&buf, &schema, 0, LE).unwrap_err();
        assert!(matches!(err, TypedefError::Schema(_)));
    }

    #[test]
    fn read_field_discriminator_no_typedef_kind_is_schema_error() {
        let schema = json!({
            "TypeDef:Union": true,
            "discriminator": {"kind": "field", "name": "type"},
            "properties": {"type": {"type": "string"}},
            "mapping": {"read": {"TypeDef:Struct": true}}
        });
        let buf = [0u8; 4];
        let err = read_field_discriminator(&buf, &schema, 0, LE).unwrap_err();
        assert!(matches!(err, TypedefError::Schema(_)));
    }

    #[test]
    fn read_field_discriminator_unsupported_kind_is_schema_error() {
        let schema = field_union_schema("type", "TypeDef:Float32");
        let buf = [0u8; 8];
        let err = read_field_discriminator(&buf, &schema, 0, LE).unwrap_err();
        assert!(matches!(err, TypedefError::Schema(_)));
    }

    #[test]
    fn read_field_discriminator_byte_kind_is_schema_error() {
        let schema = byte_union_schema(0, "TypeDef:Uint8");
        let buf = [5u8];
        let err = read_field_discriminator(&buf, &schema, 0, LE).unwrap_err();
        assert!(matches!(err, TypedefError::Schema(_)));
    }

    #[test]
    fn resolve_variant_inline_schema() {
        let schema = byte_union_schema(0, "TypeDef:Uint8");
        let variant = resolve_variant(&schema, "5").expect("resolve");
        assert_eq!(
            variant.get("TypeDef:Struct").and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn resolve_variant_ref_against_own_defs() {
        let schema = json!({
            "TypeDef:Union": true,
            "discriminator": {"kind": "byte"},
            "mapping": {
                "5": {"$ref": "#/$defs/Read"}
            },
            "$defs": {
                "Read": {"TypeDef:Struct": true, "properties": {"id": {"TypeDef:Uint32": true}}}
            }
        });
        let variant = resolve_variant(&schema, "5").expect("resolve");
        assert_eq!(
            variant.get("TypeDef:Struct").and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn resolve_variant_unknown_key_is_schema_error() {
        let schema = byte_union_schema(0, "TypeDef:Uint8");
        let err = resolve_variant(&schema, "999").unwrap_err();
        assert!(matches!(err, TypedefError::Schema(_)));
    }

    #[test]
    fn resolve_variant_missing_mapping_is_schema_error() {
        let schema = json!({"TypeDef:Union": true, "discriminator": {"kind": "byte"}});
        let err = resolve_variant(&schema, "5").unwrap_err();
        assert!(matches!(err, TypedefError::Schema(_)));
    }

    #[test]
    fn resolve_variant_unresolvable_ref_is_schema_error() {
        let schema = json!({
            "TypeDef:Union": true,
            "discriminator": {"kind": "byte"},
            "mapping": {
                "5": {"$ref": "#/$defs/Read"}
            }
        });
        let err = resolve_variant(&schema, "5").unwrap_err();
        assert!(matches!(err, TypedefError::Schema(_)));
    }

    #[test]
    fn discriminator_size_uint8() {
        let schema = byte_union_schema(0, "TypeDef:Uint8");
        assert_eq!(discriminator_size(&schema).unwrap(), 1);
    }

    #[test]
    fn discriminator_size_uint16() {
        let schema = byte_union_schema(0, "TypeDef:Uint16");
        assert_eq!(discriminator_size(&schema).unwrap(), 2);
    }

    #[test]
    fn discriminator_size_uint32() {
        let schema = byte_union_schema(0, "TypeDef:Uint32");
        assert_eq!(discriminator_size(&schema).unwrap(), 4);
    }

    #[test]
    fn discriminator_size_field_kind_is_schema_error() {
        let schema = field_union_schema("type", "TypeDef:String");
        let err = discriminator_size(&schema).unwrap_err();
        assert!(matches!(err, TypedefError::Schema(_)));
    }

    #[test]
    fn discriminator_size_missing_discriminator_is_schema_error() {
        let schema = json!({"TypeDef:Union": true});
        let err = discriminator_size(&schema).unwrap_err();
        assert!(matches!(err, TypedefError::Schema(_)));
    }
}
