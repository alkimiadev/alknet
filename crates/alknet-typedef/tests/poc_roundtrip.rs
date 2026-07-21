//! POC round-trip tests adapted from `/workspace/alknet-typedef-poc/`.
//!
//! These tests re-validate the byte-identical round-trip behaviour that
//! the POC verified: fixed-size primitives, length-prefixed strings and
//! bytes, nested structs, big-endian, alignment padding, packed-layout
//! `LayoutBuilder` with `data_access` writes, and `SequentialReader`
//! walks. Each test writes values to a buffer at computed offsets and
//! reads them back, asserting both the values and (where applicable)
//! the byte positions.

use alknet_typedef::*;
use alknet_typedef::data_access;
use alknet_typedef::tunion;
use serde_json::json;
use std::collections::HashMap;

fn var_sizes(pairs: &[(&str, usize)]) -> HashMap<String, usize> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), *v))
        .collect()
}

#[test]
fn fixed_size_round_trip_via_offset_map() -> Result<(), TypedefError> {
    let schema = json!({
        "TypeDef:Struct": true,
        "properties": {
            "id": { "TypeDef:Uint32": true },
            "score": { "TypeDef:Float32": true },
            "flag": { "TypeDef:Uint8": true },
            "count": { "TypeDef:Uint16": true }
        }
    });
    let offset_map = OffsetMap::compute(&schema)?;
    let mut buffer = vec![0u8; offset_map.total_size()];

    let id_range = offset_map.get("id").expect("id range");
    data_access::write_u32(&mut buffer, id_range.start, 42, "id", Endian::Little)?;
    let score_range = offset_map.get("score").expect("score range");
    data_access::write_f32(&mut buffer, score_range.start, 1.5, "score", Endian::Little)?;
    let flag_range = offset_map.get("flag").expect("flag range");
    data_access::write_u8(&mut buffer, flag_range.start, 1, "flag")?;
    let count_range = offset_map.get("count").expect("count range");
    data_access::write_u16(&mut buffer, count_range.start, 1000, "count", Endian::Little)?;

    assert_eq!(
        data_access::read_u32(&buffer, id_range.start, "id", Endian::Little)?,
        42
    );
    let score = data_access::read_f32(&buffer, score_range.start, "score", Endian::Little)?;
    assert!((score - 1.5).abs() < 0.001, "score: {score}");
    assert_eq!(data_access::read_u8(&buffer, flag_range.start, "flag")?, 1);
    assert_eq!(
        data_access::read_u16(&buffer, count_range.start, "count", Endian::Little)?,
        1000
    );
    Ok(())
}

#[test]
fn fixed_size_round_trip_via_engine_aligned() -> Result<(), TypedefError> {
    let mut schema = json!({
        "TypeDef:Struct": true,
        "endian": "little",
        "properties": {
            "id": { "TypeDef:Uint32": true },
            "score": { "TypeDef:Float32": true },
            "flag": { "TypeDef:Uint8": true }
        }
    });
    let engine = TypedefEngine::compile(&mut schema, LayoutMode::Aligned)?;
    let offset_map = engine.offset_map().expect("aligned mode has offset_map");
    let mut buffer = vec![0u8; offset_map.total_size()];

    engine.write_field(&mut buffer, "id", &FieldValue::U32(42))?;
    engine.write_field(&mut buffer, "score", &FieldValue::F32(1.5))?;
    engine.write_field(&mut buffer, "flag", &FieldValue::U8(1))?;

    assert_eq!(engine.read_field(&buffer, "id")?, FieldValue::U32(42));
    let score = match engine.read_field(&buffer, "score")? {
        FieldValue::F32(f) => f,
        other => panic!("expected F32, got {other:?}"),
    };
    assert!((score - 1.5).abs() < 0.001);
    assert_eq!(engine.read_field(&buffer, "flag")?, FieldValue::U8(1));
    Ok(())
}

#[test]
fn string_round_trip_via_data_access() -> Result<(), TypedefError> {
    let mut buffer = vec![0u8; 32];
    let written = data_access::write_string(&mut buffer, 0, "hello", "name", Endian::Little)?;
    assert_eq!(written, 4 + 5);
    assert_eq!(buffer[0..4], 5u32.to_le_bytes());
    assert_eq!(&buffer[4..9], b"hello");
    assert_eq!(
        data_access::read_string(&buffer, 0, "name", Endian::Little)?,
        "hello"
    );
    Ok(())
}

