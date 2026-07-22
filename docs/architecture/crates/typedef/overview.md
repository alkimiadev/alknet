---
status: draft
last_updated: 2026-07-22
---

# alknet-typedef — Overview

The binary struct engine: a small Rust crate that takes a JSON Schema
with `TypeDef:*` custom keywords and produces an offset map, read/write
functions, and validation — all driven by the schema. The schema is the
format definition; the engine is generic.

This document covers the crate's purpose, the "schema is the format"
principle, its dependency edges, consumers, and scope boundaries.
Component details are in the sibling documents.

## What

`alknet-typedef` is a library crate that consumes JSON Schemas annotated
with `TypeDef:*` custom keywords (the same kinds defined in TypeBox's
`typedef.ts`, plus `TypeDef:Bytes`, `TypeDef:Int64`, and `TypeDef:Uint64`
as alknet-typedef additions) and produces three capabilities:

1. **An offset map** — walks the schema, computes byte offsets for each
   field based on type sizes, field order, and alignment.
2. **Read/write functions** — given a `&[u8]` buffer and a field path,
   read the field's bytes at its offset (zero-copy for fixed-size types).
   Given a `&mut [u8]` buffer, write a value at its offset.
3. **Validation** — via `jsonschema` custom keywords, validates that a
   buffer's bytes match the schema's type constraints.

The heavy lifting is done by the `jsonschema` crate (validation) and
`serde_json` (schema parsing). The novel code is the offset computation
— a recursive walk of the schema JSON that computes byte positions for
each field. The custom keyword implementations are small (a few lines
each, generated from shared macros — see [validation.md](validation.md)).

The crate replaces two prior attempts that built their own jsonschema
engines — typebox-rs (~8,400 lines) and alktype (~5,600 lines) — with
`jsonschema` + an offset map + small custom keyword implementations. See
[ADR-095](../../decisions/095-alknet-typedef-purpose-scope-jsonschema-engine.md).

## Why

