---
id: typedef/error-type
name: Implement TypedefError enum with Schema, Offset, Access, and Validation variants
status: completed
depends_on: [typedef/crate-init]
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Implement the `TypedefError` enum in `crates/alknet-typedef/src/error.rs`. This is the
single error type for all three engine phases (schema parsing, offset computation,
read/write) plus validation. Decided in
[ADR-098](../../docs/architecture/decisions/098-error-handling-validation-strategy.md).

### Target shape (per ADR-098)

```rust
use std::fmt;

/// Errors produced by the typedef engine across all phases.
#[derive(Debug)]
pub enum TypedefError {
    /// Schema parsing errors — invalid JSON, missing required keywords,
    /// unknown `TypeDef:*` kinds, malformed annotations.
    Schema(String),

    /// Offset computation errors — field not found, type not supported
    /// for offset computation, recursive depth exceeded.
    Offset {
        field_path: String,
        reason: String,
    },

    /// Read/write errors — buffer too short, invalid UTF-8, value out
    /// of range for the target type.
    Access {
        field_path: String,
        reason: String,
    },

    /// Validation errors — delegated to the `jsonschema` crate.
    /// The `'static` lifetime is correct: the validator owns its schema
    /// reference and lives for the lifetime of the `TypedefEngine`.
    Validation(jsonschema::ValidationError<'static>),
}
```

### Design rationale

- **`Schema(String)`** — for errors during `TypedefEngine::compile()`. Invalid JSON,
  missing required keywords, unknown `TypeDef:*` kinds. The error message describes
  the problem.
- **`Offset { field_path, reason }`** — for errors during offset computation. Field
  not found in the schema, type not supported for offset computation. Carries the
  field path for debugging.
- **`Access { field_path, reason }`** — for errors during read/write. Buffer too
  short, invalid UTF-8 in a string field, value out of range. Carries the field
  path for debugging.
- **`Validation(ValidationError<'static>)`** — wraps `jsonschema`'s `ValidationError`.
  The `'static` lifetime is correct — the validator is built once at schema load time
  and lives for the lifetime of the `TypedefEngine`.

### Trait implementations

- `Display` — human-readable error messages including field paths where applicable
- `Error` (std::error::Error) — for `?` propagation
- `Debug` — derived

The `Validation` variant requires `jsonschema::ValidationError` to be in scope.
Since the `jsonschema` crate is a dependency, this is straightforward.

### What this does NOT include

- No `From` impls for other error types (those are added as needed by subsequent tasks)
- No `PartialEq` — `ValidationError` may not implement it
- No `Clone` — errors are typically consumed, not cloned

## Acceptance Criteria

- [ ] `TypedefError` enum defined in `crates/alknet-typedef/src/error.rs`
- [ ] Four variants: `Schema(String)`, `Offset { field_path, reason }`, `Access { field_path, reason }`, `Validation(ValidationError<'static>)`
- [ ] `Display` impl with descriptive messages including field paths for `Offset` and `Access`
- [ ] `std::error::Error` impl
- [ ] `Debug` derived
- [ ] Re-exported from `lib.rs`
- [ ] `cargo check -p alknet-typedef` succeeds
- [ ] `cargo clippy -p alknet-typedef` succeeds with no warnings
- [ ] `cargo build --workspace` still succeeds

## References

- docs/architecture/decisions/098-error-handling-validation-strategy.md — ADR-098
- docs/architecture/crates/typedef/validation.md — TypedefError section
- docs/architecture/crates/typedef/data-access.md — error handling in read/write

## Notes

> This is a small, self-contained task. The error type is used by every other module
> in the crate. `Schema` covers load-time errors, `Offset` covers layout computation
> errors, `Access` covers read/write errors, and `Validation` wraps jsonschema's
> error type. The `'static` lifetime on `ValidationError` is correct because the
> validator is built once and lives for the engine's lifetime.

## Summary

Implemented the `TypedefError` enum in `crates/alknet-typedef/src/error.rs` with four variants (`Schema`, `Offset`, `Access`, `Validation`) per ADR-098, plus `Display` (with field-path-aware messages for `Offset` and `Access`) and `std::error::Error` impls and a derived `Debug`. Re-exported `TypedefError` from `crates/alknet-typedef/src/lib.rs` so it is accessible as `alknet_typedef::TypedefError`. `cargo check`, `cargo clippy -D warnings`, and `cargo build --workspace` all pass with no warnings.
