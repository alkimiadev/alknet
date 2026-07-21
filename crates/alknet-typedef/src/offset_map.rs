//! Aligned static `OffsetMap` — Mode 2 of the two layout modes (ADR-096).
//!
//! Fields have fixed positions with natural alignment padding.
//! Variable-length fields get a 4-byte length prefix at a known offset;
//! the variable data is not included in the static layout. Used for
//! mmap-friendly formats (metatensor, safetensors).
//!
//! The offset computation is a recursive walk of the schema JSON. Nested
//! structs propagate field path prefixes (producing dotted paths like
//! `"header.version"`). Alignment padding is inserted before each field
//! to satisfy the field's alignment requirement (natural alignment by
//! default, overridable via the `"align"` annotation).

use crate::error::TypedefError;
use crate::schema::{
    get_typedef_kind, natural_alignment, parse_align, parse_discriminator, parse_encoding,
    parse_max_length, type_size, DiscriminatorKind, VariableEncoding,
};
use serde_json::Value;

/// The synthetic field path used for a TUnion byte-offset discriminator.
const DISCRIMINATOR_PATH: &str = "__discriminator";

/// A byte range within a buffer.
///
/// Produced by [`OffsetMap::compute`] for each field in a schema. The
/// range is half-open: `start..end`. `end - start` is the field's byte
/// size in the static layout (for variable-length fields, this is the
/// size of the length prefix, the `{offset, length}` pair, or the
/// `maxLength` reservation — not the variable data itself).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    /// Inclusive start byte offset.
    pub start: usize,
    /// Exclusive end byte offset.
    pub end: usize,
}

impl ByteRange {
    /// Byte length of the range (`end - start`).
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    /// True if the range covers zero bytes.
    pub fn is_empty(&self) -> bool {
        self.end == self.start
    }
}

/// A flat table of `(field_path, byte_range)` pairs computed from a schema.
///
/// Fields have fixed positions with natural alignment padding.
/// Used for mmap-friendly formats (metatensor, safetensors) where random
/// access by field path is required — the consumer can read field N
/// without reading fields `0..N-1` first.
///
/// Construct via [`OffsetMap::compute`]. Variable-length fields appear
/// in the table as their fixed-position portion only (length prefix,
/// `{offset, length}` pair, or `maxLength` reservation); the variable
/// data lives outside the static layout.
#[derive(Debug)]
pub struct OffsetMap {
    fields: Vec<(String, ByteRange)>,
    total_size: usize,
}

impl OffsetMap {
    /// Compute the offset map from a schema JSON value.
    ///
    /// Walks the schema recursively, computing byte positions for each
    /// field based on type sizes, field order, and alignment. The
    /// top-level schema must be a `TypeDef:Struct`.
    ///
    /// # Errors
    ///
    /// Returns [`TypedefError::Schema`] if the top-level schema is not a
    /// `TypeDef:Struct` or has no `TypeDef:*` kind, or if the schema is
    /// malformed (missing `properties`, unknown kind, etc.).
    ///
    /// Returns [`TypedefError::Offset`] for unsupported type combinations
    /// encountered during the walk.
    pub fn compute(schema: &Value) -> Result<Self, TypedefError> {
        let kind = get_typedef_kind(schema).ok_or_else(|| {
            TypedefError::Schema("top-level schema has no TypeDef:* kind".to_string())
        })?;
        if kind != "TypeDef:Struct" {
            return Err(TypedefError::Schema(format!(
                "OffsetMap::compute requires a TypeDef:Struct at the top level, got {kind}"
            )));
        }
        let mut ctx = ComputeCtx {
            root: schema,
            fields: Vec::new(),
            offset: 0,
        };
        let (total, _align) = ctx.compute_struct(schema, "", 1)?;
        Ok(Self {
            fields: ctx.fields,
            total_size: total,
        })
    }

    /// Look up a field's byte range by dotted path (e.g., `"header.version"`).
    ///
    /// Returns `None` if no field with the given path was recorded. For
    /// TUnion byte-offset discriminators, the discriminator is recorded
    /// under the synthetic path `"__discriminator"` (qualified by the
    /// union field's path, e.g. `"payload.__discriminator"`).
    pub fn get(&self, field_path: &str) -> Option<&ByteRange> {
        self.fields
            .iter()
            .find(|(path, _)| path == field_path)
            .map(|(_, range)| range)
    }

