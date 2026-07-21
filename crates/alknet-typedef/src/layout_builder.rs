//! Packed sequential `LayoutBuilder` — Mode 1 write-side (ADR-096).
//!
//! Fields are packed with no alignment padding. Variable-length fields
//! shift all subsequent fields. The consumer provides actual data sizes
//! for variable-length fields; the builder computes byte positions for
//! each field. Used at write time when the consumer knows the data sizes
//! upfront.
//!
//! Per [layout-engine.md](../../docs/architecture/crates/typedef/layout-engine.md)
//! §"Mode 1: Packed sequential".
//!
//! # Layout rules
//!
//! - **Fixed-size fields**: recorded at the current offset with their
//!   known byte size; the offset advances by the size. No alignment
//!   padding is inserted (the `u32` at offset 1 is unaligned — correct
//!   for protocol wire formats).
//! - **Variable-length fields** (`TypeDef:String`, `TypeDef:Bytes`,
//!   `TypeDef:Timestamp`, `TypeDef:Record`): always inline
//!   length-prefixed in packed mode. The 4-byte length prefix is
//!   recorded at the current offset; the offset advances by `4 +
//!   data_size` where `data_size` comes from `var_sizes` keyed by the
//!   field's dotted path.
//! - **`TypeDef:Struct`**: recurses into `properties`, propagating the
//!   field path prefix (e.g., `"header.version"`). No padding before,
//!   between, or after the struct's fields.
//! - **`TypeDef:Array`** of fixed-size elements with a fixed count
//!   (`minItems == maxItems`): element `i` at
//!   `array_offset + i × element_size`. Each element is recorded as
//!   `"<array_path>[i]"`.
//! - **`TypeDef:Array`** with a variable count: a 4-byte count prefix
//!   at the array's offset. The consumer provides the total element
//!   data size in `var_sizes` under the array's field path; the builder
//!   adds `4 + data_size`.
//! - **`TypeDef:Union`** with a byte-offset discriminator: the
//!   discriminator is recorded at `"<union_path>.__discriminator"`.
//!   The consumer provides the discriminator value as a `usize` in
//!   `var_sizes` under `"<union_path>.__discriminator"`. The variant
//!   struct is laid out starting at `union_offset + disc_offset +
//!   disc_size`, with field paths prefixed by the union's path.
//! - **`TypeDef:Union`** with a field-name discriminator: the consumer
//!   provides the 0-based variant index in `var_sizes` under
//!   `"<union_path>.__variant"`. The selected variant struct is laid
//!   out at the union's offset, with field paths prefixed by the
//!   union's path. The discriminator field is a regular field within
//!   the variant struct.

use crate::error::TypedefError;
use crate::schema::{self, get_typedef_kind_loose_enum, resolve_ref_or_inline, DiscriminatorKind, Endian, TypeDefKind, DISCRIMINATOR_PATH, U32_SIZE};
use serde_json::Value;
use std::collections::HashMap;

const VARIANT_KEY: &str = "__variant";

/// A field position computed by the LayoutBuilder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldPosition {
    /// Byte offset of the field within the buffer.
    pub offset: usize,
    /// Byte size of the field (4 for length prefix of variable-length
    /// fields, actual size for fixed-size fields).
    pub size: usize,
    /// The TypeDef kind of the field.
    pub kind: TypeDefKind,
}

/// The result of building a layout: a map of field_path → FieldPosition
/// and the total buffer size needed.
///
/// Construct via [`LayoutBuilder::build`]. Fields are stored in layout
/// order (the order they appear in the schema's `properties`, with
/// nested struct fields appearing inline).
#[derive(Debug)]
pub struct PackedLayout {
    fields: Vec<(String, FieldPosition)>,
    total_size: usize,
}

impl PackedLayout {
    /// Look up a field's position by dotted path (e.g., `"header.version"`).
    ///
    /// Returns `None` if no field with the given path was recorded. For
    /// TUnion byte-offset discriminators, the discriminator is recorded
    /// under the synthetic path `"<union_path>.__discriminator"`.
    pub fn get(&self, field_path: &str) -> Option<&FieldPosition> {
        self.fields
            .iter()
            .find(|(path, _)| path == field_path)
            .map(|(_, pos)| pos)
    }

    /// The total buffer size needed to hold all fields.
    pub fn total_size(&self) -> usize {
        self.total_size
    }

    /// Iterate over all `(field_path, position)` pairs in layout order.
    ///
    /// Field order matches the schema's `properties` order (preserved by
    /// `serde_json`'s `preserve_order` feature). Nested struct fields
    /// appear after their parent's path prefix.
    pub fn iter(&self) -> impl Iterator<Item = &(String, FieldPosition)> {
        self.fields.iter()
    }
}

/// Builds a packed sequential layout for protocol wire formats.
///
/// Fields are packed with no alignment padding. Variable-length fields
/// shift all subsequent fields. The consumer provides actual data sizes
/// for variable-length fields to compute correct positions.
///
/// Used at write time when the consumer knows the data sizes upfront.
/// The builder does not write data — it only computes positions. The
/// consumer uses the [`crate::data_access`] write functions at the
/// computed positions.
///
/// # Example
///
/// For a struct with fields `[u8, u32, string]` where the string is
/// 10 bytes:
///
/// ```text
/// LayoutBuilder::build(var_sizes: {"payload": 10}):
///   field[0] u8:     offset 0, size 1
///   field[1] u32:    offset 1, size 4
///   field[2] string: offset 5, size 4 (length prefix) + 10 (data) = 14
///   total: 19
/// ```
///
/// There is no alignment padding. The `u32` at offset 1 is unaligned —
/// this is correct for protocol wire formats, which pack fields tightly.
#[derive(Debug)]
pub struct LayoutBuilder {
    schema: Value,
    endian: Endian,
}

