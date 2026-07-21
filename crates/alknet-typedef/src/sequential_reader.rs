//! Packed sequential `SequentialReader` — Mode 1 read-side (ADR-096).
//!
//! Walks a buffer field-by-field according to the schema, reading length
//! prefixes to determine variable-length data positions. Used at read time
//! when the consumer is parsing an incoming frame.
//!
//! The reader is sequential — it cannot jump to field N without reading
//! fields 0..N-1 first. This is inherent to packed layouts where
//! variable-length fields shift subsequent fields. [`SequentialReader`]
//! uses the [`crate::data_access`] read functions for all typed reads
//! and applies the schema's endianness to every multi-byte value.

use crate::data_access;
use crate::error::TypedefError;
use crate::schema::{self, DiscriminatorKind, Endian};
use serde_json::Value;

const U32_SIZE: usize = 4;

/// A value read from a field during sequential traversal.
///
/// Composite kinds ([`FieldValue::Struct`], [`FieldValue::Union`],
/// [`FieldValue::Array`]) return layout descriptors; the consumer
/// recurses with a fresh [`SequentialReader`] scoped to the
/// reported byte range.
#[derive(Debug, PartialEq)]
pub enum FieldValue<'a> {
    /// `TypeDef:Int8`.
    I8(i8),
    /// `TypeDef:Int16`.
    I16(i16),
    /// `TypeDef:Int32`.
    I32(i32),
    /// `TypeDef:Uint8`.
    U8(u8),
    /// `TypeDef:Uint16`.
    U16(u16),
    /// `TypeDef:Uint32`.
    U32(u32),
    /// `TypeDef:Uint64` — included for completeness (the schema layer's
    /// `type_size` returns `None` for `TypeDef:Uint64`, but `data_access`
    /// provides `read_u64` and the reader exposes it when encountered).
    U64(u64),
    /// `TypeDef:Float32`.
    F32(f32),
    /// `TypeDef:Float64`.
    F64(f64),
    /// `TypeDef:Boolean`.
    Bool(bool),
    /// `TypeDef:Enum` — `u32` index into the schema's `"enum"` array.
    Enum(u32),
    /// `TypeDef:String` — borrows from the input buffer.
    String(&'a str),
    /// `TypeDef:Bytes` — borrows from the input buffer.
    Bytes(&'a [u8]),
    /// `TypeDef:Struct` — the consumer recurses with a new
    /// [`SequentialReader`] scoped to `start..end`.
    Struct {
        /// Inclusive start of the nested struct's byte range.
        start: usize,
        /// Exclusive end of the nested struct's byte range.
        end: usize,
    },
    /// `TypeDef:Union` — the consumer looks up the variant schema using
    /// `discriminator` and recurses at `variant_start`.
    Union {
        /// Stringified discriminator value (mapping key).
        discriminator: String,
        /// Byte offset where the variant struct begins.
        variant_start: usize,
    },
    /// `TypeDef:Array` — the consumer iterates `count` elements of
    /// stride `element_stride` starting at `element_start`.
    Array {
        /// Number of elements in the array.
        count: u32,
        /// Byte offset of the first element.
        element_start: usize,
        /// Byte distance between consecutive elements. `0` signals a
        /// variable-length element type — the consumer must walk each
        /// element sequentially.
        element_stride: usize,
    },
}

/// Walks a buffer field-by-field according to a schema, reading length
/// prefixes to determine variable-length data positions. Used at read
/// time when parsing incoming protocol frames.
///
/// The reader is sequential — it cannot jump to field N without reading
/// fields 0..N-1 first. This is inherent to packed layouts where
/// variable-length fields shift subsequent fields.
///
/// Construct with [`SequentialReader::new`], then drive with
/// [`SequentialReader::read_next`] until it returns `Ok(None)`. Use
/// [`SequentialReader::reset`] to walk the same buffer again, or
/// [`SequentialReader::read_field`] to seek a single field by name
/// (which walks all preceding fields to reach the target).
#[derive(Debug)]
pub struct SequentialReader {
    schema: Value,
    endian: Endian,
    fields: Vec<(String, Value)>,
    field_index: usize,
    position: usize,
}

impl SequentialReader {
    /// Create a new `SequentialReader` from a top-level struct schema.
    ///
    /// The schema must declare `TypeDef:Struct` and have a `properties`
    /// object. Endianness is parsed via [`Endian::from_schema`].
    ///
    /// # Errors
    ///
    /// Returns [`TypedefError::Schema`] if the schema is not an object,
    /// does not declare `TypeDef:Struct`, or has no `properties` object.
    pub fn new(schema: &Value) -> Result<Self, TypedefError> {
        let kind = schema::get_typedef_kind(schema)
            .ok_or_else(|| TypedefError::Schema("schema has no TypeDef:* kind".to_string()))?;
        if kind != "TypeDef:Struct" {
            return Err(TypedefError::Schema(format!(
                "SequentialReader only supports TypeDef:Struct at the top level, got {kind}"
            )));
        }
        let properties = schema
            .as_object()
            .and_then(|obj| obj.get("properties"))
            .and_then(Value::as_object)
            .ok_or_else(|| {
                TypedefError::Schema("struct schema has no properties object".to_string())
            })?;
        let fields: Vec<(String, Value)> = properties
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        let endian = Endian::from_schema(schema);
        Ok(Self {
            schema: schema.clone(),
            endian,
            fields,
            field_index: 0,
            position: 0,
        })
    }

    /// Read the next field from `buffer` at the current position.
    ///
    /// Returns `Ok(Some((field_name, value)))` and advances the internal
    /// position, or `Ok(None)` when all fields have been read. Variable-
    /// length fields (`TypeDef:String`/`TypeDef:Bytes`) consume their
    /// 4-byte length prefix plus the data; composite fields advance past
    /// their computed byte range.
    ///
    /// # Errors
    ///
    /// Propagates [`TypedefError::Access`] from the underlying
    /// [`crate::data_access`] reads when `buffer` is too short or
    /// contains invalid data.
    pub fn read_next<'a>(
        &mut self,
        buffer: &'a [u8],
    ) -> Result<Option<(String, FieldValue<'a>)>, TypedefError> {
        if self.field_index >= self.fields.len() {
            return Ok(None);
        }
        let (name, value) = match self.read_field_at(buffer, self.field_index, self.position) {
            Ok((value, new_position)) => {
                self.position = new_position;
                self.field_index += 1;
                (self.fields[self.field_index - 1].0.clone(), value)
            }
            Err(e) => return Err(e),
        };
        Ok(Some((name, value)))
    }

    /// Read a specific field by name. This walks through all preceding
    /// fields to reach the target (sequential access is inherent to
    /// packed layouts). Resets the reader first; the cursor is left at
    /// the position just past the target field.
    ///
    /// # Errors
    ///
    /// Returns [`TypedefError::Schema`] if `field_path` does not match
    /// any top-level field. Propagates [`TypedefError::Access`] for
    /// buffer-too-short or invalid data.
    pub fn read_field<'a>(
        &mut self,
        buffer: &'a [u8],
        field_path: &str,
    ) -> Result<FieldValue<'a>, TypedefError> {
        self.reset();
        let target_index = self
            .fields
            .iter()
            .position(|(name, _)| name == field_path)
            .ok_or_else(|| {
                TypedefError::Schema(format!("field not found in struct: {field_path}"))
            })?;
        let mut position = 0usize;
        let mut target_value: Option<FieldValue<'a>> = None;
        for index in 0..=target_index {
            let (value, new_position) = self.read_field_at(buffer, index, position)?;
            position = new_position;
            if index == target_index {
                target_value = Some(value);
            }
        }
        self.position = position;
        self.field_index = target_index + 1;
        target_value.ok_or_else(|| {
            TypedefError::Schema(format!(
                "internal: target field {field_path} not produced by loop"
            ))
        })
    }

    /// Reset the reader to the beginning of the buffer (position 0,
    /// first field).
    pub fn reset(&mut self) {
        self.field_index = 0;
        self.position = 0;
    }

    /// The current byte position in the buffer.
    pub fn position(&self) -> usize {
        self.position
    }

    /// The endianness used by this reader.
    pub fn endian(&self) -> Endian {
        self.endian
    }

    /// The schema this reader was constructed from.
    pub fn schema(&self) -> &Value {
        &self.schema
    }

    fn read_field_at<'a>(
        &self,
        buffer: &'a [u8],
        index: usize,
        offset: usize,
    ) -> Result<(FieldValue<'a>, usize), TypedefError> {
        let (_, field_schema) = self
            .fields
            .get(index)
            .ok_or_else(|| TypedefError::Schema(format!("field index {index} out of range")))?;
        let field_path = self.fields[index].0.as_str();
        read_field_value(
            buffer,
            &self.schema,
            field_schema,
            field_path,
            offset,
            self.endian,
        )
    }
}

