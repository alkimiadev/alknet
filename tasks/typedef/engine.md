---
id: typedef/engine
name: Implement TypedefEngine struct combining layout and validation, with compile() constructor
status: pending
depends_on: [typedef/offset-map, typedef/layout-builder, typedef/sequential-reader, typedef/tunion, typedef/validation]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Implement the `TypedefEngine` struct in `crates/alknet-typedef/src/engine.rs`. This is
the compiled form of a schema — the main entry point for consumers. It combines the
layout engine (both modes) and the jsonschema validator into a single struct.

Per [validation.md](../../docs/architecture/crates/typedef/validation.md) §"The TypedefEngine struct"
and [overview.md](../../docs/architecture/crates/typedef/overview.md).

### Target shape

```rust
/// The compiled form of a typedef schema. Combines the layout engine
/// (both packed and aligned modes) and the jsonschema validator.
///
/// Built once at schema load time via [`TypedefEngine::compile()`].
/// Used for repeated read/write/validate operations at access time.
pub struct TypedefEngine {
    /// The layout strategy — packed sequential or aligned static.
    layout: Layout,
    /// The compiled jsonschema validator (built once at load time).
    validator: jsonschema::Validator,
    /// The schema's endianness.
    endian: Endian,
    /// The original schema (for reference, debugging, and TUnion dispatch).
    schema: serde_json::Value,
}

/// The layout strategy selected by the consumer.
enum Layout {
    /// Packed sequential layout for protocol wire formats.
    Packed {
        builder: LayoutBuilder,
        reader: SequentialReader,
    },
    /// Aligned static layout for mmap-friendly formats.
    Aligned {
        offset_map: OffsetMap,
    },
}

/// The layout mode selected at engine construction time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMode {
    /// Packed sequential — for protocol wire formats (SFTP, channels, TTY).
    Packed,
    /// Aligned static — for mmap-friendly formats (metatensor, safetensors).
    Aligned,
}

impl TypedefEngine {
    /// Compile a schema into a TypedefEngine.
    ///
    /// This is the expensive operation — it parses the schema, normalizes
    /// `$ref` values, computes the layout, and builds the jsonschema
    /// validator. Call once at load time; use the returned engine for
    /// repeated operations.
    ///
    /// The `mode` parameter selects the layout strategy. The same schema
    /// can be compiled in either mode.
    pub fn compile(
        schema: &mut serde_json::Value,
        mode: LayoutMode,
    ) -> Result<Self, TypedefError>;

    /// The schema's endianness.
    pub fn endian(&self) -> Endian;

    /// The layout mode this engine was compiled with.
    pub fn mode(&self) -> LayoutMode;

    /// Access the aligned offset map. Returns `None` if compiled in
    /// packed mode.
    pub fn offset_map(&self) -> Option<&OffsetMap>;

    /// Access the layout builder (write-side of packed mode).
    /// Returns `None` if compiled in aligned mode.
    pub fn layout_builder(&self) -> Option<&LayoutBuilder>;

    /// Access the sequential reader (read-side of packed mode).
    /// Returns `None` if compiled in aligned mode.
    pub fn sequential_reader(&self) -> Option<&SequentialReader>;

    /// Validate a JSON value against the schema. The jsonschema validator
    /// is already compiled — this is a fast check.
    ///
    /// Returns `Ok(())` if valid, `Err(TypedefError::Validation(...))` if invalid.
    pub fn validate_json(&self, instance: &serde_json::Value) -> Result<(), TypedefError>;

    /// Check if a JSON value is valid against the schema.
    pub fn is_valid_json(&self, instance: &serde_json::Value) -> bool;

    /// Read a field from a buffer at its computed offset (aligned mode).
    /// Returns `None` if compiled in packed mode — use `sequential_reader()` instead.
    pub fn read_field<'a>(
        &self,
        buffer: &'a [u8],
        field_path: &str,
    ) -> Result<FieldValue<'a>, TypedefError>;

    /// Write a field to a buffer at its computed offset (aligned mode).
    /// Returns `None` if compiled in packed mode — use `layout_builder()` instead.
    pub fn write_field(
        &self,
        buffer: &mut [u8],
        field_path: &str,
        value: &FieldValue<'_>,
    ) -> Result<(), TypedefError>;
}
```