    /// The total size of the struct in bytes (including trailing alignment padding).
    pub fn total_size(&self) -> usize {
        self.total_size
    }

    /// Iterate over all `(field_path, byte_range)` pairs in insertion order.
    ///
    /// Field order matches the schema's `properties` order (preserved by
    /// `serde_json`'s `preserve_order` feature). Nested struct fields
    /// appear after their parent's path prefix.
    pub fn iter(&self) -> impl Iterator<Item = &(String, ByteRange)> {
        self.fields.iter()
    }
}

/// Mutable context threaded through the recursive offset computation.
///
/// Carries the running `offset`, the accumulating `fields` vec, and a
/// reference to the root schema for `$ref` resolution. Grouping these
/// keeps the recursive helper signatures small.
struct ComputeCtx<'a> {
    root: &'a Value,
    fields: Vec<(String, ByteRange)>,
    offset: usize,
}

/// Result of laying out a single field: its alignment.
struct FieldLayout {
    align: usize,
}

impl<'a> ComputeCtx<'a> {
    /// Recurse into a `TypeDef:Struct`, appending `(field_path, ByteRange)`
    /// pairs to `self.fields` and advancing `self.offset`.
    ///
    /// Returns `(total_size, alignment)` where `total_size` includes
    /// trailing alignment padding and `alignment` is the struct's
    /// effective alignment (its own `align` annotation, or the max of its
    /// fields' alignments).
    ///
    /// `struct_schema` is the schema of the struct to walk. `prefix` is the
    /// dotted path prefix for nested fields (empty at the top level).
    /// `parent_struct_align` is the default alignment a field inherits
    /// when it specifies neither its own `align` annotation nor a natural
    /// alignment larger than the default.
    fn compute_struct(
        &mut self,
        struct_schema: &Value,
        prefix: &str,
        parent_struct_align: usize,
    ) -> Result<(usize, usize), TypedefError> {
        let obj = struct_schema
            .as_object()
            .ok_or_else(|| TypedefError::Schema("struct schema is not an object".to_string()))?;
        let properties = obj
            .get("properties")
            .and_then(|v| v.as_object())
            .ok_or_else(|| {
                TypedefError::Schema("struct schema has no 'properties' object".to_string())
            })?;

        let struct_default_align = parse_align(struct_schema).unwrap_or(parent_struct_align);
        let mut max_align: usize = 1;
        let struct_start = self.offset;

        let field_schemas: Vec<(String, Value)> = properties
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (field_name, field_schema) in field_schemas {
            let field_path = if prefix.is_empty() {
                field_name.clone()
            } else {
                format!("{prefix}.{field_name}")
            };
            let layout = self.compute_field(&field_schema, &field_path, struct_default_align)?;
            if layout.align > max_align {
                max_align = layout.align;
            }
        }

        let effective_align = parse_align(struct_schema).unwrap_or(max_align).max(1);
        align_up(&mut self.offset, effective_align);
        let total = self.offset - struct_start;
        Ok((total, effective_align))
    }

    /// Compute the layout for a single field, advancing `self.offset`
    /// and appending any field paths to `self.fields`.
    fn compute_field(
        &mut self,
        field_schema: &Value,
        field_path: &str,
        struct_default_align: usize,
    ) -> Result<FieldLayout, TypedefError> {
        let kind = typedef_kind_loose(field_schema).ok_or_else(|| TypedefError::Offset {
            field_path: field_path.to_string(),
            reason: "field schema has no TypeDef:* kind".to_string(),
        })?;

        match kind {
            "TypeDef:Struct" => {
                self.compute_struct_field(field_schema, field_path, struct_default_align)
            }
            "TypeDef:Union" => {
                self.compute_union_field(field_schema, field_path, struct_default_align)
            }
            "TypeDef:Array" => {
                self.compute_array_field(field_schema, field_path, struct_default_align)
            }
            "TypeDef:String" | "TypeDef:Bytes" | "TypeDef:Record" | "TypeDef:Timestamp" => {
                self.compute_variable_field(field_schema, field_path, struct_default_align)
            }
            fixed if is_fixed_kind(fixed) => {
                self.compute_fixed_field(fixed, field_schema, field_path, struct_default_align)
            }
            other => Err(TypedefError::Offset {
                field_path: field_path.to_string(),
                reason: format!("unsupported TypeDef kind for offset computation: {other}"),
            }),
        }
    }

