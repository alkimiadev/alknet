//! `TypedefEngine` — the compiled form of a schema.
//!
//! Combines the layout engine (both packed and aligned modes) and the
//! jsonschema validator into a single struct. Built once at schema load
//! time via [`TypedefEngine::compile`]. Used for repeated read/write/
//! validate operations at access time.
//!
//! See [validation.md](../../docs/architecture/crates/typedef/validation.md)
//! §"The TypedefEngine struct" and
//! [overview.md](../../docs/architecture/crates/typedef/overview.md).

use crate::data_access;
use crate::error::TypedefError;
use crate::layout_builder::LayoutBuilder;
use crate::offset_map::OffsetMap;
use crate::schema::{self, Endian};
use crate::sequential_reader::{FieldValue, SequentialReader};
use crate::validation;
use serde_json::Value;
use std::fmt;

/// The layout mode selected at engine construction time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMode {
    /// Packed sequential — for protocol wire formats (SFTP, channels, TTY).
    Packed,
    /// Aligned static — for mmap-friendly formats (metatensor, safetensors).
    Aligned,
}

/// The layout strategy — packed sequential or aligned static.
///
/// Carries the layout-specific handles needed for read/write access in
/// the selected mode. The consumer chooses the mode at construction time
/// via [`TypedefEngine::compile`]; the engine then exposes only the
/// APIs that make sense for that mode.
#[derive(Debug)]
enum Layout {
    /// Packed sequential layout. The write-side is [`LayoutBuilder`]; the
    /// read-side is [`SequentialReader`].
    Packed {
        builder: LayoutBuilder,
        reader: SequentialReader,
    },
    /// Aligned static layout. Field offsets are precomputed in an
    /// [`OffsetMap`] for random access.
    Aligned {
        offset_map: OffsetMap,
    },
}

/// The compiled form of a typedef schema. Combines the layout engine
/// (both packed and aligned modes) and the jsonschema validator.
///
/// Built once at schema load time via [`TypedefEngine::compile`].
/// Used for repeated read/write/validate operations at access time.
///
/// The consumer selects the layout mode at construction time. The engine
/// then exposes mode-appropriate accessors: [`TypedefEngine::offset_map`]
/// for aligned mode, [`TypedefEngine::layout_builder`] and
/// [`TypedefEngine::sequential_reader`] for packed mode. The
/// jsonschema validator is mode-agnostic and always available.
pub struct TypedefEngine {
    layout: Layout,
    validator: jsonschema::Validator,
    endian: Endian,
    schema: Value,
}

impl TypedefEngine {
    /// Compile a schema into a [`TypedefEngine`].
    ///
    /// This is the expensive operation — it parses the schema, normalizes
    /// `$ref` values, computes the layout, and builds the jsonschema
    /// validator. Call once at load time; use the returned engine for
    /// repeated operations.
    ///
    /// The `mode` parameter selects the layout strategy. The same schema
    /// can be compiled in either mode.
    ///
    /// # Errors
    ///
    /// Returns [`TypedefError::Schema`] if the schema is malformed or the
    /// underlying layout/validator construction fails. The error is
    /// propagated from [`LayoutBuilder::new`], [`SequentialReader::new`],
    /// [`OffsetMap::compute`], or [`validation::build_validator`].
    pub fn compile(
        schema: &mut Value,
        mode: LayoutMode,
    ) -> Result<Self, TypedefError> {
        schema::normalize_refs(schema);
        let endian = Endian::from_schema(schema);
        let layout = match mode {
            LayoutMode::Packed => {
                let builder = LayoutBuilder::new(schema)?;
                let reader = SequentialReader::new(schema)?;
                Layout::Packed { builder, reader }
            }
            LayoutMode::Aligned => {
                let offset_map = OffsetMap::compute(schema)?;
                Layout::Aligned { offset_map }
            }
        };
        let validator = validation::build_validator(schema)?;
        Ok(Self {
            layout,
            validator,
            endian,
            schema: schema.clone(),
        })
    }

    /// The schema's endianness.
    pub fn endian(&self) -> Endian {
        self.endian
    }

    /// The layout mode this engine was compiled with.
    pub fn mode(&self) -> LayoutMode {
        match self.layout {
            Layout::Packed { .. } => LayoutMode::Packed,
            Layout::Aligned { .. } => LayoutMode::Aligned,
        }
    }