/// Read a single field value at `offset` and return the value plus the
/// position just past the field. Field paths are used for error
/// attribution only — this helper does not recurse into nested structs.
///
/// `root_schema` is the top-level schema used to resolve `$ref` pointers
/// found in nested union variants.
fn read_field_value<'a>(
    buffer: &'a [u8],
    root_schema: &Value,
    field_schema: &Value,
    field_path: &str,
    offset: usize,
    endian: Endian,
) -> Result<(FieldValue<'a>, usize), TypedefError> {
    let kind = schema::get_typedef_kind(field_schema).ok_or_else(|| {
        TypedefError::Schema(format!(
            "field {field_path} has no TypeDef:* kind: {field_schema}"
        ))
    })?;

    match kind {
        "TypeDef:Int8" => {
            let v = data_access::read_i8(buffer, offset, field_path)?;
            Ok((FieldValue::I8(v), offset + 1))
        }
        "TypeDef:Int16" => {
            let v = data_access::read_i16(buffer, offset, field_path, endian)?;
            Ok((FieldValue::I16(v), offset + 2))
        }
        "TypeDef:Int32" => {
            let v = data_access::read_i32(buffer, offset, field_path, endian)?;
            Ok((FieldValue::I32(v), offset + 4))
        }
        "TypeDef:Uint8" => {
            let v = data_access::read_u8(buffer, offset, field_path)?;
            Ok((FieldValue::U8(v), offset + 1))
        }
        "TypeDef:Uint16" => {
            let v = data_access::read_u16(buffer, offset, field_path, endian)?;
            Ok((FieldValue::U16(v), offset + 2))
        }
        "TypeDef:Uint32" => {
            let v = data_access::read_u32(buffer, offset, field_path, endian)?;
            Ok((FieldValue::U32(v), offset + 4))
        }
        "TypeDef:Uint64" => {
            let v = data_access::read_u64(buffer, offset, field_path, endian)?;
            Ok((FieldValue::U64(v), offset + 8))
        }
        "TypeDef:Float32" => {
            let v = data_access::read_f32(buffer, offset, field_path, endian)?;
            Ok((FieldValue::F32(v), offset + 4))
        }
        "TypeDef:Float64" => {
            let v = data_access::read_f64(buffer, offset, field_path, endian)?;
            Ok((FieldValue::F64(v), offset + 8))
        }
        "TypeDef:Boolean" => {
            let v = data_access::read_bool(buffer, offset, field_path)?;
            Ok((FieldValue::Bool(v), offset + 1))
        }
        "TypeDef:Enum" => {
            let v = data_access::read_enum(buffer, offset, field_path, endian)?;
            Ok((FieldValue::Enum(v), offset + 4))
        }
        "TypeDef:String" => {
            let s = data_access::read_string(buffer, offset, field_path, endian)?;
            let total = U32_SIZE + s.len();
            Ok((FieldValue::String(s), offset + total))
        }
        "TypeDef:Bytes" => {
            let b = data_access::read_bytes(buffer, offset, field_path, endian)?;
            let total = U32_SIZE + b.len();
            Ok((FieldValue::Bytes(b), offset + total))
        }
        "TypeDef:Timestamp" => {
            let s = data_access::read_string(buffer, offset, field_path, endian)?;
            let total = U32_SIZE + s.len();
            Ok((FieldValue::String(s), offset + total))
        }
        "TypeDef:Struct" => {
            let size = walk_struct_size(root_schema, field_schema, buffer, offset, endian)?;
            let end = offset
                .checked_add(size)
                .ok_or_else(|| TypedefError::Access {
                    field_path: field_path.to_string(),
                    reason: format!("struct end {offset} + {size} overflows usize"),
                })?;
            Ok((FieldValue::Struct { start: offset, end }, end))
        }
        "TypeDef:Union" => read_union_value(
            buffer,
            root_schema,
            field_schema,
            field_path,
            offset,
            endian,
        ),
        "TypeDef:Array" => read_array_value(
            buffer,
            root_schema,
            field_schema,
            field_path,
            offset,
            endian,
        ),
        "TypeDef:Record" => read_record_value(
            buffer,
            root_schema,
            field_schema,
            field_path,
            offset,
            endian,
        ),
        other => Err(TypedefError::Schema(format!(
            "unsupported TypeDef kind for sequential read: {other}"
        ))),
    }
}

