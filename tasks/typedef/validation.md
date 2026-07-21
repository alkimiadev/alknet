---
id: typedef/validation
name: Implement custom keyword validators for all 17 TypeDef kinds via jsonschema with_keyword API
status: completed
depends_on: [typedef/schema-types, typedef/error-type]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Implement custom keyword validators for all 17 `TypeDef:*` kinds in
`crates/alknet-typedef/src/validation.rs`. Each kind gets a `Keyword` implementation
registered via `jsonschema::options().with_keyword(...)`. The validators check leaf
type constraints; `jsonschema` handles all structural validation.

Per [validation.md](../../docs/architecture/crates/typedef/validation.md) and
[ADR-098](../../docs/architecture/decisions/098-error-handling-validation-strategy.md).

### Target shape

```rust
use jsonschema::{Keyword, ValidationError};
use serde_json::Value;

/// Build a jsonschema validator with all 17 TypeDef:* custom keywords registered.
/// The returned validator can validate JSON representations of data against
/// the schema's type constraints.
pub fn build_validator(schema: &Value) -> Result<jsonschema::Validator, TypedefError>;
```

### Custom keyword validators

Each `TypeDef:*` kind gets a struct implementing `Keyword`:

**Numeric validators:**

- `Float32Validator` / `Float64Validator` — value must be a finite number.
  For `Float32`: value must be representable as `f32` (no precision loss beyond
  `f32`'s mantissa).
- `Int8Validator` / `Int16Validator` / `Int32Validator` — value must be an integer
  within the type's range. Int8: -128..127, Int16: -32768..32767, Int32: -2147483648..2147483647.
- `Uint8Validator` / `Uint16Validator` / `Uint32Validator` — value must be a
  non-negative integer within the type's range. Uint8: 0..255, Uint16: 0..65535,
  Uint32: 0..4294967295.

**String and binary validators:**

- `StringValidator` — value must be a valid UTF-8 string. If `maxLength` is
  specified in the parent schema, the string's byte length must not exceed it.
- `BytesValidator` — value must be a string (JSON represents binary data as a
  string). If `maxLength` is specified, the byte length must not exceed it.
- `EnumValidator` — the `TypeDef:Enum` custom keyword signals that the type is an
  enum for *layout* purposes. The built-in `enum` keyword handles value-membership
  validation. The custom keyword validator is a no-op beyond the built-in check —
  it exists solely for the layout engine to recognize the type.
- `TimestampValidator` — value must be a valid RFC 3339 timestamp string (the
  internet profile of ISO 8601, e.g., `"2026-07-20T15:30:00Z"`).

**Composite validators:**

- `StructValidator` — value must be an object. Each property must match its
  declared `TypeDef:*` kind. The `jsonschema` crate's built-in `properties` and
  `required` keywords handle structural checks — the custom keyword only validates
  that each field's value matches its `TypeDef:*` kind.
- `UnionValidator` — the discriminator value must be one of the mapping keys.
  The variant struct must match the declared schema for that discriminator value.
- `ArrayValidator` — value must be an array. Each element must match the array's
  declared element type. If `minItems`/`maxItems` is specified, the array length
  must be within bounds.

**Other validators:**

- `BooleanValidator` — value must be `true` or `false`.
- `RecordValidator` — value must be an object. All values must match the record's
  declared value type (specified via the `"values"` property).

### Validator implementation pattern

Each validator is ~10 lines. Example:

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

### Registration

```rust
pub fn build_validator(schema: &Value) -> Result<jsonschema::Validator, TypedefError> {
    jsonschema::options()
        .with_keyword("TypeDef:Float32", |_parent, _value, _path| {
            Ok(Box::new(Float32Validator))
        })
        .with_keyword("TypeDef:Float64", |_parent, _value, _path| {
            Ok(Box::new(Float64Validator))
        })
        .with_keyword("TypeDef:Int8", |_parent, _value, _path| {
            Ok(Box::new(Int8Validator))
        })
        // ... all 17 kinds
        .with_keyword("TypeDef:Struct", |parent, _value, _path| {
            Ok(Box::new(StructValidator::from_schema(parent)?))
        })
        .build(schema)
        .map_err(|e| TypedefError::Schema(format!("validator build failed: {e}")))
}
```

The factory closure receives the parent schema object, the keyword's value, and the
schema path. This enables cross-keyword awareness — for example, `StructValidator`
inspects the parent's `properties` to validate each field against its declared
`TypeDef:*` kind.

### What this does NOT include

- The `TypedefEngine` struct (that's `engine.rs`) — though `build_validator()` is
  called by the engine
- Schema parsing or annotation extraction (that's `schema.rs`)
- Binary-level validation (that's in `data_access.rs` — bounds checking, UTF-8
  validation at read time)

## Acceptance Criteria

- [ ] `build_validator(schema)` returns a `jsonschema::Validator` with all 17 custom keywords registered
- [ ] `Float32Validator` / `Float64Validator`: rejects non-finite numbers
- [ ] `Int8Validator` / `Int16Validator` / `Int32Validator`: rejects out-of-range values
- [ ] `Uint8Validator` / `Uint16Validator` / `Uint32Validator`: rejects negative and out-of-range values
- [ ] `StringValidator`: rejects non-string values; respects `maxLength` from parent schema
- [ ] `BytesValidator`: rejects non-string values; respects `maxLength`
- [ ] `EnumValidator`: no-op (built-in `enum` keyword handles validation)
- [ ] `TimestampValidator`: rejects non-RFC 3339 strings
- [ ] `StructValidator`: validates each property against its declared `TypeDef:*` kind
- [ ] `UnionValidator`: validates discriminator membership and variant conformance
- [ ] `ArrayValidator`: validates element type conformance and length bounds
- [ ] `BooleanValidator`: rejects non-boolean values
- [ ] `RecordValidator`: validates all values match declared value type
- [ ] Each validator is ~10 lines (not hundreds)
- [ ] Factory closures handle errors gracefully (return `TypedefError::Schema`)
- [ ] No `unwrap()` or `expect()` in factory closures
- [ ] All validators have doc comments
- [ ] `cargo check -p alknet-typedef` succeeds
- [ ] `cargo clippy -p alknet-typedef` succeeds with no warnings
- [ ] `cargo build --workspace` still succeeds

## References

- docs/architecture/crates/typedef/validation.md — custom keyword validators, TypedefError, validation timing
- docs/architecture/crates/typedef/schema-layer.md — the 17 TypeDef kinds
- docs/architecture/decisions/098-error-handling-validation-strategy.md — ADR-098
- docs/research/alknet-typedef/findings.md — POC validation results
- /workspace/alknet-typedef-poc/src/validate.rs — POC reference for validators
- /workspace/jsonschema/ — the jsonschema crate API

## Notes

> This module registers custom keyword validators with the `jsonschema` crate. Each
> validator is small (~10 lines) because `jsonschema` handles all structural
> validation. The `StructValidator` is the most complex — it needs to inspect the
> parent schema's `properties` to validate each field. The `EnumValidator` is a
> no-op because the built-in `enum` keyword already handles value-membership
> validation; the custom keyword exists solely for the layout engine to recognize
> the type as a fixed-size u32 index. The POC's `validate.rs` is a good reference.

## Summary

Implemented `build_validator(schema) -> Result<jsonschema::Validator, TypedefError>` in `crates/alknet-typedef/src/validation.rs`, registering all 17 `TypeDef:*` custom keywords with the `jsonschema` crate via `options().with_keyword(...)`. Each kind has a small (~10 line) `Keyword` struct: numeric validators (`Float32/64`, `Int8/16/32`, `Uint8/16/32`) check finiteness and range bounds; `String`/`Bytes` verify UTF-8 string shape and enforce `maxLength` from the parent schema; `Timestamp` checks RFC 3339 / ISO 8601 structure; `Boolean` rejects non-booleans; composite `Struct`/`Union`/`Array`/`Record` assert container shape and delegate structural checks to jsonschema's built-in keywords; and `Enum` is a deliberate no-op (the built-in `enum` keyword handles value-membership — the custom keyword exists solely as a layout-engine marker). Factory closures reject schemas where a `TypeDef:*` keyword is not set to `true` via `ValidationError::schema`, and `.build()` errors are mapped to `TypedefError::Schema`. All 16 unit tests pass, `cargo check`, `cargo clippy -D warnings`, and `cargo build --workspace` succeed cleanly.
