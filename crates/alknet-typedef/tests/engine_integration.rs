//! Integration tests for the `TypedefEngine` public API.
//!
//! Exercises the engine across both layout modes, the convenience
//! accessors, validation convenience methods, and the aligned-mode
//! `read_field` / `write_field` round-trip for the fixed-size primitive
//! kinds and length-prefixed `String` / `Bytes`.

use alknet_typedef::*;
use serde_json::json;

fn mixed_fixed_struct_schema() -> serde_json::Value {
    json!({
        "TypeDef:Struct": true,
        "endian": "little",
        "properties": {
            "flag": { "TypeDef:Uint8": true },
            "id": { "TypeDef:Uint32": true },
            "score": { "TypeDef:Float32": true },
            "tag": { "TypeDef:String": true }
        }
    })
}

#[test]
fn compile_aligned_builds_engine_with_offset_map() -> Result<(), TypedefError> {
    let mut schema = mixed_fixed_struct_schema();
    let engine = TypedefEngine::compile(&mut schema, LayoutMode::Aligned)?;
    assert_eq!(engine.mode(), LayoutMode::Aligned);
    assert!(engine.offset_map().is_some());
    assert!(engine.layout_builder().is_none());
    assert!(engine.sequential_reader().is_none());
    Ok(())
}

#[test]
fn compile_packed_builds_engine_with_builder_and_reader() -> Result<(), TypedefError> {
    let mut schema = mixed_fixed_struct_schema();
    let engine = TypedefEngine::compile(&mut schema, LayoutMode::Packed)?;
    assert_eq!(engine.mode(), LayoutMode::Packed);
    assert!(engine.offset_map().is_none());
    assert!(engine.layout_builder().is_some());
    assert!(engine.sequential_reader().is_some());
    Ok(())
}

#[test]
fn compile_normalizes_bare_name_refs() -> Result<(), TypedefError> {
    let mut schema = json!({
        "TypeDef:Struct": true,
        "properties": {
            "child": { "$ref": "Child" }
        },
        "$defs": {
            "Child": {
                "TypeDef:Struct": true,
                "properties": { "x": { "TypeDef:Uint8": true } }
            }
        }
    });
    let _engine = TypedefEngine::compile(&mut schema, LayoutMode::Packed)?;
    assert_eq!(
        schema["properties"]["child"]["$ref"],
        json!("#/$defs/Child")
    );
    Ok(())
}

#[test]
fn compile_leaves_full_pointer_refs_unchanged() -> Result<(), TypedefError> {
    let mut schema = json!({
        "TypeDef:Struct": true,
        "properties": {
            "child": { "$ref": "#/$defs/Child" }
        },
        "$defs": {
            "Child": {
                "TypeDef:Struct": true,
                "properties": { "x": { "TypeDef:Uint8": true } }
            }
        }
    });
    let _engine = TypedefEngine::compile(&mut schema, LayoutMode::Packed)?;
    assert_eq!(
        schema["properties"]["child"]["$ref"],
        json!("#/$defs/Child")
    );
    Ok(())
}

#[test]
fn compile_returns_schema_error_when_no_typedef_kind() {
    let mut schema = json!({ "type": "object", "properties": {} });
    let err = TypedefEngine::compile(&mut schema, LayoutMode::Aligned).unwrap_err();
    assert!(matches!(err, TypedefError::Schema(_)), "got {err:?}");
}

#[test]
fn endian_parsed_from_schema_big() -> Result<(), TypedefError> {
    let mut schema = json!({
        "TypeDef:Struct": true,
        "endian": "big",
        "properties": { "id": { "TypeDef:Uint32": true } }
    });
    let engine = TypedefEngine::compile(&mut schema, LayoutMode::Packed)?;
    assert_eq!(engine.endian(), Endian::Big);
    Ok(())
}

#[test]
fn endian_defaults_to_little() -> Result<(), TypedefError> {
    let mut schema = json!({
        "TypeDef:Struct": true,
        "properties": { "id": { "TypeDef:Uint32": true } }
    });
    let engine = TypedefEngine::compile(&mut schema, LayoutMode::Packed)?;
    assert_eq!(engine.endian(), Endian::Little);
    Ok(())
}

#[test]
fn validate_json_accepts_valid_instance() -> Result<(), TypedefError> {
    let mut schema = json!({
        "TypeDef:Struct": true,
        "type": "object",
        "properties": {
            "id": { "TypeDef:Uint32": true, "type": "integer" }
        },
        "required": ["id"]
    });
    let engine = TypedefEngine::compile(&mut schema, LayoutMode::Aligned)?;
    assert!(engine.validate_json(&json!({"id": 42})).is_ok());
    Ok(())
}

#[test]
fn validate_json_rejects_invalid_instance() -> Result<(), TypedefError> {
    let mut schema = json!({
        "TypeDef:Struct": true,
        "type": "object",
        "properties": {
            "id": { "TypeDef:Uint32": true, "type": "integer" }
        },
        "required": ["id"]
    });
    let engine = TypedefEngine::compile(&mut schema, LayoutMode::Aligned)?;
    let err = engine.validate_json(&json!({"id": -1})).unwrap_err();
    assert!(matches!(err, TypedefError::Validation(_)), "got {err:?}");
    Ok(())
}