impl LayoutBuilder {
    /// Create a new LayoutBuilder from a schema.
    ///
    /// The top-level schema must declare `TypeDef:Struct`. Endianness is
    /// parsed via [`Endian::from_schema`] (defaults to little-endian).
    ///
    /// # Errors
    ///
    /// Returns [`TypedefError::Schema`] if the schema has no
    /// `TypeDef:*` kind or the top-level kind is not `TypeDef:Struct`.
    pub fn new(schema: &Value) -> Result<Self, TypedefError> {
        let kind = schema::get_typedef_kind(schema)
            .and_then(|s| s.parse::<TypeDefKind>().ok())
            .ok_or_else(|| {
                TypedefError::Schema("top-level schema has no TypeDef:* kind".to_string())
            })?;
        if kind != TypeDefKind::Struct {
            return Err(TypedefError::Schema(format!(
                "LayoutBuilder requires a TypeDef:Struct at the top level, got {kind}"
            )));
        }
        let endian = Endian::from_schema(schema);
        Ok(Self {
            schema: schema.clone(),
            endian,
        })
    }

    /// Build the packed layout given actual data sizes for variable-length
    /// fields.
    ///
    /// `var_sizes` maps field paths to their actual byte sizes (not
    /// including the 4-byte length prefix — the builder adds that).
    ///
    /// For fixed-size fields, the size is known from the schema. For
    /// variable-length fields, the size comes from `var_sizes`. For
    /// TUnion, the consumer provides the discriminator value (byte-offset)
    /// or variant index (field-name) plus the variant's field sizes.
    ///
    /// # Errors
    ///
    /// - [`TypedefError::Schema`] for malformed schemas (missing
    ///   `properties`, unknown kind, unresolvable `$ref`).
    /// - [`TypedefError::Offset`] for missing variable-length field
    ///   sizes in `var_sizes`, missing discriminator values, or unknown
    ///   discriminator values.
    pub fn build(&self, var_sizes: &HashMap<String, usize>) -> Result<PackedLayout, TypedefError> {
        let mut ctx = BuildCtx {
            root: &self.schema,
            var_sizes,
            fields: Vec::new(),
        };
        let mut offset: usize = 0;
        ctx.walk_struct(&self.schema, "", &mut offset)?;
        Ok(PackedLayout {
            fields: ctx.fields,
            total_size: offset,
        })
    }

    /// The endianness parsed from the schema.
    pub fn endian(&self) -> Endian {
        self.endian
    }
}

/// Mutable context threaded through the recursive layout computation.
struct BuildCtx<'a> {
    root: &'a Value,
    var_sizes: &'a HashMap<String, usize>,
    fields: Vec<(String, FieldPosition)>,
}