/// Read a `TypeDef:Union` field: read the discriminator, return the
/// variant start offset, and advance past the entire union payload.
///
/// For byte-offset discriminators the union occupies
/// `discriminator_size + variant_size` bytes. Because the variant is a
/// struct (or a ref to one) whose size depends on variable-length fields,
/// the variant size is computed by walking the variant struct. The
/// variant schema is resolved from the `"mapping"` table using the
/// stringified discriminator value.
///
/// For field-name discriminators the discriminator is itself a
/// length-prefixed string field. The variant begins immediately after
/// the discriminator field and is sized by walking the variant struct.
fn read_union_value<'a>(
    buffer: &'a [u8],
    root_schema: &Value,
    schema: &Value,
    field_path: &str,
    offset: usize,
    endian: Endian,
) -> Result<(FieldValue<'a>, usize), TypedefError> {
    let disc = schema::parse_discriminator(schema)?;
    let mapping = schema
        .as_object()
        .and_then(|obj| obj.get("mapping"))
        .and_then(Value::as_object)
        .ok_or_else(|| TypedefError::Schema(format!("union {field_path} has no mapping object")))?;

    match disc {
        DiscriminatorKind::Byte {
            offset: disc_offset,
            disc_type,
        } => {
            let abs_offset =
                offset
                    .checked_add(disc_offset)
                    .ok_or_else(|| TypedefError::Access {
                        field_path: field_path.to_string(),
                        reason: format!(
                            "discriminator offset {offset} + {disc_offset} overflows usize"
                        ),
                    })?;
            let (disc_value, disc_size) =
                read_byte_discriminator(buffer, abs_offset, field_path, &disc_type, endian)?;
            let key = disc_value.to_string();
            let variant_schema = mapping.get(&key).ok_or_else(|| TypedefError::Access {
                field_path: field_path.to_string(),
                reason: format!("unknown union discriminator value: {key}"),
            })?;
            let variant_start =
                abs_offset
                    .checked_add(disc_size)
                    .ok_or_else(|| TypedefError::Access {
                        field_path: field_path.to_string(),
                        reason: format!("variant start {abs_offset} + {disc_size} overflows usize"),
                    })?;
            let variant_size = resolve_and_walk_variant(
                root_schema,
                schema,
                variant_schema,
                buffer,
                variant_start,
                endian,
                field_path,
            )?;
            let end =
                variant_start
                    .checked_add(variant_size)
                    .ok_or_else(|| TypedefError::Access {
                        field_path: field_path.to_string(),
                        reason: format!(
                            "union end {variant_start} + {variant_size} overflows usize"
                        ),
                    })?;
            Ok((
                FieldValue::Union {
                    discriminator: key,
                    variant_start,
                },
                end,
            ))
        }
        DiscriminatorKind::Field { name } => {
            let properties = schema
                .as_object()
                .and_then(|obj| obj.get("properties"))
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    TypedefError::Schema(format!(
                        "field-name union {field_path} has no properties object"
                    ))
                })?;
            let disc_schema = properties.get(&name).ok_or_else(|| {
                TypedefError::Schema(format!(
                    "union {field_path} has no discriminator field '{name}'"
                ))
            })?;
            let (disc_value, after_disc) =
                read_field_value(buffer, root_schema, disc_schema, field_path, offset, endian)?;
            let key = discriminator_string_value(&disc_value, field_path)?;
            let variant_schema = mapping.get(&key).ok_or_else(|| TypedefError::Access {
                field_path: field_path.to_string(),
                reason: format!("unknown union discriminator value: {key}"),
            })?;
            let variant_size = resolve_and_walk_variant(
                root_schema,
                schema,
                variant_schema,
                buffer,
                after_disc,
                endian,
                field_path,
            )?;
            let end = after_disc
                .checked_add(variant_size)
                .ok_or_else(|| TypedefError::Access {
                    field_path: field_path.to_string(),
                    reason: format!("union end {after_disc} + {variant_size} overflows usize"),
                })?;
            Ok((
                FieldValue::Union {
                    discriminator: key,
                    variant_start: after_disc,
                },
                end,
            ))
        }
    }
}

/// Read a byte-offset discriminator integer and return its value (as a
/// `u32`) plus its byte size.
fn read_byte_discriminator(
    buffer: &[u8],
    offset: usize,
    field_path: &str,
    disc_type: &str,
    endian: Endian,
) -> Result<(u32, usize), TypedefError> {
    match disc_type {
        "TypeDef:Uint8" => {
            let v = data_access::read_u8(buffer, offset, field_path)?;
            Ok((v as u32, 1))
        }
        "TypeDef:Uint16" => {
            let v = data_access::read_u16(buffer, offset, field_path, endian)?;
            Ok((v as u32, 2))
        }
        "TypeDef:Uint32" => {
            let v = data_access::read_u32(buffer, offset, field_path, endian)?;
            Ok((v, 4))
        }
        other => Err(TypedefError::Schema(format!(
            "unsupported byte discriminator type: {other}"
        ))),
    }
}

/// Stringify a field-name discriminator value. Only the common kinds
/// (String, Uint8/16/32, Enum) are supported — anything else is a schema
/// error.
fn discriminator_string_value(
    value: &FieldValue<'_>,
    field_path: &str,
) -> Result<String, TypedefError> {
    match value {
        FieldValue::String(s) => Ok(s.to_string()),
        FieldValue::U8(v) => Ok(v.to_string()),
        FieldValue::U16(v) => Ok(v.to_string()),
        FieldValue::U32(v) => Ok(v.to_string()),
        FieldValue::Enum(v) => Ok(v.to_string()),
        other => Err(TypedefError::Schema(format!(
            "union {field_path} has unsupported field discriminator kind: {other:?}"
        ))),
    }
}