    /// Compute the layout for a fixed-size primitive field.
    fn compute_fixed_field(
        &mut self,
        kind: &str,
        field_schema: &Value,
        field_path: &str,
        struct_default_align: usize,
    ) -> Result<FieldLayout, TypedefError> {
        let size = type_size(kind).ok_or_else(|| TypedefError::Offset {
            field_path: field_path.to_string(),
            reason: format!("type_size returned None for fixed kind {kind}"),
        })?;
        let natural = natural_alignment(kind);
        let align = field_alignment(field_schema, struct_default_align, natural);
        align_up(&mut self.offset, align);
        let start = self.offset;
        self.offset += size;
        self.push(field_path, start, start + size);
        Ok(FieldLayout { align })
    }

    /// Compute the layout for a nested `TypeDef:Struct` field.
    ///
    /// Probes the nested struct's layout at a temporary offset of 0 to
    /// determine its total size and alignment, aligns the parent offset,
    /// then shifts the nested fields to their final positions.
    fn compute_struct_field(
        &mut self,
        field_schema: &Value,
        field_path: &str,
        struct_default_align: usize,
    ) -> Result<FieldLayout, TypedefError> {
        let inner_parent_align = parse_align(field_schema).unwrap_or(struct_default_align);
        let mut probe = ComputeCtx {
            root: self.root,
            fields: Vec::new(),
            offset: 0,
        };
        let (inner_total, inner_align) =
            probe.compute_struct(field_schema, field_path, inner_parent_align)?;

        let align = field_alignment(field_schema, struct_default_align, inner_align);
        align_up(&mut self.offset, align);
        let struct_start = self.offset;
        for (path, range) in probe.fields {
            self.fields.push((
                path,
                ByteRange {
                    start: struct_start + range.start,
                    end: struct_start + range.end,
                },
            ));
        }
        self.offset = struct_start + inner_total;
        Ok(FieldLayout { align })
    }

    /// Compute the layout for a `TypeDef:Union` field.
    fn compute_union_field(
        &mut self,
        field_schema: &Value,
        field_path: &str,
        struct_default_align: usize,
    ) -> Result<FieldLayout, TypedefError> {
        let disc = parse_discriminator(field_schema)?;
        let mapping = field_schema
            .as_object()
            .and_then(|o| o.get("mapping"))
            .and_then(|v| v.as_object())
            .ok_or_else(|| TypedefError::Offset {
                field_path: field_path.to_string(),
                reason: "TUnion is missing 'mapping' object".to_string(),
            })?;
        let mapping = mapping.clone();

        let union_align_annotation = parse_align(field_schema);
        let union_default_align = union_align_annotation.unwrap_or(struct_default_align);

        match disc {
            DiscriminatorKind::Byte {
                offset: disc_off,
                disc_type,
            } => {
                let disc_size = type_size(&disc_type).ok_or_else(|| TypedefError::Offset {
                    field_path: field_path.to_string(),
                    reason: format!("discriminator type {disc_type} has no fixed size"),
                })?;
                let disc_natural = natural_alignment(&disc_type);
                let disc_align = union_default_align.max(disc_natural);
                align_up(&mut self.offset, disc_align);
                let union_start = self.offset;

                let disc_range_start = union_start + disc_off;
                let disc_range_end = disc_range_start + disc_size;
                let disc_path = format!("{field_path}.{DISCRIMINATOR_PATH}");
                self.push(&disc_path, disc_range_start, disc_range_end);

                let (variant_max_size, variant_max_align) =
                    self.variant_size_range(field_path, &mapping)?;
                let union_align = union_align_annotation
                    .unwrap_or(variant_max_align.max(disc_align))
                    .max(1);
                let union_total = round_up(disc_off + disc_size + variant_max_size, union_align);
                self.offset = union_start + union_total;
                Ok(FieldLayout { align: union_align })
            }
            DiscriminatorKind::Field { name } => {
                let (variant_max_size, variant_max_align) =
                    self.variant_size_range(field_path, &mapping)?;
                let disc_field_range =
                    self.find_discriminator_field(&mapping, &name, field_path)?;

                let union_align = union_align_annotation
                    .unwrap_or(variant_max_align.max(union_default_align))
                    .max(1);
                align_up(&mut self.offset, union_align);
                let union_start = self.offset;
                if let Some(r) = disc_field_range {
                    let disc_path = format!("{field_path}.{name}");
                    self.push(&disc_path, union_start + r.start, union_start + r.end);
                }
                let union_total = round_up(variant_max_size, union_align);
                self.offset = union_start + union_total;
                Ok(FieldLayout { align: union_align })
            }
        }
    }