### `compile()` constructor

The `compile()` method performs these steps in order:

1. **Normalize `$ref` values** — call `schema::normalize_refs()` to rewrite bare-name
   refs to full JSON Pointer paths.
2. **Parse endianness** — extract `"endian"` annotation (defaults to `Little`).
3. **Build the layout** — depending on `mode`:
   - `Packed`: build `LayoutBuilder` and `SequentialReader` from the schema.
   - `Aligned`: compute `OffsetMap` from the schema.
4. **Build the validator** — call `validation::build_validator()` to register all 17
   custom keywords and compile the jsonschema validator.
5. **Return the engine** — all components ready for repeated use.

### Aligned mode convenience methods

When compiled in `Aligned` mode, `read_field()` and `write_field()` provide
convenient access using the `OffsetMap`:

- `read_field(buffer, "header.version")` → looks up the field's byte range in the
  `OffsetMap`, reads the appropriate type using `data_access` functions.
- `write_field(buffer, "header.version", &FieldValue::U32(1))` → looks up the byte
  range, writes using `data_access` functions.

### What this does NOT include

- A builder API for schema construction (deferred, OQ-071)
- Schema evolution / Value system (out of scope for v1)
- Code generation (out of scope)

## Acceptance Criteria

- [ ] `TypedefEngine` struct with `layout`, `validator`, `endian`, `schema` fields
- [ ] `Layout` enum with `Packed { builder, reader }` and `Aligned { offset_map }` variants
- [ ] `LayoutMode` enum with `Packed` and `Aligned` variants
- [ ] `TypedefEngine::compile(&mut schema, mode)` performs all load-time work
- [ ] `compile()` normalizes `$ref` values before building
- [ ] `compile()` builds the correct layout for the selected mode
- [ ] `compile()` builds the jsonschema validator with all 17 custom keywords
- [ ] `compile()` returns `TypedefError::Schema` for invalid schemas
- [ ] `endian()` returns the schema's endianness
- [ ] `mode()` returns the layout mode
- [ ] `offset_map()` returns `Some(&OffsetMap)` in aligned mode, `None` in packed mode
- [ ] `layout_builder()` returns `Some(&LayoutBuilder)` in packed mode, `None` in aligned mode
- [ ] `sequential_reader()` returns `Some(&SequentialReader)` in packed mode, `None` in aligned mode
- [ ] `validate_json()` delegates to the compiled jsonschema validator
- [ ] `is_valid_json()` returns true/false without error details
- [ ] `read_field()` works in aligned mode for all fixed-size and variable-length types
- [ ] `write_field()` works in aligned mode for all fixed-size and variable-length types
- [ ] `read_field()` and `write_field()` return errors in packed mode (use layout-specific APIs)
- [ ] No `unwrap()` or `expect()` on error paths
- [ ] All public types and functions have doc comments
- [ ] `cargo check -p alknet-typedef` succeeds
- [ ] `cargo clippy -p alknet-typedef` succeeds with no warnings
- [ ] `cargo build --workspace` still succeeds

## References

- docs/architecture/crates/typedef/validation.md — TypedefEngine struct, compile() constructor
- docs/architecture/crates/typedef/overview.md — architecture, consumers
- docs/architecture/crates/typedef/layout-engine.md — the two layout modes
- docs/architecture/crates/typedef/data-access.md — read/write functions
- docs/architecture/decisions/095-alknet-typedef-purpose-scope-jsonschema-engine.md — ADR-095
- docs/architecture/decisions/096-two-layout-modes-packed-vs-aligned.md — ADR-096
- docs/architecture/decisions/098-error-handling-validation-strategy.md — ADR-098

## Notes

> This is the integration task — it wires together all the components built in the
> preceding tasks. The `TypedefEngine` is the main entry point for consumers. The
> `compile()` constructor does all the expensive work once at load time. The engine
> supports both layout modes via the `Layout` enum — the consumer selects the mode
> at construction time. The aligned-mode convenience methods (`read_field`,
> `write_field`) provide a simple API for the common mmap use case. Protocol
> consumers use the layout-specific APIs (`layout_builder()`, `sequential_reader()`)
> directly.

## Summary

> To be filled on completion
