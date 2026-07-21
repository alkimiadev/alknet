//! Error path integration tests for `alknet-typedef`.
//!
//! Exercises the `TypedefError` variants across the crate:
//! `Access` (buffer too short, invalid UTF-8, invalid boolean byte,
//! unknown discriminator value), `Schema` (missing TypeDef kind,
//! malformed discriminator annotation), and `Offset` (missing
//! variable-length field size in `LayoutBuilder::build`).

use alknet_typedef::data_access;
use alknet_typedef::tunion;
use alknet_typedef::*;
use serde_json::json;
use std::collections::HashMap;

#[test]
fn read_u32_buffer_too_short_returns_access_error() {
    let buffer = [0u8; 2];
    let err = data_access::read_u32(&buffer, 0, "header.id", Endian::Little).unwrap_err();
    match err {
        TypedefError::Access { field_path, reason } => {
            assert_eq!(field_path, "header.id");
            assert!(reason.contains("bounds"), "reason: {reason}");
        }
        other => panic!("expected Access, got {other:?}"),
    }
}

#[test]
fn read_u16_buffer_too_short_returns_access_error() {
    let buffer = [0u8; 1];
    let err = data_access::read_u16(&buffer, 0, "tag", Endian::Little).unwrap_err();
    assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
}

#[test]
fn read_f32_buffer_too_short_returns_access_error() {
    let buffer = [0u8; 2];
    let err = data_access::read_f32(&buffer, 0, "score", Endian::Little).unwrap_err();
    assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
}

#[test]
fn read_f64_buffer_too_short_returns_access_error() {
    let buffer = [0u8; 4];
    let err = data_access::read_f64(&buffer, 0, "score", Endian::Little).unwrap_err();
    assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
}

#[test]
fn read_i32_buffer_too_short_returns_access_error() {
    let buffer = [0u8; 2];
    let err = data_access::read_i32(&buffer, 0, "id", Endian::Little).unwrap_err();
    assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
}

#[test]
fn read_bool_buffer_too_short_returns_access_error() {
    let buffer: [u8; 0] = [];
    let err = data_access::read_bool(&buffer, 0, "flag").unwrap_err();
    assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
}

#[test]
fn read_string_buffer_too_short_on_prefix_returns_access_error() {
    let buffer = [0u8; 2];
    let err = data_access::read_string(&buffer, 0, "name", Endian::Little).unwrap_err();
    assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
}

#[test]
fn read_string_buffer_too_short_on_data_returns_access_error() {
    let mut buffer = vec![0u8; 6];
    buffer[0..4].copy_from_slice(&100u32.to_le_bytes());
    let err = data_access::read_string(&buffer, 0, "name", Endian::Little).unwrap_err();
    assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
}

#[test]
fn read_bytes_buffer_too_short_on_data_returns_access_error() {
    let mut buffer = vec![0u8; 5];
    buffer[0..4].copy_from_slice(&100u32.to_le_bytes());
    let err = data_access::read_bytes(&buffer, 0, "blob", Endian::Little).unwrap_err();
    assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
}

#[test]
fn read_string_invalid_utf8_returns_access_error() {
    let mut buffer = vec![0u8; 16];
    let invalid = [0xFFu8, 0xFE, 0xFD];
    let _ = data_access::write_bytes(&mut buffer, 0, &invalid, "name", Endian::Little);
    let err = data_access::read_string(&buffer, 0, "name", Endian::Little).unwrap_err();
    assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
}

#[test]
fn read_bool_invalid_byte_returns_access_error() {
    let buffer = [0x02u8];
    let err = data_access::read_bool(&buffer, 0, "flag").unwrap_err();
    match err {
        TypedefError::Access { field_path, reason } => {
            assert_eq!(field_path, "flag");
            assert!(reason.contains("0x02"), "reason: {reason}");
        }
        other => panic!("expected Access, got {other:?}"),
    }
}

#[test]
fn read_bool_zero_is_false() -> Result<(), TypedefError> {
    let buffer = [0x00u8];
    assert!(!data_access::read_bool(&buffer, 0, "flag")?);
    Ok(())
}

#[test]
fn read_bool_one_is_true() -> Result<(), TypedefError> {
    let buffer = [0x01u8];
    assert!(data_access::read_bool(&buffer, 0, "flag")?);
    Ok(())
}

#[test]
fn read_bool_three_is_access_error() {
    let buffer = [0x03u8];
    let err = data_access::read_bool(&buffer, 0, "flag").unwrap_err();
    assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
}

#[test]
fn write_u32_buffer_too_short_returns_access_error() {
    let mut buffer = [0u8; 2];
    let err = data_access::write_u32(&mut buffer, 0, 1, "id", Endian::Little).unwrap_err();
    assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
}

#[test]
fn write_string_buffer_too_short_returns_access_error() {
    let mut buffer = vec![0u8; 4];
    let err =
        data_access::write_string(&mut buffer, 0, "hello", "name", Endian::Little).unwrap_err();
    assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
}

