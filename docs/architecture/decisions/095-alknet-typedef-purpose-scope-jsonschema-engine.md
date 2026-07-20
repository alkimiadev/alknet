# ADR-095: alknet-typedef — Purpose, Scope, and the jsonschema Engine

## Status
Accepted

## Context

Three threads in the codebase converge on the same pattern: a JSON Schema
describes the shape of binary data, and the binary data is the struct's
bytes at computed offsets.

1. **typedef.ts** (`/workspace/@alkdev/typebox/example/typedef/typedef.ts`,
   619 lines) defines custom TypeBox schema kinds (`TFloat32`, `TStruct`,
   `TUnion`, etc.) that carry binary layout semantics. These are registered
   via `TypeRegistry.Set` with custom validators.

2. **russh-sftp** has 29 packet types, each a struct with typed fields
   (`Read { id: u32, handle: String, offset: u64, len: u32 }`). The wire
   format is `[length: u32][type: u8][payload]` where payload is the
   struct's serde bytes. The `Packet` enum dispatches on the type byte —
   a tagged union of structs. Under the typedef lens, each packet is a
   `TStruct`; the `Packet` enum is a `TUnion` with a byte-offset
   discriminator.

3. **metatensor** needs an offset map for mmap-friendly tensor access —
   given a schema describing a model layout (ConvNet struct, tensor refs),
   compute byte offsets for each field so the consumer can read tensor
   data at known positions without parsing.

The common pattern: **a JSON Schema with `TypeDef:*` custom keywords
describes the shape of binary data; the binary data is the struct's bytes
at computed offsets.** The schema is the format definition; the engine is
generic.

Two prior attempts built their own jsonschema engines — the fatal flaw:

- **typebox-rs** (`/workspace/@alkimiadev/typebox-rs/`, ~8,400 lines):
  a full 26-variant `SchemaKind` enum, a custom `Value` type with typed
  arrays, and a 912-line hand-written validator.
- **alktype** (`/workspace/@alkimiadev/alktype/`, ~5,600 lines): a
  handler-registry pattern that also implements its own validation for
  each type.

The `jsonschema` crate (v0.46.5, Draft 2020-12) is already in the
workspace at `/workspace/jsonschema/`. It handles validation with custom
keyword support — the novel code is the offset computation, not the
validation.

The call-channels-unification research
(`docs/research/call-channels-unification/findings.md` §"alknet-typedef:
JSON Schema as the binary struct engine") identified the convergence and
bumped typedef up in the timeline. The POC
(`docs/research/alknet-typedef/findings.md`, 26 tests passing) validated
the approach: a ~1,900-line Rust crate that takes a JSON Schema with
`TypeDef:*` custom keywords and produces an offset map, read/write
functions, and validation — all driven by the schema.

## Decision

**alknet-typedef is a small Rust crate that takes a JSON Schema with
`TypeDef:*` custom keywords and produces three capabilities:**

1. **An offset map** — walks the schema, computes byte offsets for each
   field based on type sizes, field order, and alignment.
2. **Read/write functions** — given a `&[u8]` buffer and a field path,
   read the field's bytes at its offset (zero-copy for fixed-size types).
   Given a `&mut [u8]` buffer, write a value at its offset.
3. **Validation** — via `jsonschema` custom keywords, validates that a
   buffer's bytes match the schema's type constraints.

**The heavy lifting is done by the `jsonschema` crate (validation) and
`serde_json` (schema parsing).** The novel code is the offset computation
— a recursive walk of the schema JSON that computes byte positions for
each field. The custom keyword implementations are ~10 lines each.

**The schema is the format.** A JSON Schema with `TypeDef:Float32`,
`TypeDef:Struct`, `TypeDef:Union` etc. is both the validation spec and
the layout spec. No separate format definition, no separate parser, no
separate validator. One schema, three uses: validate, compute offsets,
access data.

**The crate depends on `jsonschema` and `serde_json` (with
`preserve_order`).** No tokio, no platform deps. Compiles to
`wasm32-unknown-unknown` for browser use. The `jsonschema` crate's
`with_keyword("TypeDef:Float32", factory)` API is the integration point
for custom type kinds — each `TypeDef:*` kind maps to a custom keyword
validator in Rust. Same semantics as TypeBox's `TypeRegistry.Set`, same
JSON Schema wire format.

**The crate targets `std` for v1.** The WASM target has `std` available
via `wasm-bindgen`. If embedded use cases emerge, `no_std` + `alloc` can
be added as a feature gate later — the engine's core (offset computation,
read/write) is already allocation-free. See OQ-070.

## Consequences

### Positive

- **Eliminates ~14,000 lines of hand-rolled schema engines.** typebox-rs
  and alktype are replaced by `jsonschema` + an offset map + ~50 lines of
  custom keyword implementations. The codebase drops from "a port of
  TypeBox" to "jsonschema + an offset map."
- **One schema, three uses.** The same JSON Schema validates, computes
  offsets, and drives data access. No separate format definition, parser,
  or validator per protocol.
- **Schema-driven, not code-driven.** Adding a new SFTP packet type is
  adding a variant to the schema JSON, not writing a new Rust struct +
  serde impl. The engine is generic; the schema is the configuration.
- **WASM-clean.** `serde_json` + `jsonschema` + byte manipulation. No
  tokio, no platform deps. The same typedef schemas work in browser,
  Node, Python (via `wasmtime-py`), Go (via `wazero`), and any other
  WASM host.
- **TypeBox interop.** TypeBox modules render to standard JSON Schema
  under `$defs`. That JSON feeds directly into `jsonschema::validator_for`
  on the Rust side. Zero translation. The same schema validates in both
  ecosystems.
- **Defense in depth.** Schema validation at the byte level — a malformed
  binary payload fails validation before any consumer touches it. The
  `jsonschema` crate's compiled validators are fast enough to run on
  every incoming frame.

### Negative

- **New dependency on `jsonschema`.** The crate is already in the
  workspace but not yet used by any alknet crate. This is the first
  consumer.
- **`serde_json` with `preserve_order` is required.** Field order is
  load-bearing for binary layouts. The `preserve_order` feature adds a
  small compile-time cost.
- **Schema authoring is external.** Schemas are authored in TypeBox (JS)
  or hand-written JSON. The typedef engine consumes schemas; it does not
  generate them. A builder API is deferred (OQ-071).

## Scope Boundaries (What This Is Not)

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
  API for constructing schemas. Schemas are plain JSON.
- **Not a serialization framework.** The typedef engine is not a
  general-purpose serde replacement. It operates on raw byte buffers at
  computed offsets — no intermediate `Value` tree, no reflection, no
  dynamic dispatch per field. For JSON data, use serde. For binary data
  with a known schema, use typedef.

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
- [ADR-096](096-two-layout-modes-packed-vs-aligned.md) — the two layout
  modes decision
- [ADR-097](097-schema-annotations.md) — schema annotation shapes
- [ADR-098](098-error-handling-validation-strategy.md) — error handling
  and validation strategy