/// Read a `TypeDef:Array` field: read the count (fixed via `minItems`/
/// `maxItems` equality, or a 4-byte count prefix) and compute the
/// element stride. Fixed-size elements produce a non-zero stride so the
/// consumer can index directly; variable-length elements produce a
/// stride of `0` so the consumer must walk each element sequentially.
fn read_array_value<'a>(
    buffer: &'a [u8],
    root_schema: &Value,
    schema: &Value,
    field_path: &str,
    offset: usize,
    endian: Endian,
) -> Result<(FieldValue<'a>, usize), TypedefError> {
    let obj = schema.as_object().ok_or_else(|| {
        TypedefError::Schema(format!("array {field_path} schema is not an object"))
    })?;
    let items_schema = obj
        .get("items")
        .ok_or_else(|| TypedefError::Schema(format!("array {field_path} has no items schema")))?;

    let min = obj
        .get("minItems")
        .and_then(Value::as_u64)
        .map(|n| n as u32);
    let max = obj
        .get("maxItems")
        .and_then(Value::as_u64)
        .map(|n| n as u32);
    let fixed_count = matches!((min, max), (Some(a), Some(b)) if a == b);
    let (count, element_start) = if fixed_count {
        let count = min.ok_or_else(|| {
            TypedefError::Schema(format!(
                "array {field_path} declared fixed count but minItems is absent"
            ))
        })?;
        (count, offset)
    } else {
        let count = data_access::read_u32(buffer, offset, field_path, endian)?;
        (count, offset + U32_SIZE)
    };

    let element_kind = schema::get_typedef_kind(items_schema).ok_or_else(|| {
        TypedefError::Schema(format!(
            "array {field_path} items schema has no TypeDef:* kind"
        ))
    })?;

    let element_stride = if schema::is_fixed_size(element_kind) {
        schema::type_size(element_kind).unwrap_or(0)
    } else {
        0
    };

    let total = if element_stride == 0 {
        walk_variable_array_size(
            root_schema,
            items_schema,
            buffer,
            element_start,
            count,
            endian,
            field_path,
        )?
    } else {
        (count as usize)
            .checked_mul(element_stride)
            .ok_or_else(|| TypedefError::Access {
                field_path: field_path.to_string(),
                reason: format!("array size {count} × stride {element_stride} overflows usize"),
            })?
    };

    let end = element_start
        .checked_add(total)
        .ok_or_else(|| TypedefError::Access {
            field_path: field_path.to_string(),
            reason: format!("array end {element_start} + {total} overflows usize"),
        })?;

    Ok((
        FieldValue::Array {
            count,
            element_start,
            element_stride,
        },
        end,
    ))
}

/// Walk `count` variable-length array elements starting at `offset` and
/// return the total byte size of the element data (excluding any count
/// prefix, which the caller has already accounted for).
fn walk_variable_array_size(
    root_schema: &Value,
    items_schema: &Value,
    buffer: &[u8],
    start: usize,
    count: u32,
    endian: Endian,
    field_path: &str,
) -> Result<usize, TypedefError> {
    let mut position = start;
    for i in 0..count {
        let element_path = format!("{field_path}[{i}]");
        let (_, new_position) = read_field_value(
            buffer,
            root_schema,
            items_schema,
            &element_path,
            position,
            endian,
        )?;
        if new_position < position {
            return Err(TypedefError::Access {
                field_path: element_path,
                reason: format!("array element walked backwards: {position} → {new_position}"),
            });
        }
        position = new_position;
    }
    Ok(position - start)
}

/// Read a `TypeDef:Record` field: `[count: u32]` followed by `count`
/// entries of `[key_len: u32][key_bytes][value]`. Returns the total size
/// consumed. The reader does not decode the entries — the consumer
/// recurses into the record's value schema.
fn read_record_value<'a>(
    buffer: &'a [u8],
    root_schema: &Value,
    schema: &Value,
    field_path: &str,
    offset: usize,
    endian: Endian,
) -> Result<(FieldValue<'a>, usize), TypedefError> {
    let count = data_access::read_u32(buffer, offset, field_path, endian)?;
    let value_schema = schema
        .as_object()
        .and_then(|obj| obj.get("values"))
        .ok_or_else(|| TypedefError::Schema(format!("record {field_path} has no values schema")))?;
    let mut position = offset + U32_SIZE;
    for i in 0..count {
        let entry_path = format!("{field_path}[{i}].key");
        let key = data_access::read_string(buffer, position, &entry_path, endian)?;
        position += U32_SIZE + key.len();
        let value_path = format!("{field_path}[{i}].value");
        let (_, new_position) = read_field_value(
            buffer,
            root_schema,
            value_schema,
            &value_path,
            position,
            endian,
        )?;
        position = new_position;
    }
    Ok((FieldValue::Bytes(&buffer[offset..position]), position))
}

/// Resolve a variant schema and walk its size starting at
/// `variant_start`. Used by [`read_union_value`]. Inline schemas are
/// returned as-is; `$ref` pointers are resolved against the union
/// schema's own `$defs` block, then the root schema's `$defs` block.
fn resolve_and_walk_variant(
    root_schema: &Value,
    union_schema: &Value,
    variant_schema: &Value,
    buffer: &[u8],
    variant_start: usize,
    endian: Endian,
    field_path: &str,
) -> Result<usize, TypedefError> {
    let resolved =
        resolve_variant_schema(root_schema, union_schema, variant_schema).ok_or_else(|| {
            TypedefError::Schema(format!(
                "union {field_path} variant could not be resolved: {variant_schema}"
            ))
        })?;
    let kind = schema::get_typedef_kind(resolved).ok_or_else(|| {
        TypedefError::Schema(format!(
            "union {field_path} variant has no TypeDef:* kind: {resolved}"
        ))
    })?;
    match kind {
        "TypeDef:Struct" => walk_struct_size(root_schema, resolved, buffer, variant_start, endian),
        "TypeDef:Union" => {
            let (_, end) = read_union_value(
                buffer,
                root_schema,
                resolved,
                field_path,
                variant_start,
                endian,
            )?;
            Ok(end - variant_start)
        }
        other => Err(TypedefError::Schema(format!(
            "union {field_path} variant must be Struct or Union, got {other}"
        ))),
    }
}

