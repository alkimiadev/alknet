# ADR-098: Error Handling and Validation Strategy

## Status
Accepted

## Context

The typedef engine operates in three phases, each with distinct error
conditions:

1. **Schema parsing** — invalid JSON, missing required keywords, unknown
   `TypeDef:*` kinds, malformed annotations.
2. **Offset computation** — field not found, type not supported for
   offset computation, recursive schema depth exceeded.
3. **Read/write** — buffer too short, invalid UTF-8, value out of range
   for the target type.
4. **Validation** — type constraint violations (range, UTF-8, field
   presence, discriminator membership).

The engine also needs a clear strategy for *when* validation happens:
once at schema load time (build the validator) vs repeatedly at access
time (validate each buffer).

## Decision

### Error type: `TypedefError`

A single `TypedefError` enum with variants for each error category:

```rust
pub enum TypedefError {
    /// Schema parsing errors.
    Schema(String),
    /// Offset computation errors.
    Offset { field_path: String, reason: String },
    /// Read/write errors.
    Access { field_path: String, reason: String },
    /// Validation errors (delegated to jsonschema).
    Validation(ValidationError<'static>),
}
```

- `Schema` — for invalid JSON, missing required keywords, unknown
  `TypeDef:*` kinds. The error message describes the problem.
- `Offset` — for field-not-found, unsupported type for offset
  computation, etc. Carries the field path for debugging.
- `Access` — for buffer-too-short, invalid UTF-8, value out of range.
  Carries the field path for debugging.
- `Validation` — wraps `jsonschema`'s `ValidationError`. The
  `jsonschema` crate already provides rich error messages with schema
  paths; the typedef engine does not re-wrap or re-interpret them.

The `Validation` variant uses `ValidationError<'static>` because the
validator is built once at schema load time and lives for the lifetime
of the `TypedefEngine`. The `'static` lifetime is correct — the validator
owns its schema reference.

### Validation timing: load-time build, access-time check

The jsonschema validator is built once at schema load time
(`validator_for(&schema)?`) and then called repeatedly
(`validator.is_valid(&instance)`). The typedef engine follows the same
pattern:

1. **Load time:** Parse the schema JSON, build the offset map (or
   `LayoutBuilder`/`SequentialReader`), build the jsonschema validator.
   This is the `TypedefEngine::compile(schema: &Value) -> Result<Self,
   TypedefError>` constructor.
2. **Access time:** Use the compiled engine for repeated read/write
   operations. Validation is opt-in per operation — the consumer calls
   `engine.validate(buffer)` when validation is desired.

The `TypedefEngine` struct is the compiled form of a schema:

```rust
pub struct TypedefEngine {
    offset_map: OffsetMap,           // or LayoutBuilder/SequentialReader
    validator: jsonschema::Validator, // compiled once at load time
}
```

### Custom keyword validators

Each `TypeDef:*` kind gets a `Keyword` implementation registered via
`jsonschema::options().with_keyword(...)`. The validators check:

- **Numeric types** (`TypeDef:Float32`, `TypeDef:Int8`, etc.): range
  constraints (Int8: -128..127, Uint8: 0..255, etc.), finiteness for
  floats.
- **`TypeDef:String`**: UTF-8 validity.
- **`TypeDef:Struct`**: field presence and types (delegated to
  jsonschema's structural validation — the custom keyword only needs to
  validate that the struct's fields match their declared `TypeDef:*`
  kinds).
- **`TypeDef:Union`**: discriminator value membership in the mapping.
- **`TypeDef:Array`**: element type conformance.
- **`TypeDef:Boolean`**: value is `true` or `false`.
- **`TypeDef:Timestamp`**: ISO 8601 string format.

The `jsonschema` crate handles all the structural validation (object
properties, required fields, array items, enum values) — the custom
keywords only need to validate the leaf type constraints. Each custom
keyword implementation is ~10 lines.

### Read/write errors carry field paths

Read/write errors include the field path for debugging:

```rust
// Example: reading a u32 from a buffer that's too short
Err(TypedefError::Access {
    field_path: "header.version".to_string(),
    reason: "buffer too short: need 4 bytes at offset 12, have 2".to_string(),
})
```

This makes debugging binary format issues tractable — the error tells
you exactly which field failed and why.

## Consequences

### Positive

- **Single error type.** Consumers handle one `TypedefError` enum, not
  multiple error types from different engine phases.
- **Field-path-carrying errors.** Read/write errors include the field
  path, making binary format debugging tractable.
- **Validation is opt-in.** The consumer decides when to validate.
  High-throughput paths can skip validation; security-sensitive paths
  can validate every frame.
- **jsonschema integration is clean.** The `ValidationError` is wrapped
  as-is — no re-interpretation, no information loss.
- **Load-time build, access-time use.** The expensive work (schema
  parsing, validator compilation, offset computation) happens once at
  load time. Access-time operations are cheap (pointer casts, slice
  operations, length-prefix reads).

### Negative

- **`ValidationError<'static>` lifetime.** The `'static` lifetime on the
  `Validation` variant means the error cannot borrow from the buffer
  being validated. This is correct (the validator owns its schema
  reference) but may surprise readers who expect a shorter lifetime.
- **No error recovery.** The engine does not attempt to recover from
  partial reads or writes. A buffer-too-short error on field N means
  fields N+1.. are also unreadable. This is inherent to binary formats
  — there is no "skip to next field" without a schema-driven parser.

## References

- `docs/research/alknet-typedef/findings.md` §"Open Questions" — error
  handling strategy question (OQ 8)
- [ADR-095](095-alknet-typedef-purpose-scope-jsonschema-engine.md) —
  purpose and scope
- [ADR-096](096-two-layout-modes-packed-vs-aligned.md) — the two layout
  modes
- [ADR-097](097-schema-annotations.md) — schema annotations
