---
status: draft
last_updated: 2026-07-22
---

# alknet-typedef — Validation

The validation layer: custom keyword validators for all 19 `TypeDef:*`
kinds, the `TypedefError` enum, load-time vs access-time validation
strategy, and the `TypedefEngine` as the compiled form of a schema.

## Validation Strategy

Validation is delegated to the `jsonschema` crate (v0.46.5, Draft
2020-12). The typedef engine does not implement its own validation —
it registers custom keyword validators for each `TypeDef:*` kind and
lets `jsonschema` handle the structural validation (object properties,
required fields, array items, enum values).

The strategy is decided in [ADR-098](../../decisions/098-error-handling-validation-strategy.md):

1. **Load time:** Parse the schema JSON, build the layout engine, build the
   jsonschema validator. This is the `TypedefEngine::compile(schema)` constructor.
2. **Access time:** Use the compiled engine for repeated read/write
   operations. Validation is opt-in per operation.

### What validation validates

The jsonschema validator operates on `serde_json::Value` instances — it
validates JSON representations of data, not raw byte buffers. This is
the correct separation of concerns:

- **JSON validation** (jsonschema): validates that a JSON document
  conforms to the schema. Used for validating hand-written schemas,
  TypeBox output, JSON payloads, or the JSON representation of a binary
  struct after deserialization.
- **Binary access validation** (data access layer): the read/write
  functions perform type-level validation at access time — range checks
  for integers, UTF-8 validity for strings, buffer bounds checking.
  These return `TypedefError::Access` with field paths.

The "schema is the format" principle means the same schema describes
both the JSON shape and the binary layout. The jsonschema validator
checks the JSON shape; the data access layer checks the binary layout.
A consumer that wants to validate a binary buffer end-to-end reads the
buffer into a `Value` tree via the data access layer, then validates
that `Value` against the jsonschema validator. This is a two-step
process, not a single `validate(buffer)` call.

### The `TypedefEngine` struct

The `TypedefEngine` is the compiled form of a schema. It supports both
layout modes (ADR-096) via an internal `Layout` enum:

```rust
pub struct TypedefEngine {
    layout: Layout,                   // packed or aligned (private enum)
    validator: jsonschema::Validator, // compiled once at load time
    endian: Endian,                   // parsed from the schema's "endian" annotation
    schema: Value,                    // the normalized schema (refs resolved)
}

// Private — the consumer selects via LayoutMode at compile time.
enum Layout {
    Packed { builder: LayoutBuilder },
    Aligned { offset_map: OffsetMap },
}
```

The consumer selects the mode at construction time via `LayoutMode`
(see [layout-engine.md](layout-engine.md) §"Mode Selection"). The `Layout`
enum is private — the engine exposes mode-appropriate accessors instead:

```rust
impl TypedefEngine {
    pub fn compile(schema: &mut Value, mode: LayoutMode) -> Result<Self, TypedefError>;
    pub fn mode(&self) -> LayoutMode;
    pub fn endian(&self) -> Endian;
    pub fn offset_map(&self) -> Option<&OffsetMap>;          // Some in aligned mode
    pub fn layout_builder(&self) -> Option<&LayoutBuilder>;  // Some in packed mode
    pub fn sequential_reader(&self) -> Option<SequentialReader>; // owned fresh reader (ADR-101)
}
```

