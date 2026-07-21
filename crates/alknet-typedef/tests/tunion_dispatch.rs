//! Integration tests for TUnion discriminator dispatch.
//!
//! Exercises both discriminator kinds end-to-end: byte-offset (SFTP
//! pattern) and field-name (typedef.ts pattern). Verifies that
//! `read_byte_discriminator` / `read_field_discriminator` produce the
//! correct mapping key and variant offset, that `resolve_variant`
//! follows `$ref` pointers, and that `discriminator_size` reports the
//! right fixed sizes.

use alknet_typedef::data_access;
use alknet_typedef::tunion;
use alknet_typedef::{Endian, TypedefError};
use serde_json::json;

fn sftp_like_byte_union() -> serde_json::Value {
    json!({
        "TypeDef:Union": true,
        "discriminator": {
            "kind": "byte",
            "offset": 0,
            "type": "TypeDef:Uint8"
        },
        "mapping": {
            "5": { "$ref": "#/$defs/Read" },
            "6": { "$ref": "#/$defs/Write" }
        },
        "$defs": {
            "Read": {
                "TypeDef:Struct": true,
                "properties": {
                    "handle": { "TypeDef:Uint32": true },
                    "length": { "TypeDef:Uint32": true }
                }
            },
            "Write": {
                "TypeDef:Struct": true,
                "properties": {
                    "handle": { "TypeDef:Uint32": true },
                    "length": { "TypeDef:Uint32": true },
                    "data": { "TypeDef:Uint32": true }
                }
            }
        }
    })
}

#[test]
fn read_byte_discriminator_uint8_dispatches_to_read() -> Result<(), TypedefError> {
    let union_schema = sftp_like_byte_union();
    let mut buffer = vec![0u8; 16];
    buffer[0] = 5;
    data_access::write_u32(&mut buffer, 1, 0x01020304, "Read.handle", Endian::Big)?;

    let dispatch = tunion::read_byte_discriminator(&buffer, &union_schema, Endian::Big)?;
    assert_eq!(dispatch.key, "5");
    assert_eq!(dispatch.variant_offset, 1);
    assert_eq!(dispatch.discriminator_size, 1);

    let variant = tunion::resolve_variant(&union_schema, &dispatch.key)?;
    assert_eq!(
        variant.get("TypeDef:Struct").and_then(serde_json::Value::as_bool),
        Some(true)
    );
    Ok(())
}

#[test]
fn read_byte_discriminator_uint8_dispatches_to_write() -> Result<(), TypedefError> {
    let union_schema = sftp_like_byte_union();
    let mut buffer = vec![0u8; 16];
    buffer[0] = 6;
    data_access::write_u32(&mut buffer, 1, 0xDEADBEEF, "Write.handle", Endian::Big)?;

    let dispatch = tunion::read_byte_discriminator(&buffer, &union_schema, Endian::Big)?;
    assert_eq!(dispatch.key, "6");
    assert_eq!(dispatch.variant_offset, 1);
    assert_eq!(dispatch.discriminator_size, 1);

    let variant = tunion::resolve_variant(&union_schema, &dispatch.key)?;
    let props = variant
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .expect("variant has properties");
    assert!(props.contains_key("data"));
    Ok(())
}

#[test]
fn read_byte_discriminator_uint16_little_endian() -> Result<(), TypedefError> {
    let schema = json!({
        "TypeDef:Union": true,
        "discriminator": {
            "kind": "byte",
            "offset": 2,
            "type": "TypeDef:Uint16"
        },
        "mapping": {
            "5": {"TypeDef:Struct": true, "properties": {"id": {"TypeDef:Uint32": true}}}
        }
    });
    let mut buffer = vec![0u8; 16];
    buffer[2..4].copy_from_slice(&5u16.to_le_bytes());
    let dispatch = tunion::read_byte_discriminator(&buffer, &schema, Endian::Little)?;
    assert_eq!(dispatch.key, "5");
    assert_eq!(dispatch.variant_offset, 4);
    assert_eq!(dispatch.discriminator_size, 2);
    Ok(())
}