/// Resolve a variant schema. Inline schemas (objects with a `TypeDef:*`
/// kind) are returned directly. `$ref` pointers of the form
/// `#/$defs/<name>` are resolved against `union_schema["$defs"]` first,
/// then `root_schema["$defs"]`. Returns `None` if the ref cannot be
/// resolved or the target is absent.
fn resolve_variant_schema<'a>(
    root_schema: &'a Value,
    union_schema: &'a Value,
    variant: &'a Value,
) -> Option<&'a Value> {
    let obj = variant.as_object()?;
    if let Some(ref_value) = obj.get("$ref").and_then(Value::as_str) {
        if !ref_value.starts_with("#/$defs/") {
            return None;
        }
        let name = &ref_value["#/$defs/".len()..];
        for host in [union_schema, root_schema] {
            if let Some(target) = host
                .as_object()
                .and_then(|o| o.get("$defs"))
                .and_then(Value::as_object)
                .and_then(|d| d.get(name))
            {
                return Some(target);
            }
        }
        return None;
    }
    if schema::get_typedef_kind(variant).is_some() {
        Some(variant)
    } else {
        None
    }
}

/// Walk the fields of a struct schema sequentially, reading length
/// prefixes for variable-length fields, and return the total byte size
/// of the struct starting at `offset`. Does not return field values —
/// only advances the cursor to compute the struct's end position.
///
/// `root_schema` is the top-level schema used to resolve `$ref` pointers
/// found in nested union variants.
fn walk_struct_size(
    root_schema: &Value,
    schema: &Value,
    buffer: &[u8],
    offset: usize,
    endian: Endian,
) -> Result<usize, TypedefError> {
    let properties = schema
        .as_object()
        .and_then(|obj| obj.get("properties"))
        .and_then(Value::as_object)
        .ok_or_else(|| {
            TypedefError::Schema("struct schema has no properties object".to_string())
        })?;
    let mut position = offset;
    for (name, field_schema) in properties.iter() {
        let (_, new_position) = read_field_value(
            buffer,
            root_schema,
            field_schema,
            name.as_str(),
            position,
            endian,
        )?;
        if new_position < position {
            return Err(TypedefError::Access {
                field_path: name.clone(),
                reason: format!("struct field walked backwards: {position} → {new_position}"),
            });
        }
        position = new_position;
    }
    Ok(position - offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const LE: Endian = Endian::Little;
    const BE: Endian = Endian::Big;

    fn write_u32(buf: &mut [u8], offset: usize, value: u32, endian: Endian) {
        let bytes = match endian {
            Endian::Little => value.to_le_bytes(),
            Endian::Big => value.to_be_bytes(),
        };
        buf[offset..offset + 4].copy_from_slice(&bytes);
    }

    fn write_string(buf: &mut [u8], offset: usize, value: &str, endian: Endian) -> usize {
        let bytes = value.as_bytes();
        let total = 4 + bytes.len();
        write_u32(buf, offset, bytes.len() as u32, endian);
        buf[offset + 4..offset + 4 + bytes.len()].copy_from_slice(bytes);
        total
    }

    #[test]
    fn reads_fixed_size_fields_in_sequence() {
        let schema = json!({
            "TypeDef:Struct": true,
            "endian": "little",
            "properties": {
                "a": { "TypeDef:Uint8": true },
                "b": { "TypeDef:Uint32": true },
                "c": { "TypeDef:Uint16": true }
            }
        });
        let mut buf = vec![0u8; 16];
        buf[0] = 42;
        write_u32(&mut buf, 1, 0x01020304, LE);
        buf[5..7].copy_from_slice(&1000u16.to_le_bytes());

        let mut reader = SequentialReader::new(&schema).expect("reader");
        assert_eq!(reader.position(), 0);

        let (name, value) = reader.read_next(&buf).unwrap().expect("field 0");
        assert_eq!(name, "a");
        assert_eq!(value, FieldValue::U8(42));
        assert_eq!(reader.position(), 1);

        let (name, value) = reader.read_next(&buf).unwrap().expect("field 1");
        assert_eq!(name, "b");
        assert_eq!(value, FieldValue::U32(0x01020304));
        assert_eq!(reader.position(), 5);

        let (name, value) = reader.read_next(&buf).unwrap().expect("field 2");
        assert_eq!(name, "c");
        assert_eq!(value, FieldValue::U16(1000));
        assert_eq!(reader.position(), 7);

        assert!(reader.read_next(&buf).unwrap().is_none());
    }

    #[test]
    fn reads_variable_length_string_with_length_prefix() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "id": { "TypeDef:Uint8": true },
                "name": { "TypeDef:String": true },
                "tail": { "TypeDef:Uint8": true }
            }
        });
        let mut buf = vec![0u8; 32];
        buf[0] = 7;
        let written = write_string(&mut buf, 1, "hello", LE);
        let after = 1 + written;
        buf[after] = 99;

        let mut reader = SequentialReader::new(&schema).unwrap();
        let (name, value) = reader.read_next(&buf).unwrap().unwrap();
        assert_eq!(name, "id");
        assert_eq!(value, FieldValue::U8(7));
        assert_eq!(reader.position(), 1);

        let (name, value) = reader.read_next(&buf).unwrap().unwrap();
        assert_eq!(name, "name");
        assert_eq!(value, FieldValue::String("hello"));
        assert_eq!(reader.position(), after);

        let (name, value) = reader.read_next(&buf).unwrap().unwrap();
        assert_eq!(name, "tail");
        assert_eq!(value, FieldValue::U8(99));
        assert_eq!(reader.position(), after + 1);

        assert!(reader.read_next(&buf).unwrap().is_none());
    }

    #[test]
    fn reads_bytes_field() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "blob": { "TypeDef:Bytes": true }
            }
        });
        let mut buf = vec![0u8; 16];
        let payload = [0xAA, 0xBB, 0xCC];
        write_u32(&mut buf, 0, 3, LE);
        buf[4..7].copy_from_slice(&payload);

        let mut reader = SequentialReader::new(&schema).unwrap();
        let (name, value) = reader.read_next(&buf).unwrap().unwrap();
        assert_eq!(name, "blob");
        assert_eq!(value, FieldValue::Bytes(&payload[..]));
        assert_eq!(reader.position(), 7);
    }

    #[test]
    fn respects_big_endian() {
        let schema = json!({
            "TypeDef:Struct": true,
            "endian": "big",
            "properties": {
                "id": { "TypeDef:Uint32": true }
            }
        });
        let mut buf = vec![0u8; 8];
        write_u32(&mut buf, 0, 0x01020304, BE);
        let mut reader = SequentialReader::new(&schema).unwrap();
        let (_, value) = reader.read_next(&buf).unwrap().unwrap();
        assert_eq!(value, FieldValue::U32(0x01020304));
        assert_eq!(reader.endian(), BE);
    }

    #[test]
    fn reset_rewinds_cursor() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "a": { "TypeDef:Uint8": true },
                "b": { "TypeDef:Uint8": true }
            }
        });
        let buf = [10u8, 20u8];
        let mut reader = SequentialReader::new(&schema).unwrap();
        let _ = reader.read_next(&buf).unwrap().unwrap();
        let _ = reader.read_next(&buf).unwrap().unwrap();
        assert_eq!(reader.position(), 2);
        reader.reset();
        assert_eq!(reader.position(), 0);
        assert_eq!(reader.field_index, 0);
        let (name, value) = reader.read_next(&buf).unwrap().unwrap();
        assert_eq!(name, "a");
        assert_eq!(value, FieldValue::U8(10));
    }

    #[test]
    fn read_field_walks_preceding_fields() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "a": { "TypeDef:Uint8": true },
                "b": { "TypeDef:Uint32": true },
                "c": { "TypeDef:Uint8": true }
            }
        });
        let mut buf = vec![0u8; 16];
        buf[0] = 1;
        write_u32(&mut buf, 1, 0xDEADBEEF, LE);
        buf[5] = 9;

        let mut reader = SequentialReader::new(&schema).unwrap();
        let value = reader.read_field(&buf, "c").unwrap();
        assert_eq!(value, FieldValue::U8(9));
        assert_eq!(reader.position(), 6);

        reader.reset();
        let value = reader.read_field(&buf, "b").unwrap();
        assert_eq!(value, FieldValue::U32(0xDEADBEEF));
    }

    #[test]
    fn read_field_unknown_returns_schema_error() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": { "a": { "TypeDef:Uint8": true } }
        });
        let buf = [0u8; 4];
        let mut reader = SequentialReader::new(&schema).unwrap();
        let err = reader.read_field(&buf, "missing").unwrap_err();
        assert!(matches!(err, TypedefError::Schema(_)), "got {err:?}");
    }

    #[test]
    fn buffer_too_short_returns_access_error() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "id": { "TypeDef:Uint32": true }
            }
        });
        let buf = [0u8; 2];
        let mut reader = SequentialReader::new(&schema).unwrap();
        let err = reader.read_next(&buf).unwrap_err();
        assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
    }

    #[test]
    fn rejects_non_struct_top_level() {
        let schema = json!({ "TypeDef:Uint32": true });
        let err = SequentialReader::new(&schema).unwrap_err();
        assert!(matches!(err, TypedefError::Schema(_)), "got {err:?}");
    }

    #[test]
    fn rejects_schema_without_typedef_kind() {
        let schema = json!({ "type": "object", "properties": {} });
        let err = SequentialReader::new(&schema).unwrap_err();
        assert!(matches!(err, TypedefError::Schema(_)), "got {err:?}");
    }

    #[test]
    fn reads_all_fixed_size_kinds() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "i8":  { "TypeDef:Int8": true },
                "i16": { "TypeDef:Int16": true },
                "i32": { "TypeDef:Int32": true },
                "u8":  { "TypeDef:Uint8": true },
                "u16": { "TypeDef:Uint16": true },
                "u32": { "TypeDef:Uint32": true },
                "u64": { "TypeDef:Uint64": true },
                "f32": { "TypeDef:Float32": true },
                "f64": { "TypeDef:Float64": true },
                "b":   { "TypeDef:Boolean": true },
                "e":   { "TypeDef:Enum": true }
            }
        });
        let mut buf = vec![0u8; 64];
        buf[0] = 0x80;
        buf[1..3].copy_from_slice(&(-1i16).to_le_bytes());
        buf[3..7].copy_from_slice(&(-5i32).to_le_bytes());
        buf[7] = 200;
        buf[8..10].copy_from_slice(&0xBEEFu16.to_le_bytes());
        buf[10..14].copy_from_slice(&0xDEADBEEFu32.to_le_bytes());
        buf[14..22].copy_from_slice(&0x0102030405060708u64.to_le_bytes());
        buf[22..26].copy_from_slice(&std::f32::consts::PI.to_le_bytes());
        buf[26..34].copy_from_slice(&std::f64::consts::PI.to_le_bytes());
        buf[34] = 0x01;
        buf[35..39].copy_from_slice(&7u32.to_le_bytes());

        let mut reader = SequentialReader::new(&schema).unwrap();
        let (_, v) = reader.read_next(&buf).unwrap().unwrap();
        assert_eq!(v, FieldValue::I8(-128));
        let (_, v) = reader.read_next(&buf).unwrap().unwrap();
        assert_eq!(v, FieldValue::I16(-1));
        let (_, v) = reader.read_next(&buf).unwrap().unwrap();
        assert_eq!(v, FieldValue::I32(-5));
        let (_, v) = reader.read_next(&buf).unwrap().unwrap();
        assert_eq!(v, FieldValue::U8(200));
        let (_, v) = reader.read_next(&buf).unwrap().unwrap();
        assert_eq!(v, FieldValue::U16(0xBEEF));
        let (_, v) = reader.read_next(&buf).unwrap().unwrap();
        assert_eq!(v, FieldValue::U32(0xDEADBEEF));
        let (_, v) = reader.read_next(&buf).unwrap().unwrap();
        assert_eq!(v, FieldValue::U64(0x0102030405060708));
        let (_, v) = reader.read_next(&buf).unwrap().unwrap();
        assert!(matches!(v, FieldValue::F32(x) if (x - std::f32::consts::PI).abs() < 1e-6));
        let (_, v) = reader.read_next(&buf).unwrap().unwrap();
        assert!(matches!(v, FieldValue::F64(x) if (x - std::f64::consts::PI).abs() < 1e-12));
        let (_, v) = reader.read_next(&buf).unwrap().unwrap();
        assert_eq!(v, FieldValue::Bool(true));
        let (_, v) = reader.read_next(&buf).unwrap().unwrap();
        assert_eq!(v, FieldValue::Enum(7));
        assert!(reader.read_next(&buf).unwrap().is_none());
    }

    #[test]
    fn nested_struct_reports_byte_range() {
        let nested = json!({
            "TypeDef:Struct": true,
            "properties": {
                "inner": {
                    "TypeDef:Struct": true,
                    "properties": {
                        "x": { "TypeDef:Uint8": true },
                        "y": { "TypeDef:Uint16": true }
                    }
                },
                "tail": { "TypeDef:Uint8": true }
            }
        });
        let mut buf = vec![0u8; 16];
        buf[0] = 1;
        buf[1..3].copy_from_slice(&0x0203u16.to_le_bytes());
        buf[3] = 9;

        let mut reader = SequentialReader::new(&nested).unwrap();
        let (name, value) = reader.read_next(&buf).unwrap().unwrap();
        assert_eq!(name, "inner");
        match value {
            FieldValue::Struct { start, end } => {
                assert_eq!(start, 0);
                assert_eq!(end, 3);
                let inner_schema = &nested["properties"]["inner"];
                let inner_reader = SequentialReader::new(inner_schema).unwrap();
                let inner_end =
                    walk_struct_size(inner_schema, inner_schema, &buf, start, LE).unwrap();
                assert_eq!(inner_end, end - start);
                let _ = inner_reader;
            }
            other => panic!("expected Struct, got {other:?}"),
        }
        assert_eq!(reader.position(), 3);

        let (name, value) = reader.read_next(&buf).unwrap().unwrap();
        assert_eq!(name, "tail");
        assert_eq!(value, FieldValue::U8(9));
        assert_eq!(reader.position(), 4);
    }

    #[test]
    fn byte_discriminator_union_reads_value() {
        let variant = json!({
            "TypeDef:Struct": true,
            "properties": {
                "x": { "TypeDef:Uint8": true }
            }
        });
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "packet": {
                    "TypeDef:Union": true,
                    "discriminator": { "kind": "byte", "offset": 0, "type": "TypeDef:Uint8" },
                    "mapping": { "5": variant.clone() },
                    "$defs": { "Read": variant.clone() }
                }
            }
        });
        let mut buf = vec![0u8; 8];
        buf[0] = 5;
        buf[1] = 42;

        let mut reader = SequentialReader::new(&schema).unwrap();
        let (name, value) = reader.read_next(&buf).unwrap().unwrap();
        assert_eq!(name, "packet");
        match value {
            FieldValue::Union {
                discriminator,
                variant_start,
            } => {
                assert_eq!(discriminator, "5");
                assert_eq!(variant_start, 1);
            }
            other => panic!("expected Union, got {other:?}"),
        }
        assert_eq!(reader.position(), 2);
    }

    #[test]
    fn field_discriminator_union_reads_value() {
        let variant = json!({
            "TypeDef:Struct": true,
            "properties": {
                "payload": { "TypeDef:Uint8": true }
            }
        });
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "event": {
                    "TypeDef:Union": true,
                    "discriminator": { "kind": "field", "name": "type" },
                    "properties": {
                        "type": { "TypeDef:String": true }
                    },
                    "mapping": { "read": variant.clone() }
                }
            }
        });
        let mut buf = vec![0u8; 32];
        let written = write_string(&mut buf, 0, "read", LE);
        let after = written;
        buf[after] = 7;

        let mut reader = SequentialReader::new(&schema).unwrap();
        let (name, value) = reader.read_next(&buf).unwrap().unwrap();
        assert_eq!(name, "event");
        match value {
            FieldValue::Union {
                discriminator,
                variant_start,
            } => {
                assert_eq!(discriminator, "read");
                assert_eq!(variant_start, after);
            }
            other => panic!("expected Union, got {other:?}"),
        }
        assert_eq!(reader.position(), after + 1);
    }

    #[test]
    fn fixed_count_array_reads_count_inline() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "vals": {
                    "TypeDef:Array": true,
                    "minItems": 3,
                    "maxItems": 3,
                    "items": { "TypeDef:Uint8": true }
                }
            }
        });
        let buf = [1u8, 2, 3];

        let mut reader = SequentialReader::new(&schema).unwrap();
        let (name, value) = reader.read_next(&buf).unwrap().unwrap();
        assert_eq!(name, "vals");
        match value {
            FieldValue::Array {
                count,
                element_start,
                element_stride,
            } => {
                assert_eq!(count, 3);
                assert_eq!(element_start, 0);
                assert_eq!(element_stride, 1);
            }
            other => panic!("expected Array, got {other:?}"),
        }
        assert_eq!(reader.position(), 3);
    }

    #[test]
    fn variable_count_array_reads_count_prefix() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "vals": {
                    "TypeDef:Array": true,
                    "items": { "TypeDef:Uint16": true }
                }
            }
        });
        let mut buf = vec![0u8; 16];
        write_u32(&mut buf, 0, 2, LE);
        buf[4..6].copy_from_slice(&100u16.to_le_bytes());
        buf[6..8].copy_from_slice(&200u16.to_le_bytes());

        let mut reader = SequentialReader::new(&schema).unwrap();
        let (name, value) = reader.read_next(&buf).unwrap().unwrap();
        assert_eq!(name, "vals");
        match value {
            FieldValue::Array {
                count,
                element_start,
                element_stride,
            } => {
                assert_eq!(count, 2);
                assert_eq!(element_start, 4);
                assert_eq!(element_stride, 2);
            }
            other => panic!("expected Array, got {other:?}"),
        }
        assert_eq!(reader.position(), 8);
    }

    #[test]
    fn variable_length_element_array_walks_sequentially() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "items": {
                    "TypeDef:Array": true,
                    "items": { "TypeDef:String": true }
                }
            }
        });
        let mut buf = vec![0u8; 64];
        write_u32(&mut buf, 0, 2, LE);
        let mut pos = 4;
        pos += write_string(&mut buf, pos, "ab", LE);
        pos += write_string(&mut buf, pos, "cdef", LE);

        let mut reader = SequentialReader::new(&schema).unwrap();
        let (name, value) = reader.read_next(&buf).unwrap().unwrap();
        assert_eq!(name, "items");
        match value {
            FieldValue::Array {
                count,
                element_start,
                element_stride,
            } => {
                assert_eq!(count, 2);
                assert_eq!(element_start, 4);
                assert_eq!(element_stride, 0);
            }
            other => panic!("expected Array, got {other:?}"),
        }
        assert_eq!(reader.position(), pos);
    }

    #[test]
    fn timestamp_field_reads_as_length_prefixed_string() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "ts": { "TypeDef:Timestamp": true }
            }
        });
        let mut buf = vec![0u8; 64];
        let stamp = "2026-07-20T15:30:00Z";
        let written = write_string(&mut buf, 0, stamp, LE);

        let mut reader = SequentialReader::new(&schema).unwrap();
        let (name, value) = reader.read_next(&buf).unwrap().unwrap();
        assert_eq!(name, "ts");
        assert_eq!(value, FieldValue::String(stamp));
        assert_eq!(reader.position(), written);
    }

    #[test]
    fn record_field_walks_entries() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "counts": {
                    "TypeDef:Record": true,
                    "values": { "TypeDef:Uint32": true }
                }
            }
        });
        let mut buf = vec![0u8; 64];
        write_u32(&mut buf, 0, 2, LE);
        let mut pos = 4;
        pos += write_string(&mut buf, pos, "a", LE);
        buf[pos..pos + 4].copy_from_slice(&1u32.to_le_bytes());
        pos += 4;
        pos += write_string(&mut buf, pos, "bb", LE);
        buf[pos..pos + 4].copy_from_slice(&2u32.to_le_bytes());
        pos += 4;

        let mut reader = SequentialReader::new(&schema).unwrap();
        let (name, _value) = reader.read_next(&buf).unwrap().unwrap();
        assert_eq!(name, "counts");
        assert_eq!(reader.position(), pos);
    }

    #[test]
    fn union_unknown_discriminator_returns_access_error() {
        let variant = json!({
            "TypeDef:Struct": true,
            "properties": { "x": { "TypeDef:Uint8": true } }
        });
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "packet": {
                    "TypeDef:Union": true,
                    "discriminator": { "kind": "byte", "offset": 0, "type": "TypeDef:Uint8" },
                    "mapping": { "5": variant },
                    "$defs": { "Read": variant }
                }
            }
        });
        let buf = [99u8, 0];
        let mut reader = SequentialReader::new(&schema).unwrap();
        let err = reader.read_next(&buf).unwrap_err();
        assert!(matches!(err, TypedefError::Access { .. }), "got {err:?}");
    }

    #[test]
    fn union_via_ref_resolves_variant() {
        let read_variant = json!({
            "TypeDef:Struct": true,
            "properties": { "len": { "TypeDef:Uint32": true } }
        });
        let schema = json!({
            "TypeDef:Struct": true,
            "$defs": { "Read": read_variant },
            "properties": {
                "packet": {
                    "TypeDef:Union": true,
                    "discriminator": { "kind": "byte", "offset": 0, "type": "TypeDef:Uint8" },
                    "mapping": { "5": { "$ref": "#/$defs/Read" } }
                }
            }
        });
        let mut buf = vec![0u8; 16];
        buf[0] = 5;
        write_u32(&mut buf, 1, 1234, LE);

        let mut reader = SequentialReader::new(&schema).unwrap();
        let (name, value) = reader.read_next(&buf).unwrap().unwrap();
        assert_eq!(name, "packet");
        match value {
            FieldValue::Union {
                discriminator,
                variant_start,
            } => {
                assert_eq!(discriminator, "5");
                assert_eq!(variant_start, 1);
            }
            other => panic!("expected Union, got {other:?}"),
        }
        assert_eq!(reader.position(), 5);
    }

    #[test]
    fn union_via_ref_with_local_defs_resolves_variant() {
        let read_variant = json!({
            "TypeDef:Struct": true,
            "properties": { "len": { "TypeDef:Uint32": true } }
        });
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "packet": {
                    "TypeDef:Union": true,
                    "discriminator": { "kind": "byte", "offset": 0, "type": "TypeDef:Uint8" },
                    "mapping": { "5": { "$ref": "#/$defs/Read" } },
                    "$defs": { "Read": read_variant }
                }
            }
        });
        let mut buf = vec![0u8; 16];
        buf[0] = 5;
        write_u32(&mut buf, 1, 1234, LE);

        let mut reader = SequentialReader::new(&schema).unwrap();
        let (name, value) = reader.read_next(&buf).unwrap().unwrap();
        assert_eq!(name, "packet");
        match value {
            FieldValue::Union {
                discriminator,
                variant_start,
            } => {
                assert_eq!(discriminator, "5");
                assert_eq!(variant_start, 1);
            }
            other => panic!("expected Union, got {other:?}"),
        }
        assert_eq!(reader.position(), 5);
    }

    #[test]
    fn empty_struct_returns_none_immediately() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {}
        });
        let buf = [];
        let mut reader = SequentialReader::new(&schema).unwrap();
        assert!(reader.read_next(&buf).unwrap().is_none());
        assert_eq!(reader.position(), 0);
    }

    #[test]
    fn nested_struct_with_variable_field_computes_end() {
        let nested = json!({
            "TypeDef:Struct": true,
            "properties": {
                "header": {
                    "TypeDef:Struct": true,
                    "properties": {
                        "id": { "TypeDef:Uint8": true },
                        "name": { "TypeDef:String": true }
                    }
                },
                "tail": { "TypeDef:Uint8": true }
            }
        });
        let mut buf = vec![0u8; 64];
        buf[0] = 1;
        let written = write_string(&mut buf, 1, "abc", LE);
        let header_end = 1 + written;
        buf[header_end] = 7;
        let expected_end = header_end + 1;

        let mut reader = SequentialReader::new(&nested).unwrap();
        let (name, value) = reader.read_next(&buf).unwrap().unwrap();
        assert_eq!(name, "header");
        match value {
            FieldValue::Struct { start, end } => {
                assert_eq!(start, 0);
                assert_eq!(end, header_end);
            }
            other => panic!("expected Struct, got {other:?}"),
        }
        assert_eq!(reader.position(), header_end);

        let (name, value) = reader.read_next(&buf).unwrap().unwrap();
        assert_eq!(name, "tail");
        assert_eq!(value, FieldValue::U8(7));
        assert_eq!(reader.position(), expected_end);
    }
}
