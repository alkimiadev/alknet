//! Schema layer: TypeDef kind detection, annotation parsing, `$ref`
//! normalization, and the `Endian` enum.
//!
//! Per ADR-097 and the schema-layer spec. This module provides the
//! foundational types and functions that every other module depends on:
//! `TypeDef:*` kind detection, fixed byte-size lookups, natural alignment,
//! schema annotation parsing (`encoding`, `align`, `maxLength`, `endian`),
//! TUnion discriminator parsing, and `$ref` normalization from TypeBox
//! bare-name refs to JSON Pointer refs.

use crate::error::TypedefError;
use serde_json::Value;

const TYPEDEF_PREFIX: &str = "TypeDef:";

const BYTE_DISCRIMINATOR_TYPES: &[&str] = &[
    "TypeDef:Uint8",
    "TypeDef:Uint16",
    "TypeDef:Uint32",
];

/// Returns the `TypeDef:*` kind string if the schema node declares one.
/// Returns `None` if the node has no `TypeDef:*` keyword.
///
/// A TypeDef kind is recognized when the schema object has a key starting
/// with `TypeDef:` whose value is `true`. (Object form with annotations
/// like `{ "encoding": "..." }` is handled by the annotation parsers, not
/// here — `get_typedef_kind` only checks for the presence of the keyword.)
pub fn get_typedef_kind(node: &Value) -> Option<&str> {
    let obj = node.as_object()?;
    for key in obj.keys() {
        if key.starts_with(TYPEDEF_PREFIX) && obj.get(key) == Some(&Value::Bool(true)) {
            return Some(key.as_str());
        }
    }
    None
}

/// Returns the fixed byte size for a TypeDef kind, or `None` if variable-size
/// or composite (Struct/Union/Array/Record).
///
/// Only the 17 alknet-typedef kinds are recognized. `TypeDef:Int64` and
/// `TypeDef:Uint64` are NOT among the 17 kinds (TypeBox defines them but
/// alknet-typedef omits them), so they return `None` here.
pub fn type_size(kind: &str) -> Option<usize> {
    match kind {
        "TypeDef:Float32" | "TypeDef:Int32" | "TypeDef:Uint32" | "TypeDef:Enum" => Some(4),
        "TypeDef:Float64" => Some(8),
        "TypeDef:Int8" | "TypeDef:Uint8" | "TypeDef:Boolean" => Some(1),
        "TypeDef:Int16" | "TypeDef:Uint16" => Some(2),
        "TypeDef:String"
        | "TypeDef:Bytes"
        | "TypeDef:Struct"
        | "TypeDef:Union"
        | "TypeDef:Array"
        | "TypeDef:Record"
        | "TypeDef:Timestamp" => None,
        _ => None,
    }
}

/// Returns the natural alignment for a TypeDef kind.
///
/// Default alignment: 1 for u8/i8/bool, 2 for u16/i16, 4 for u32/i32/f32/enum,
/// 8 for u64/i64/f64. Variable-length types (String/Bytes/Record/Timestamp)
/// align to 4 (the length prefix is u32). Composite types (Struct/Union/Array)
/// return 1 here — their alignment is computed from their fields during
/// offset computation.
pub fn natural_alignment(kind: &str) -> usize {
    match kind {
        "TypeDef:Int8" | "TypeDef:Uint8" | "TypeDef:Boolean" => 1,
        "TypeDef:Int16" | "TypeDef:Uint16" => 2,
        "TypeDef:Int32" | "TypeDef:Uint32" | "TypeDef:Float32" | "TypeDef:Enum" => 4,
        "TypeDef:Float64" => 8,
        "TypeDef:String" | "TypeDef:Bytes" | "TypeDef:Record" | "TypeDef:Timestamp" => 4,
        "TypeDef:Struct" | "TypeDef:Union" | "TypeDef:Array" => 1,
        _ => 1,
    }
}

/// Returns true if the kind is a fixed-size type (known byte size at schema time).
pub fn is_fixed_size(kind: &str) -> bool {
    matches!(
        kind,
        "TypeDef:Float32"
            | "TypeDef:Float64"
            | "TypeDef:Int8"
            | "TypeDef:Int16"
            | "TypeDef:Int32"
            | "TypeDef:Uint8"
            | "TypeDef:Uint16"
            | "TypeDef:Uint32"
            | "TypeDef:Boolean"
            | "TypeDef:Enum"
    )
}