`compile` takes `&mut Value` because it normalizes `$ref` values in place
(via [`normalize_refs`](schema-layer.md#ref-resolution-and-normalization))
before computing the layout and building the validator. The `schema`
field retains the normalized schema for `read_field`'s kind lookup and
for `sequential_reader()`'s factory construction. The validator is
mode-agnostic (it operates on `Value`, not raw bytes).

The `Layout::Packed` variant stores only the `LayoutBuilder` (write-side).
The `SequentialReader` (read-side) is not stored — it has mutable cursor
state that the consumer owns, so `sequential_reader()` constructs a fresh
reader on each call (ADR-101).

The `read_field`/`write_field` methods on `TypedefEngine` are the
aligned-mode data-access API — see [data-access.md](data-access.md)
§"Higher-level read/write".

## Custom Keyword Validators

Each `TypeDef:*` kind gets a `Keyword` implementation registered via
`jsonschema::options().with_keyword(...)`. The validators check leaf
type constraints; `jsonschema` handles all structural validation.

### Numeric type validators

**`TypeDef:Float32` / `TypeDef:Float64`:**
- Value must be a finite number.
- For `Float32`: value must be representable as `f32` (no precision loss
  beyond `f32`'s mantissa).

**`TypeDef:Int8` / `TypeDef:Int16` / `TypeDef:Int32`:**
- Value must be an integer within the type's range.
- Int8: -128..127, Int16: -32768..32767, Int32: -2147483648..2147483647.

**`TypeDef:Uint8` / `TypeDef:Uint16` / `TypeDef:Uint32`:**
- Value must be a non-negative integer within the type's range.
- Uint8: 0..255, Uint16: 0..65535, Uint32: 0..4294967295.

### String and binary validators

**`TypeDef:String`:**
- Value must be a valid UTF-8 string.
- If `maxLength` is specified in the schema, the string's byte length
  must not exceed it.

**`TypeDef:Bytes`:**
- Value must be a string (JSON represents binary data as a string — JSON
  has no native byte type).
- If `maxLength` is specified, the byte length must not exceed it.
- **Binary representation:** In the binary layout, `TBytes` is raw bytes
  with no encoding (not base64, not hex). The JSON representation (for
  validation) uses a string; the binary representation (for data access)
  uses `&[u8]` directly.

**`TypeDef:Enum`:**
- The `TypeDef:Enum` custom keyword signals that the type is an enum for
  *layout* purposes (the engine needs to know it's a fixed-size u32 index,
  not a variable-length string). The built-in `enum` keyword provides the
  value list and handles value-membership validation. The custom keyword
  validator is a no-op beyond the built-in check — it exists solely for
  the layout engine to recognize the type.

**`TypeDef:Timestamp`:**
- Value must be a valid RFC 3339 timestamp string (the internet profile
  of ISO 8601, e.g., `"2026-07-20T15:30:00Z"`).

### Composite type validators

**`TypeDef:Struct`:**
- Value must be an object.
- Each property must match its declared `TypeDef:*` kind.
- Required fields must be present.
- The `jsonschema` crate's built-in `properties` and `required` keywords
  handle the structural checks — the custom keyword only needs to
  validate that each field's value matches its `TypeDef:*` kind.

**`TypeDef:Union`:**
- The discriminator value must be one of the mapping keys.
- The variant struct must match the declared schema for that discriminator
  value.

**`TypeDef:Array`:**
- Value must be an array.
- Each element must match the array's declared element type.
- If `minItems`/`maxItems` is specified, the array length must be within
  bounds.

### Other validators

**`TypeDef:Boolean`:**
- Value must be `true` or `false`.

**`TypeDef:Record`:**
- Value must be an object.
- All values must match the record's declared value type (specified via
  the `"values"` property in the schema, e.g.,
  `"values": { "TypeDef:Float32": true }`).

### Validator implementation pattern

Each custom keyword implementation is ~10 lines. Example for
`TypeDef:Float32`:

```rust
struct Float32Validator;

impl Keyword for Float32Validator {
    fn validate<'i>(&self, instance: &'i Value) -> Result<(), ValidationError<'i>> {
        match instance {
            Value::Number(n) if n.as_f64().map_or(false, |f| f.is_finite()) => Ok(()),
            _ => Err(ValidationError::custom("expected finite f32-compatible number")),
        }
    }
    fn is_valid(&self, instance: &Value) -> bool {
        instance.as_f64().map_or(false, |f| f.is_finite())
    }
}
```

Registration:

```rust
let validator = jsonschema::options()
    .with_keyword("TypeDef:Float32", |parent, value, path| {
        Ok(Box::new(Float32Validator))
    })
    .build(&schema)?;
```

The factory closure receives the parent schema object, the keyword's
value, and the schema path. This enables cross-keyword awareness — for
example, a `TypeDef:Struct` validator can inspect the parent's
`properties` to validate each field against its declared `TypeDef:*` kind.

## TypedefError

A single `TypedefError` enum covers all error conditions across the
engine's three phases (schema parsing, offset computation, read/write)
plus validation. Decided in [ADR-098](../../decisions/098-error-handling-validation-strategy.md).

```rust
pub enum TypedefError {
    /// Schema parsing errors (invalid JSON, missing keywords, unknown TypeDef kinds).
    Schema(String),
    /// Offset computation errors (field not found, unsupported type).
    Offset { field_path: String, reason: String },
    /// Read/write errors (buffer too short, invalid UTF-8, value out of range).
    Access { field_path: String, reason: String },
    /// Validation errors (delegated to jsonschema).
    Validation(ValidationError<'static>),
}
```

- **`Schema`** — for errors during `TypedefEngine::compile()`. Invalid
  JSON, missing required keywords, unknown `TypeDef:*` kinds.
- **`Offset`** — for errors during offset computation. Field not found
  in the schema, type not supported for offset computation, recursive
  depth exceeded. Carries the field path.
- **`Access`** — for errors during read/write. Buffer too short, invalid
  UTF-8 in a string field, value out of range for the target type.
  Carries the field path.
- **`Validation`** — wraps `jsonschema`'s `ValidationError`. The
  `'static` lifetime is correct — the validator owns its schema reference
  and lives for the lifetime of the `TypedefEngine`.

### Field-path-carrying errors

Read/write and offset errors include the field path for debugging:

```rust
Err(TypedefError::Access {
    field_path: "header.version".to_string(),
    reason: "buffer too short: need 4 bytes at offset 12, have 2".to_string(),
})
```

This makes debugging binary format issues tractable — the error tells
you exactly which field failed and why.

## Validation Timing

### Load time: `TypedefEngine::compile()`

The expensive work happens once at schema load time:
1. Normalize `$ref` values in the schema (`normalize_refs`).
2. Parse the schema's `"endian"` annotation.
3. Compute the layout (`LayoutBuilder`/`SequentialReader` for packed, `OffsetMap` for aligned).
4. Build the jsonschema validator (`jsonschema::options().with_keyword(...).build(&schema)?`).

The result is a `TypedefEngine` that can be used for repeated operations.

### Access time: `engine.validate_json(&Value)` / `engine.is_valid_json(&Value)`

Validation is opt-in per operation. The consumer calls
`engine.validate_json(instance)` when validation is desired, or
`engine.is_valid_json(instance)` for a boolean check. The jsonschema
validator is already compiled — these are fast checks against the
compiled validator.

```rust
pub fn validate_json(&self, instance: &Value) -> Result<(), TypedefError>;
pub fn is_valid_json(&self, instance: &Value) -> bool;
```

The argument is a `serde_json::Value` (the JSON representation of the
data), not a raw byte buffer — see §"What validation validates" above.
To validate a binary buffer end-to-end, the consumer reads it into a
`Value` tree via the data access layer, then validates that `Value`.

High-throughput paths can skip validation. Security-sensitive paths
(parsing incoming frames from untrusted peers) can validate every frame.
The choice is the consumer's.

## Relationship to Read/Write

Validation and data access are independent operations on the same data.
The consumer can:

1. Validate the JSON representation of a buffer to ensure it conforms to
   the schema.
2. Read fields from the binary buffer at computed offsets.
3. Both — validate the JSON representation first, then read the binary
   buffer (defense in depth).

The engine does not couple validation and access. A consumer that trusts
its data source can skip validation and go straight to read/write. A
consumer that parses untrusted input can validate the JSON
representation first, then access the binary buffer.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| Error handling and validation | [ADR-098](../../decisions/098-error-handling-validation-strategy.md) | `TypedefError` enum; load-time build, access-time check; field-path-carrying errors; jsonschema `ValidationError` wrapping |
| Purpose and scope | [ADR-095](../../decisions/095-alknet-typedef-purpose-scope-jsonschema-engine.md) | Why jsonschema not a custom engine |

## Open Questions

None specific to validation. The three typedef OQs (OQ-069, OQ-070,
OQ-071) are about layout, platform support, and schema construction —
not validation.

## References

- `docs/research/alknet-typedef/findings.md` §"Validation" — the POC's
  custom keyword validators for all 17 kinds
- [ADR-098](../../decisions/098-error-handling-validation-strategy.md) —
  error handling and validation strategy
- [schema-layer.md](schema-layer.md) — the 17 TypeDef kinds that the
  validators check
- [data-access.md](data-access.md) — read/write functions that operate
  on the same buffers