#[test]
fn string_round_trip_via_engine_aligned() -> Result<(), TypedefError> {
    let mut schema = json!({
        "TypeDef:Struct": true,
        "properties": {
            "name": { "TypeDef:String": true }
        }
    });
    let engine = TypedefEngine::compile(&mut schema, LayoutMode::Aligned)?;
    let offset_map = engine.offset_map().expect("aligned mode has offset_map");
    let mut buffer = vec![0u8; offset_map.total_size() + 64];
    engine.write_field(&mut buffer, "name", &FieldValue::String("hello"))?;
    assert_eq!(
        engine.read_field(&buffer, "name")?,
        FieldValue::String("hello")
    );
    Ok(())
}

#[test]
fn bytes_round_trip_via_data_access() -> Result<(), TypedefError> {
    let payload = [0xAA, 0xBB, 0xCC, 0xDD];
    let mut buffer = vec![0u8; 32];
    let written = data_access::write_bytes(&mut buffer, 0, &payload, "data", Endian::Little)?;
    assert_eq!(written, 4 + 4);
    assert_eq!(buffer[0..4], 4u32.to_le_bytes());
    assert_eq!(&buffer[4..8], &payload);
    assert_eq!(
        data_access::read_bytes(&buffer, 0, "data", Endian::Little)?,
        &payload[..]
    );
    Ok(())
}

#[test]
fn nested_struct_round_trip_via_offset_map() -> Result<(), TypedefError> {
    let schema = json!({
        "TypeDef:Struct": true,
        "properties": {
            "header": {
                "TypeDef:Struct": true,
                "properties": {
                    "version": { "TypeDef:Uint32": true },
                    "magic": { "TypeDef:Uint32": true }
                }
            },
            "payload": { "TypeDef:Bytes": true }
        }
    });
    let offset_map = OffsetMap::compute(&schema)?;

    let header_version = offset_map.get("header.version").expect("header.version");
    let header_magic = offset_map.get("header.magic").expect("header.magic");
    let payload_prefix = offset_map.get("payload").expect("payload");

    assert_eq!(header_version.start, 0);
    assert_eq!(header_magic.start, 4);
    assert_eq!(payload_prefix.start, 8);

    let data = b"body-data".to_vec();
    let mut buffer = vec![0u8; offset_map.total_size() + data.len()];
    data_access::write_u32(&mut buffer, header_version.start, 1, "header.version", Endian::Little)?;
    data_access::write_u32(&mut buffer, header_magic.start, 0xCAFEBABE, "header.magic", Endian::Little)?;
    data_access::write_bytes(&mut buffer, payload_prefix.start, &data, "payload", Endian::Little)?;

    assert_eq!(
        data_access::read_u32(&buffer, header_version.start, "header.version", Endian::Little)?,
        1
    );
    assert_eq!(
        data_access::read_u32(&buffer, header_magic.start, "header.magic", Endian::Little)?,
        0xCAFEBABE
    );
    assert_eq!(
        data_access::read_bytes(&buffer, payload_prefix.start, "payload", Endian::Little)?,
        &data[..]
    );
    Ok(())
}

#[test]
fn nested_struct_round_trip_via_engine_aligned() -> Result<(), TypedefError> {
    let mut schema = json!({
        "TypeDef:Struct": true,
        "properties": {
            "header": {
                "TypeDef:Struct": true,
                "properties": {
                    "version": { "TypeDef:Uint8": true },
                    "flags": { "TypeDef:Uint8": true }
                }
            },
            "payload_len": { "TypeDef:Uint32": true }
        }
    });
    let engine = TypedefEngine::compile(&mut schema, LayoutMode::Aligned)?;
    let offset_map = engine.offset_map().expect("aligned mode");

    assert_eq!(offset_map.get("header.version").unwrap().start, 0);
    assert_eq!(offset_map.get("header.flags").unwrap().start, 1);
    assert_eq!(offset_map.get("payload_len").unwrap().start, 4);

    let mut buffer = vec![0u8; offset_map.total_size()];
    engine.write_field(&mut buffer, "header.version", &FieldValue::U8(1))?;
    engine.write_field(&mut buffer, "header.flags", &FieldValue::U8(0x0F))?;
    engine.write_field(&mut buffer, "payload_len", &FieldValue::U32(1024))?;

    assert_eq!(
        engine.read_field(&buffer, "header.version")?,
        FieldValue::U8(1)
    );
    assert_eq!(
        engine.read_field(&buffer, "header.flags")?,
        FieldValue::U8(0x0F)
    );
    assert_eq!(
        engine.read_field(&buffer, "payload_len")?,
        FieldValue::U32(1024)
    );
    Ok(())
}