    /// Iterate the union's `mapping` and compute `(max_variant_size, max_variant_align)`.
    fn variant_size_range(
        &self,
        field_path: &str,
        mapping: &serde_json::Map<String, Value>,
    ) -> Result<(usize, usize), TypedefError> {
        let mut variant_max_size: usize = 0;
        let mut variant_max_align: usize = 1;
        for (_key, variant_schema) in mapping.iter() {
            let resolved = resolve_ref_or_inline(variant_schema, self.root).ok_or_else(|| {
                TypedefError::Offset {
                    field_path: field_path.to_string(),
                    reason: "could not resolve TUnion variant schema ($ref not found)".to_string(),
                }
            })?;
            let v_kind = get_typedef_kind(resolved).ok_or_else(|| TypedefError::Offset {
                field_path: field_path.to_string(),
                reason: "TUnion variant schema has no TypeDef:* kind".to_string(),
            })?;
            if v_kind != "TypeDef:Struct" {
                return Err(TypedefError::Offset {
                    field_path: field_path.to_string(),
                    reason: format!("TUnion variant must be TypeDef:Struct, got {v_kind}"),
                });
            }
            let v_align_ann = parse_align(resolved).unwrap_or(1);
            let mut probe = ComputeCtx {
                root: self.root,
                fields: Vec::new(),
                offset: 0,
            };
            let (v_total, v_effective_align) = probe.compute_struct(resolved, "", v_align_ann)?;
            if v_total > variant_max_size {
                variant_max_size = v_total;
            }
            if v_effective_align > variant_max_align {
                variant_max_align = v_effective_align;
            }
        }
        Ok((variant_max_size, variant_max_align))
    }

    /// Find the discriminator field's offset within the first variant that contains it.
    fn find_discriminator_field(
        &self,
        mapping: &serde_json::Map<String, Value>,
        disc_name: &str,
        field_path: &str,
    ) -> Result<Option<ByteRange>, TypedefError> {
        for (_key, variant_schema) in mapping.iter() {
            let Some(resolved) = resolve_ref_or_inline(variant_schema, self.root) else {
                continue;
            };
            let v_kind = get_typedef_kind(resolved).ok_or_else(|| TypedefError::Offset {
                field_path: field_path.to_string(),
                reason: "TUnion variant schema has no TypeDef:* kind".to_string(),
            })?;
            if v_kind != "TypeDef:Struct" {
                return Err(TypedefError::Offset {
                    field_path: field_path.to_string(),
                    reason: format!("TUnion variant must be TypeDef:Struct, got {v_kind}"),
                });
            }
            let v_align_ann = parse_align(resolved).unwrap_or(1);
            let mut probe = ComputeCtx {
                root: self.root,
                fields: Vec::new(),
                offset: 0,
            };
            probe.compute_struct(resolved, "", v_align_ann)?;
            if let Some(r) = probe
                .fields
                .iter()
                .find(|(p, _)| p == disc_name)
                .map(|(_, r)| *r)
            {
                return Ok(Some(r));
            }
        }
        Ok(None)
    }