impl<'a> BuildCtx<'a> {
    /// Recurse into a `TypeDef:Struct`, appending `(field_path, FieldPosition)`
    /// pairs to `self.fields` and advancing `offset`.
    ///
    /// `prefix` is the dotted path prefix for nested fields (empty at the
    /// top level).
    fn walk_struct(
        &mut self,
        schema: &Value,
        prefix: &str,
        offset: &mut usize,
    ) -> Result<(), TypedefError> {
        let properties = schema
            .as_object()
            .and_then(|o| o.get("properties"))
            .and_then(Value::as_object)
            .ok_or_else(|| {
                TypedefError::Schema("struct schema has no 'properties' object".to_string())
            })?;

        let field_schemas: Vec<(String, Value)> = properties
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (name, field_schema) in field_schemas {
            let field_path = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}.{name}")
            };
            self.walk_field(&field_schema, &field_path, offset)?;
        }
        Ok(())
    }

    /// Compute the layout for a single field, advancing `offset` and
    /// appending any field paths to `self.fields`.
    fn walk_field(
        &mut self,
        field_schema: &Value,
        field_path: &str,
        offset: &mut usize,
    ) -> Result<(), TypedefError> {
        let kind = get_typedef_kind_loose_enum(field_schema).ok_or_else(|| TypedefError::Offset {
            field_path: field_path.to_string(),
            reason: "field schema has no TypeDef:* kind".to_string(),
        })?;

        match kind {
            TypeDefKind::Struct => self.walk_struct(field_schema, field_path, offset),
            TypeDefKind::Union => self.walk_union(field_schema, field_path, offset),
            TypeDefKind::Array => self.walk_array(field_schema, field_path, offset),
            TypeDefKind::String
            | TypeDefKind::Bytes
            | TypeDefKind::Timestamp
            | TypeDefKind::Record => {
                self.walk_variable(field_path, offset, kind)
            }
            k if k.is_fixed_size() => {
                let size = k.type_size().ok_or_else(|| TypedefError::Offset {
                    field_path: field_path.to_string(),
                    reason: format!("type_size returned None for fixed kind {k}"),
                })?;
                let start = *offset;
                *offset = start
                    .checked_add(size)
                    .ok_or_else(|| TypedefError::Offset {
                        field_path: field_path.to_string(),
                        reason: format!("offset {start} + size {size} overflows usize"),
                    })?;
                self.push(field_path, start, size, k);
                Ok(())
            }
            _ => unreachable!("all TypeDefKind variants are covered above"),
        }
    }

    /// Compute the layout for a variable-length field (String/Bytes/
    /// Timestamp/Record). Always inline length-prefixed in packed mode.
    fn walk_variable(
        &mut self,
        field_path: &str,
        offset: &mut usize,
        kind: TypeDefKind,
    ) -> Result<(), TypedefError> {
        let data_size =
            self.var_sizes
                .get(field_path)
                .copied()
                .ok_or_else(|| TypedefError::Offset {
                    field_path: field_path.to_string(),
                    reason: "missing variable-length field size".to_string(),
                })?;
        let start = *offset;
        let total = U32_SIZE
            .checked_add(data_size)
            .ok_or_else(|| TypedefError::Offset {
                field_path: field_path.to_string(),
                reason: format!("prefix {U32_SIZE} + data size {data_size} overflows usize"),
            })?;
        *offset = start
            .checked_add(total)
            .ok_or_else(|| TypedefError::Offset {
                field_path: field_path.to_string(),
                reason: format!("offset {start} + total {total} overflows usize"),
            })?;
        self.push(field_path, start, U32_SIZE, kind);
        Ok(())
    }

    /// Compute the layout for a `TypeDef:Array` field.
    fn walk_array(
        &mut self,
        field_schema: &Value,
        field_path: &str,
        offset: &mut usize,
    ) -> Result<(), TypedefError> {
        let obj = field_schema
            .as_object()
            .ok_or_else(|| TypedefError::Offset {
                field_path: field_path.to_string(),
                reason: "array schema is not an object".to_string(),
            })?;

        let items = obj.get("items").ok_or_else(|| TypedefError::Offset {
            field_path: field_path.to_string(),
            reason: "TArray is missing 'items'".to_string(),
        })?;
        let element_schema =
            resolve_ref_or_inline(items, self.root).ok_or_else(|| TypedefError::Offset {
                field_path: field_path.to_string(),
                reason: "could not resolve TArray items schema".to_string(),
            })?;
        let elem_kind = get_typedef_kind_loose_enum(element_schema).ok_or_else(|| TypedefError::Offset {
            field_path: field_path.to_string(),
            reason: "TArray element schema has no TypeDef:* kind".to_string(),
        })?;

        if !elem_kind.is_fixed_size() {
            return Err(TypedefError::Offset {
                field_path: field_path.to_string(),
                reason: format!(
                    "TArray of variable-length element kind {elem_kind} is not supported (OQ-069)"
                ),
            });
        }

        let elem_size = elem_kind.type_size().ok_or_else(|| TypedefError::Offset {
            field_path: field_path.to_string(),
            reason: format!("element kind {elem_kind} has no fixed size"),
        })?;

        let min_items = obj
            .get("minItems")
            .and_then(Value::as_u64)
            .map(|n| n as usize);
        let max_items = obj
            .get("maxItems")
            .and_then(Value::as_u64)
            .map(|n| n as usize);
        let fixed_count = match (min_items, max_items) {
            (Some(mn), Some(mx)) if mn == mx => Some(mn),
            _ => None,
        };

        if let Some(count) = fixed_count {
            let start = *offset;
            for i in 0..count {
                let elem_offset = start
                    .checked_add(
                        i.checked_mul(elem_size)
                            .ok_or_else(|| TypedefError::Offset {
                                field_path: field_path.to_string(),
                                reason: format!(
                                    "element index {i} × size {elem_size} overflows usize"
                                ),
                            })?,
                    )
                    .ok_or_else(|| TypedefError::Offset {
                        field_path: field_path.to_string(),
                        reason: format!("element offset {start} + {i}×{elem_size} overflows usize"),
                    })?;
                self.push(
                    &format!("{field_path}[{i}]"),
                    elem_offset,
                    elem_size,
                    elem_kind,
                );
            }
            let array_size = count
                .checked_mul(elem_size)
                .ok_or_else(|| TypedefError::Offset {
                    field_path: field_path.to_string(),
                    reason: format!("array size {count} × {elem_size} overflows usize"),
                })?;
            *offset = start
                .checked_add(array_size)
                .ok_or_else(|| TypedefError::Offset {
                    field_path: field_path.to_string(),
                    reason: format!("offset {start} + array size {array_size} overflows usize"),
                })?;
            Ok(())
        } else {
            let data_size =
                self.var_sizes
                    .get(field_path)
                    .copied()
                    .ok_or_else(|| TypedefError::Offset {
                        field_path: field_path.to_string(),
                        reason: "missing variable-count array element data size".to_string(),
                    })?;
            let start = *offset;
            let total = U32_SIZE
                .checked_add(data_size)
                .ok_or_else(|| TypedefError::Offset {
                    field_path: field_path.to_string(),
                    reason: format!(
                        "count prefix {U32_SIZE} + data size {data_size} overflows usize"
                    ),
                })?;
            *offset = start
                .checked_add(total)
                .ok_or_else(|| TypedefError::Offset {
                    field_path: field_path.to_string(),
                    reason: format!("offset {start} + total {total} overflows usize"),
                })?;
            self.push(field_path, start, U32_SIZE, TypeDefKind::Array);
            Ok(())
        }
    }

    /// Compute the layout for a `TypeDef:Union` field.
    fn walk_union(
        &mut self,
        field_schema: &Value,
        field_path: &str,
        offset: &mut usize,
    ) -> Result<(), TypedefError> {
        let disc = schema::parse_discriminator(field_schema)?;
        let mapping = field_schema
            .as_object()
            .and_then(|o| o.get("mapping"))
            .and_then(Value::as_object)
            .ok_or_else(|| TypedefError::Offset {
                field_path: field_path.to_string(),
                reason: "TUnion is missing 'mapping' object".to_string(),
            })?;

        match disc {
            DiscriminatorKind::Byte {
                offset: disc_off,
                disc_type,
            } => self
                .walk_byte_discriminator_union(field_path, offset, disc_type, disc_off, mapping),
            DiscriminatorKind::Field { name: _ } => {
                self.walk_field_discriminator_union(field_path, offset, mapping)
            }
        }
    }

    /// Lay out a TUnion with a byte-offset discriminator.
    fn walk_byte_discriminator_union(
        &mut self,
        field_path: &str,
        offset: &mut usize,
        disc_type: TypeDefKind,
        disc_off: usize,
        mapping: &serde_json::Map<String, Value>,
    ) -> Result<(), TypedefError> {
        let disc_size = disc_type.type_size().ok_or_else(|| TypedefError::Offset {
            field_path: field_path.to_string(),
            reason: format!("discriminator type {disc_type} has no fixed size"),
        })?;

        let disc_key = format!("{field_path}.{DISCRIMINATOR_PATH}");
        let disc_value =
            self.var_sizes
                .get(&disc_key)
                .copied()
                .ok_or_else(|| TypedefError::Offset {
                    field_path: field_path.to_string(),
                    reason: format!("missing discriminator value at key '{disc_key}'"),
                })?;

        let union_start = *offset;
        let disc_abs_offset =
            union_start
                .checked_add(disc_off)
                .ok_or_else(|| TypedefError::Offset {
                    field_path: field_path.to_string(),
                    reason: format!(
                        "union offset {union_start} + disc offset {disc_off} overflows usize"
                    ),
                })?;
        self.push(&disc_key, disc_abs_offset, disc_size, disc_type);

        let variant_key = disc_value.to_string();
        let variant_schema = mapping
            .get(&variant_key)
            .ok_or_else(|| TypedefError::Offset {
                field_path: field_path.to_string(),
                reason: format!("unknown discriminator value: {variant_key}"),
            })?;
        let resolved = resolve_ref_or_inline(variant_schema, self.root).ok_or_else(|| {
            TypedefError::Offset {
                field_path: field_path.to_string(),
                reason: "could not resolve TUnion variant schema ($ref not found)".to_string(),
            }
        })?;
        let v_kind = get_typedef_kind_loose_enum(resolved).ok_or_else(|| TypedefError::Offset {
            field_path: field_path.to_string(),
            reason: "TUnion variant schema has no TypeDef:* kind".to_string(),
        })?;
        if v_kind != TypeDefKind::Struct {
            return Err(TypedefError::Offset {
                field_path: field_path.to_string(),
                reason: format!("TUnion variant must be TypeDef:Struct, got {v_kind}"),
            });
        }

        let variant_start =
            disc_abs_offset
                .checked_add(disc_size)
                .ok_or_else(|| TypedefError::Offset {
                    field_path: field_path.to_string(),
                    reason: format!(
                        "variant start {disc_abs_offset} + disc size {disc_size} overflows usize"
                    ),
                })?;
        *offset = variant_start;
        self.walk_struct(resolved, field_path, offset)?;
        Ok(())
    }

    /// Lay out a TUnion with a field-name discriminator.
    ///
    /// The discriminator is a regular field within the variant struct.
    /// The consumer selects the variant by 0-based index in
    /// `var_sizes["<union_path>.__variant"]`.
    fn walk_field_discriminator_union(
        &mut self,
        field_path: &str,
        offset: &mut usize,
        mapping: &serde_json::Map<String, Value>,
    ) -> Result<(), TypedefError> {
        let variant_key = format!("{field_path}.{VARIANT_KEY}");
        let variant_index =
            self.var_sizes
                .get(&variant_key)
                .copied()
                .ok_or_else(|| TypedefError::Offset {
                    field_path: field_path.to_string(),
                    reason: format!("missing variant index at key '{variant_key}'"),
                })?;
        let variant_entry =
            mapping
                .iter()
                .nth(variant_index)
                .ok_or_else(|| TypedefError::Offset {
                    field_path: field_path.to_string(),
                    reason: format!(
                        "variant index {variant_index} out of range (mapping has {} entries)",
                        mapping.len()
                    ),
                })?;
        let variant_schema = variant_entry.1;
        let resolved = resolve_ref_or_inline(variant_schema, self.root).ok_or_else(|| {
            TypedefError::Offset {
                field_path: field_path.to_string(),
                reason: "could not resolve TUnion variant schema ($ref not found)".to_string(),
            }
        })?;
        let v_kind = get_typedef_kind_loose_enum(resolved).ok_or_else(|| TypedefError::Offset {
            field_path: field_path.to_string(),
            reason: "TUnion variant schema has no TypeDef:* kind".to_string(),
        })?;
        if v_kind != TypeDefKind::Struct {
            return Err(TypedefError::Offset {
                field_path: field_path.to_string(),
                reason: format!("TUnion variant must be TypeDef:Struct, got {v_kind}"),
            });
        }
        self.walk_struct(resolved, field_path, offset)?;
        Ok(())
    }

    /// Push a `(field_path, FieldPosition)` pair onto the fields vec.
    fn push(&mut self, path: &str, offset: usize, size: usize, kind: TypeDefKind) {
        self.fields.push((
            path.to_string(),
            FieldPosition {
                offset,
                size,
                kind,
            },
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn build(schema: &Value, var_sizes: &HashMap<String, usize>) -> PackedLayout {
        LayoutBuilder::new(schema)
            .expect("builder")
            .build(var_sizes)
            .expect("layout")
    }

    fn var_sizes(pairs: &[(&str, usize)]) -> HashMap<String, usize> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn fixed_fields_packed_no_alignment_padding() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "a": { "TypeDef:Uint8": true },
                "b": { "TypeDef:Uint32": true },
                "c": { "TypeDef:Uint16": true }
            }
        });
        let layout = build(&schema, &var_sizes(&[]));
        assert_eq!(
            layout.get("a"),
            Some(&FieldPosition {
                offset: 0,
                size: 1,
                kind: TypeDefKind::Uint8
            })
        );
        assert_eq!(
            layout.get("b"),
            Some(&FieldPosition {
                offset: 1,
                size: 4,
                kind: TypeDefKind::Uint32
            })
        );
        assert_eq!(
            layout.get("c"),
            Some(&FieldPosition {
                offset: 5,
                size: 2,
                kind: TypeDefKind::Uint16
            })
        );
        assert_eq!(layout.total_size(), 7);
    }

    #[test]
    fn spec_example_u8_u32_string_total_19() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "flag": { "TypeDef:Uint8": true },
                "id": { "TypeDef:Uint32": true },
                "payload": { "TypeDef:String": true }
            }
        });
        let layout = build(&schema, &var_sizes(&[("payload", 10)]));
        assert_eq!(
            layout.get("flag"),
            Some(&FieldPosition {
                offset: 0,
                size: 1,
                kind: TypeDefKind::Uint8
            })
        );
        assert_eq!(
            layout.get("id"),
            Some(&FieldPosition {
                offset: 1,
                size: 4,
                kind: TypeDefKind::Uint32
            })
        );
        assert_eq!(
            layout.get("payload"),
            Some(&FieldPosition {
                offset: 5,
                size: 4,
                kind: TypeDefKind::String
            })
        );
        assert_eq!(layout.total_size(), 19);
    }

    #[test]
    fn variable_length_field_shifts_subsequent_fields() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "name": { "TypeDef:String": true },
                "tail": { "TypeDef:Uint8": true }
            }
        });
        let layout = build(&schema, &var_sizes(&[("name", 5)]));
        assert_eq!(
            layout.get("name"),
            Some(&FieldPosition {
                offset: 0,
                size: 4,
                kind: TypeDefKind::String
            })
        );
        assert_eq!(
            layout.get("tail"),
            Some(&FieldPosition {
                offset: 9,
                size: 1,
                kind: TypeDefKind::Uint8
            })
        );
        assert_eq!(layout.total_size(), 10);
    }

    #[test]
    fn bytes_field_uses_var_sizes() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "blob": { "TypeDef:Bytes": true }
            }
        });
        let layout = build(&schema, &var_sizes(&[("blob", 3)]));
        assert_eq!(
            layout.get("blob"),
            Some(&FieldPosition {
                offset: 0,
                size: 4,
                kind: TypeDefKind::Bytes
            })
        );
        assert_eq!(layout.total_size(), 7);
    }

    #[test]
    fn timestamp_field_uses_var_sizes() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "ts": { "TypeDef:Timestamp": true }
            }
        });
        let layout = build(&schema, &var_sizes(&[("ts", 20)]));
        assert_eq!(
            layout.get("ts"),
            Some(&FieldPosition {
                offset: 0,
                size: 4,
                kind: TypeDefKind::Timestamp
            })
        );
        assert_eq!(layout.total_size(), 24);
    }

    #[test]
    fn record_field_uses_var_sizes() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "counts": {
                    "TypeDef:Record": true,
                    "values": { "TypeDef:Uint32": true }
                }
            }
        });
        let layout = build(&schema, &var_sizes(&[("counts", 100)]));
        assert_eq!(
            layout.get("counts"),
            Some(&FieldPosition {
                offset: 0,
                size: 4,
                kind: TypeDefKind::Record
            })
        );
        assert_eq!(layout.total_size(), 104);
    }

    #[test]
    fn missing_var_size_returns_offset_error() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "name": { "TypeDef:String": true }
            }
        });
        let builder = LayoutBuilder::new(&schema).expect("builder");
        let err = builder.build(&var_sizes(&[])).unwrap_err();
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
    fn nested_struct_dotted_paths() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "header": {
                    "TypeDef:Struct": true,
                    "properties": {
                        "magic": { "TypeDef:Uint32": true },
                        "version": { "TypeDef:Uint8": true }
                    }
                },
                "body": { "TypeDef:Uint32": true }
            }
        });
        let layout = build(&schema, &var_sizes(&[]));
        assert_eq!(
            layout.get("header.magic"),
            Some(&FieldPosition {
                offset: 0,
                size: 4,
                kind: TypeDefKind::Uint32
            })
        );
        assert_eq!(
            layout.get("header.version"),
            Some(&FieldPosition {
                offset: 4,
                size: 1,
                kind: TypeDefKind::Uint8
            })
        );
        assert_eq!(
            layout.get("body"),
            Some(&FieldPosition {
                offset: 5,
                size: 4,
                kind: TypeDefKind::Uint32
            })
        );
        assert_eq!(layout.total_size(), 9);
    }

    #[test]
    fn nested_struct_with_variable_field() {
        let schema = json!({
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
        let layout = build(&schema, &var_sizes(&[("header.name", 3)]));
        assert_eq!(
            layout.get("header.id"),
            Some(&FieldPosition {
                offset: 0,
                size: 1,
                kind: TypeDefKind::Uint8
            })
        );
        assert_eq!(
            layout.get("header.name"),
            Some(&FieldPosition {
                offset: 1,
                size: 4,
                kind: TypeDefKind::String
            })
        );
        assert_eq!(
            layout.get("tail"),
            Some(&FieldPosition {
                offset: 8,
                size: 1,
                kind: TypeDefKind::Uint8
            })
        );
        assert_eq!(layout.total_size(), 9);
    }

    #[test]
    fn array_fixed_count_element_offsets() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "vals": {
                    "TypeDef:Array": true,
                    "items": { "TypeDef:Uint32": true },
                    "minItems": 3,
                    "maxItems": 3
                }
            }
        });
        let layout = build(&schema, &var_sizes(&[]));
        assert_eq!(
            layout.get("vals[0]"),
            Some(&FieldPosition {
                offset: 0,
                size: 4,
                kind: TypeDefKind::Uint32
            })
        );
        assert_eq!(
            layout.get("vals[1]"),
            Some(&FieldPosition {
                offset: 4,
                size: 4,
                kind: TypeDefKind::Uint32
            })
        );
        assert_eq!(
            layout.get("vals[2]"),
            Some(&FieldPosition {
                offset: 8,
                size: 4,
                kind: TypeDefKind::Uint32
            })
        );
        assert_eq!(layout.total_size(), 12);
    }

    #[test]
    fn array_fixed_count_after_preceding_field() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "id": { "TypeDef:Uint8": true },
                "vals": {
                    "TypeDef:Array": true,
                    "items": { "TypeDef:Uint16": true },
                    "minItems": 2,
                    "maxItems": 2
                }
            }
        });
        let layout = build(&schema, &var_sizes(&[]));
        assert_eq!(
            layout.get("id"),
            Some(&FieldPosition {
                offset: 0,
                size: 1,
                kind: TypeDefKind::Uint8
            })
        );
        assert_eq!(
            layout.get("vals[0]"),
            Some(&FieldPosition {
                offset: 1,
                size: 2,
                kind: TypeDefKind::Uint16
            })
        );
        assert_eq!(
            layout.get("vals[1]"),
            Some(&FieldPosition {
                offset: 3,
                size: 2,
                kind: TypeDefKind::Uint16
            })
        );
        assert_eq!(layout.total_size(), 5);
    }

    #[test]
    fn array_variable_count_uses_count_prefix() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "vals": {
                    "TypeDef:Array": true,
                    "items": { "TypeDef:Uint32": true }
                }
            }
        });
        let layout = build(&schema, &var_sizes(&[("vals", 12)]));
        assert_eq!(
            layout.get("vals"),
            Some(&FieldPosition {
                offset: 0,
                size: 4,
                kind: TypeDefKind::Array
            })
        );
        assert_eq!(layout.total_size(), 16);
    }

    #[test]
    fn array_variable_count_missing_size_is_offset_error() {
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
        let err = builder.build(&var_sizes(&[])).unwrap_err();
        assert!(matches!(err, TypedefError::Offset { .. }));
    }

    #[test]
    fn array_variable_length_element_is_not_supported() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "items": {
                    "TypeDef:Array": true,
                    "items": { "TypeDef:String": true }
                }
            }
        });
        let builder = LayoutBuilder::new(&schema).expect("builder");
        let err = builder.build(&var_sizes(&[("items", 10)])).unwrap_err();
        match err {
            TypedefError::Offset { reason, .. } => {
                assert!(reason.contains("OQ-069"), "reason: {reason}");
            }
            other => panic!("expected Offset, got {other:?}"),
        }
    }

    #[test]
    fn union_byte_discriminator_sftp_pattern() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "payload": {
                    "TypeDef:Union": true,
                    "discriminator": {
                        "kind": "byte",
                        "offset": 0,
                        "type": "TypeDef:Uint8"
                    },
                    "mapping": {
                        "5": { "$ref": "#/$defs/Read" },
                        "6": { "$ref": "#/$defs/Write" }
                    }
                }
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
        let vs = var_sizes(&[("payload.__discriminator", 5)]);
        let layout = build(&schema, &vs);
        assert_eq!(
            layout.get("payload.__discriminator"),
            Some(&FieldPosition {
                offset: 0,
                size: 1,
                kind: TypeDefKind::Uint8
            })
        );
        assert_eq!(
            layout.get("payload.handle"),
            Some(&FieldPosition {
                offset: 1,
                size: 4,
                kind: TypeDefKind::Uint32
            })
        );
        assert_eq!(
            layout.get("payload.length"),
            Some(&FieldPosition {
                offset: 5,
                size: 4,
                kind: TypeDefKind::Uint32
            })
        );
        assert_eq!(layout.total_size(), 9);
    }

    #[test]
    fn union_byte_discriminator_write_variant() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "payload": {
                    "TypeDef:Union": true,
                    "discriminator": {
                        "kind": "byte",
                        "offset": 0,
                        "type": "TypeDef:Uint8"
                    },
                    "mapping": {
                        "5": { "$ref": "#/$defs/Read" },
                        "6": { "$ref": "#/$defs/Write" }
                    }
                }
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
        let vs = var_sizes(&[("payload.__discriminator", 6)]);
        let layout = build(&schema, &vs);
        assert_eq!(
            layout.get("payload.__discriminator"),
            Some(&FieldPosition {
                offset: 0,
                size: 1,
                kind: TypeDefKind::Uint8
            })
        );
        assert_eq!(
            layout.get("payload.data"),
            Some(&FieldPosition {
                offset: 9,
                size: 4,
                kind: TypeDefKind::Uint32
            })
        );
        assert_eq!(layout.total_size(), 13);
    }

    #[test]
    fn union_byte_discriminator_with_variable_variant_field() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "packet": {
                    "TypeDef:Union": true,
                    "discriminator": {
                        "kind": "byte",
                        "offset": 0,
                        "type": "TypeDef:Uint8"
                    },
                    "mapping": {
                        "5": { "$ref": "#/$defs/Read" }
                    }
                }
            },
            "$defs": {
                "Read": {
                    "TypeDef:Struct": true,
                    "properties": {
                        "handle": { "TypeDef:Uint32": true },
                        "path": { "TypeDef:String": true }
                    }
                }
            }
        });
        let vs = var_sizes(&[("packet.__discriminator", 5), ("packet.path", 8)]);
        let layout = build(&schema, &vs);
        assert_eq!(
            layout.get("packet.__discriminator"),
            Some(&FieldPosition {
                offset: 0,
                size: 1,
                kind: TypeDefKind::Uint8
            })
        );
        assert_eq!(
            layout.get("packet.handle"),
            Some(&FieldPosition {
                offset: 1,
                size: 4,
                kind: TypeDefKind::Uint32
            })
        );
        assert_eq!(
            layout.get("packet.path"),
            Some(&FieldPosition {
                offset: 5,
                size: 4,
                kind: TypeDefKind::String
            })
        );
        assert_eq!(layout.total_size(), 17);
    }

    #[test]
    fn union_byte_discriminator_unknown_value_is_offset_error() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "payload": {
                    "TypeDef:Union": true,
                    "discriminator": {
                        "kind": "byte",
                        "offset": 0,
                        "type": "TypeDef:Uint8"
                    },
                    "mapping": {
                        "5": { "$ref": "#/$defs/Read" }
                    }
                }
            },
            "$defs": {
                "Read": {
                    "TypeDef:Struct": true,
                    "properties": { "x": { "TypeDef:Uint8": true } }
                }
            }
        });
        let vs = var_sizes(&[("payload.__discriminator", 99)]);
        let builder = LayoutBuilder::new(&schema).expect("builder");
        let err = builder.build(&vs).unwrap_err();
        match err {
            TypedefError::Offset { reason, .. } => {
                assert!(reason.contains("99"), "reason: {reason}");
            }
            other => panic!("expected Offset, got {other:?}"),
        }
    }

    #[test]
    fn union_byte_discriminator_missing_value_is_offset_error() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "payload": {
                    "TypeDef:Union": true,
                    "discriminator": {
                        "kind": "byte",
                        "offset": 0,
                        "type": "TypeDef:Uint8"
                    },
                    "mapping": {
                        "5": { "$ref": "#/$defs/Read" }
                    }
                }
            },
            "$defs": {
                "Read": {
                    "TypeDef:Struct": true,
                    "properties": { "x": { "TypeDef:Uint8": true } }
                }
            }
        });
        let builder = LayoutBuilder::new(&schema).expect("builder");
        let err = builder.build(&var_sizes(&[])).unwrap_err();
        assert!(matches!(err, TypedefError::Offset { .. }));
    }

    #[test]
    fn union_field_name_discriminator() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "event": {
                    "TypeDef:Union": true,
                    "discriminator": { "kind": "field", "name": "type" },
                    "mapping": {
                        "read": { "$ref": "#/$defs/Read" },
                        "write": { "$ref": "#/$defs/Write" }
                    }
                }
            },
            "$defs": {
                "Read": {
                    "TypeDef:Struct": true,
                    "properties": {
                        "type": { "TypeDef:Uint8": true },
                        "handle": { "TypeDef:Uint32": true }
                    }
                },
                "Write": {
                    "TypeDef:Struct": true,
                    "properties": {
                        "type": { "TypeDef:Uint8": true },
                        "handle": { "TypeDef:Uint32": true },
                        "length": { "TypeDef:Uint32": true }
                    }
                }
            }
        });
        let vs = var_sizes(&[("event.__variant", 0)]);
        let layout = build(&schema, &vs);
        assert_eq!(
            layout.get("event.type"),
            Some(&FieldPosition {
                offset: 0,
                size: 1,
                kind: TypeDefKind::Uint8
            })
        );
        assert_eq!(
            layout.get("event.handle"),
            Some(&FieldPosition {
                offset: 1,
                size: 4,
                kind: TypeDefKind::Uint32
            })
        );
        assert_eq!(layout.total_size(), 5);
    }

    #[test]
    fn union_field_name_discriminator_write_variant() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "event": {
                    "TypeDef:Union": true,
                    "discriminator": { "kind": "field", "name": "type" },
                    "mapping": {
                        "read": { "$ref": "#/$defs/Read" },
                        "write": { "$ref": "#/$defs/Write" }
                    }
                }
            },
            "$defs": {
                "Read": {
                    "TypeDef:Struct": true,
                    "properties": {
                        "type": { "TypeDef:Uint8": true },
                        "handle": { "TypeDef:Uint32": true }
                    }
                },
                "Write": {
                    "TypeDef:Struct": true,
                    "properties": {
                        "type": { "TypeDef:Uint8": true },
                        "handle": { "TypeDef:Uint32": true },
                        "length": { "TypeDef:Uint32": true }
                    }
                }
            }
        });
        let vs = var_sizes(&[("event.__variant", 1)]);
        let layout = build(&schema, &vs);
        assert_eq!(
            layout.get("event.length"),
            Some(&FieldPosition {
                offset: 5,
                size: 4,
                kind: TypeDefKind::Uint32
            })
        );
        assert_eq!(layout.total_size(), 9);
    }

    #[test]
    fn union_field_name_missing_variant_index_is_offset_error() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "event": {
                    "TypeDef:Union": true,
                    "discriminator": { "kind": "field", "name": "type" },
                    "mapping": {
                        "read": { "$ref": "#/$defs/Read" }
                    }
                }
            },
            "$defs": {
                "Read": {
                    "TypeDef:Struct": true,
                    "properties": { "type": { "TypeDef:Uint8": true } }
                }
            }
        });
        let builder = LayoutBuilder::new(&schema).expect("builder");
        let err = builder.build(&var_sizes(&[])).unwrap_err();
        assert!(matches!(err, TypedefError::Offset { .. }));
    }

    #[test]
    fn union_field_name_variant_index_out_of_range_is_offset_error() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "event": {
                    "TypeDef:Union": true,
                    "discriminator": { "kind": "field", "name": "type" },
                    "mapping": {
                        "read": { "$ref": "#/$defs/Read" }
                    }
                }
            },
            "$defs": {
                "Read": {
                    "TypeDef:Struct": true,
                    "properties": { "type": { "TypeDef:Uint8": true } }
                }
            }
        });
        let vs = var_sizes(&[("event.__variant", 5)]);
        let builder = LayoutBuilder::new(&schema).expect("builder");
        let err = builder.build(&vs).unwrap_err();
        assert!(matches!(err, TypedefError::Offset { .. }));
    }

    #[test]
    fn iter_returns_fields_in_layout_order() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "a": { "TypeDef:Uint8": true },
                "b": { "TypeDef:Uint32": true },
                "c": { "TypeDef:Uint16": true }
            }
        });
        let layout = build(&schema, &var_sizes(&[]));
        let paths: Vec<&str> = layout.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(paths, vec!["a", "b", "c"]);
    }

    #[test]
    fn iter_includes_nested_struct_fields() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "header": {
                    "TypeDef:Struct": true,
                    "properties": {
                        "magic": { "TypeDef:Uint32": true },
                        "version": { "TypeDef:Uint8": true }
                    }
                },
                "body": { "TypeDef:Uint32": true }
            }
        });
        let layout = build(&schema, &var_sizes(&[]));
        let paths: Vec<&str> = layout.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(paths, vec!["header.magic", "header.version", "body"]);
    }

    #[test]
    fn get_returns_none_for_unknown_path() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "a": { "TypeDef:Uint8": true }
            }
        });
        let layout = build(&schema, &var_sizes(&[]));
        assert!(layout.get("missing").is_none());
    }

    #[test]
    fn endian_parsed_from_schema() {
        let schema = json!({
            "TypeDef:Struct": true,
            "endian": "big",
            "properties": {
                "id": { "TypeDef:Uint32": true }
            }
        });
        let builder = LayoutBuilder::new(&schema).expect("builder");
        assert_eq!(builder.endian(), Endian::Big);
    }

    #[test]
    fn endian_defaults_to_little() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "id": { "TypeDef:Uint32": true }
            }
        });
        let builder = LayoutBuilder::new(&schema).expect("builder");
        assert_eq!(builder.endian(), Endian::Little);
    }

    #[test]
    fn new_rejects_non_struct_top_level() {
        let schema = json!({ "TypeDef:Uint32": true });
        let err = LayoutBuilder::new(&schema).unwrap_err();
        assert!(matches!(err, TypedefError::Schema(_)));
    }

    #[test]
    fn new_rejects_missing_typedef_kind() {
        let schema = json!({ "type": "object", "properties": {} });
        let err = LayoutBuilder::new(&schema).unwrap_err();
        assert!(matches!(err, TypedefError::Schema(_)));
    }

    #[test]
    fn build_rejects_struct_without_properties() {
        let schema = json!({ "TypeDef:Struct": true });
        let builder = LayoutBuilder::new(&schema).expect("builder");
        let err = builder.build(&var_sizes(&[])).unwrap_err();
        assert!(matches!(err, TypedefError::Schema(_)));
    }

    #[test]
    fn build_rejects_field_without_typedef_kind() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "a": { "type": "integer" }
            }
        });
        let builder = LayoutBuilder::new(&schema).expect("builder");
        let err = builder.build(&var_sizes(&[])).unwrap_err();
        assert!(matches!(err, TypedefError::Offset { .. }));
    }

    #[test]
    fn object_form_keyword_is_recognized() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "name": { "TypeDef:String": { "encoding": "length-prefixed" } }
            }
        });
        let layout = build(&schema, &var_sizes(&[("name", 5)]));
        assert_eq!(
            layout.get("name"),
            Some(&FieldPosition {
                offset: 0,
                size: 4,
                kind: TypeDefKind::String
            })
        );
        assert_eq!(layout.total_size(), 9);
    }

    #[test]
    fn all_fixed_size_kinds_get_correct_sizes() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "i8":  { "TypeDef:Int8": true },
                "i16": { "TypeDef:Int16": true },
                "i32": { "TypeDef:Int32": true },
                "u8":  { "TypeDef:Uint8": true },
                "u16": { "TypeDef:Uint16": true },
                "u32": { "TypeDef:Uint32": true },
                "f32": { "TypeDef:Float32": true },
                "f64": { "TypeDef:Float64": true },
                "b":   { "TypeDef:Boolean": true },
                "e":   { "TypeDef:Enum": true }
            }
        });
        let layout = build(&schema, &var_sizes(&[]));
        assert_eq!(layout.get("i8").unwrap().size, 1);
        assert_eq!(layout.get("i16").unwrap().size, 2);
        assert_eq!(layout.get("i32").unwrap().size, 4);
        assert_eq!(layout.get("u8").unwrap().size, 1);
        assert_eq!(layout.get("u16").unwrap().size, 2);
        assert_eq!(layout.get("u32").unwrap().size, 4);
        assert_eq!(layout.get("f32").unwrap().size, 4);
        assert_eq!(layout.get("f64").unwrap().size, 8);
        assert_eq!(layout.get("b").unwrap().size, 1);
        assert_eq!(layout.get("e").unwrap().size, 4);
        assert_eq!(layout.total_size(), 1 + 2 + 4 + 1 + 2 + 4 + 4 + 8 + 1 + 4);
    }

    #[test]
    fn empty_struct_produces_zero_size_layout() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {}
        });
        let layout = build(&schema, &var_sizes(&[]));
        assert_eq!(layout.total_size(), 0);
        assert_eq!(layout.iter().count(), 0);
    }

    #[test]
    fn inline_variant_schema_works_without_ref() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "packet": {
                    "TypeDef:Union": true,
                    "discriminator": {
                        "kind": "byte",
                        "offset": 0,
                        "type": "TypeDef:Uint8"
                    },
                    "mapping": {
                        "5": {
                            "TypeDef:Struct": true,
                            "properties": {
                                "x": { "TypeDef:Uint32": true }
                            }
                        }
                    }
                }
            }
        });
        let vs = var_sizes(&[("packet.__discriminator", 5)]);
        let layout = build(&schema, &vs);
        assert_eq!(
            layout.get("packet.__discriminator"),
            Some(&FieldPosition {
                offset: 0,
                size: 1,
                kind: TypeDefKind::Uint8
            })
        );
        assert_eq!(
            layout.get("packet.x"),
            Some(&FieldPosition {
                offset: 1,
                size: 4,
                kind: TypeDefKind::Uint32
            })
        );
        assert_eq!(layout.total_size(), 5);
    }

    #[test]
    fn union_nested_inside_struct_after_preceding_field() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "id": { "TypeDef:Uint8": true },
                "payload": {
                    "TypeDef:Union": true,
                    "discriminator": {
                        "kind": "byte",
                        "offset": 0,
                        "type": "TypeDef:Uint8"
                    },
                    "mapping": {
                        "5": { "$ref": "#/$defs/Read" }
                    }
                }
            },
            "$defs": {
                "Read": {
                    "TypeDef:Struct": true,
                    "properties": {
                        "handle": { "TypeDef:Uint32": true }
                    }
                }
            }
        });
        let vs = var_sizes(&[("payload.__discriminator", 5)]);
        let layout = build(&schema, &vs);
        assert_eq!(
            layout.get("id"),
            Some(&FieldPosition {
                offset: 0,
                size: 1,
                kind: TypeDefKind::Uint8
            })
        );
        assert_eq!(
            layout.get("payload.__discriminator"),
            Some(&FieldPosition {
                offset: 1,
                size: 1,
                kind: TypeDefKind::Uint8
            })
        );
        assert_eq!(
            layout.get("payload.handle"),
            Some(&FieldPosition {
                offset: 2,
                size: 4,
                kind: TypeDefKind::Uint32
            })
        );
        assert_eq!(layout.total_size(), 6);
    }
}
