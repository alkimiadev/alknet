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
use std::fmt;
use std::str::FromStr;

const TYPEDEF_PREFIX: &str = "TypeDef:";

pub(crate) const U32_SIZE: usize = 4;
pub(crate) const DISCRIMINATOR_PATH: &str = "__discriminator";

const BYTE_DISCRIMINATOR_TYPES: &[TypeDefKind] = &[
    TypeDefKind::Uint8,
    TypeDefKind::Uint16,
    TypeDefKind::Uint32,
];

/// The 17 `TypeDef:*` kinds recognized by the engine.
///
/// Each variant corresponds to a `TypeDef:<name>` JSON Schema keyword.
/// The enum provides compile-time exhaustiveness checking and integer
/// discriminant dispatch (jump table) instead of string comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TypeDefKind {
    Int8,
    Int16,
    Int32,
    Uint8,
    Uint16,
    Uint32,
    Uint64,
    Float32,
    Float64,
    Boolean,
    Enum,
    String,
    Bytes,
    Struct,
    Union,
    Array,
    Record,
    Timestamp,
}

impl TypeDefKind {
    /// The JSON Schema keyword string, e.g. `"TypeDef:Int8"`.
    pub fn as_str(self) -> &'static str {
        match self {
            TypeDefKind::Int8 => "TypeDef:Int8",
            TypeDefKind::Int16 => "TypeDef:Int16",
            TypeDefKind::Int32 => "TypeDef:Int32",
            TypeDefKind::Uint8 => "TypeDef:Uint8",
            TypeDefKind::Uint16 => "TypeDef:Uint16",
            TypeDefKind::Uint32 => "TypeDef:Uint32",
            TypeDefKind::Uint64 => "TypeDef:Uint64",
            TypeDefKind::Float32 => "TypeDef:Float32",
            TypeDefKind::Float64 => "TypeDef:Float64",
            TypeDefKind::Boolean => "TypeDef:Boolean",
            TypeDefKind::Enum => "TypeDef:Enum",
            TypeDefKind::String => "TypeDef:String",
            TypeDefKind::Bytes => "TypeDef:Bytes",
            TypeDefKind::Struct => "TypeDef:Struct",
            TypeDefKind::Union => "TypeDef:Union",
            TypeDefKind::Array => "TypeDef:Array",
            TypeDefKind::Record => "TypeDef:Record",
            TypeDefKind::Timestamp => "TypeDef:Timestamp",
        }
    }

    /// Fixed byte size, or `None` for variable-size / composite kinds.
    pub fn type_size(self) -> Option<usize> {
        match self {
            TypeDefKind::Float32 | TypeDefKind::Int32 | TypeDefKind::Uint32 | TypeDefKind::Enum => {
                Some(4)
            }
            TypeDefKind::Float64 => Some(8),
            TypeDefKind::Int8 | TypeDefKind::Uint8 | TypeDefKind::Boolean => Some(1),
            TypeDefKind::Int16 | TypeDefKind::Uint16 => Some(2),
            TypeDefKind::String
            | TypeDefKind::Bytes
            | TypeDefKind::Struct
            | TypeDefKind::Union
            | TypeDefKind::Array
            | TypeDefKind::Record
            | TypeDefKind::Timestamp
            | TypeDefKind::Uint64 => None,
        }
    }

    /// Natural alignment: 1 for u8/i8/bool, 2 for u16/i16, 4 for u32/i32/f32/enum,
    /// 8 for f64, 4 for variable-length (u32 length prefix), 1 for composites.
    pub fn natural_alignment(self) -> usize {
        match self {
            TypeDefKind::Int8 | TypeDefKind::Uint8 | TypeDefKind::Boolean => 1,
            TypeDefKind::Int16 | TypeDefKind::Uint16 => 2,
            TypeDefKind::Int32
            | TypeDefKind::Uint32
            | TypeDefKind::Float32
            | TypeDefKind::Enum => 4,
            TypeDefKind::Float64 => 8,
            TypeDefKind::String
            | TypeDefKind::Bytes
            | TypeDefKind::Record
            | TypeDefKind::Timestamp => 4,
            TypeDefKind::Struct | TypeDefKind::Union | TypeDefKind::Array => 1,
            TypeDefKind::Uint64 => 8,
        }
    }

    /// Returns `true` for fixed-size primitive kinds.
    pub fn is_fixed_size(self) -> bool {
        matches!(
            self,
            TypeDefKind::Float32
                | TypeDefKind::Float64
                | TypeDefKind::Int8
                | TypeDefKind::Int16
                | TypeDefKind::Int32
                | TypeDefKind::Uint8
                | TypeDefKind::Uint16
                | TypeDefKind::Uint32
                | TypeDefKind::Boolean
                | TypeDefKind::Enum
        )
    }

    /// Returns `true` for kinds whose read/write functions need an `Endian` parameter.
    pub fn needs_endian(self) -> bool {
        matches!(
            self,
            TypeDefKind::Int16
                | TypeDefKind::Int32
                | TypeDefKind::Uint16
                | TypeDefKind::Uint32
                | TypeDefKind::Uint64
                | TypeDefKind::Float32
                | TypeDefKind::Float64
                | TypeDefKind::Enum
                | TypeDefKind::String
                | TypeDefKind::Bytes
                | TypeDefKind::Timestamp
        )
    }

    /// Returns `true` for composite kinds (Struct, Union, Array, Record).
    pub fn is_composite(self) -> bool {
        matches!(
            self,
            TypeDefKind::Struct | TypeDefKind::Union | TypeDefKind::Array | TypeDefKind::Record
        )
    }

    /// Returns `true` for variable-length kinds (String, Bytes, Timestamp, Record).
    pub fn is_variable_length(self) -> bool {
        matches!(
            self,
            TypeDefKind::String
                | TypeDefKind::Bytes
                | TypeDefKind::Timestamp
                | TypeDefKind::Record
        )
    }
}

