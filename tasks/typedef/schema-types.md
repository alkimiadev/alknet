---
id: typedef/schema-types
name: Implement TypeDef kind detection, schema annotation parsing, $ref normalization, and Endian enum
status: completed
depends_on: [typedef/crate-init]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Implement the schema layer in `crates/alknet-typedef/src/schema.rs`. This module
provides the foundational types and functions that every other module depends on:
TypeDef kind detection, type size constants, schema annotation parsing, `$ref`
normalization, and the `Endian` enum.

Per [schema-layer.md](../../docs/architecture/crates/typedef/schema-layer.md) and
[ADR-097](../../docs/architecture/decisions/097-schema-annotations.md).

### TypeDef kind detection

The engine needs to identify which `TypeDef:*` kind a schema node declares. The
17 kinds (16 from TypeBox's `typedef.ts` + `TypeDef:Bytes` added by alknet-typedef):

| Kind | Keyword | Category | Byte size |
|------|---------|----------|-----------|
| Float32 | `TypeDef:Float32` | fixed | 4 |
| Float64 | `TypeDef:Float64` | fixed | 8 |
| Int8 | `TypeDef:Int8` | fixed | 1 |
| Int16 | `TypeDef:Int16` | fixed | 2 |
| Int32 | `TypeDef:Int32` | fixed | 4 |
| Uint8 | `TypeDef:Uint8` | fixed | 1 |
| Uint16 | `TypeDef:Uint16` | fixed | 2 |
| Uint32 | `TypeDef:Uint32` | fixed | 4 |
| Boolean | `TypeDef:Boolean` | fixed | 1 |
| Enum | `TypeDef:Enum` | fixed | 4 (u32 index) |
| String | `TypeDef:String` | variable | — |
| Bytes | `TypeDef:Bytes` | variable | — |
| Struct | `TypeDef:Struct` | composite | sum of fields |
| Union | `TypeDef:Union` | composite | discriminator + variant |
| Array | `TypeDef:Array` | composite | count × element |
| Record | `TypeDef:Record` | variable | — |
| Timestamp | `TypeDef:Timestamp` | variable | — |

Implement:

```rust
/// Returns the `TypeDef:*` kind string if the schema node declares one.
/// Returns `None` if the node has no `TypeDef:*` keyword.
pub fn get_typedef_kind(node: &serde_json::Value) -> Option<&str>;

/// Returns the fixed byte size for a TypeDef kind, or `None` if variable-size.
pub fn type_size(kind: &str) -> Option<usize>;

/// Returns the natural alignment for a TypeDef kind.
pub fn natural_alignment(kind: &str) -> usize;

/// Returns true if the kind is a fixed-size type.
pub fn is_fixed_size(kind: &str) -> bool;
```

### Endian enum

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endian {
    Little,
    Big,
}

impl Endian {
    /// Parse from the schema's "endian" annotation. Defaults to Little.
    pub fn from_schema(schema: &serde_json::Value) -> Self;
}
```

### Schema annotation parsing

Parse the annotations decided in ADR-097:

```rust
/// Parse the "encoding" annotation from a variable-length type's keyword value.
/// The keyword value may be `true` (shorthand for length-prefixed) or an object
/// with an "encoding" field.
pub fn parse_encoding(keyword_value: &serde_json::Value) -> VariableEncoding;

pub enum VariableEncoding {
    LengthPrefixed,
    OffsetIndirect,
}

/// Parse the "align" annotation from a schema node. Returns None if not specified.
pub fn parse_align(node: &serde_json::Value) -> Option<usize>;

/// Parse the "maxLength" annotation (standard JSON Schema keyword).
pub fn parse_max_length(node: &serde_json::Value) -> Option<usize>;

/// Parse the "endian" annotation. Defaults to Little if absent or unrecognized.
pub fn parse_endian(node: &serde_json::Value) -> Endian;
```

### TUnion discriminator parsing

Parse the discriminator shape from a `TypeDef:Union` schema node:

```rust
pub enum DiscriminatorKind {
    Byte { offset: usize, disc_type: String },
    Field { name: String },
}

/// Parse the "discriminator" annotation from a TUnion schema node.
pub fn parse_discriminator(node: &serde_json::Value) -> Result<DiscriminatorKind, TypedefError>;
```

### `$ref` normalization

TypeBox generates bare-name `$ref` values (e.g., `"$ref": "Read"`). The `jsonschema`
crate requires full JSON Pointer paths (e.g., `"$ref": "#/$defs/Read"`). Normalize
at schema load time:

```rust
/// Walk the schema tree. For every "$ref" whose value is a bare name
/// (no "#" prefix), rewrite it to "#/$defs/<name>".
/// Full JSON Pointer refs pass through unchanged. Idempotent.
pub fn normalize_refs(schema: &mut serde_json::Value);
```

This is a ~20-line recursive walk. It runs once at load time, before the schema is
passed to `jsonschema::validator_for` or the offset computation.

### What this does NOT include

- The actual offset computation (that's `offset_map.rs` and `layout_builder.rs`)
- The read/write functions (that's `data_access.rs`)
- The custom keyword validators (that's `validation.rs`)
- The `TypedefEngine` struct (that's `engine.rs`)

## Acceptance Criteria

- [ ] `get_typedef_kind()` correctly identifies all 17 `TypeDef:*` kinds
- [ ] `type_size()` returns correct byte sizes for all fixed-size types
- [ ] `type_size()` returns `None` for variable-length and composite types
- [ ] `natural_alignment()` returns correct alignment for each kind
- [ ] `Endian` enum with `Little` and `Big` variants
- [ ] `Endian::from_schema()` defaults to `Little` when annotation is absent
- [ ] `Endian::from_schema()` returns `Big` when `"endian": "big"`
- [ ] `parse_encoding()` handles both `true` (shorthand) and `{ "encoding": "..." }` (object)
- [ ] `parse_encoding()` defaults to `LengthPrefixed` when annotation is absent
- [ ] `parse_align()` returns `None` when no `"align"` annotation
- [ ] `parse_max_length()` returns `None` when no `"maxLength"` annotation
- [ ] `parse_discriminator()` handles byte-offset (`kind: "byte"`) and field-name (`kind: "field"`)
- [ ] `parse_discriminator()` returns `Err(TypedefError::Schema(...))` for malformed discriminators
- [ ] `normalize_refs()` rewrites `"$ref": "Read"` → `"$ref": "#/$defs/Read"`
- [ ] `normalize_refs()` leaves `"$ref": "#/$defs/Read"` unchanged (idempotent)
- [ ] `normalize_refs()` handles nested objects and arrays recursively
- [ ] All public functions have doc comments
- [ ] `cargo check -p alknet-typedef` succeeds
- [ ] `cargo clippy -p alknet-typedef` succeeds with no warnings
- [ ] `cargo build --workspace` still succeeds

## References

- docs/architecture/crates/typedef/schema-layer.md — the 17 TypeDef kinds, annotations, $ref normalization
- docs/architecture/decisions/097-schema-annotations.md — ADR-097 (concrete JSON shapes)
- docs/architecture/decisions/095-alknet-typedef-purpose-scope-jsonschema-engine.md — ADR-095
- docs/research/alknet-typedef/findings.md — POC results, open questions
- /workspace/alknet-typedef-poc/src/offset.rs — POC reference for kind detection

## Notes

> This is the foundational types module. Every other module depends on it for
> TypeDef kind identification, size lookups, and annotation parsing. The `$ref`
> normalization bridges TypeBox output to jsonschema input — without it, bare-name
> refs from TypeBox would fail to resolve. The `TEnum` kind uses a `u32` index
> (not a variable-length string) — a deliberate deviation from TypeBox fidelity
> for binary efficiency, per the schema-layer spec.

## Summary

Implemented the schema layer in `crates/alknet-typedef/src/schema.rs` with full
coverage of all acceptance criteria. The module exposes `get_typedef_kind`,
`type_size`, `natural_alignment`, and `is_fixed_size` for the 17 TypeDef kinds;
the `Endian` enum with `from_schema`/`parse_endian` (defaulting to Little);
the `VariableEncoding` enum with `parse_encoding` (shorthand `true` and object
form with `length-prefixed`/`offset-indirect`); `parse_align`/`parse_max_length`
JSON Schema keyword readers; `parse_discriminator` returning the
`DiscriminatorKind::Byte`/`Field` variants with `TypedefError::Schema` on
malformed input; and `normalize_refs` rewriting bare-name `$ref`s to
`#/$defs/<name>` recursively and idempotently. 31 unit tests pass, and
`cargo check`, `cargo clippy -D warnings`, and `cargo build --workspace` all
succeed clean. The `TypedefError` dependency was already provided by the
parallel `error.rs` implementation (no stub needed).
