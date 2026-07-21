---
status: draft
last_updated: 2026-07-21
---

# alknet-typedef

The binary struct engine: a small Rust crate that takes a JSON Schema
with `TypeDef:*` custom keywords and produces an offset map, read/write
functions, and validation — all driven by the schema. The schema is the
format definition; the engine is generic.

## Documents

| Document | Status | Description |
|----------|--------|-------------|
| [overview.md](overview.md) | draft | Crate purpose, "schema is the format" principle, dependencies, consumers, scope boundaries |
| [schema-layer.md](schema-layer.md) | draft | The 17 `TypeDef:*` kinds, jsonschema custom keyword integration, TypeBox interop, schema annotations |
| [layout-engine.md](layout-engine.md) | draft | Offset computation, the two layout modes (packed sequential vs aligned static), alignment, endianness, variable-length handling |
| [data-access.md](data-access.md) | draft | Read/write functions, TUnion dispatch, field paths, zero-copy access, length-prefix reading |
| [validation.md](validation.md) | draft | Custom keyword validators for all 17 `TypeDef:*` kinds, `TypedefError`, load-time vs access-time validation, `TypedefEngine` |

## Applicable ADRs

| ADR | Title | Relevance |
|-----|-------|-----------|
| [095](../../decisions/095-alknet-typedef-purpose-scope-jsonschema-engine.md) | Purpose, Scope, and the jsonschema Engine | What the crate is/isn't; why jsonschema not a custom engine; "schema is the format" principle; scope boundaries |
| [096](../../decisions/096-two-layout-modes-packed-vs-aligned.md) | Two Layout Modes — Packed Sequential vs Aligned Static | The most important architectural finding; when to use each mode; `LayoutBuilder`/`SequentialReader` vs `OffsetMap` |
| [097](../../decisions/097-schema-annotations.md) | Schema Annotations — Endianness, Alignment, Encoding, TUnion Discriminators | Concrete JSON shapes for all schema-level annotations |
| [098](../../decisions/098-error-handling-validation-strategy.md) | Error Handling and Validation Strategy | `TypedefError` enum; load-time build, access-time check; field-path-carrying errors |

## Relevant Open Questions

| OQ | Title | Status | Relevance |
|----|-------|--------|-----------|
| OQ-069 | Arrays of variable-length-element structs | deferred(scope) | Requires lazy walking logic; blocked on a concrete consumer that needs it |
| OQ-070 | `no_std` + `alloc` support | deferred(scope) | Target `std` for v1; blocked on an embedded use case |
| OQ-071 | Builder API for schema construction | deferred(scope) | Schemas are authored in TypeBox or hand-written JSON for v1; blocked on a concrete need |

## Key Design Principles

1. **The schema is the format.** A JSON Schema with `TypeDef:*` custom
   keywords is both the validation spec and the layout spec. No separate
   format definition, no separate parser, no separate validator. One
   schema, three uses: validate, compute offsets, access data. See
   [overview.md](overview.md) and [ADR-095](../../decisions/095-alknet-typedef-purpose-scope-jsonschema-engine.md).

2. **jsonschema is the validation engine, not a custom engine.** The
   `jsonschema` crate (v0.46.5, Draft 2020-12) handles validation with
   custom keyword support. The novel code is the offset computation, not
   the validation. This eliminates ~14,000 lines of hand-rolled schema
   engines (typebox-rs, alktype). See [schema-layer.md](schema-layer.md)
   and [ADR-095](../../decisions/095-alknet-typedef-purpose-scope-jsonschema-engine.md).

3. **Two layout modes for two use cases.** Packed sequential
   (`LayoutBuilder`/`SequentialReader`) for protocol wire formats (SFTP,
   channels, TTY). Aligned static (`OffsetMap`) for mmap-friendly formats
   (metatensor). The consumer selects the mode; the schema is the same.
   See [layout-engine.md](layout-engine.md) and
   [ADR-096](../../decisions/096-two-layout-modes-packed-vs-aligned.md).

4. **Variable-length types default to inline length-prefixing.**
   `[length: u32][data]` is the universal pattern used by channels, SFTP,
   TTY, and most binary protocols. Offset indirection (the metatensor
   blob tensor pattern) is opt-in via the `encoding` annotation. See
   [layout-engine.md](layout-engine.md) and
   [ADR-097](../../decisions/097-schema-annotations.md).

5. **TUnion supports both byte-offset and field-name discriminators.**
   Byte-offset for protocol dispatch (SFTP type bytes, call protocol
   event types). Field-name for the typedef.ts string pattern. See
   [data-access.md](data-access.md) and
   [ADR-097](../../decisions/097-schema-annotations.md).

6. **Endianness is per-schema, default little-endian.** The engine reads
   the `"endian"` annotation and byte-swaps accordingly. SFTP consumers
   specify `"endian": "big"`. See [layout-engine.md](layout-engine.md)
   and [ADR-097](../../decisions/097-schema-annotations.md).

7. **Validation is opt-in, built once at load time.** The jsonschema
   validator is compiled once at schema load time. Access-time validation
   is a fast `is_valid()` check. High-throughput paths can skip
   validation; security-sensitive paths can validate every frame. See
   [validation.md](validation.md) and
   [ADR-098](../../decisions/098-error-handling-validation-strategy.md).

8. **Not a serialization framework.** The typedef engine is not a
   general-purpose serde replacement. It operates on raw byte buffers at
   computed offsets — no intermediate `Value` tree, no reflection, no
   dynamic dispatch per field. For JSON data, use serde. For binary data
   with a known schema, use typedef. See [overview.md](overview.md) and
   [ADR-095](../../decisions/095-alknet-typedef-purpose-scope-jsonschema-engine.md).

## References

- `docs/research/alknet-typedef/findings.md` — POC results (26 tests
  passing, two layout modes, TUnion dispatch, endianness)
- `docs/research/call-channels-unification/findings.md` §"alknet-typedef:
  JSON Schema as the binary struct engine" — the origin of this research
  thread
- `/workspace/@alkdev/typebox/example/typedef/typedef.ts` — the TypeBox
  schema kinds (619 lines)
- `/workspace/jsonschema/` — the jsonschema crate (v0.46.5, Draft 2020-12)
- `/workspace/alknet-typedef-poc/` — the POC code (disposable)
- `/workspace/@alkimiadev/typebox-rs/` — prior attempt, replaced by typedef
- `/workspace/@alkimiadev/alktype/` — prior attempt, replaced by typedef