impl fmt::Display for TypeDefKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TypeDefKind {
    type Err = TypedefError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "TypeDef:Int8" => Ok(TypeDefKind::Int8),
            "TypeDef:Int16" => Ok(TypeDefKind::Int16),
            "TypeDef:Int32" => Ok(TypeDefKind::Int32),
            "TypeDef:Uint8" => Ok(TypeDefKind::Uint8),
            "TypeDef:Uint16" => Ok(TypeDefKind::Uint16),
            "TypeDef:Uint32" => Ok(TypeDefKind::Uint32),
            "TypeDef:Uint64" => Ok(TypeDefKind::Uint64),
            "TypeDef:Float32" => Ok(TypeDefKind::Float32),
            "TypeDef:Float64" => Ok(TypeDefKind::Float64),
            "TypeDef:Boolean" => Ok(TypeDefKind::Boolean),
            "TypeDef:Enum" => Ok(TypeDefKind::Enum),
            "TypeDef:String" => Ok(TypeDefKind::String),
            "TypeDef:Bytes" => Ok(TypeDefKind::Bytes),
            "TypeDef:Struct" => Ok(TypeDefKind::Struct),
            "TypeDef:Union" => Ok(TypeDefKind::Union),
            "TypeDef:Array" => Ok(TypeDefKind::Array),
            "TypeDef:Record" => Ok(TypeDefKind::Record),
            "TypeDef:Timestamp" => Ok(TypeDefKind::Timestamp),
            other => Err(TypedefError::Schema(format!(
                "unknown TypeDef kind: {other}"
            ))),
        }
    }
}

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