    /// Access the aligned offset map. Returns `None` if compiled in
    /// packed mode.
    pub fn offset_map(&self) -> Option<&OffsetMap> {
        match &self.layout {
            Layout::Aligned { offset_map } => Some(offset_map),
            Layout::Packed { .. } => None,
        }
    }

    /// Access the layout builder (write-side of packed mode).
    /// Returns `None` if compiled in aligned mode.
    pub fn layout_builder(&self) -> Option<&LayoutBuilder> {
        match &self.layout {
            Layout::Packed { builder, .. } => Some(builder),
            Layout::Aligned { .. } => None,
        }
    }

    /// Access the sequential reader (read-side of packed mode).
    /// Returns `None` if compiled in aligned mode.
    pub fn sequential_reader(&self) -> Option<&SequentialReader> {
        match &self.layout {
            Layout::Packed { reader, .. } => Some(reader),
            Layout::Aligned { .. } => None,
        }
    }

    /// Validate a JSON value against the schema. The jsonschema validator
    /// is already compiled — this is a fast check.
    ///
    /// Returns `Ok(())` if valid, `Err(TypedefError::Validation(...))` if
    /// invalid.
    pub fn validate_json(&self, instance: &Value) -> Result<(), TypedefError> {
        self.validator
            .validate(instance)
            .map_err(|e| TypedefError::Validation(e.to_owned()))
    }

    /// Check if a JSON value is valid against the schema.
    pub fn is_valid_json(&self, instance: &Value) -> bool {
        self.validator.is_valid(instance)
    }