#[test]
fn read_byte_discriminator_uint32_big_endian() -> Result<(), TypedefError> {
    let schema = json!({
        "TypeDef:Union": true,
        "discriminator": {
            "kind": "byte",
            "offset": 0,
            "type": "TypeDef:Uint32"
        },
        "mapping": {
            "101": {"TypeDef:Struct": true, "properties": {"id": {"TypeDef:Uint32": true}}}
        }
    });
    let mut buffer = vec![0u8; 16];
    buffer[0..4].copy_from_slice(&101u32.to_be_bytes());
    let dispatch = tunion::read_byte_discriminator(&buffer, &schema, Endian::Big)?;
    assert_eq!(dispatch.key, "101");
    assert_eq!(dispatch.variant_offset, 4);
    assert_eq!(dispatch.discriminator_size, 4);
    Ok(())
}

#[test]
fn read_byte_discriminator_unknown_value_returns_access_error() -> Result<(), TypedefError> {
    let union_schema = sftp_like_byte_union();
    let buffer = [99u8, 0x00, 0x00, 0x00];
    let err = tunion::read_byte_discriminator(&buffer, &union_schema, Endian::Big).unwrap_err();
    assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
    Ok(())
}

#[test]
fn read_field_discriminator_string_dispatches_to_read() -> Result<(), TypedefError> {
    let union_schema = json!({
        "TypeDef:Union": true,
        "discriminator": {"kind": "field", "name": "type"},
        "properties": {
            "type": { "TypeDef:String": true }
        },
        "mapping": {
            "read": {"$ref": "#/$defs/Read"},
            "write": {"$ref": "#/$defs/Write"}
        },
        "$defs": {
            "Read": {
                "TypeDef:Struct": true,
                "properties": {
                    "handle": { "TypeDef:Uint32": true },
                    "length": { "TypeDef:Uint32": true }
                }
            },
            "Write": {
                "TypeDef:Struct": true,
                "properties": {
                    "handle": { "TypeDef:Uint32": true },
                    "data": { "TypeDef:Bytes": true }
                }
            }
        }
    });
    let value = "read";
    let mut buffer = vec![0u8; 32];
    data_access::write_string(&mut buffer, 0, value, "type", Endian::Little)?;
    let dispatch = tunion::read_field_discriminator(&buffer, &union_schema, 0, Endian::Little)?;
    assert_eq!(dispatch.key, "read");
    assert_eq!(dispatch.variant_offset, 4 + value.len());
    assert_eq!(dispatch.discriminator_size, 4 + value.len());

    let variant = tunion::resolve_variant(&union_schema, &dispatch.key)?;
    assert_eq!(
        variant.get("TypeDef:Struct").and_then(serde_json::Value::as_bool),
        Some(true)
    );
    Ok(())
}

#[test]
fn read_field_discriminator_string_dispatches_to_write() -> Result<(), TypedefError> {
    let union_schema = json!({
        "TypeDef:Union": true,
        "discriminator": {"kind": "field", "name": "type"},
        "properties": {
            "type": { "TypeDef:String": true }
        },
        "mapping": {
            "read": {"$ref": "#/$defs/Read"},
            "write": {"$ref": "#/$defs/Write"}
        },
        "$defs": {
            "Read": {
                "TypeDef:Struct": true,
                "properties": {"x": {"TypeDef:Uint8": true}}
            },
            "Write": {
                "TypeDef:Struct": true,
                "properties": {"y": {"TypeDef:Uint16": true}}
            }
        }
    });
    let value = "write";
    let mut buffer = vec![0u8; 32];
    data_access::write_string(&mut buffer, 0, value, "type", Endian::Little)?;
    let dispatch = tunion::read_field_discriminator(&buffer, &union_schema, 0, Endian::Little)?;
    assert_eq!(dispatch.key, "write");
    assert_eq!(dispatch.variant_offset, 4 + value.len());

    let variant = tunion::resolve_variant(&union_schema, &dispatch.key)?;
    let props = variant
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .expect("variant has properties");
    assert!(props.contains_key("y"));
    assert!(!props.contains_key("x"));
    Ok(())
}

