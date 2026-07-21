---
id: typedef/review-typedef
name: Review alknet-typedef implementation for spec conformance, API shape, and test coverage
status: completed
depends_on: [typedef/tests]
scope: moderate
risk: low
impact: phase
level: review
---

## Description

Review checkpoint for the `alknet-typedef` crate. Verify the implementation is
spec-conformant, self-contained, and ready for downstream consumption by metatensor,
SFTP, binary call frames, and TTY negotiation.

### Review Checklist

#### 1. Crate structure

- Module layout matches spec: `error.rs`, `schema.rs`, `data_access.rs`, `offset_map.rs`,
  `layout_builder.rs`, `sequential_reader.rs`, `tunion.rs`, `validation.rs`, `engine.rs`
- Public API types: `TypedefEngine`, `TypedefError`, `OffsetMap`, `LayoutBuilder`,
  `SequentialReader`, `LayoutMode`, `Endian`, `ByteRange`, `FieldValue`, `UnionDispatch`
- Re-exports in `lib.rs` are correct and minimal
- No tokio dependency (WASM-clean by construction)
- `serde_json` has `preserve_order` feature enabled

#### 2. Schema layer (ADR-097)

- All 17 `TypeDef:*` kinds correctly identified by `get_typedef_kind()`
- `type_size()` returns correct sizes for all fixed-size types
- `Endian` enum with `Little` (default) and `Big`
- `parse_encoding()` handles both `true` (shorthand) and `{ "encoding": "..." }` (object)
- `parse_discriminator()` handles byte-offset and field-name discriminators
- `normalize_refs()` rewrites bare-name refs to full JSON Pointer paths
- `TEnum` uses `u32` index (not variable-length string) — deliberate deviation from TypeBox

#### 3. Data access layer

- All fixed-size read/write functions implemented with endianness support
- `read_bool`: `0x00` = false, `0x01` = true, other values → error
- `read_enum`: reads `u32` index with endianness
- Variable-length types: inline length-prefixing (default) and offset indirection (opt-in)
- Zero-copy: read functions return slices, not owned data
- All functions perform bounds checking and return `TypedefError::Access` with field path
- No `unwrap()` or `expect()` on error paths — all fallible operations use `Result`

#### 4. Layout engine (ADR-096)

- **Aligned static mode** (`OffsetMap`): fields have fixed positions with natural alignment
  padding. Variable-length fields get a 4-byte length prefix at known offset.
- **Packed sequential mode** (`LayoutBuilder` + `SequentialReader`): fields packed with
  no alignment padding. Variable-length fields shift subsequent fields.
- Nested structs produce dotted field paths (`"header.version"`)
- `TArray` with fixed count (`minItems == maxItems`) and variable count (length-prefixed)
- `TUnion` with byte-offset and field-name discriminators
- Alignment annotations: struct-level and field-level, field-level overrides
- `maxLength` annotation: fixed-size reservation in aligned mode, validation constraint in packed mode
- Endianness: per-schema, default little-endian, applied at access time

#### 5. TUnion dispatch (ADR-097 §4)

- Byte-offset discriminator: reads fixed-size integer at known offset, returns mapping key
- Field-name discriminator: reads named field, returns mapping key
- `resolve_variant()` resolves `$ref` pointers to `$defs`
- Supports `TypeDef:Uint8`, `TypeDef:Uint16`, `TypeDef:Uint32` discriminator types

#### 6. Validation (ADR-098)

- `build_validator()` registers all 17 custom keywords with `jsonschema`
- Each validator is ~10 lines (not hundreds)
- `StructValidator` inspects parent's `properties` for cross-keyword awareness
- `EnumValidator` is a no-op (built-in `enum` keyword handles validation)
- `TypedefError::Validation` wraps `jsonschema::ValidationError<'static>`

#### 7. TypedefEngine