#[test]
fn is_valid_json_returns_bool() -> Result<(), TypedefError> {
    let mut schema = json!({
        "TypeDef:Struct": true,
        "type": "object",
        "properties": {
            "id": { "TypeDef:Uint32": true, "type": "integer" }
        },
        "required": ["id"]
    });
    let engine = TypedefEngine::compile(&mut schema, LayoutMode::Aligned)?;
    assert!(engine.is_valid_json(&json!({"id": 42})));
    assert!(!engine.is_valid_json(&json!({"id": -1})));
    Ok(())
}

#[test]
fn read_write_aligned_round_trips_all_fixed_size_kinds() -> Result<(), TypedefError> {
    let mut schema = json!({
        "TypeDef:Struct": true,
        "endian": "little",
        "properties": {
            "i8":  { "TypeDef:Int8": true },
            "u8":  { "TypeDef:Uint8": true },
            "i16": { "TypeDef:Int16": true },
            "u16": { "TypeDef:Uint16": true },
            "i32": { "TypeDef:Int32": true },
            "u32": { "TypeDef:Uint32": true },
            "i64": { "TypeDef:Int64": true },
            "u64": { "TypeDef:Uint64": true },
            "f32": { "TypeDef:Float32": true },
            "f64": { "TypeDef:Float64": true },
            "b":   { "TypeDef:Boolean": true },
            "e":   { "TypeDef:Enum": true }
        }
    });
    let engine = TypedefEngine::compile(&mut schema, LayoutMode::Aligned)?;
    let offset_map = engine.offset_map().expect("aligned mode has offset_map");
    let mut buffer = vec![0u8; offset_map.total_size()];

    engine.write_field(&mut buffer, "i8", &FieldValue::I8(-127))?;
    engine.write_field(&mut buffer, "u8", &FieldValue::U8(0xAB))?;
    engine.write_field(&mut buffer, "i16", &FieldValue::I16(-32000))?;
    engine.write_field(&mut buffer, "u16", &FieldValue::U16(0xBEEF))?;
    engine.write_field(&mut buffer, "i32", &FieldValue::I32(-2_000_000_007))?;
    engine.write_field(&mut buffer, "u32", &FieldValue::U32(0xDEADBEEF))?;
    engine.write_field(&mut buffer, "i64", &FieldValue::I64(-9_000_000_000_000_000_000))?;
    engine.write_field(&mut buffer, "u64", &FieldValue::U64(0x0102030405060708))?;
    engine.write_field(&mut buffer, "f32", &FieldValue::F32(1.5))?;
    engine.write_field(&mut buffer, "f64", &FieldValue::F64(2.5))?;
    engine.write_field(&mut buffer, "b", &FieldValue::Bool(true))?;
    engine.write_field(&mut buffer, "e", &FieldValue::Enum(7))?;

    assert_eq!(engine.read_field(&buffer, "i8")?, FieldValue::I8(-127));
    assert_eq!(engine.read_field(&buffer, "u8")?, FieldValue::U8(0xAB));
    assert_eq!(engine.read_field(&buffer, "i16")?, FieldValue::I16(-32000));
    assert_eq!(engine.read_field(&buffer, "u16")?, FieldValue::U16(0xBEEF));
    assert_eq!(
        engine.read_field(&buffer, "i32")?,
        FieldValue::I32(-2_000_000_007)
    );
    assert_eq!(
        engine.read_field(&buffer, "u32")?,
        FieldValue::U32(0xDEADBEEF)
    );
    assert_eq!(
        engine.read_field(&buffer, "i64")?,
        FieldValue::I64(-9_000_000_000_000_000_000)
    );
    assert_eq!(
        engine.read_field(&buffer, "u64")?,
        FieldValue::U64(0x0102030405060708)
    );
    assert_eq!(engine.read_field(&buffer, "f32")?, FieldValue::F32(1.5));
    assert_eq!(engine.read_field(&buffer, "f64")?, FieldValue::F64(2.5));
    assert_eq!(engine.read_field(&buffer, "b")?, FieldValue::Bool(true));
    assert_eq!(engine.read_field(&buffer, "e")?, FieldValue::Enum(7));
    Ok(())
}

#[test]
fn read_write_aligned_round_trips_string() -> Result<(), TypedefError> {
    let mut schema = json!({
        "TypeDef:Struct": true,
        "properties": {
            "name": { "TypeDef:String": true }
        }
    });
    let engine = TypedefEngine::compile(&mut schema, LayoutMode::Aligned)?;
    let offset_map = engine.offset_map().expect("aligned mode has offset_map");
    let mut buffer = vec![0u8; offset_map.total_size() + 64];
    engine.write_field(&mut buffer, "name", &FieldValue::String("hello world"))?;
    assert_eq!(
        engine.read_field(&buffer, "name")?,
        FieldValue::String("hello world")
    );
    Ok(())
}