/// Returns the `TypeDefKind` enum variant if the schema node declares one
/// (boolean form only, like `get_typedef_kind`).
pub fn get_typedef_kind_enum(node: &Value) -> Option<TypeDefKind> {
    get_typedef_kind(node).and_then(|s| s.parse().ok())
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
    match node
        .as_object()
        .and_then(|o| o.get("endian"))
        .and_then(Value::as_str)
    {
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
        disc_type: TypeDefKind,
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
    let disc_obj = disc
        .as_object()
        .ok_or_else(|| TypedefError::Schema("'discriminator' must be an object".to_string()))?;
    let kind = disc_obj
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| TypedefError::Schema("discriminator is missing 'kind' field".to_string()))?;
    match kind {
        "byte" => {
            let offset = disc_obj.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
            let disc_type_str = disc_obj
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("TypeDef:Uint8");
            let disc_type: TypeDefKind = disc_type_str.parse().map_err(|_| {
                TypedefError::Schema(format!(
                    "discriminator 'type' must be one of {BYTE_DISCRIMINATOR_TYPES:?}, got {disc_type_str:?}"
                ))
            })?;
            if !BYTE_DISCRIMINATOR_TYPES.contains(&disc_type) {
                return Err(TypedefError::Schema(format!(
                    "discriminator 'type' must be one of {BYTE_DISCRIMINATOR_TYPES:?}, got {disc_type:?}"
                )));
            }
            Ok(DiscriminatorKind::Byte {
                offset,
                disc_type,
            })
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

/// Detect a `TypeDef:*` kind from a schema node, accepting either the
/// boolean form (`{ "TypeDef:String": true }`) or the object-annotation
/// form (`{ "TypeDef:String": { "encoding": "..." } }`).
///
/// [`get_typedef_kind`] only recognizes the boolean form; layout computation
/// and engine dispatch also need to recognize the object form so that
/// variable-length encoding annotations don't hide the kind.
pub fn get_typedef_kind_loose(node: &Value) -> Option<&str> {
    let obj = node.as_object()?;
    for key in obj.keys() {
        if key.starts_with(TYPEDEF_PREFIX) && obj.get(key).is_some_and(|v| !v.is_null()) {
            return Some(key.as_str());
        }
    }
    None
}

/// Like [`get_typedef_kind_loose`] but returns the parsed [`TypeDefKind`] enum.
pub fn get_typedef_kind_loose_enum(node: &Value) -> Option<TypeDefKind> {
    get_typedef_kind_loose(node).and_then(|s| s.parse().ok())
}

/// Resolve a `$ref` against the root schema, or return the inline schema.
///
/// If `node` has a `"$ref"` key, parse the JSON Pointer and walk `root`.
/// Otherwise, return `node` itself (it's an inline schema).
pub fn resolve_ref_or_inline<'a>(node: &'a Value, root: &'a Value) -> Option<&'a Value> {
    let obj = node.as_object()?;
    if let Some(Value::String(ref_path)) = obj.get("$ref") {
        return resolve_ref(root, ref_path);
    }
    Some(node)
}

/// Resolve a JSON Pointer `$ref` (e.g., `"#/$defs/Read"`) against `root`.
pub fn resolve_ref<'a>(root: &'a Value, ref_path: &str) -> Option<&'a Value> {
    let stripped = ref_path.strip_prefix('#').unwrap_or(ref_path);
    let stripped = stripped.strip_prefix('/').unwrap_or(stripped);
    if stripped.is_empty() {
        return Some(root);
    }
    let mut current = root;
    for segment in stripped.split('/') {
        let decoded = segment.replace("~1", "/").replace("~0", "~");
        if let Ok(idx) = decoded.parse::<usize>() {
            current = current.get(idx)?;
        } else {
            current = current.get(&decoded)?;
        }
    }
    Some(current)
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
        assert_eq!(TypeDefKind::Float32.type_size(), Some(4));
        assert_eq!(TypeDefKind::Float64.type_size(), Some(8));
        assert_eq!(TypeDefKind::Int8.type_size(), Some(1));
        assert_eq!(TypeDefKind::Int16.type_size(), Some(2));
        assert_eq!(TypeDefKind::Int32.type_size(), Some(4));
        assert_eq!(TypeDefKind::Uint8.type_size(), Some(1));
        assert_eq!(TypeDefKind::Uint16.type_size(), Some(2));
        assert_eq!(TypeDefKind::Uint32.type_size(), Some(4));
        assert_eq!(TypeDefKind::Boolean.type_size(), Some(1));
        assert_eq!(TypeDefKind::Enum.type_size(), Some(4));
    }

    #[test]
    fn type_size_variable_and_composite_kinds() {
        for kind in [
            TypeDefKind::String,
            TypeDefKind::Bytes,
            TypeDefKind::Struct,
            TypeDefKind::Union,
            TypeDefKind::Array,
            TypeDefKind::Record,
            TypeDefKind::Timestamp,
        ] {
            assert_eq!(kind.type_size(), None, "failed for {kind}");
        }
    }

    #[test]
    fn type_size_unknown_kind_returns_none() {
        assert!("TypeDef:Int64".parse::<TypeDefKind>().is_err());
        assert!("TypeDef:Uint128".parse::<TypeDefKind>().is_err());
        assert!("not-a-typedef".parse::<TypeDefKind>().is_err());
    }

    #[test]
    fn natural_alignment_matches_spec() {
        assert_eq!(TypeDefKind::Int8.natural_alignment(), 1);
        assert_eq!(TypeDefKind::Uint8.natural_alignment(), 1);
        assert_eq!(TypeDefKind::Boolean.natural_alignment(), 1);
        assert_eq!(TypeDefKind::Int16.natural_alignment(), 2);
        assert_eq!(TypeDefKind::Uint16.natural_alignment(), 2);
        assert_eq!(TypeDefKind::Int32.natural_alignment(), 4);
        assert_eq!(TypeDefKind::Uint32.natural_alignment(), 4);
        assert_eq!(TypeDefKind::Float32.natural_alignment(), 4);
        assert_eq!(TypeDefKind::Enum.natural_alignment(), 4);
        assert_eq!(TypeDefKind::Float64.natural_alignment(), 8);
        assert_eq!(TypeDefKind::String.natural_alignment(), 4);
        assert_eq!(TypeDefKind::Bytes.natural_alignment(), 4);
        assert_eq!(TypeDefKind::Record.natural_alignment(), 4);
        assert_eq!(TypeDefKind::Timestamp.natural_alignment(), 4);
        assert_eq!(TypeDefKind::Struct.natural_alignment(), 1);
        assert_eq!(TypeDefKind::Union.natural_alignment(), 1);
        assert_eq!(TypeDefKind::Array.natural_alignment(), 1);
    }

    #[test]
    fn is_fixed_size_classifies_correctly() {
        for kind in [
            TypeDefKind::Float32,
            TypeDefKind::Float64,
            TypeDefKind::Int8,
            TypeDefKind::Int16,
            TypeDefKind::Int32,
            TypeDefKind::Uint8,
            TypeDefKind::Uint16,
            TypeDefKind::Uint32,
            TypeDefKind::Boolean,
            TypeDefKind::Enum,
        ] {
            assert!(kind.is_fixed_size(), "expected fixed: {kind}");
        }
        for kind in [
            TypeDefKind::String,
            TypeDefKind::Bytes,
            TypeDefKind::Struct,
            TypeDefKind::Union,
            TypeDefKind::Array,
            TypeDefKind::Record,
            TypeDefKind::Timestamp,
        ] {
            assert!(!kind.is_fixed_size(), "expected variable: {kind}");
        }
    }

    #[test]
    fn endian_from_schema_defaults_to_little() {
        assert_eq!(Endian::from_schema(&json!({})), Endian::Little);
        assert_eq!(
            Endian::from_schema(&json!({"endian": "little"})),
            Endian::Little
        );
        assert_eq!(
            Endian::from_schema(&json!({"endian": "weird"})),
            Endian::Little
        );
    }

    #[test]
    fn endian_from_schema_big() {
        assert_eq!(Endian::from_schema(&json!({"endian": "big"})), Endian::Big);
    }

    #[test]
    fn parse_encoding_shorthand_true() {
        assert_eq!(
            parse_encoding(&json!(true)),
            VariableEncoding::LengthPrefixed
        );
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
        assert_eq!(
            parse_encoding(&json!(null)),
            VariableEncoding::LengthPrefixed
        );
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
        assert_eq!(
            disc,
            DiscriminatorKind::Byte {
                offset: 0,
                disc_type: TypeDefKind::Uint8,
            }
        );
    }

    #[test]
    fn parse_discriminator_byte_explicit() {
        let schema = json!({
            "discriminator": {"kind": "byte", "offset": 4, "type": "TypeDef:Uint16"}
        });
        let disc = parse_discriminator(&schema).expect("byte discriminator");
        assert_eq!(
            disc,
            DiscriminatorKind::Byte {
                offset: 4,
                disc_type: TypeDefKind::Uint16,
            }
        );
    }

    #[test]
    fn parse_discriminator_field() {
        let schema = json!({"discriminator": {"kind": "field", "name": "type"}});
        let disc = parse_discriminator(&schema).expect("field discriminator");
        assert_eq!(
            disc,
            DiscriminatorKind::Field {
                name: "type".to_string()
            }
        );
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