    /// Compute the layout for a `TypeDef:Array` field.
    fn compute_array_field(
        &mut self,
        field_schema: &Value,
        field_path: &str,
        struct_default_align: usize,
    ) -> Result<FieldLayout, TypedefError> {
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
        let elem_kind = get_typedef_kind(element_schema).ok_or_else(|| TypedefError::Offset {
            field_path: field_path.to_string(),
            reason: "TArray element schema has no TypeDef:* kind".to_string(),
        })?;
        if !is_fixed_kind(elem_kind) {
            return Err(TypedefError::Offset {
                field_path: field_path.to_string(),
                reason: format!(
                    "TArray of variable-length element kind {elem_kind} is not supported (OQ-069)"
                ),
            });
        }

        let elem_size = type_size(elem_kind).ok_or_else(|| TypedefError::Offset {
            field_path: field_path.to_string(),
            reason: format!("element kind {elem_kind} has no fixed size"),
        })?;
        let elem_natural = natural_alignment(elem_kind);
        let elem_align = field_alignment(element_schema, struct_default_align, elem_natural);
        let stride = round_up(elem_size, elem_align);

        let min_items = obj
            .get("minItems")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);
        let max_items = obj
            .get("maxItems")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);
        let fixed_count = match (min_items, max_items) {
            (Some(mn), Some(mx)) if mn == mx => Some(mn),
            _ => None,
        };

        let array_align = field_alignment(field_schema, struct_default_align, elem_align);

        if let Some(count) = fixed_count {
            align_up(&mut self.offset, array_align);
            let start = self.offset;
            for i in 0..count {
                let elem_start = start + i * stride;
                let elem_end = elem_start + elem_size;
                let elem_path = format!("{field_path}[{i}]");
                self.push(&elem_path, elem_start, elem_end);
            }
            let array_size = count * stride;
            self.offset = start + array_size;
            Ok(FieldLayout { align: array_align })
        } else {
            let count_prefix_align = array_align.max(4);
            align_up(&mut self.offset, count_prefix_align);
            let start = self.offset;
            self.push(field_path, start, start + 4);
            self.offset = start + 4;
            Ok(FieldLayout {
                align: count_prefix_align,
            })
        }
    }

    /// Compute the layout for a variable-length field (String/Bytes/Record/Timestamp).
    ///
    /// In aligned static mode, three strategies are supported:
    /// - `maxLength` reservation: `maxLength` bytes at a fixed offset.
    /// - `offset-indirect` encoding: an 8-byte `{offset: u32, length: u32}` pair.
    /// - inline length-prefixing (default): a 4-byte length prefix.
    fn compute_variable_field(
        &mut self,
        field_schema: &Value,
        field_path: &str,
        struct_default_align: usize,
    ) -> Result<FieldLayout, TypedefError> {
        let keyword_value = field_schema
            .as_object()
            .and_then(|o| {
                o.keys()
                    .find(|k| k.starts_with("TypeDef:"))
                    .and_then(|k| o.get(k))
            })
            .cloned()
            .unwrap_or(Value::Bool(true));
        let encoding = parse_encoding(&keyword_value);
        let max_length = parse_max_length(field_schema);

        let (size, natural) = match (max_length, encoding) {
            (Some(max_len), _) => (max_len, 1),
            (None, VariableEncoding::OffsetIndirect) => (8, 4),
            (None, VariableEncoding::LengthPrefixed) => (4, 4),
        };

        let align = field_alignment(field_schema, struct_default_align, natural);
        align_up(&mut self.offset, align);
        let start = self.offset;
        self.offset += size;
        self.push(field_path, start, start + size);
        Ok(FieldLayout { align })
    }

    /// Push a `(field_path, ByteRange)` pair onto the fields vec.
    fn push(&mut self, path: &str, start: usize, end: usize) {
        self.fields
            .push((path.to_string(), ByteRange { start, end }));
    }
}

/// Resolve the field's alignment: field-level `align` annotation,
/// then the struct default, then the natural alignment.
fn field_alignment(field_schema: &Value, struct_default_align: usize, natural: usize) -> usize {
    if let Some(a) = parse_align(field_schema) {
        return a.max(1);
    }
    struct_default_align.max(natural).max(1)
}

/// Round `offset` up to the next multiple of `align`. No-op if `align <= 1`.
fn align_up(offset: &mut usize, align: usize) {
    if align <= 1 {
        return;
    }
    let rem = *offset % align;
    if rem != 0 {
        *offset += align - rem;
    }
}

/// Round `n` up to the next multiple of `align`.
fn round_up(n: usize, align: usize) -> usize {
    if align <= 1 {
        return n;
    }
    let rem = n % align;
    if rem == 0 {
        n
    } else {
        n + align - rem
    }
}