- `compile(&mut schema, mode)` performs all load-time work: normalize refs, build layout, build validator
- `Layout` enum with `Packed { builder, reader }` and `Aligned { offset_map }` variants
- `LayoutMode` enum with `Packed` and `Aligned` variants
- Accessor methods: `endian()`, `mode()`, `offset_map()`, `layout_builder()`, `sequential_reader()`
- `validate_json()` and `is_valid_json()` delegate to compiled validator
- `read_field()` and `write_field()` convenience methods for aligned mode

#### 8. Error handling (ADR-098)

- `TypedefError` enum with four variants: `Schema`, `Offset`, `Access`, `Validation`
- `Offset` and `Access` variants carry field paths for debugging
- `Display` and `Error` trait implementations
- No `unwrap()` or `expect()` on error paths anywhere in the crate

#### 9. Test coverage

- Schema layer tests: all public functions tested
- Data access tests: all read/write functions with round-trip, endianness, error paths
- OffsetMap tests: aligned static layout with alignment, nesting, arrays, unions
- LayoutBuilder tests: packed sequential layout with variable-length shifting
- SequentialReader tests: sequential field reading with position tracking
- TUnion tests: both discriminator kinds, variant resolution
- Validation tests: all 17 custom keyword validators
- Engine tests: compile, accessors, convenience methods
- POC round-trip tests: fixed-size, string, nested struct, endianness
- Error path tests: buffer-too-short, invalid UTF-8, malformed schemas

#### 10. Cross-cutting checks

- `cargo build -p alknet-typedef` succeeds
- `cargo test -p alknet-typedef` succeeds (all tests pass)
- `cargo clippy -p alknet-typedef --all-targets` succeeds with no warnings
- `cargo fmt --check -p alknet-typedef` passes
- `cargo build --workspace` still succeeds (old code untouched)
- `cargo test --workspace` still succeeds (old tests untouched)
- No `unwrap()` or `expect()` in production code (spec pseudocode uses `unwrap` for brevity only)
- `TEnum` uses `u32` index (not variable-length string) — deliberate deviation from TypeBox

## Acceptance Criteria

- [ ] Crate structure matches spec (9 source files, correct module layout)
- [ ] All 17 `TypeDef:*` kinds correctly identified and sized
- [ ] Both layout modes work correctly (aligned static and packed sequential)
- [ ] TUnion dispatch supports both byte-offset and field-name discriminators
- [ ] All 17 custom keyword validators registered and working
- [ ] `TypedefEngine::compile()` correctly wires all components
- [ ] `TypedefError` has correct variants with field-path-carrying errors
- [ ] No `unwrap()` or `expect()` in production code
- [ ] All tests pass (unit + integration)
- [ ] `cargo build -p alknet-typedef` succeeds
- [ ] `cargo test -p alknet-typedef` succeeds
- [ ] `cargo clippy -p alknet-typedef --all-targets` succeeds with no warnings
- [ ] `cargo fmt --check -p alknet-typedef` passes
- [ ] Workspace still green: `cargo build --workspace` + `cargo test --workspace` pass

## References

- docs/architecture/crates/typedef/README.md — crate overview and design principles
- docs/architecture/crates/typedef/overview.md — purpose, dependencies, scope boundaries
- docs/architecture/crates/typedef/schema-layer.md — the 17 TypeDef kinds, annotations
- docs/architecture/crates/typedef/layout-engine.md — the two layout modes
- docs/architecture/crates/typedef/data-access.md — read/write functions
- docs/architecture/crates/typedef/validation.md — custom keyword validators
- docs/architecture/decisions/095-alknet-typedef-purpose-scope-jsonschema-engine.md — ADR-095
- docs/architecture/decisions/096-two-layout-modes-packed-vs-aligned.md — ADR-096
- docs/architecture/decisions/097-schema-annotations.md — ADR-097
- docs/architecture/decisions/098-error-handling-validation-strategy.md — ADR-098
- docs/research/alknet-typedef/findings.md — POC results
- All task files in tasks/typedef/