/// Byte endianness for multi-byte integer and float fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endian {
    Little,
    Big,
}

impl Endian {
    /// Parse from the schema's `"endian"` annotation. Defaults to `Little`
    /// if the annotation is absent or unrecognized.
    pub fn from_schema(schema: &Value) -> Self {
        parse_endian(schema)
    }
}

/// The encoding strategy for a variable-length type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VariableEncoding {
    /// `[length: u32][data]` — the default. Length prefix at a known offset,
    /// variable data follows immediately.
    LengthPrefixed,
    /// `{offset: u32, length: u32}` pointing into a separate data region.
    /// The metatensor blob tensor pattern.
    OffsetIndirect,
}

/// Parse the `"encoding"` annotation from a variable-length type's keyword value.
///
/// The keyword value may be `true` (shorthand for length-prefixed) or an
/// object with an `"encoding"` field. Defaults to `LengthPrefixed` when
/// absent or unrecognized.
pub fn parse_encoding(keyword_value: &Value) -> VariableEncoding {
    match keyword_value {
        Value::Bool(true) => VariableEncoding::LengthPrefixed,
        Value::Object(obj) => {
            let encoding = obj.get("encoding").and_then(Value::as_str);
            match encoding {
                Some("offset-indirect") => VariableEncoding::OffsetIndirect,
                _ => VariableEncoding::LengthPrefixed,
            }
        }
        _ => VariableEncoding::LengthPrefixed,
    }
}

/// Parse the `"align"` annotation from a schema node. Returns `None` if not
/// specified or not a non-negative integer.
pub fn parse_align(node: &Value) -> Option<usize> {
    let n = node.as_object()?.get("align")?.as_u64()?;
    Some(n as usize)
}

/// Parse the `"maxLength"` annotation (standard JSON Schema keyword).
/// Returns `None` if not specified or not a non-negative integer.
pub fn parse_max_length(node: &Value) -> Option<usize> {
    let n = node.as_object()?.get("maxLength")?.as_u64()?;
    Some(n as usize)
}

/// Parse the `"endian"` annotation. Defaults to `Little` if absent or
/// unrecognized. Operates on any node, not just the root.
pub fn parse_endian(node: &Value) -> Endian {
    match node.as_object().and_then(|o| o.get("endian")).and_then(Value::as_str) {
        Some("big") => Endian::Big,
        _ => Endian::Little,
    }
}

/// The kind of TUnion discriminator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscriminatorKind {
    /// Byte-offset discriminator: a fixed-size integer at a known byte offset.
    /// Mapping keys are stringified integers. Used by SFTP type bytes and
    /// call protocol event types.
    Byte {
        /// Byte position of the discriminator within the union's buffer.
        offset: usize,
        /// The `TypeDef:*` kind of the discriminator (typically
        /// `TypeDef:Uint8`).
        disc_type: String,
    },
    /// Field-name discriminator: a named field within the struct. Mapping keys
    /// are string values matching the discriminator field's value. The
    /// typedef.ts pattern.
    Field {
        /// The field name that holds the discriminator value.
        name: String,
    },
}

/// Parse the `"discriminator"` annotation from a TUnion schema node.
///
/// Returns [`TypedefError::Schema`] for malformed discriminators (unknown
/// `kind`, missing required `name`, or an unsupported discriminator `type`).
pub fn parse_discriminator(node: &Value) -> Result<DiscriminatorKind, TypedefError> {
    let obj = node.as_object().ok_or_else(|| {
        TypedefError::Schema("discriminator requires a schema object".to_string())
    })?;
    let disc = obj.get("discriminator").ok_or_else(|| {
        TypedefError::Schema("union is missing 'discriminator' annotation".to_string())
    })?;
    let disc_obj = disc.as_object().ok_or_else(|| {
        TypedefError::Schema("'discriminator' must be an object".to_string())
    })?;
    let kind = disc_obj.get("kind").and_then(Value::as_str).ok_or_else(|| {
        TypedefError::Schema("discriminator is missing 'kind' field".to_string())
    })?;
    match kind {
        "byte" => {
            let offset = disc_obj
                .get("offset")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let disc_type = disc_obj
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("TypeDef:Uint8")
                .to_string();
            if !BYTE_DISCRIMINATOR_TYPES.contains(&disc_type.as_str()) {
                return Err(TypedefError::Schema(format!(
                    "discriminator 'type' must be one of {BYTE_DISCRIMINATOR_TYPES:?}, got {disc_type:?}"
                )));
            }
            Ok(DiscriminatorKind::Byte { offset, disc_type })
        }
        "field" => {
            let name = disc_obj
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    TypedefError::Schema(
                        "field discriminator is missing required 'name' field".to_string(),
                    )
                })?
                .to_string();
            Ok(DiscriminatorKind::Field { name })
        }
        other => Err(TypedefError::Schema(format!(
            "unknown discriminator 'kind': {other:?} (expected \"byte\" or \"field\")"
        ))),
    }
}