/// Returns true if `kind` is one of the fixed-size primitive kinds.
fn is_fixed_kind(kind: &str) -> bool {
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

/// Detect a `TypeDef:*` kind from a schema node, accepting either the
/// boolean form (`{ "TypeDef:String": true }`) or the object-annotation
/// form (`{ "TypeDef:String": { "encoding": "..." } }`).
///
/// [`crate::schema::get_typedef_kind`] only recognizes the boolean form;
/// offset computation also needs to recognize the object form so that
/// variable-length encoding annotations are honored.
fn typedef_kind_loose(node: &Value) -> Option<&str> {
    let obj = node.as_object()?;
    for key in obj.keys() {
        if key.starts_with("TypeDef:") && obj.get(key).is_some_and(|v| !v.is_null()) {
            return Some(key.as_str());
        }
    }
    None
}

/// Resolve a `$ref` against the root schema, or return the inline schema.
///
/// If `node` has a `"$ref"` key, parse the JSON Pointer and walk `root`.
/// Otherwise, return `node` itself (it's an inline schema).
fn resolve_ref_or_inline<'a>(node: &'a Value, root: &'a Value) -> Option<&'a Value> {
    let obj = node.as_object()?;
    if let Some(Value::String(ref_path)) = obj.get("$ref") {
        return resolve_ref(root, ref_path);
    }
    Some(node)
}

/// Resolve a JSON Pointer `$ref` (e.g., `"#/$defs/Read"`) against `root`.
fn resolve_ref<'a>(root: &'a Value, ref_path: &str) -> Option<&'a Value> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn map(schema: &Value) -> OffsetMap {
        OffsetMap::compute(schema).expect("offset map computation")
    }

    #[test]
    fn simple_fixed_fields_natural_alignment() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "flag": { "TypeDef:Uint8": true },
                "id": { "TypeDef:Uint32": true }
            }
        });
        let m = map(&schema);
        assert_eq!(m.get("flag"), Some(&ByteRange { start: 0, end: 1 }));
        assert_eq!(m.get("id"), Some(&ByteRange { start: 4, end: 8 }));
        assert_eq!(m.total_size(), 8);
    }

    #[test]
    fn u8_then_u32_three_bytes_padding() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "a": { "TypeDef:Uint8": true },
                "b": { "TypeDef:Uint32": true }
            }
        });
        let m = map(&schema);
        assert_eq!(m.get("a"), Some(&ByteRange { start: 0, end: 1 }));
        assert_eq!(m.get("b"), Some(&ByteRange { start: 4, end: 8 }));
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
        let m = map(&schema);
        assert_eq!(m.get("header.magic"), Some(&ByteRange { start: 0, end: 4 }));
        assert_eq!(
            m.get("header.version"),
            Some(&ByteRange { start: 4, end: 5 })
        );
        assert_eq!(m.get("body"), Some(&ByteRange { start: 8, end: 12 }));
        assert_eq!(m.total_size(), 12);
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
        let m = map(&schema);
        assert_eq!(m.get("vals[0]"), Some(&ByteRange { start: 0, end: 4 }));
        assert_eq!(m.get("vals[1]"), Some(&ByteRange { start: 4, end: 8 }));
        assert_eq!(m.get("vals[2]"), Some(&ByteRange { start: 8, end: 12 }));
        assert_eq!(m.total_size(), 12);
    }

    #[test]
    fn array_variable_count_length_prefix() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "vals": {
                    "TypeDef:Array": true,
                    "items": { "TypeDef:Uint32": true }
                }
            }
        });
        let m = map(&schema);
        assert_eq!(m.get("vals"), Some(&ByteRange { start: 0, end: 4 }));
        assert_eq!(m.total_size(), 4);
    }

    #[test]
    fn variable_string_length_prefix_at_known_offset() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "id": { "TypeDef:Uint32": true },
                "name": { "TypeDef:String": true }
            }
        });
        let m = map(&schema);
        assert_eq!(m.get("id"), Some(&ByteRange { start: 0, end: 4 }));
        assert_eq!(m.get("name"), Some(&ByteRange { start: 4, end: 8 }));
        assert_eq!(m.total_size(), 8);
    }

    #[test]
    fn variable_string_max_length_reservation() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "id": { "TypeDef:Uint32": true },
                "name": { "TypeDef:String": true, "maxLength": 256 }
            }
        });
        let m = map(&schema);
        assert_eq!(m.get("id"), Some(&ByteRange { start: 0, end: 4 }));
        assert_eq!(m.get("name"), Some(&ByteRange { start: 4, end: 260 }));
        assert_eq!(m.total_size(), 260);
    }

    #[test]
    fn variable_string_offset_indirect_eight_bytes() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "id": { "TypeDef:Uint32": true },
                "blob": { "TypeDef:String": { "encoding": "offset-indirect" } }
            }
        });
        let m = map(&schema);
        assert_eq!(m.get("id"), Some(&ByteRange { start: 0, end: 4 }));
        assert_eq!(m.get("blob"), Some(&ByteRange { start: 4, end: 12 }));
        assert_eq!(m.total_size(), 12);
    }

    #[test]
    fn union_byte_discriminator() {
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
        let m = map(&schema);
        assert_eq!(
            m.get("payload.__discriminator"),
            Some(&ByteRange { start: 0, end: 1 })
        );
        assert_eq!(m.total_size(), 16);
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
        let m = map(&schema);
        assert_eq!(m.get("event.type"), Some(&ByteRange { start: 0, end: 1 }));
        assert_eq!(m.total_size(), 12);
    }

    #[test]
    fn struct_level_align_rounds_up_total() {
        let schema = json!({
            "TypeDef:Struct": true,
            "align": 16,
            "properties": {
                "flag": { "TypeDef:Uint8": true }
            }
        });
        let m = map(&schema);
        assert_eq!(m.get("flag"), Some(&ByteRange { start: 0, end: 1 }));
        assert_eq!(m.total_size(), 16);
    }

    #[test]
    fn field_level_align_overrides_struct_default() {
        let schema = json!({
            "TypeDef:Struct": true,
            "align": 1,
            "properties": {
                "tag": { "TypeDef:Uint8": true },
                "flag": { "TypeDef:Uint8": true, "align": 16 },
                "id": { "TypeDef:Uint32": true }
            }
        });
        let m = map(&schema);
        assert_eq!(m.get("tag"), Some(&ByteRange { start: 0, end: 1 }));
        assert_eq!(m.get("flag"), Some(&ByteRange { start: 16, end: 17 }));
        assert_eq!(m.get("id"), Some(&ByteRange { start: 20, end: 24 }));
        assert_eq!(m.total_size(), 24);
    }

    #[test]
    fn field_align_smaller_than_struct_default() {
        let schema = json!({
            "TypeDef:Struct": true,
            "align": 8,
            "properties": {
                "a": { "TypeDef:Uint8": true },
                "b": { "TypeDef:Uint32": true, "align": 1 }
            }
        });
        let m = map(&schema);
        assert_eq!(m.get("a"), Some(&ByteRange { start: 0, end: 1 }));
        assert_eq!(m.get("b"), Some(&ByteRange { start: 1, end: 5 }));
        assert_eq!(m.total_size(), 8);
    }

    #[test]
    fn iter_returns_all_paths_in_order() {
        let schema = json!({
            "TypeDef:Struct": true,
            "properties": {
                "a": { "TypeDef:Uint8": true },
                "b": { "TypeDef:Uint32": true }
            }
        });
        let m = map(&schema);
        let paths: Vec<&String> = m.iter().map(|(p, _)| p).collect();
        assert_eq!(paths, vec!["a", "b"]);
    }

    #[test]
    fn compute_rejects_non_struct_top_level() {
        let schema =
            json!({ "TypeDef:Union": true, "discriminator": { "kind": "byte" }, "mapping": {} });
        let err = OffsetMap::compute(&schema).unwrap_err();
        assert!(matches!(err, TypedefError::Schema(_)));
    }

    #[test]
    fn compute_rejects_missing_typedef_kind() {
        let schema = json!({ "type": "object", "properties": {} });
        let err = OffsetMap::compute(&schema).unwrap_err();
        assert!(matches!(err, TypedefError::Schema(_)));
    }

    #[test]
    fn byte_range_len_and_is_empty() {
        let r = ByteRange { start: 4, end: 8 };
        assert_eq!(r.len(), 4);
        assert!(!r.is_empty());
        let empty = ByteRange { start: 5, end: 5 };
        assert_eq!(empty.len(), 0);
        assert!(empty.is_empty());
    }
}