#[test]
fn big_endian_round_trip_via_offset_map() -> Result<(), TypedefError> {
    let schema = json!({
        "TypeDef:Struct": true,
        "endian": "big",
        "properties": {
            "id": { "TypeDef:Uint32": true },
            "offset": { "TypeDef:Float64": true }
        }
    });
    let offset_map = OffsetMap::compute(&schema)?;
    let endian = Endian::from_schema(&schema);
    assert_eq!(endian, Endian::Big);

    let id_range = offset_map.get("id").expect("id");
    let offset_range = offset_map.get("offset").expect("offset");

    assert_eq!(id_range.start, 0);
    assert_eq!(offset_range.start, 8);

    let value: f64 = std::f64::consts::PI;
    let mut buffer = vec![0u8; offset_map.total_size()];
    data_access::write_u32(&mut buffer, id_range.start, 0x01020304, "id", endian)?;
    data_access::write_f64(&mut buffer, offset_range.start, value, "offset", endian)?;

    assert_eq!(&buffer[0..4], &[0x01, 0x02, 0x03, 0x04]);
    assert_eq!(&buffer[4..8], &[0x00, 0x00, 0x00, 0x00]);
    assert_eq!(&buffer[8..16], value.to_be_bytes());

    assert_eq!(
        data_access::read_u32(&buffer, id_range.start, "id", endian)?,
        0x01020304
    );
    let read = data_access::read_f64(&buffer, offset_range.start, "offset", endian)?;
    assert!((read - value).abs() < 1e-12);
    Ok(())
}

#[test]
fn alignment_padding_round_trip_u8_then_u32() -> Result<(), TypedefError> {
    let schema = json!({
        "TypeDef:Struct": true,
        "properties": {
            "flag": { "TypeDef:Uint8": true },
            "id": { "TypeDef:Uint32": true }
        }
    });
    let offset_map = OffsetMap::compute(&schema)?;

    let flag_range = offset_map.get("flag").expect("flag");
    let id_range = offset_map.get("id").expect("id");

    assert_eq!(flag_range.start, 0);
    assert_eq!(flag_range.end, 1);
    assert_eq!(id_range.start, 4);
    assert_eq!(id_range.end, 8);
    assert_eq!(offset_map.total_size(), 8);

    let mut buffer = vec![0u8; offset_map.total_size()];
    data_access::write_u8(&mut buffer, flag_range.start, 0xAB, "flag")?;
    data_access::write_u32(&mut buffer, id_range.start, 0x01020304, "id", Endian::Little)?;

    assert_eq!(buffer[0], 0xAB);
    assert_eq!(&buffer[1..4], &[0x00, 0x00, 0x00]);
    assert_eq!(&buffer[4..8], 0x01020304u32.to_le_bytes());

    assert_eq!(data_access::read_u8(&buffer, flag_range.start, "flag")?, 0xAB);
    assert_eq!(
        data_access::read_u32(&buffer, id_range.start, "id", Endian::Little)?,
        0x01020304
    );
    Ok(())
}

#[test]
fn packed_layout_round_trip_via_layout_builder() -> Result<(), TypedefError> {
    let schema = json!({
        "TypeDef:Struct": true,
        "endian": "little",
        "properties": {
            "flag": { "TypeDef:Uint8": true },
            "id": { "TypeDef:Uint32": true },
            "payload": { "TypeDef:String": true }
        }
    });
    let builder = LayoutBuilder::new(&schema)?;
    let layout = builder.build(&var_sizes(&[("payload", 10)]))?;

    let flag_pos = layout.get("flag").expect("flag");
    let id_pos = layout.get("id").expect("id");
    let payload_pos = layout.get("payload").expect("payload");

    assert_eq!(flag_pos.offset, 0);
    assert_eq!(id_pos.offset, 1);
    assert_eq!(payload_pos.offset, 5);
    assert_eq!(layout.total_size(), 19);

    let payload_str = "ten bytes!";
    let payload_bytes = payload_str.as_bytes();
    assert_eq!(payload_bytes.len(), 10);
    let mut buffer = vec![0u8; layout.total_size()];
    data_access::write_u8(&mut buffer, flag_pos.offset, 0xAB, "flag")?;
    data_access::write_u32(&mut buffer, id_pos.offset, 0x01020304, "id", Endian::Little)?;
    data_access::write_string(&mut buffer, payload_pos.offset, payload_str, "payload", Endian::Little)?;

    assert_eq!(buffer[0], 0xAB);
    assert_eq!(&buffer[1..5], 0x01020304u32.to_le_bytes());
    assert_eq!(&buffer[5..9], 10u32.to_le_bytes());
    assert_eq!(&buffer[9..19], payload_bytes);

    assert_eq!(data_access::read_u8(&buffer, flag_pos.offset, "flag")?, 0xAB);
    assert_eq!(
        data_access::read_u32(&buffer, id_pos.offset, "id", Endian::Little)?,
        0x01020304
    );
    assert_eq!(
        data_access::read_string(&buffer, payload_pos.offset, "payload", Endian::Little)?,
        payload_str
    );
    Ok(())
}