/// Walk the schema tree. For every `"$ref"` whose value is a bare name
/// (no `#` prefix), rewrite it to `"#/$defs/<name>"`. Full JSON Pointer refs
/// (starting with `#`) pass through unchanged. Idempotent.
pub fn normalize_refs(schema: &mut Value) {
    normalize_refs_recursive(schema);
}

fn normalize_refs_recursive(node: &mut Value) {
    if let Value::Object(obj) = node {
        if let Some(Value::String(ref s)) = obj.get("$ref") {
            if !s.starts_with('#') && !s.is_empty() {
                let new_ref = format!("#/$defs/{s}");
                if let Some(slot) = obj.get_mut("$ref") {
                    *slot = Value::String(new_ref);
                }
            }
        }
        for value in obj.values_mut() {
            normalize_refs_recursive(value);
        }
    } else if let Value::Array(arr) = node {
        for item in arr.iter_mut() {
            normalize_refs_recursive(item);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn get_typedef_kind_detects_bool_keyword() {
        let schema = json!({"TypeDef:Uint32": true});
        assert_eq!(get_typedef_kind(&schema), Some("TypeDef:Uint32"));
    }

    #[test]
    fn get_typedef_kind_ignores_object_keyword() {
        let schema = json!({"TypeDef:String": {"encoding": "length-prefixed"}});
        assert_eq!(get_typedef_kind(&schema), None);
    }

    #[test]
    fn get_typedef_kind_none_for_plain_schema() {
        let schema = json!({"type": "object", "properties": {}});
        assert_eq!(get_typedef_kind(&schema), None);
    }

    #[test]
    fn type_size_fixed_kinds() {
        assert_eq!(type_size("TypeDef:Float32"), Some(4));
        assert_eq!(type_size("TypeDef:Float64"), Some(8));
        assert_eq!(type_size("TypeDef:Int8"), Some(1));
        assert_eq!(type_size("TypeDef:Int16"), Some(2));
        assert_eq!(type_size("TypeDef:Int32"), Some(4));
        assert_eq!(type_size("TypeDef:Uint8"), Some(1));
        assert_eq!(type_size("TypeDef:Uint16"), Some(2));
        assert_eq!(type_size("TypeDef:Uint32"), Some(4));
        assert_eq!(type_size("TypeDef:Boolean"), Some(1));
        assert_eq!(type_size("TypeDef:Enum"), Some(4));
    }

    #[test]
    fn type_size_variable_and_composite_kinds() {
        for kind in [
            "TypeDef:String",
            "TypeDef:Bytes",
            "TypeDef:Struct",
            "TypeDef:Union",
            "TypeDef:Array",
            "TypeDef:Record",
            "TypeDef:Timestamp",
        ] {
            assert_eq!(type_size(kind), None, "failed for {kind}");
        }
    }

    #[test]
    fn type_size_unknown_kind_returns_none() {
        assert_eq!(type_size("TypeDef:Int64"), None);
        assert_eq!(type_size("TypeDef:Uint64"), None);
        assert_eq!(type_size("not-a-typedef"), None);
    }

    #[test]
    fn natural_alignment_matches_spec() {
        assert_eq!(natural_alignment("TypeDef:Int8"), 1);
        assert_eq!(natural_alignment("TypeDef:Uint8"), 1);
        assert_eq!(natural_alignment("TypeDef:Boolean"), 1);
        assert_eq!(natural_alignment("TypeDef:Int16"), 2);
        assert_eq!(natural_alignment("TypeDef:Uint16"), 2);
        assert_eq!(natural_alignment("TypeDef:Int32"), 4);
        assert_eq!(natural_alignment("TypeDef:Uint32"), 4);
        assert_eq!(natural_alignment("TypeDef:Float32"), 4);
        assert_eq!(natural_alignment("TypeDef:Enum"), 4);
        assert_eq!(natural_alignment("TypeDef:Float64"), 8);
        assert_eq!(natural_alignment("TypeDef:String"), 4);
        assert_eq!(natural_alignment("TypeDef:Bytes"), 4);
        assert_eq!(natural_alignment("TypeDef:Record"), 4);
        assert_eq!(natural_alignment("TypeDef:Timestamp"), 4);
        assert_eq!(natural_alignment("TypeDef:Struct"), 1);
        assert_eq!(natural_alignment("TypeDef:Union"), 1);
        assert_eq!(natural_alignment("TypeDef:Array"), 1);
    }

    #[test]
    fn is_fixed_size_classifies_correctly() {
        for kind in [
            "TypeDef:Float32",
            "TypeDef:Float64",
            "TypeDef:Int8",
            "TypeDef:Int16",
            "TypeDef:Int32",
            "TypeDef:Uint8",
            "TypeDef:Uint16",
            "TypeDef:Uint32",
            "TypeDef:Boolean",
            "TypeDef:Enum",
        ] {
            assert!(is_fixed_size(kind), "expected fixed: {kind}");
        }
        for kind in [
            "TypeDef:String",
            "TypeDef:Bytes",
            "TypeDef:Struct",
            "TypeDef:Union",
            "TypeDef:Array",
            "TypeDef:Record",
            "TypeDef:Timestamp",
        ] {
            assert!(!is_fixed_size(kind), "expected variable: {kind}");
        }
    }

    #[test]
    fn endian_from_schema_defaults_to_little() {
        assert_eq!(Endian::from_schema(&json!({})), Endian::Little);
        assert_eq!(Endian::from_schema(&json!({"endian": "little"})), Endian::Little);
        assert_eq!(Endian::from_schema(&json!({"endian": "weird"})), Endian::Little);
    }

    #[test]
    fn endian_from_schema_big() {
        assert_eq!(Endian::from_schema(&json!({"endian": "big"})), Endian::Big);
    }

    #[test]
    fn parse_encoding_shorthand_true() {
        assert_eq!(parse_encoding(&json!(true)), VariableEncoding::LengthPrefixed);
    }

    #[test]
    fn parse_encoding_object_length_prefixed() {
        assert_eq!(
            parse_encoding(&json!({"encoding": "length-prefixed"})),
            VariableEncoding::LengthPrefixed
        );
    }

    #[test]
    fn parse_encoding_object_offset_indirect() {
        assert_eq!(
            parse_encoding(&json!({"encoding": "offset-indirect"})),
            VariableEncoding::OffsetIndirect
        );
    }

    #[test]
    fn parse_encoding_unknown_defaults_to_length_prefixed() {
        assert_eq!(
            parse_encoding(&json!({"encoding": "weird"})),
            VariableEncoding::LengthPrefixed
        );
        assert_eq!(parse_encoding(&json!(42)), VariableEncoding::LengthPrefixed);
        assert_eq!(parse_encoding(&json!(null)), VariableEncoding::LengthPrefixed);
    }

    #[test]
    fn parse_align_returns_value() {
        assert_eq!(parse_align(&json!({"align": 256})), Some(256));
        assert_eq!(parse_align(&json!({"align": 0})), Some(0));
    }

    #[test]
    fn parse_align_none_when_absent() {
        assert_eq!(parse_align(&json!({})), None);
        assert_eq!(parse_align(&json!({"align": "not-a-number"})), None);
    }

    #[test]
    fn parse_max_length_returns_value() {
        assert_eq!(parse_max_length(&json!({"maxLength": 1024})), Some(1024));
    }

    #[test]
    fn parse_max_length_none_when_absent() {
        assert_eq!(parse_max_length(&json!({})), None);
        assert_eq!(parse_max_length(&json!({"maxLength": "x"})), None);
    }

    #[test]
    fn parse_endian_alias_matches_from_schema() {
        assert_eq!(parse_endian(&json!({"endian": "big"})), Endian::Big);
        assert_eq!(parse_endian(&json!({})), Endian::Little);
    }

    #[test]
    fn parse_discriminator_byte_default_offset_and_type() {
        let schema = json!({"discriminator": {"kind": "byte"}});
        let disc = parse_discriminator(&schema).expect("byte discriminator");
        assert_eq!(disc, DiscriminatorKind::Byte {
            offset: 0,
            disc_type: "TypeDef:Uint8".to_string(),
        });
    }

    #[test]
    fn parse_discriminator_byte_explicit() {
        let schema = json!({
            "discriminator": {"kind": "byte", "offset": 4, "type": "TypeDef:Uint16"}
        });
        let disc = parse_discriminator(&schema).expect("byte discriminator");
        assert_eq!(disc, DiscriminatorKind::Byte {
            offset: 4,
            disc_type: "TypeDef:Uint16".to_string(),
        });
    }

    #[test]
    fn parse_discriminator_field() {
        let schema = json!({"discriminator": {"kind": "field", "name": "type"}});
        let disc = parse_discriminator(&schema).expect("field discriminator");
        assert_eq!(disc, DiscriminatorKind::Field {
            name: "type".to_string()
        });
    }

    #[test]
    fn parse_discriminator_missing_discriminator_is_error() {
        let schema = json!({"TypeDef:Union": true});
        assert!(matches!(
            parse_discriminator(&schema),
            Err(TypedefError::Schema(_))
        ));
    }

    #[test]
    fn parse_discriminator_field_missing_name_is_error() {
        let schema = json!({"discriminator": {"kind": "field"}});
        assert!(matches!(
            parse_discriminator(&schema),
            Err(TypedefError::Schema(_))
        ));
    }

    #[test]
    fn parse_discriminator_unknown_kind_is_error() {
        let schema = json!({"discriminator": {"kind": "magic"}});
        assert!(matches!(
            parse_discriminator(&schema),
            Err(TypedefError::Schema(_))
        ));
    }

    #[test]
    fn parse_discriminator_byte_invalid_type_is_error() {
        let schema = json!({
            "discriminator": {"kind": "byte", "type": "TypeDef:Float32"}
        });
        assert!(matches!(
            parse_discriminator(&schema),
            Err(TypedefError::Schema(_))
        ));
    }

    #[test]
    fn normalize_refs_rewrites_bare_name() {
        let mut schema = json!({"$ref": "Read"});
        normalize_refs(&mut schema);
        assert_eq!(schema, json!({"$ref": "#/$defs/Read"}));
    }

    #[test]
    fn normalize_refs_leaves_pointer_ref_unchanged() {
        let mut schema = json!({"$ref": "#/$defs/Read"});
        normalize_refs(&mut schema);
        assert_eq!(schema, json!({"$ref": "#/$defs/Read"}));
    }

    #[test]
    fn normalize_refs_is_idempotent() {
        let mut schema = json!({"$ref": "Read"});
        normalize_refs(&mut schema);
        normalize_refs(&mut schema);
        assert_eq!(schema, json!({"$ref": "#/$defs/Read"}));
    }

    #[test]
    fn normalize_refs_walks_nested_objects() {
        let mut schema = json!({
            "properties": {
                "child": {"$ref": "Child"},
                "other": {"$ref": "#/$defs/Other"}
            },
            "items": [
                {"$ref": "InArray"},
                {"foo": {"$ref": "Deep"}}
            ]
        });
        normalize_refs(&mut schema);
        assert_eq!(
            schema,
            json!({
                "properties": {
                    "child": {"$ref": "#/$defs/Child"},
                    "other": {"$ref": "#/$defs/Other"}
                },
                "items": [
                    {"$ref": "#/$defs/InArray"},
                    {"foo": {"$ref": "#/$defs/Deep"}}
                ]
            })
        );
    }

    #[test]
    fn normalize_refs_preserves_sibling_keys() {
        let mut schema = json!({
            "$ref": "Read",
            "typedef:annotation": "kept"
        });
        normalize_refs(&mut schema);
        assert_eq!(
            schema,
            json!({
                "$ref": "#/$defs/Read",
                "typedef:annotation": "kept"
            })
        );
    }
}