    /// Read a field from a buffer at its computed offset (aligned mode).
    ///
    /// Looks up the field's byte range in the [`OffsetMap`] and reads the
    /// appropriate type using the [`crate::data_access`] functions. Works
    /// for fixed-size primitive kinds and length-prefixed `String`/
    /// `Bytes`/`Timestamp` fields.
    ///
    /// Returns an error if compiled in packed mode — use
    /// [`TypedefEngine::sequential_reader`] for packed mode. Also
    /// returns an error for composite kinds (`Struct`, `Union`, `Array`,
    /// `Record`) — those are better handled via the layout-specific APIs.
    ///
    /// # Errors
    ///
    /// - [`TypedefError::Access`] if compiled in packed mode.
    /// - [`TypedefError::Offset`] if `field_path` is not in the offset map.
    /// - [`TypedefError::Access`] for buffer-too-short or invalid data,
    ///   propagated from [`crate::data_access`].
    pub fn read_field<'a>(
        &self,
        buffer: &'a [u8],
        field_path: &str,
    ) -> Result<FieldValue<'a>, TypedefError> {
        let offset_map = match &self.layout {
            Layout::Aligned { offset_map } => offset_map,
            Layout::Packed { .. } => {
                return Err(TypedefError::Access {
                    field_path: field_path.to_string(),
                    reason: "read_field is only available in aligned mode; \
                             use sequential_reader() for packed mode"
                        .to_string(),
                });
            }
        };
        let range = offset_map.get(field_path).ok_or_else(|| TypedefError::Offset {
            field_path: field_path.to_string(),
            reason: "field not found in offset map".to_string(),
        })?;
        let field_schema =
            lookup_field_schema(&self.schema, field_path).ok_or_else(|| TypedefError::Offset {
                field_path: field_path.to_string(),
                reason: "field schema not found in schema tree".to_string(),
            })?;
        let kind = typedef_kind_loose(field_schema).ok_or_else(|| TypedefError::Offset {
            field_path: field_path.to_string(),
            reason: "field schema has no TypeDef:* kind".to_string(),
        })?;
        let endian = self.endian;
        match kind {
            "TypeDef:Int8" => {
                let v = data_access::read_i8(buffer, range.start, field_path)?;
                Ok(FieldValue::I8(v))
            }
            "TypeDef:Int16" => {
                let v = data_access::read_i16(buffer, range.start, field_path, endian)?;
                Ok(FieldValue::I16(v))
            }
            "TypeDef:Int32" => {
                let v = data_access::read_i32(buffer, range.start, field_path, endian)?;
                Ok(FieldValue::I32(v))
            }
            "TypeDef:Uint8" => {
                let v = data_access::read_u8(buffer, range.start, field_path)?;
                Ok(FieldValue::U8(v))
            }
            "TypeDef:Uint16" => {
                let v = data_access::read_u16(buffer, range.start, field_path, endian)?;
                Ok(FieldValue::U16(v))
            }
            "TypeDef:Uint32" => {
                let v = data_access::read_u32(buffer, range.start, field_path, endian)?;
                Ok(FieldValue::U32(v))
            }
            "TypeDef:Uint64" => {
                let v = data_access::read_u64(buffer, range.start, field_path, endian)?;
                Ok(FieldValue::U64(v))
            }
            "TypeDef:Float32" => {
                let v = data_access::read_f32(buffer, range.start, field_path, endian)?;
                Ok(FieldValue::F32(v))
            }
            "TypeDef:Float64" => {
                let v = data_access::read_f64(buffer, range.start, field_path, endian)?;
                Ok(FieldValue::F64(v))
            }
            "TypeDef:Boolean" => {
                let v = data_access::read_bool(buffer, range.start, field_path)?;
                Ok(FieldValue::Bool(v))
            }
            "TypeDef:Enum" => {
                let v = data_access::read_enum(buffer, range.start, field_path, endian)?;
                Ok(FieldValue::Enum(v))
            }
            "TypeDef:String" => {
                let v = data_access::read_string(buffer, range.start, field_path, endian)?;
                Ok(FieldValue::String(v))
            }
            "TypeDef:Bytes" => {
                let v = data_access::read_bytes(buffer, range.start, field_path, endian)?;
                Ok(FieldValue::Bytes(v))
            }
            "TypeDef:Timestamp" => {
                let v = data_access::read_string(buffer, range.start, field_path, endian)?;
                Ok(FieldValue::String(v))
            }
            "TypeDef:Struct" => Ok(FieldValue::Struct {
                start: range.start,
                end: range.end,
            }),
            "TypeDef:Union" | "TypeDef:Array" | "TypeDef:Record" => {
                Err(TypedefError::Access {
                    field_path: field_path.to_string(),
                    reason: "read_field does not support composite types; \
                             use the layout-specific APIs"
                        .to_string(),
                })
            }
            other => Err(TypedefError::Access {
                field_path: field_path.to_string(),
                reason: format!("unsupported TypeDef kind for read_field: {other}"),
            }),
        }
    }

    /// Write a field to a buffer at its computed offset (aligned mode).
    ///
    /// Looks up the field's byte range in the [`OffsetMap`] and writes the
    /// appropriate type using the [`crate::data_access`] functions. Works
    /// for fixed-size primitive kinds and length-prefixed `String`/
    /// `Bytes`/`Timestamp` fields.
    ///
    /// Returns an error if compiled in packed mode — use
    /// [`TypedefEngine::layout_builder`] for packed mode. Also returns an
    /// error for composite kinds (`Struct`, `Union`, `Array`, `Record`).
    ///
    /// # Errors
    ///
    /// - [`TypedefError::Access`] if compiled in packed mode.
    /// - [`TypedefError::Offset`] if `field_path` is not in the offset map.
    /// - [`TypedefError::Access`] for buffer-too-short or invalid data,
    ///   propagated from [`crate::data_access`].
    pub fn write_field(
        &self,
        buffer: &mut [u8],
        field_path: &str,
        value: &FieldValue<'_>,
    ) -> Result<(), TypedefError> {
        let offset_map = match &self.layout {
            Layout::Aligned { offset_map } => offset_map,
            Layout::Packed { .. } => {
                return Err(TypedefError::Access {
                    field_path: field_path.to_string(),
                    reason: "write_field is only available in aligned mode; \
                             use layout_builder() for packed mode"
                        .to_string(),
                });
            }
        };
        let range = offset_map.get(field_path).ok_or_else(|| TypedefError::Offset {
            field_path: field_path.to_string(),
            reason: "field not found in offset map".to_string(),
        })?;
        let endian = self.endian;
        match value {
            FieldValue::I8(v) => {
                data_access::write_i8(buffer, range.start, *v, field_path)
            }
            FieldValue::I16(v) => {
                data_access::write_i16(buffer, range.start, *v, field_path, endian)
            }
            FieldValue::I32(v) => {
                data_access::write_i32(buffer, range.start, *v, field_path, endian)
            }
            FieldValue::U8(v) => {
                data_access::write_u8(buffer, range.start, *v, field_path)
            }
            FieldValue::U16(v) => {
                data_access::write_u16(buffer, range.start, *v, field_path, endian)
            }
            FieldValue::U32(v) => {
                data_access::write_u32(buffer, range.start, *v, field_path, endian)
            }
            FieldValue::U64(v) => {
                data_access::write_u64(buffer, range.start, *v, field_path, endian)
            }
            FieldValue::F32(v) => {
                data_access::write_f32(buffer, range.start, *v, field_path, endian)
            }
            FieldValue::F64(v) => {
                data_access::write_f64(buffer, range.start, *v, field_path, endian)
            }
            FieldValue::Bool(v) => {
                data_access::write_bool(buffer, range.start, *v, field_path)
            }
            FieldValue::Enum(v) => {
                data_access::write_enum(buffer, range.start, *v, field_path, endian)
            }
            FieldValue::String(v) => {
                data_access::write_string(buffer, range.start, v, field_path, endian)?;
                Ok(())
            }
            FieldValue::Bytes(v) => {
                data_access::write_bytes(buffer, range.start, v, field_path, endian)?;
                Ok(())
            }
            FieldValue::Struct { .. }
            | FieldValue::Union { .. }
            | FieldValue::Array { .. } => Err(TypedefError::Access {
                field_path: field_path.to_string(),
                reason: "write_field does not support composite types; \
                         use the layout-specific APIs"
                    .to_string(),
            }),
        }
    }
}