#[test]
fn sequential_reader_round_trip_packed_buffer() -> Result<(), TypedefError> {
    let schema = json!({
        "TypeDef:Struct": true,
        "endian": "little",
        "properties": {
            "id": { "TypeDef:Uint8": true },
            "name": { "TypeDef:String": true },
            "tail": { "TypeDef:Uint8": true }
        }
    });
    let builder = LayoutBuilder::new(&schema)?;
    let payload = "hello";
    let layout = builder.build(&var_sizes(&[("name", payload.len())]))?;

    let mut buffer = vec![0u8; layout.total_size()];
    data_access::write_u8(&mut buffer, 0, 7, "id")?;
    data_access::write_string(&mut buffer, 1, payload, "name", Endian::Little)?;
    let after = 1 + 4 + payload.len();
    data_access::write_u8(&mut buffer, after, 99, "tail")?;

    let mut reader = SequentialReader::new(&schema)?;
    assert_eq!(reader.endian(), Endian::Little);
    assert_eq!(reader.position(), 0);

    let (name, value) = reader.read_next(&buffer)?.expect("field 0");
    assert_eq!(name, "id");
    assert_eq!(value, FieldValue::U8(7));
    assert_eq!(reader.position(), 1);

    let (name, value) = reader.read_next(&buffer)?.expect("field 1");
    assert_eq!(name, "name");
    assert_eq!(value, FieldValue::String("hello"));
    assert_eq!(reader.position(), after);

    let (name, value) = reader.read_next(&buffer)?.expect("field 2");
    assert_eq!(name, "tail");
    assert_eq!(value, FieldValue::U8(99));
    assert_eq!(reader.position(), after + 1);

    assert!(reader.read_next(&buffer)?.is_none());
    Ok(())
}

#[test]
fn sequential_reader_read_field_walks_preceding_fields() -> Result<(), TypedefError> {
    let schema = json!({
        "TypeDef:Struct": true,
        "endian": "little",
        "properties": {
            "a": { "TypeDef:Uint8": true },
            "b": { "TypeDef:Uint32": true },
            "c": { "TypeDef:Uint8": true }
        }
    });
    let mut buffer = vec![0u8; 16];
    data_access::write_u8(&mut buffer, 0, 1, "a")?;
    data_access::write_u32(&mut buffer, 1, 0xDEADBEEF, "b", Endian::Little)?;
    data_access::write_u8(&mut buffer, 5, 9, "c")?;

    let mut reader = SequentialReader::new(&schema)?;
    let value = reader.read_field(&buffer, "c")?;
    assert_eq!(value, FieldValue::U8(9));
    assert_eq!(reader.position(), 6);

    reader.reset();
    let value = reader.read_field(&buffer, "b")?;
    assert_eq!(value, FieldValue::U32(0xDEADBEEF));
    Ok(())
}

#[test]
fn tunion_byte_offset_discriminator_dispatch() -> Result<(), TypedefError> {
    let union_schema = json!({
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
    });
    let mut buffer = vec![0u8; 32];
    buffer[0] = 5;
    data_access::write_u32(&mut buffer, 1, 0x01020304, "Read.handle", Endian::Big)?;
    data_access::write_u32(&mut buffer, 5, 4096, "Read.length", Endian::Big)?;

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
fn tunion_byte_offset_discriminator_size_lookup() -> Result<(), TypedefError> {
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