The crate's purpose is to be the binary struct engine for every alknet
component that reads or writes binary data at computed offsets. Instead
of per-protocol serde structs (russh-sftp's 29 packet types), per-handler
wire format code (TTY's 5-byte format parser), or per-format offset
computation (metatensor's tensor access), all of these become instances
of the same engine with different schemas.

The guiding insight:

> **The schema is the format.** A JSON Schema with `TypeDef:Float32`,
> `TypeDef:Struct`, `TypeDef:Union` etc. is both the validation spec and
> the layout spec. No separate format definition, no separate parser, no
> separate validator. One schema, three uses: validate, compute offsets,
> access data.

This is the convergence of three threads identified in the
call-channels-unification research: the `typedef.ts` schema kinds from
TypeBox, the russh-sftp protocol packets, and the metatensor format. The
common pattern: a JSON Schema describes the shape of binary data, and
the binary data is the struct's bytes at computed offsets.

The crate was bumped up in the timeline when the call-channels-unification
research surfaced that channels, TTY, and the binary call protocol are
all variations on the same wire-format family — `[discriminant][length][payload]`.
The typedef engine makes the "channels is call with a binary data plane"
unification concrete: the binary data plane's wire format is the call
protocol's own schema system, just binary-encoded. The `channel_open`
marker says "use binary framing"; the typedef engine says "here's how to
read/write the binary payload."

## The "Schema Is the Format" Principle

A JSON Schema with `TypeDef:*` custom keywords serves three roles
simultaneously:

| Role | Mechanism | When |
|------|-----------|------|
| **Validation spec** | `jsonschema` custom keywords | Load time (build validator), access time (validate buffer) |
| **Layout spec** | Offset computation from type sizes + field order | Load time (build offset map) |
| **Data access** | Read/write at computed offsets | Access time (read field, write field) |

No separate format definition, no separate parser, no separate validator.
The schema is the single source of truth for the binary format. Adding a
new field to a protocol is adding a property to the schema JSON — the
engine computes the new offsets automatically.

This is the same principle as `#[repr(C)]` struct field access, but at
runtime from a portable JSON Schema instead of at compile-time from
language-specific annotations. The schema is the ABI contract.

## Dependencies

```
alknet-typedef
├── jsonschema (v0.46.5, Draft 2020-12) — validation engine, custom keyword support
├── serde_json (with preserve_order)     — schema parsing; field order is load-bearing
└── (no tokio, no platform deps)         — WASM-clean by construction
```

`alknet-typedef` is dependency-light: `jsonschema` + `serde_json` only.
No tokio, no platform deps. Compiles to `wasm32-unknown-unknown` for
browser use. The `jsonschema` crate is already in the workspace at
`/workspace/jsonschema/` but not yet used by any alknet crate — typedef
is the first consumer.

`serde_json` requires the `preserve_order` feature because field order
is load-bearing for binary layouts. The order of properties in the
schema JSON determines the order of fields in the binary struct.

## Consumers

| Consumer | Schema describes | Engine provides |
|----------|-----------------|-----------------|
| russh-sftp | 29 packet structs + Packet union (byte discriminator) | Read/write SFTP frames from bytes |
| metatensor | Model layout (ConvNet struct, tensor refs) | Offset map for mmap'd tensor access |
| binary call frames | `call.requested` / `call.responded` / etc. structs | Read/write binary call frames |
| TTY negotiation | `NegotiateRequest` / `NegotiateResponse` structs | Read/write TTY control frames |
| channels wire | `ChunkHeader { channel_id, length }` | Already trivial (8 bytes, no schema needed) |

The russh-sftp case is the most instructive and the highest-value POC
target. The `Packet` enum's `TryFrom<&mut Bytes>` impl is a hand-written
dispatch on a type byte followed by serde deserialization. Under typedef,
the dispatch is `TUnion` with a byte-offset discriminator — the schema
says "byte 0 is the discriminator, bytes 1..N are the variant struct."
The engine reads the discriminator, looks up the variant schema, computes
offsets, reads fields. Same result, no per-packet-type code.

## Scope Boundaries (What This Is Not)

These boundaries are decided in [ADR-095](../../decisions/095-alknet-typedef-purpose-scope-jsonschema-engine.md).

- **Not metatensor.** typedef is the binary struct *engine*. Metatensor
  is a *format* (8-byte header + JSON header + binary data) that uses the
  typedef engine for its offset computation and tensor access.
- **Not a Value system.** TypeBox's `Value.Diff`, `Value.Migrate`,
  `Value.Convert` — schema evolution — is out of scope for v1. The engine
  should not do anything that explicitly blocks adding a Value system
  later.
- **Not a code generator.** typebox-rs's `codegen/` module is a separate
  concern. The typedef engine consumes schemas; it does not generate them.
- **Not a schema builder.** The typedef engine does not provide a fluent
  API for constructing schemas. Schemas are plain JSON — authored in
  TypeBox, generated by ujsx components, or hand-written. A builder API
  is deferred (OQ-071).
- **Not a serialization framework.** The typedef engine is not a
  general-purpose serde replacement. It operates on raw byte buffers at
  computed offsets — no intermediate `Value` tree, no reflection, no
  dynamic dispatch per field. For JSON data, use serde. For binary data
  with a known schema, use typedef.

## Architecture (component pointers)

- **[schema-layer.md](schema-layer.md)** — the 19 `TypeDef:*` kinds,
  jsonschema custom keyword integration, TypeBox interop, schema
  annotations (endianness, alignment, encoding, TUnion discriminators).
- **[layout-engine.md](layout-engine.md)** — offset computation, the two
  layout modes (packed sequential vs aligned static), alignment,
  endianness, variable-length field handling.
- **[data-access.md](data-access.md)** — read/write functions, TUnion
  dispatch, field paths, zero-copy access for fixed-size types,
  length-prefix reading for variable-length types.
- **[validation.md](validation.md)** — custom keyword validators for all
  19 `TypeDef:*` kinds, `TypedefError`, load-time vs access-time
  validation, `TypedefEngine` as the compiled form of a schema.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| Purpose, scope, and the jsonschema engine | [ADR-095](../../decisions/095-alknet-typedef-purpose-scope-jsonschema-engine.md) | What the crate is/isn't; why jsonschema not a custom engine; "schema is the format" principle; scope boundaries |
| Two layout modes | [ADR-096](../../decisions/096-two-layout-modes-packed-vs-aligned.md) | Packed sequential (`LayoutBuilder`/`SequentialReader`) for protocols; aligned static (`OffsetMap`) for mmap formats |
| Schema annotations | [ADR-097](../../decisions/097-schema-annotations.md) | Endianness (schema-level, default LE), alignment (struct + field-level), encoding (length-prefixed vs offset-indirect), TUnion discriminators (byte-offset vs field-name) |
| Error handling and validation | [ADR-098](../../decisions/098-error-handling-validation-strategy.md) | `TypedefError` enum; load-time build, access-time check; field-path-carrying errors; jsonschema `ValidationError` wrapping |
| Int64/Uint64 kinds | [ADR-099](../../decisions/099-int64-uint64-first-class-kinds.md) | 64-bit integers as first-class kinds (SFTP offsets, metatensor data_offsets) |
| Non-final inline variable fields | [ADR-100](../../decisions/100-reject-non-final-inline-length-prefixed-in-aligned-mode.md) | Rejected in aligned mode (would clobber subsequent fields) |
| Packed-mode read factory | [ADR-101](../../decisions/101-packed-mode-read-factory.md) | `engine.sequential_reader()` returns an owned fresh reader |
| TUnion in aligned mode | [ADR-102](../../decisions/102-reject-tunion-in-aligned-mode.md) | Rejected for v1 (broken semantics; no current consumer needs it) |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-069** (deferred(scope)): Arrays of variable-length-element structs.
- **OQ-070** (deferred(scope)): `no_std` + `alloc` support.
- **OQ-071** (deferred(scope)): Builder API for schema construction.

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