#[test]
fn compile_missing_typedef_kind_returns_schema_error() {
    let mut schema = json!({ "type": "object", "properties": {} });
    let err = TypedefEngine::compile(&mut schema, LayoutMode::Aligned).unwrap_err();
    assert!(matches!(err, TypedefError::Schema(_)), "got {err:?}");
}

#[test]
fn offset_map_compute_missing_typedef_kind_returns_schema_error() {
    let schema = json!({ "type": "object", "properties": {} });
    let err = OffsetMap::compute(&schema).unwrap_err();
    assert!(matches!(err, TypedefError::Schema(_)), "got {err:?}");
}

#[test]
fn offset_map_compute_non_struct_top_level_returns_schema_error() {
    let schema = json!({ "TypeDef:Uint32": true });
    let err = OffsetMap::compute(&schema).unwrap_err();
    assert!(matches!(err, TypedefError::Schema(_)), "got {err:?}");
}

#[test]
fn layout_builder_new_missing_typedef_kind_returns_schema_error() {
    let schema = json!({ "type": "object", "properties": {} });
    let err = LayoutBuilder::new(&schema).unwrap_err();
    assert!(matches!(err, TypedefError::Schema(_)), "got {err:?}");
}

#[test]
fn layout_builder_new_non_struct_top_level_returns_schema_error() {
    let schema = json!({ "TypeDef:Uint32": true });
    let err = LayoutBuilder::new(&schema).unwrap_err();
    assert!(matches!(err, TypedefError::Schema(_)), "got {err:?}");
}

#[test]
fn parse_discriminator_missing_returns_schema_error() {
    let schema = json!({"TypeDef:Union": true});
    let err = parse_discriminator(&schema).unwrap_err();
    assert!(matches!(err, TypedefError::Schema(_)), "got {err:?}");
}

#[test]
fn parse_discriminator_field_missing_name_returns_schema_error() {
    let schema = json!({
        "TypeDef:Union": true,
        "discriminator": {"kind": "field"}
    });
    let err = parse_discriminator(&schema).unwrap_err();
    assert!(matches!(err, TypedefError::Schema(_)), "got {err:?}");
}

#[test]
fn parse_discriminator_unknown_kind_returns_schema_error() {
    let schema = json!({
        "TypeDef:Union": true,
        "discriminator": {"kind": "magic"}
    });
    let err = parse_discriminator(&schema).unwrap_err();
    assert!(matches!(err, TypedefError::Schema(_)), "got {err:?}");
}

#[test]
fn parse_discriminator_byte_invalid_type_returns_schema_error() {
    let schema = json!({
        "TypeDef:Union": true,
        "discriminator": {"kind": "byte", "type": "TypeDef:Float32"}
    });
    let err = parse_discriminator(&schema).unwrap_err();
    assert!(matches!(err, TypedefError::Schema(_)), "got {err:?}");
}

#[test]
fn read_byte_discriminator_unknown_value_returns_access_error() -> Result<(), TypedefError> {
    let union_schema = json!({
        "TypeDef:Union": true,
        "discriminator": {"kind": "byte", "type": "TypeDef:Uint8"},
        "mapping": {"5": {"TypeDef:Struct": true, "properties": {"x": {"TypeDef:Uint8": true}}}}
    });
    let buffer = [99u8, 0x00, 0x00];
    let err = tunion::read_byte_discriminator(&buffer, &union_schema, Endian::Little).unwrap_err();
    assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
    Ok(())
}

#[test]
fn read_byte_discriminator_buffer_too_short_returns_access_error() -> Result<(), TypedefError> {
    let union_schema = json!({
        "TypeDef:Union": true,
        "discriminator": {"kind": "byte", "offset": 4, "type": "TypeDef:Uint32"},
        "mapping": {"5": {"TypeDef:Struct": true, "properties": {"x": {"TypeDef:Uint8": true}}}}
    });
    let buffer = [0u8; 2];
    let err = tunion::read_byte_discriminator(&buffer, &union_schema, Endian::Little).unwrap_err();
    assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
    Ok(())
}

#[test]
fn read_field_discriminator_unknown_value_returns_access_error() -> Result<(), TypedefError> {
    let union_schema = json!({
        "TypeDef:Union": true,
        "discriminator": {"kind": "field", "name": "type"},
        "properties": {"type": {"TypeDef:Uint8": true}},
        "mapping": {"0": {"TypeDef:Struct": true, "properties": {"x": {"TypeDef:Uint8": true}}}}
    });
    let mut buffer = vec![0u8; 8];
    buffer[0] = 99;
    let err =
        tunion::read_field_discriminator(&buffer, &union_schema, 0, Endian::Little).unwrap_err();
    assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
    Ok(())
}

#[test]
fn layout_builder_missing_var_size_returns_offset_error() {
    let schema = json!({
        "TypeDef:Struct": true,
        "properties": {
            "name": { "TypeDef:String": true }
        }
    });
    let builder = LayoutBuilder::new(&schema).expect("builder");
    let empty: HashMap<String, usize> = HashMap::new();
    let err = builder.build(&empty).unwrap_err();
    match err {
        TypedefError::Offset { field_path, reason } => {
            assert_eq!(field_path, "name");
            assert!(
                reason.contains("missing variable-length field size"),
                "reason: {reason}"
            );
        }
        other => panic!("expected Offset, got {other:?}"),
    }
}