## Notes

> This review gates the alknet-typedef implementation. The crate must be self-contained
> and spec-conformant before downstream consumers (metatensor, SFTP, binary call frames,
> TTY negotiation) can depend on it. The POC validated the approach with 26 passing
> tests; the production implementation should match or exceed that coverage. Key things
> to verify: no `unwrap()` in production code (the spec pseudocode uses it for brevity),
> `TEnum` uses `u32` index (not variable-length string), and both layout modes produce
> correct offsets for their respective use cases.

## Summary

Review checkpoint complete. The `alknet-typedef` crate is spec-conformant, self-contained, and ready for downstream consumption.

### Verification results (all green)

- `cargo build -p alknet-typedef` — succeeds
- `cargo test -p alknet-typedef` — **288 tests pass** (202 unit + 86 integration), 0 failures
- `cargo clippy -p alknet-typedef --all-targets -- -D warnings` — no warnings
- `cargo fmt --check -p alknet-typedef` — passes
- `cargo build --workspace` — succeeds (old code untouched)
- `cargo test --workspace` — all workspace tests pass (old tests untouched)
- WASM-clean: no tokio/reqwest/rustls/hyper/openssl in the dependency tree (`jsonschema` with `default-features = false`)

### Crate structure (matches spec)

9 source files in `crates/alknet-typedef/src/`: `error.rs`, `schema.rs`, `data_access.rs`, `offset_map.rs`, `layout_builder.rs`, `sequential_reader.rs`, `tunion.rs`, `validation.rs`, `engine.rs`. 4 integration test files in `tests/`: `engine_integration.rs`, `poc_roundtrip.rs`, `tunion_dispatch.rs`, `error_paths.rs`. ~7,114 lines of source + ~1,650 lines of tests = ~9,764 total.

### Public API (re-exported at crate root)

`TypedefEngine`, `LayoutMode`, `TypedefError`, `Endian`, `VariableEncoding`, `DiscriminatorKind`, `OffsetMap`, `ByteRange`, `LayoutBuilder`, `PackedLayout`, `FieldPosition`, `SequentialReader`, `FieldValue`, `UnionDispatch`, `build_validator`, `normalize_refs`, and the `parse_*` functions. Module-qualified access for `data_access::*` (28 read/write functions) and `tunion::*` (discriminator dispatch functions).

### Spec conformance highlights

- All 17 `TypeDef:*` kinds correctly identified by `get_typedef_kind()` and registered as custom keywords with `jsonschema`
- `TEnum` uses `u32` index (not variable-length string) — deliberate deviation from TypeBox for binary efficiency
- Both layout modes work: aligned static (`OffsetMap`) with natural alignment padding, packed sequential (`LayoutBuilder`/`SequentialReader`) with no padding
- TUnion dispatch supports both byte-offset (SFTP pattern) and field-name (typedef.ts pattern) discriminators
- `TypedefError` has four variants with field-path-carrying `Offset` and `Access` errors
- `TypedefEngine::compile()` normalizes `$ref` values, builds the layout, and builds the validator
- `serde_json` has `preserve_order` feature enabled (field order is load-bearing)
- No `unwrap()` or `expect()` in production code (verified by audit — all occurrences are in `#[cfg(test)]` blocks)

### Notes for downstream consumers

- `resolve_variant()` in `tunion.rs` resolves `$ref` against the union schema's own `$defs` block. For nested unions whose `$defs` live on an ancestor, the engine's `compile()` normalizes refs at the root level, and the layout modules resolve refs against the root schema passed internally. This covers the common case; consumers with deeply nested `$defs` should verify ref resolution works for their schema structure.
- `read_field`/`write_field` on `TypedefEngine` (aligned mode) support all fixed-size types and string/bytes. Composite types (Struct/Union/Array/Record) return a descriptive `TypedefError::Access` pointing to the layout-specific APIs.
- Arrays of variable-length-element structs are deferred (OQ-069).