#[test]
fn read_write_aligned_round_trips_bytes() -> Result<(), TypedefError> {
    let mut schema = json!({
        "TypeDef:Struct": true,
        "properties": {
            "blob": { "TypeDef:Bytes": true }
        }
    });
    let engine = TypedefEngine::compile(&mut schema, LayoutMode::Aligned)?;
    let offset_map = engine.offset_map().expect("aligned mode has offset_map");
    let payload = b"the quick brown fox".to_vec();
    let mut buffer = vec![0u8; offset_map.total_size() + payload.len()];
    engine.write_field(&mut buffer, "blob", &FieldValue::Bytes(&payload))?;
    assert_eq!(
        engine.read_field(&buffer, "blob")?,
        FieldValue::Bytes(&payload)
    );
    Ok(())
}

#[test]
fn read_field_returns_access_error_in_packed_mode() -> Result<(), TypedefError> {
    let mut schema = json!({
        "TypeDef:Struct": true,
        "properties": { "id": { "TypeDef:Uint32": true } }
    });
    let engine = TypedefEngine::compile(&mut schema, LayoutMode::Packed)?;
    let buffer = [0u8; 4];
    let err = engine.read_field(&buffer, "id").unwrap_err();
    assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
    Ok(())
}

#[test]
fn write_field_returns_access_error_in_packed_mode() -> Result<(), TypedefError> {
    let mut schema = json!({
        "TypeDef:Struct": true,
        "properties": { "id": { "TypeDef:Uint32": true } }
    });
    let engine = TypedefEngine::compile(&mut schema, LayoutMode::Packed)?;
    let mut buffer = [0u8; 4];
    let err = engine
        .write_field(&mut buffer, "id", &FieldValue::U32(1))
        .unwrap_err();
    assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
    Ok(())
}

#[test]
fn read_field_returns_offset_error_for_missing_path() -> Result<(), TypedefError> {
    let mut schema = json!({
        "TypeDef:Struct": true,
        "properties": { "id": { "TypeDef:Uint32": true } }
    });
    let engine = TypedefEngine::compile(&mut schema, LayoutMode::Aligned)?;
    let buffer = [0u8; 8];
    let err = engine.read_field(&buffer, "missing").unwrap_err();
    assert!(matches!(err, TypedefError::Offset { .. }), "got {err:?}");
    Ok(())
}

#[test]
fn write_field_returns_offset_error_for_missing_path() -> Result<(), TypedefError> {
    let mut schema = json!({
        "TypeDef:Struct": true,
        "properties": { "id": { "TypeDef:Uint32": true } }
    });
    let engine = TypedefEngine::compile(&mut schema, LayoutMode::Aligned)?;
    let mut buffer = [0u8; 8];
    let err = engine
        .write_field(&mut buffer, "missing", &FieldValue::U32(1))
        .unwrap_err();
    assert!(matches!(err, TypedefError::Offset { .. }), "got {err:?}");
    Ok(())
}

#[test]
fn read_field_returns_access_error_for_composite_types() -> Result<(), TypedefError> {
    let mut schema = json!({
        "TypeDef:Struct": true,
        "properties": {
            "vals": {
                "TypeDef:Array": true,
                "items": { "TypeDef:Uint32": true }
            }
        }
    });
    let engine = TypedefEngine::compile(&mut schema, LayoutMode::Aligned)?;
    let buffer = [0u8; 8];
    let err = engine.read_field(&buffer, "vals").unwrap_err();
    assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
    Ok(())
}

#[test]
fn write_field_returns_access_error_for_composite_value() -> Result<(), TypedefError> {
    let mut schema = json!({
        "TypeDef:Struct": true,
        "properties": { "id": { "TypeDef:Uint32": true } }
    });
    let engine = TypedefEngine::compile(&mut schema, LayoutMode::Aligned)?;
    let mut buffer = [0u8; 8];
    let err = engine
        .write_field(&mut buffer, "id", &FieldValue::Struct { start: 0, end: 4 })
        .unwrap_err();
    assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
    Ok(())
}

#[test]
fn read_field_aligned_reads_nested_struct_byte_range() -> Result<(), TypedefError> {
    let mut schema = json!({
        "TypeDef:Struct": true,
        "properties": {
            "header": {
                "TypeDef:Struct": true,
                "properties": {
                    "version": { "TypeDef:Uint8": true },
                    "magic": { "TypeDef:Uint32": true }
                }
            }
        }
    });
    let engine = TypedefEngine::compile(&mut schema, LayoutMode::Aligned)?;
    let offset_map = engine.offset_map().expect("aligned mode");
    let mut buffer = vec![0u8; offset_map.total_size()];

    engine.write_field(&mut buffer, "header.version", &FieldValue::U8(3))?;
    engine.write_field(&mut buffer, "header.magic", &FieldValue::U32(0xCAFEBABE))?;

    assert_eq!(
        engine.read_field(&buffer, "header.version")?,
        FieldValue::U8(3)
    );
    assert_eq!(
        engine.read_field(&buffer, "header.magic")?,
        FieldValue::U32(0xCAFEBABE)
    );
    Ok(())
}