#[test]
fn layout_builder_missing_array_data_size_returns_offset_error() {
    let schema = json!({
        "TypeDef:Struct": true,
        "properties": {
            "vals": {
                "TypeDef:Array": true,
                "items": { "TypeDef:Uint32": true }
            }
        }
    });
    let builder = LayoutBuilder::new(&schema).expect("builder");
    let empty: HashMap<String, usize> = HashMap::new();
    let err = builder.build(&empty).unwrap_err();
    assert!(matches!(err, TypedefError::Offset { .. }), "got {err:?}");
}

#[test]
fn layout_builder_missing_discriminator_value_returns_offset_error() {
    let schema = json!({
        "TypeDef:Struct": true,
        "properties": {
            "payload": {
                "TypeDef:Union": true,
                "discriminator": {"kind": "byte", "type": "TypeDef:Uint8"},
                "mapping": {"5": {"$ref": "#/$defs/Read"}}
            }
        },
        "$defs": {
            "Read": {"TypeDef:Struct": true, "properties": {"x": {"TypeDef:Uint8": true}}}
        }
    });
    let builder = LayoutBuilder::new(&schema).expect("builder");
    let empty: HashMap<String, usize> = HashMap::new();
    let err = builder.build(&empty).unwrap_err();
    assert!(matches!(err, TypedefError::Offset { .. }), "got {err:?}");
}

#[test]
fn layout_builder_unknown_discriminator_value_returns_offset_error() {
    let schema = json!({
        "TypeDef:Struct": true,
        "properties": {
            "payload": {
                "TypeDef:Union": true,
                "discriminator": {"kind": "byte", "type": "TypeDef:Uint8"},
                "mapping": {"5": {"$ref": "#/$defs/Read"}}
            }
        },
        "$defs": {
            "Read": {"TypeDef:Struct": true, "properties": {"x": {"TypeDef:Uint8": true}}}
        }
    });
    let builder = LayoutBuilder::new(&schema).expect("builder");
    let mut vs = HashMap::new();
    vs.insert("payload.__discriminator".to_string(), 99);
    let err = builder.build(&vs).unwrap_err();
    match err {
        TypedefError::Offset { reason, .. } => {
            assert!(reason.contains("99"), "reason: {reason}");
        }
        other => panic!("expected Offset, got {other:?}"),
    }
}

#[test]
fn sequential_reader_buffer_too_short_returns_access_error() {
    let schema = json!({
        "TypeDef:Struct": true,
        "properties": {
            "id": { "TypeDef:Uint32": true }
        }
    });
    let buffer = [0u8; 2];
    let mut reader = SequentialReader::new(&schema).unwrap();
    let err = reader.read_next(&buffer).unwrap_err();
    assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
}

#[test]
fn sequential_reader_unknown_field_returns_schema_error() {
    let schema = json!({
        "TypeDef:Struct": true,
        "properties": { "a": { "TypeDef:Uint8": true } }
    });
    let buffer = [0u8; 4];
    let mut reader = SequentialReader::new(&schema).unwrap();
    let err = reader.read_field(&buffer, "missing").unwrap_err();
    assert!(matches!(err, TypedefError::Schema(_)), "got {err:?}");
}

#[test]
fn sequential_reader_new_non_struct_returns_schema_error() {
    let schema = json!({ "TypeDef:Uint32": true });
    let err = SequentialReader::new(&schema).unwrap_err();
    assert!(matches!(err, TypedefError::Schema(_)), "got {err:?}");
}

#[test]
fn read_string_indirect_data_region_too_short_returns_access_error() {
    let mut index = [0u8; 8];
    let _ = data_access::write_u32(&mut index, 0, 100, "idx.off", Endian::Little);
    let _ = data_access::write_u32(&mut index, 4, 10, "idx.len", Endian::Little);
    let data_region = b"too short";
    let err = data_access::read_bytes_indirect(&index, 0, data_region, "blob", Endian::Little)
        .unwrap_err();
    assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
}

#[test]
fn read_bytes_indirect_index_too_short_returns_access_error() {
    let buffer = [0u8; 4];
    let data_region = b"anything";
    let err = data_access::read_bytes_indirect(&buffer, 0, data_region, "blob", Endian::Little)
        .unwrap_err();
    assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
}

#[test]
fn read_string_indirect_invalid_utf8_returns_access_error() {
    let data_region: &[u8] = &[0xFF, 0xFE, 0xFD];
    let mut index = [0u8; 8];
    let _ = data_access::write_u32(&mut index, 0, 0, "idx.off", Endian::Little);
    let _ = data_access::write_u32(&mut index, 4, 3, "idx.len", Endian::Little);
    let err = data_access::read_string_indirect(&index, 0, data_region, "name", Endian::Little)
        .unwrap_err();
    assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
}