#[test]
fn read_field_discriminator_uint8_field() -> Result<(), TypedefError> {
    let union_schema = json!({
        "TypeDef:Union": true,
        "discriminator": {"kind": "field", "name": "tag"},
        "properties": {
            "tag": { "TypeDef:Uint8": true }
        },
        "mapping": {
            "0": {"TypeDef:Struct": true, "properties": {"a": {"TypeDef:Uint32": true}}},
            "1": {"TypeDef:Struct": true, "properties": {"b": {"TypeDef:Uint16": true}}}
        }
    });
    let mut buffer = vec![0u8; 8];
    buffer[0] = 0;
    let dispatch = tunion::read_field_discriminator(&buffer, &union_schema, 0, Endian::Little)?;
    assert_eq!(dispatch.key, "0");
    assert_eq!(dispatch.variant_offset, 1);
    assert_eq!(dispatch.discriminator_size, 1);

    buffer[0] = 1;
    let dispatch = tunion::read_field_discriminator(&buffer, &union_schema, 0, Endian::Little)?;
    assert_eq!(dispatch.key, "1");
    Ok(())
}

#[test]
fn read_field_discriminator_unknown_value_returns_access_error() -> Result<(), TypedefError> {
    let union_schema = json!({
        "TypeDef:Union": true,
        "discriminator": {"kind": "field", "name": "tag"},
        "properties": {
            "tag": { "TypeDef:Uint8": true }
        },
        "mapping": {
            "0": {"TypeDef:Struct": true, "properties": {"a": {"TypeDef:Uint32": true}}}
        }
    });
    let mut buffer = vec![0u8; 8];
    buffer[0] = 99;
    let err = tunion::read_field_discriminator(&buffer, &union_schema, 0, Endian::Little).unwrap_err();
    assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
    Ok(())
}

#[test]
fn discriminator_size_returns_correct_values() -> Result<(), TypedefError> {
    let u8_schema = json!({
        "TypeDef:Union": true,
        "discriminator": {"kind": "byte", "type": "TypeDef:Uint8"},
        "mapping": {}
    });
    let u16_schema = json!({
        "TypeDef:Union": true,
        "discriminator": {"kind": "byte", "type": "TypeDef:Uint16"},
        "mapping": {}
    });
    let u32_schema = json!({
        "TypeDef:Union": true,
        "discriminator": {"kind": "byte", "type": "TypeDef:Uint32"},
        "mapping": {}
    });
    assert_eq!(tunion::discriminator_size(&u8_schema)?, 1);
    assert_eq!(tunion::discriminator_size(&u16_schema)?, 2);
    assert_eq!(tunion::discriminator_size(&u32_schema)?, 4);
    Ok(())
}

#[test]
fn discriminator_size_field_kind_returns_schema_error() {
    let schema = json!({
        "TypeDef:Union": true,
        "discriminator": {"kind": "field", "name": "type"},
        "properties": {"type": {"TypeDef:Uint8": true}},
        "mapping": {}
    });
    let err = tunion::discriminator_size(&schema).unwrap_err();
    assert!(matches!(err, TypedefError::Schema(_)), "got {err:?}");
}

#[test]
fn resolve_variant_returns_schema_error_for_unknown_key() {
    let union_schema = sftp_like_byte_union();
    let err = tunion::resolve_variant(&union_schema, "999").unwrap_err();
    assert!(matches!(err, TypedefError::Schema(_)), "got {err:?}");
}

#[test]
fn parse_discriminator_missing_returns_schema_error() {
    let schema = json!({"TypeDef:Union": true});
    let err = alknet_typedef::parse_discriminator(&schema).unwrap_err();
    assert!(matches!(err, TypedefError::Schema(_)), "got {err:?}");
}