impl fmt::Debug for TypedefEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TypedefEngine")
            .field("layout", &self.layout)
            .field("validator", &"<jsonschema::Validator>")
            .field("endian", &self.endian)
            .field("schema", &self.schema)
            .finish()
    }
}

/// Walk a schema tree to find the node for a dotted field path.
///
/// Splits `field_path` on `.` and descends into `schema["properties"][segment]`
/// at each step. Returns `None` if any segment is missing or the schema is
/// not an object. Does not resolve `$ref` pointers — the engine stores the
/// normalized schema, and the aligned offset map only records paths for
/// inline fields, so refs at intermediate levels are not expected here.
fn lookup_field_schema<'a>(schema: &'a Value, field_path: &str) -> Option<&'a Value> {
    let mut current = schema;
    for segment in field_path.split('.') {
        current = current
            .as_object()?
            .get("properties")?
            .get(segment)?;
    }
    Some(current)
}

/// Detect a `TypeDef:*` kind from a schema node, accepting either the
/// boolean form (`{ "TypeDef:String": true }`) or the object-annotation
/// form (`{ "TypeDef:String": { "encoding": "..." } }`).
///
/// [`crate::schema::get_typedef_kind`] only recognizes the boolean form;
/// the engine's read/write dispatch needs to recognize the object form
/// too so that variable-length encoding annotations are honored.
fn typedef_kind_loose(node: &Value) -> Option<&str> {
    let obj = node.as_object()?;
    for key in obj.keys() {
        if key.starts_with("TypeDef:") && obj.get(key).is_some_and(|v| !v.is_null()) {
            return Some(key.as_str());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixed_struct_schema() -> Value {
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
    fn compile_aligned_builds_offset_map() {
        let mut schema = fixed_struct_schema();
        let engine = TypedefEngine::compile(&mut schema, LayoutMode::Aligned).expect("compile");
        assert_eq!(engine.mode(), LayoutMode::Aligned);
        assert!(engine.offset_map().is_some());
        assert!(engine.layout_builder().is_none());
        assert!(engine.sequential_reader().is_none());
    }

    #[test]
    fn compile_packed_builds_builder_and_reader() {
        let mut schema = fixed_struct_schema();
        let engine = TypedefEngine::compile(&mut schema, LayoutMode::Packed).expect("compile");
        assert_eq!(engine.mode(), LayoutMode::Packed);
        assert!(engine.layout_builder().is_some());
        assert!(engine.sequential_reader().is_some());
        assert!(engine.offset_map().is_none());
    }

    #[test]
    fn compile_normalizes_refs() {
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
        let engine = TypedefEngine::compile(&mut schema, LayoutMode::Packed).expect("compile");
        assert_eq!(
            engine.schema["properties"]["child"]["$ref"],
            json!("#/$defs/Child")
        );
    }

    #[test]
    fn endian_parsed_from_schema() {
        let mut schema = json!({
            "TypeDef:Struct": true,
            "endian": "big",
            "properties": { "id": { "TypeDef:Uint32": true } }
        });
        let engine = TypedefEngine::compile(&mut schema, LayoutMode::Packed).expect("compile");
        assert_eq!(engine.endian(), Endian::Big);
    }

    #[test]
    fn endian_defaults_to_little() {
        let mut schema = json!({
            "TypeDef:Struct": true,
            "properties": { "id": { "TypeDef:Uint32": true } }
        });
        let engine = TypedefEngine::compile(&mut schema, LayoutMode::Packed).expect("compile");
        assert_eq!(engine.endian(), Endian::Little);
    }

    #[test]
    fn validate_json_accepts_valid_instance() {
        let mut schema = json!({
            "TypeDef:Struct": true,
            "type": "object",
            "properties": {
                "id": { "TypeDef:Uint32": true, "type": "integer" }
            },
            "required": ["id"]
        });
        let engine = TypedefEngine::compile(&mut schema, LayoutMode::Aligned).expect("compile");
        assert!(engine.validate_json(&json!({"id": 42})).is_ok());
    }

    #[test]
    fn validate_json_rejects_invalid_instance() {
        let mut schema = json!({
            "TypeDef:Struct": true,
            "type": "object",
            "properties": {
                "id": { "TypeDef:Uint32": true, "type": "integer" }
            },
            "required": ["id"]
        });
        let engine = TypedefEngine::compile(&mut schema, LayoutMode::Aligned).expect("compile");
        let err = engine.validate_json(&json!({"id": -1})).unwrap_err();
        assert!(matches!(err, TypedefError::Validation(_)), "got {err:?}");
    }

    #[test]
    fn is_valid_json_returns_bool() {
        let mut schema = json!({
            "TypeDef:Struct": true,
            "type": "object",
            "properties": {
                "id": { "TypeDef:Uint32": true, "type": "integer" }
            },
            "required": ["id"]
        });
        let engine = TypedefEngine::compile(&mut schema, LayoutMode::Aligned).expect("compile");
        assert!(engine.is_valid_json(&json!({"id": 42})));
        assert!(!engine.is_valid_json(&json!({"id": -1})));
    }

    #[test]
    fn read_field_aligned_reads_fixed_fields() {
        let mut schema = json!({
            "TypeDef:Struct": true,
            "endian": "little",
            "properties": {
                "flag": { "TypeDef:Uint8": true },
                "id": { "TypeDef:Uint32": true }
            }
        });
        let engine = TypedefEngine::compile(&mut schema, LayoutMode::Aligned).expect("compile");
        let mut buf = vec![0u8; 8];
        buf[0] = 0xAB;
        buf[4..8].copy_from_slice(&0x01020304u32.to_le_bytes());
        assert_eq!(
            engine.read_field(&buf, "flag").unwrap(),
            FieldValue::U8(0xAB)
        );
        assert_eq!(
            engine.read_field(&buf, "id").unwrap(),
            FieldValue::U32(0x01020304)
        );
    }

    #[test]
    fn read_field_aligned_reads_string_length_prefixed() {
        let mut schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "name": { "TypeDef:String": true }
            }
        });
        let engine = TypedefEngine::compile(&mut schema, LayoutMode::Aligned).expect("compile");
        let mut buf = vec![0u8; 32];
        let len_bytes = 5u32.to_le_bytes();
        buf[0..4].copy_from_slice(&len_bytes);
        buf[4..9].copy_from_slice(b"hello");
        assert_eq!(
            engine.read_field(&buf, "name").unwrap(),
            FieldValue::String("hello")
        );
    }

    #[test]
    fn read_field_returns_access_error_in_packed_mode() {
        let mut schema = json!({
            "TypeDef:Struct": true,
            "properties": { "id": { "TypeDef:Uint32": true } }
        });
        let engine = TypedefEngine::compile(&mut schema, LayoutMode::Packed).expect("compile");
        let buf = [0u8; 4];
        let err = engine.read_field(&buf, "id").unwrap_err();
        assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
    }

    #[test]
    fn read_field_returns_offset_error_for_missing_field() {
        let mut schema = json!({
            "TypeDef:Struct": true,
            "properties": { "id": { "TypeDef:Uint32": true } }
        });
        let engine = TypedefEngine::compile(&mut schema, LayoutMode::Aligned).expect("compile");
        let buf = [0u8; 4];
        let err = engine.read_field(&buf, "missing").unwrap_err();
        assert!(matches!(err, TypedefError::Offset { .. }), "got {err:?}");
    }

    #[test]
    fn read_field_returns_error_for_composite_types() {
        let mut schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "vals": {
                    "TypeDef:Array": true,
                    "items": { "TypeDef:Uint32": true }
                }
            }
        });
        let engine = TypedefEngine::compile(&mut schema, LayoutMode::Aligned).expect("compile");
        let buf = [0u8; 8];
        let err = engine.read_field(&buf, "vals").unwrap_err();
        assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
    }

    #[test]
    fn write_field_aligned_writes_fixed_fields() {
        let mut schema = json!({
            "TypeDef:Struct": true,
            "endian": "little",
            "properties": {
                "flag": { "TypeDef:Uint8": true },
                "id": { "TypeDef:Uint32": true }
            }
        });
        let engine = TypedefEngine::compile(&mut schema, LayoutMode::Aligned).expect("compile");
        let mut buf = vec![0u8; 8];
        engine
            .write_field(&mut buf, "flag", &FieldValue::U8(0xAB))
            .unwrap();
        engine
            .write_field(&mut buf, "id", &FieldValue::U32(0x01020304))
            .unwrap();
        assert_eq!(buf[0], 0xAB);
        assert_eq!(&buf[4..8], &0x01020304u32.to_le_bytes());
        assert_eq!(
            engine.read_field(&buf, "flag").unwrap(),
            FieldValue::U8(0xAB)
        );
        assert_eq!(
            engine.read_field(&buf, "id").unwrap(),
            FieldValue::U32(0x01020304)
        );
    }

    #[test]
    fn write_field_round_trips_string() {
        let mut schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "name": { "TypeDef:String": true }
            }
        });
        let engine = TypedefEngine::compile(&mut schema, LayoutMode::Aligned).expect("compile");
        let mut buf = vec![0u8; 32];
        engine
            .write_field(&mut buf, "name", &FieldValue::String("hello"))
            .unwrap();
        assert_eq!(
            engine.read_field(&buf, "name").unwrap(),
            FieldValue::String("hello")
        );
    }

    #[test]
    fn write_field_returns_access_error_in_packed_mode() {
        let mut schema = json!({
            "TypeDef:Struct": true,
            "properties": { "id": { "TypeDef:Uint32": true } }
        });
        let engine = TypedefEngine::compile(&mut schema, LayoutMode::Packed).expect("compile");
        let mut buf = [0u8; 4];
        let err = engine
            .write_field(&mut buf, "id", &FieldValue::U32(1))
            .unwrap_err();
        assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
    }

    #[test]
    fn write_field_returns_offset_error_for_missing_field() {
        let mut schema = json!({
            "TypeDef:Struct": true,
            "properties": { "id": { "TypeDef:Uint32": true } }
        });
        let engine = TypedefEngine::compile(&mut schema, LayoutMode::Aligned).expect("compile");
        let mut buf = [0u8; 4];
        let err = engine
            .write_field(&mut buf, "missing", &FieldValue::U32(1))
            .unwrap_err();
        assert!(matches!(err, TypedefError::Offset { .. }), "got {err:?}");
    }

    #[test]
    fn write_field_returns_error_for_composite_value() {
        let mut schema = json!({
            "TypeDef:Struct": true,
            "properties": { "id": { "TypeDef:Uint32": true } }
        });
        let engine = TypedefEngine::compile(&mut schema, LayoutMode::Aligned).expect("compile");
        let mut buf = [0u8; 8];
        let err = engine
            .write_field(
                &mut buf,
                "id",
                &FieldValue::Struct { start: 0, end: 4 },
            )
            .unwrap_err();
        assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
    }

    #[test]
    fn compile_returns_schema_error_for_invalid_top_level() {
        let mut schema = json!({ "type": "object", "properties": {} });
        let err = TypedefEngine::compile(&mut schema, LayoutMode::Packed).unwrap_err();
        assert!(matches!(err, TypedefError::Schema(_)), "got {err:?}");
    }

    #[test]
    fn debug_formats_without_panicking() {
        let mut schema = fixed_struct_schema();
        let engine = TypedefEngine::compile(&mut schema, LayoutMode::Aligned).expect("compile");
        let s = format!("{engine:?}");
        assert!(s.contains("TypedefEngine"));
        assert!(s.contains("Aligned"));
    }

    #[test]
    fn lookup_field_schema_walks_dotted_path() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "header": {
                    "TypeDef:Struct": true,
                    "properties": {
                        "version": { "TypeDef:Uint8": true }
                    }
                }
            }
        });
        let node = lookup_field_schema(&schema, "header.version").expect("found");
        assert_eq!(node, &json!({ "TypeDef:Uint8": true }));
        assert!(lookup_field_schema(&schema, "header.missing").is_none());
        assert!(lookup_field_schema(&schema, "missing").is_none());
    }

    #[test]
    fn typedef_kind_loose_recognizes_object_form() {
        let node = json!({ "TypeDef:String": { "encoding": "offset-indirect" } });
        assert_eq!(typedef_kind_loose(&node), Some("TypeDef:String"));
    }
}