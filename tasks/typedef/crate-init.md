---
id: typedef/crate-init
name: Initialize alknet-typedef crate with Cargo.toml, dependencies, and module skeleton
status: completed
depends_on: []
scope: moderate
risk: low
impact: project
level: implementation
---

## Description

Initialize the `alknet-typedef` crate from scratch. This is a greenfield crate — the
binary struct engine that takes a JSON Schema with `TypeDef:*` custom keywords and
produces an offset map, read/write functions, and validation. The schema is the format
definition; the engine is generic.

Per [ADR-095](../../docs/architecture/decisions/095-alknet-typedef-purpose-scope-jsonschema-engine.md),
the crate depends on `jsonschema` (v0.46.5, Draft 2020-12) for validation and
`serde_json` (with `preserve_order`) for schema parsing. No tokio, no platform deps.
Compiles to `wasm32-unknown-unknown`.

### Crate setup

Create `crates/alknet-typedef/` with:

- `Cargo.toml` — package metadata, dependencies
- `src/lib.rs` — crate root with module declarations and re-exports
- Module skeleton files for:
  - `src/error.rs` — `TypedefError` enum (ADR-098)
  - `src/schema.rs` — TypeDef kind detection, annotation parsing, `$ref` normalization, `Endian` enum (ADR-097)
  - `src/data_access.rs` — primitive read/write functions for all 17 TypeDef kinds
  - `src/offset_map.rs` — aligned static `OffsetMap` computation (ADR-096 Mode 2)
  - `src/layout_builder.rs` — packed sequential `LayoutBuilder` (ADR-096 Mode 1, write side)
  - `src/sequential_reader.rs` — packed sequential `SequentialReader` (ADR-096 Mode 1, read side)
  - `src/tunion.rs` — TUnion discriminator dispatch (byte-offset and field-name, ADR-097 §4)
  - `src/validation.rs` — custom keyword validators for all 17 `TypeDef:*` kinds
  - `src/engine.rs` — `TypedefEngine` struct combining layout + validator

### Dependencies

Per the architecture spec ([overview.md](../../docs/architecture/crates/typedef/overview.md)):

| Crate | Purpose |
|-------|---------|
| `jsonschema` 0.46 | Validation engine, custom keyword support (workspace path: `../../jsonschema`) |
| `serde_json` (preserve_order) | Schema parsing; field order is load-bearing for binary layouts |

No other dependencies. No tokio, no platform deps.

### Workspace Cargo.toml

Add `crates/alknet-typedef` to the workspace `members` list in the root `Cargo.toml`.

### Module skeleton

```rust
// src/lib.rs
//! alknet-typedef: The binary struct engine.
//!
//! Takes a JSON Schema with `TypeDef:*` custom keywords and produces
//! an offset map, read/write functions, and validation — all driven
//! by the schema. The schema is the format definition; the engine is
//! generic.
//!
//! ## Architecture
//!
//! - **Schema layer** ([`schema`]): TypeDef kind detection, annotation
//!   parsing, `$ref` normalization, endianness.
//! - **Layout engine** ([`offset_map`], [`layout_builder`],
//!   [`sequential_reader`]): Two layout modes — aligned static for
//!   mmap-friendly formats, packed sequential for protocol wire formats.
//! - **Data access** ([`data_access`]): Typed read/write at computed
//!   offsets, zero-copy for fixed-size types.
//! - **TUnion dispatch** ([`tunion`]): Byte-offset and field-name
//!   discriminator dispatch.
//! - **Validation** ([`validation`]): Custom keyword validators for all
//!   17 `TypeDef:*` kinds, delegated to the `jsonschema` crate.
//! - **Engine** ([`engine`]): `TypedefEngine` — the compiled form of a
//!   schema, combining layout and validation.

pub mod data_access;
pub mod engine;
pub mod error;
pub mod layout_builder;
pub mod offset_map;
pub mod schema;
pub mod sequential_reader;
pub mod tunion;
pub mod validation;

// Re-exports (filled in by subsequent tasks)
```

Each module file gets a doc comment and `// TODO: implement` marker.

## Acceptance Criteria

- [ ] `crates/alknet-typedef/Cargo.toml` exists with `jsonschema` and `serde_json` (preserve_order) dependencies
- [ ] `jsonschema` dependency uses workspace path (`path = "../../jsonschema"`) or git/crates.io as appropriate
- [ ] `crates/alknet-typedef/src/lib.rs` exists with module declarations for all 9 modules
- [ ] Module skeleton files exist: `error.rs`, `schema.rs`, `data_access.rs`, `offset_map.rs`, `layout_builder.rs`, `sequential_reader.rs`, `tunion.rs`, `validation.rs`, `engine.rs`
- [ ] Root `Cargo.toml` `members` list includes `crates/alknet-typedef`
- [ ] `cargo check -p alknet-typedef` succeeds
- [ ] `cargo clippy -p alknet-typedef` succeeds with no warnings
- [ ] Dual licensing: `MIT OR Apache-2.0` (workspace-inherited)
- [ ] No tokio dependency (WASM-clean by construction)
- [ ] `serde_json` has `preserve_order` feature enabled
- [ ] `cargo build --workspace` still succeeds (old code untouched)

## References

- docs/architecture/crates/typedef/README.md — crate overview and design principles
- docs/architecture/crates/typedef/overview.md — purpose, dependencies, scope boundaries
- docs/architecture/decisions/095-alknet-typedef-purpose-scope-jsonschema-engine.md — ADR-095
- docs/research/alknet-typedef/findings.md — POC results
- /workspace/jsonschema/ — the jsonschema crate (v0.46.5)
- /workspace/alknet-typedef-poc/ — POC code (disposable reference)

## Notes

> This is the foundational setup task for alknet-typedef. All subsequent typedef/*
> tasks depend on this one. The crate is dependency-light: `jsonschema` + `serde_json`
> only. No tokio, no platform deps — WASM-clean by construction. The `jsonschema` crate
> is already in the workspace at `/workspace/jsonschema/` but not yet used by any
> alknet crate — typedef is the first consumer.

## Summary

Initialized the `alknet-typedef` crate skeleton. Created `crates/alknet-typedef/`
with `Cargo.toml` (depending on `jsonschema = "0.46"` with `default-features = false`
for WASM-cleanliness, and `serde_json` with `preserve_order`), `src/lib.rs` with
module declarations for all 9 modules, and 9 skeleton source files (`error.rs`,
`schema.rs`, `data_access.rs`, `offset_map.rs`, `layout_builder.rs`,
`sequential_reader.rs`, `tunion.rs`, `validation.rs`, `engine.rs`) each with a
doc comment and `// TODO: implement` marker. Added the crate to the workspace
`members` list in the root `Cargo.toml`. Verified: `cargo check -p alknet-typedef`,
`cargo clippy -p alknet-typedef -- -D warnings`, and `cargo build --workspace` all
succeed. Confirmed no tokio/reqwest/rustls in the dependency tree (WASM-clean by
construction). Used the published `jsonschema` crate from crates.io (v0.46.10)
rather than the workspace reference copy at `/workspace/jsonschema/`.
