---
status: poc-complete
last_updated: 2026-07-20
---

# alknet-typedef — Findings: JSON Schema as the binary struct engine

**Status:** Draft findings, iterating. Per the research-then-sync
pattern (see `docs/research/stream-unification/findings.md` for the
precedent), this doc iterates in `docs/research/`; we fix
inter-document drift here, then sync to `docs/architecture/` and the
ADRs only after it settles.

**Scope:** A small Rust crate (`alknet-typedef`) that takes a JSON
Schema with `TypeDef:*` custom keywords and produces an offset map,
read/write functions, and validation — all driven by the schema. The
schema is the format definition; the engine is generic. This is the
binary struct engine that metatensor, binary call frames, SFTP
packets, and TTY negotiation all consume.

**Date:** 2026-07-20

**Origin:** The call-channels-unification findings
(`docs/research/call-channels-unification/findings.md` §"alknet-typedef:
JSON Schema as the binary struct engine") identified the convergence of
three threads: the `typedef.ts` schema kinds from TypeBox, the
russh-sftp protocol packets, and the metatensor format. The common
pattern: a JSON Schema describes the shape of binary data, and the
binary data is the struct's bytes at computed offsets. Two prior
attempts (typebox-rs, alktype) built their own jsonschema engines — the
fatal flaw. The `jsonschema` crate (v0.46.5, Draft 2020-12) already
handles validation with custom keyword support; the novel code is the
offset computation.

---

## TL;DR

`alknet-typedef` is a ~1,900-line Rust crate (POC verified). It takes a
JSON Schema with `TypeDef:*` custom keywords (the same kinds defined in
`typedef.ts`) and produces:

1. **An offset map** — walks the schema, computes byte offsets for each
   field based on type sizes, field order, and alignment.
2. **Read/write functions** — given a `&[u8]` buffer and a field path,
   read the field's bytes at its offset (zero-copy for fixed-size
   types). Given a `&mut [u8]` buffer, write a value at its offset.
3. **Validation** — via `jsonschema` custom keywords, validates that a
   buffer's bytes match the schema's type constraints.

The heavy lifting is done by the `jsonschema` crate (validation) and
`serde_json` (schema parsing). The novel code is the offset computation
— a recursive walk of the schema JSON that computes byte positions for
each field. The custom keyword implementations are ~10 lines each.

**The schema is the format.** A JSON Schema with `TypeDef:Float32`,
`TypeDef:Struct`, `TypeDef:Union` etc. is both the validation spec and
the layout spec. No separate format definition, no separate parser, no
separate validator. One schema, three uses: validate, compute offsets,
access data.

**Variable-length types default to inline length-prefixing**
(`[length: u32][data]`) — the universal pattern used by channels, SFTP,
TTY, and most binary protocols. Offset indirection (the metatensor blob
tensor pattern) is an opt-in alternative for mmap-friendly layouts.

**TUnion supports both byte-offset and field-name discriminators.**
The byte-offset variant (`"discriminator": {"kind": "byte", "offset":
0, "type": "Uint8"}`) handles protocol dispatch (SFTP type bytes, call
protocol event types). The field-name variant (`"discriminator":
{"kind": "field", "name": "type"}`) handles the typedef.ts string
pattern.

**Key architectural finding: two layout modes are needed.** The POCs
surfaced that protocols and mmap-friendly formats need different layout
strategies:

- **Packed sequential layout** (`LayoutBuilder` / `SequentialReader`) —
  for protocol wire formats (SFTP, channels, TTY). Fields are packed
  with no alignment padding. Variable-length fields shift all subsequent
  fields. Writing requires knowing actual data sizes upfront; reading
  walks the buffer sequentially, reading length prefixes to determine
  positions.

- **Aligned static layout** (`OffsetMap`) — for mmap-friendly formats
  (metatensor). Fields have fixed positions with natural alignment
  padding. Variable-length fields get a 4-byte length prefix at a known
  offset; the variable data is not included in the static layout.

**POC results (26 tests, all passing):**
- POC 1 (core): Fixed-size read/write round-trip, alignment padding,
  nested structs, variable-length strings, jsonschema custom keyword
  validation for all 16 `TypeDef:*` kinds.
- POC 2 (SFTP): Byte-identical round-trip against russh-sftp for Read,
  Write, and Status packets. TUnion byte-offset discriminator dispatch.
  Big-endian integer encoding. Mixed fixed/variable fields in a single
  struct.

---

## The Schema Layer: TypeBox ↔ jsonschema

### Why this works

TypeBox modules render to standard JSON Schema under `$defs`. A TypeBox
schema like:

```typescript
const TensorRef = Type.Object({
  dtype: Type.Union([Type.Literal("F32"), Type.Literal("I16"), ...]),
  shape: Type.Array(Type.Number()),
  data_offsets: Type.Tuple([Type.Number(), Type.Number()])
});
```

serialized to JSON is:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "dtype": { "type": "string", "enum": ["F32", "I16", ...] },
    "shape": { "type": "array", "items": { "type": "number" } },
    "data_offsets": { "type": "array", "minItems": 2, "maxItems": 2, ... }
  },
  "required": ["dtype", "shape", "data_offsets"]
}
```

That JSON feeds directly into `jsonschema::validator_for(&schema)` on
the Rust side. Zero translation. The same schema validates in both
ecosystems.

### Custom type kinds (typedef.ts port)

`typedef.ts` (`/workspace/@alkdev/typebox/example/typedef/typedef.ts`,
619 lines) defines custom TypeBox schema kinds that carry *binary
layout* semantics:

| Kind | TypeBox key | Rust type | Size |
|------|-------------|-----------|------|
| `TFloat32` | `TypeDef:Float32` | `f32` | 4 |
| `TFloat64` | `TypeDef:Float64` | `f64` | 8 |
| `TInt8` | `TypeDef:Int8` | `i8` | 1 |
| `TInt16` | `TypeDef:Int16` | `i16` | 2 |
| `TInt32` | `TypeDef:Int32` | `i32` | 4 |
| `TUint8` | `TypeDef:Uint8` | `u8` | 1 |
| `TUint16` | `TypeDef:Uint16` | `u16` | 2 |
| `TUint32` | `TypeDef:Uint32` | `u32` | 4 |
| `TString` | `TypeDef:String` | length-prefixed UTF-8 | variable |
| `TStruct` | `TypeDef:Struct` | record of fields | sum of field sizes |
| `TUnion` | `TypeDef:Union` | tagged union | discriminator + variant |
| `TArray` | `TypeDef:Array` | repeated element | count × element size |
| `TEnum` | `TypeDef:Enum` | string enum | variable |
| `TRecord` | `TypeDef:Record` | string-keyed map | variable |
| `TBoolean` | `TypeDef:Boolean` | `bool` | 1 |
| `TTimestamp` | `TypeDef:Timestamp` | ISO 8601 string | variable |

These are registered in TypeBox via `TypeRegistry.Set` with custom
validators. The Rust `jsonschema` crate supports the same pattern via
`with_keyword("TypeDef:Float32", factory)` — each `TypeDef:*` kind maps
to a custom keyword validator in Rust. Same semantics, different
language, same JSON Schema wire format.

### The jsonschema crate's custom keyword API

The `jsonschema` crate (v0.46.5, Draft 2020-12 default) is already in
the workspace at `/workspace/jsonschema/` but not yet used by any
alknet crate. It supports custom keywords via:

```rust
pub trait Keyword: Send + Sync {
    fn validate<'i>(&self, instance: &'i Value) -> Result<(), ValidationError<'i>>;
    fn is_valid(&self, instance: &Value) -> bool;
}
```

Registration:

```rust
let validator = jsonschema::options()
    .with_keyword("TypeDef:Float32", |parent, value, path| {
        Ok(Box::new(Float32Validator))
    })
    .with_keyword("TypeDef:Int32", |parent, value, path| {
        Ok(Box::new(Int32Validator))
    })
    .with_keyword("TypeDef:Struct", |parent, value, path| {
        Ok(Box::new(StructValidator::from_schema(parent)?))
    })
    .build(&schema)?;
```

The factory closure receives the parent schema object, the keyword's
value, and the schema path — enabling cross-keyword awareness (e.g., a
custom `"minimum"` keyword that inspects the parent's `"format"` field).

Custom formats are also supported via `with_format("f32", |s| ...)`.

### What this eliminates

**typebox-rs** (`/workspace/@alkimiadev/typebox-rs/`, ~8,400 lines) and
**alktype** (`/workspace/@alkimiadev/alktype/`, ~5,600 lines) both
built their own jsonschema engines — the fatal flaw. typebox-rs has a
full 26-variant `SchemaKind` enum, a custom `Value` type with typed
arrays, and a 912-line hand-written validator. alktype uses a handler
registry pattern but also implements its own validation for each type.

Both are replaced by:
- `jsonschema` crate — validation, custom keywords, custom formats,
  compiled validators
- A small offset-computation module — walks a schema, computes byte
  offsets for each field based on type sizes (the one piece of real
  logic beyond jsonschema)
- Custom keyword implementations — the `TypeDef:Float32` /
  `TypeDef:Int32` / `TypeDef:Struct` validators (small functions, ~10
  lines each)

No hand-rolled schema type system, no builder, no registry. The
codebase drops from "a port of TypeBox" to "jsonschema + an offset map
+ ~50 lines of custom keyword implementations."

The layout computation from typebox-rs (`layout.rs`, 193 lines) is the
salvageable part — the offset algorithm is correct and can be adapted
to work with jsonschema's parsed JSON rather than typebox-rs's own
`SchemaKind` enum.

---

## The Three Core Capabilities

### 1. Offset Computation

The novel code. Given a JSON Schema with `TypeDef:*` keywords, walk the
schema tree and compute the byte offset and size of each field.

```rust
struct OffsetMap {
    fields: Vec<(String, ByteRange)>,
    total_size: usize,
}

struct ByteRange {
    start: usize,
    end: usize,
}

fn compute_offsets(schema: &serde_json::Value) -> OffsetMap {
    // Walk the schema:
    // - TypeDef:Float32 → 4 bytes, align 4
    // - TypeDef:Int32 → 4 bytes, align 4
    // - TypeDef:Uint8 → 1 byte, align 1
    // - TypeDef:Struct → recurse, sum field sizes, align to max field alignment
    // - TypeDef:Array of fixed-size T → element_size * count
    // - TypeDef:String (length-prefixed) → 4 (length prefix), align 4
    // - TypeDef:String (offset-indirect) → 0 (variable, handled by consumer)
    // - TypeDef:Union → discriminator size + max variant size
    // ...
}
```

The offset computation is schema-driven: the schema's type kinds
determine byte sizes, the struct's field order determines offsets,
alignment annotations (or defaults) determine padding. The output is an
`OffsetMap` — a flat table of `(field_path, byte_range)` pairs.

For fixed-size types, the offset is computed from the schema alone. For
variable-length types with inline length-prefixing, the fixed part (the
4-byte length prefix) gets an offset; the variable part is accessed via
the length prefix at runtime. For offset-indirection, the offset is
stored *in* the data (the `{offset, length}` pair in the index struct).

### 2. Read/Write

Given a `&[u8]` buffer and a field path, read the field's bytes at its
offset:

```rust
impl OffsetMap {
    fn read_f32(&self, buffer: &[u8], field_path: &str) -> f32 {
        let range = self.fields.get(field_path).unwrap();
        f32::from_le_bytes(buffer[range.start..range.end].try_into().unwrap())
    }

    fn read_str<'a>(&self, buffer: &'a [u8], field_path: &str) -> &'a str {
        let range = self.fields.get(field_path).unwrap();
        let len = u32::from_le_bytes(buffer[range.start..range.start+4].try_into().unwrap()) as usize;
        std::str::from_utf8(&buffer[range.start+4..range.start+4+len]).unwrap()
    }

    fn write_u32(&self, buffer: &mut [u8], field_path: &str, value: u32) {
        let range = self.fields.get(field_path).unwrap();
        buffer[range.start..range.end].copy_from_slice(&value.to_le_bytes());
    }
}
```

Fixed-size types: zero-copy pointer cast (`&[f32]`, `&[u8; 4]`).
Variable-length types with inline length-prefixing: read the 4-byte
length prefix at the field's offset, then slice the data that follows.
Offset-indirection: the consumer provides the data region; the engine
reads `{offset, length}` from the index struct and slices the data
region.

### 3. Validation

Delegated to `jsonschema` custom keywords. Each `TypeDef:*` kind gets a
`Keyword` implementation:

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

The validators check:
- Range constraints for ints (Int8: -128..127, Uint8: 0..255, etc.)
- Finiteness for floats
- UTF-8 validity for strings
- Field presence and types for structs
- Discriminator value membership for unions
- Element type conformance for arrays

The `jsonschema` crate handles all the structural validation (object
properties, required fields, array items, enum values) — the custom
keywords only need to validate the leaf type constraints.

---

## The Hard Problems

### Problem 1: Variable-length types in fixed-offset layouts

Variable-length types (`TString`, `TBytes`, `TArray`, `TRecord`) don't
have a fixed byte size. Two strategies are needed:

**Strategy A: Inline length-prefixing (default).** The field's fixed
portion is a 4-byte length prefix at a computed offset. The variable
data follows immediately after. This is the universal pattern used by
channels (`[channel_id: u32][size: u32][payload]`), SFTP (all strings
and byte arrays), TTY (`[stream_type: u8][length: u32][payload]`), and
most binary protocols.

Schema annotation:
```json
{
  "TypeDef:String": true,
  "encoding": "length-prefixed"
}
```

The offset map records the position of the 4-byte length prefix. At
read time, the engine reads the length, then slices the data that
follows. At write time, the engine writes the length prefix + data.

**Strategy B: Offset indirection (opt-in).** The field is a struct
`{offset: u32, length: u32}` that points into a separate data region.
This is the metatensor blob tensor pattern — the index struct lives in
one region, the blob data lives in another. Enables mmap-friendly
random access to variable-length data without parsing length prefixes.

Schema annotation:
```json
{
  "TypeDef:String": true,
  "encoding": "offset-indirect"
}
```

The offset map records the position of the `{offset, length}` pair. The
consumer provides the data region separately; the engine slices
`data_region[offset..offset+length]`.

**Default:** Inline length-prefixing is the default for all
variable-length types. Offset indirection is opt-in via the `encoding`
annotation. This matches the volume: most binary protocols use inline
length-prefixing; metatensor is the oddball that needs indirection.

**The channels protocol example.** The channels wire format is
`[channel_id: u32][size: u32][payload]`. Under typedef, this is a
struct with three fields:

```json
{
  "TypeDef:Struct": true,
  "properties": {
    "channel_id": { "TypeDef:Uint32": true },
    "size": { "TypeDef:Uint32": true },
    "payload": { "TypeDef:Bytes": true, "encoding": "length-prefixed" }
  }
}
```

The first two fields are fixed-size (4 bytes each, offsets 0 and 4).
The third field's *length prefix* is at offset 8 (4 bytes), and the
payload data follows at offset 12. The `size` field and the payload's
length prefix are redundant — the channels layer writes both, but the
typedef engine only reads the length prefix. This is a small amount of
waste per frame, but the trade-off is that the schema is
self-describing: the engine doesn't need to know that `size` and
`payload.length` are the same value.

### Problem 2: TUnion discriminator encoding

`typedef.ts`'s `TUnion` has a `discriminator` field name (like
`"type"`) and a `mapping` of string values to structs. But in binary
protocols, the discriminator is typically a type *byte* at a fixed
position (byte 0), not a named field. Two discriminator kinds are
needed:

**Kind A: Byte-offset discriminator (binary protocols).** The
discriminator is a fixed-size integer at a known byte offset. The
mapping is integer value → variant struct.

```json
{
  "TypeDef:Union": true,
  "discriminator": {
    "kind": "byte",
    "offset": 0,
    "type": "TypeDef:Uint8"
  },
  "mapping": {
    "1": { "$ref": "#/$defs/Init" },
    "3": { "$ref": "#/$defs/Open" },
    "5": { "$ref": "#/$defs/Read" },
    "6": { "$ref": "#/$defs/Write" },
    "101": { "$ref": "#/$defs/Status" },
    "102": { "$ref": "#/$defs/Handle" },
    "103": { "$ref": "#/$defs/Data" }
  }
}
```

This is the SFTP `Packet` enum pattern — byte 0 is the type byte, the
payload (bytes 1..N) is the variant struct. The call protocol's 5 event
types (`call.requested` → 0x01, `call.responded` → 0x02, etc.) use the
same pattern.

**Kind B: Field-name discriminator (typedef.ts pattern).** The
discriminator is a named field within the struct, and the mapping is
string value → variant struct.

```json
{
  "TypeDef:Union": true,
  "discriminator": {
    "kind": "field",
    "name": "type"
  },
  "mapping": {
    "0": { "$ref": "#/$defs/Read" },
    "1": { "$ref": "#/$defs/Write" }
  }
}
```

This is the typedef.ts `TUnion` pattern — the discriminator is a field
like any other, and the mapping keys are string indices.

**The discriminator type determines the offset computation.** For
byte-offset discriminators, the discriminator occupies `offset..offset
+ discriminator_size` bytes, and the variant struct starts at `offset +
discriminator_size`. For field-name discriminators, the discriminator
is just another field in the struct — its offset is computed like any
other field, and the variant struct follows at the end of the
discriminator field.

### Problem 3: Nested structs and arrays of structs

**Nested structs** (`TStruct` containing `TStruct`): recursive offset
computation. The outer struct's field offset is the inner struct's
start position; the inner struct's fields are computed relative to that
start. Straightforward for fixed-size inner structs.

**Arrays of structs** (`TArray` of `TStruct`): element stride = struct
size. For fixed-size structs, the stride is the struct's total size
(including alignment padding). Element `i` starts at `array_offset + i
* stride`. Straightforward.

**The hard case: arrays of structs containing variable-length fields.**
If each element has a `TString` (length-prefixed), the string data for
element 0 follows the fixed fields of element 0, then element 1's
fixed fields follow, then element 1's string data, etc. The elements
are interleaved: `[fixed_0][str_0][fixed_1][str_1]...`. The engine
can't compute a fixed stride — it must walk the array sequentially,
reading each element's length prefixes to find the next element's
start.

This is the general case of Problem 1. The resolution: for arrays of
structs with variable-length fields, the engine computes offsets
lazily — it reads each element's fixed fields, uses the length prefixes
to skip the variable data, and advances to the next element. The
`OffsetMap` records the array's start offset and element count; the
read function walks the array on demand.

**For v1, arrays of fixed-size structs are fully supported. Arrays of
variable-length-element structs are deferred** — they require the lazy
walking logic and don't appear in the initial consumers (SFTP's `Name`
packet has `Vec<File>` where `File` contains strings, but SFTP
serializes the whole array with length prefixes per field, not per
element — the serde `SeqAccess` pattern handles this).

### Problem 4: Alignment

Different backends have different alignment requirements:
- wgpu: 256-byte alignment for some buffer types
- Protocols: natural alignment (4-byte for u32, 8-byte for u64)
- mmap: page alignment (4096-byte)

**Resolution: schema-annotated with sensible defaults.** Each type has
a default alignment (1 for u8, 2 for u16, 4 for u32/f32, 8 for u64/f64,
max field alignment for structs). The schema can override:

```json
{
  "TypeDef:Struct": true,
  "align": 256,
  "properties": { ... }
}
```

The offset computation respects the annotation. Consumers can also
provide a global alignment override (e.g., "round all structs to
256-byte boundaries for wgpu"). The engine computes natural offsets;
the consumer applies the override as a post-processing step.

**For v1, natural alignment with schema-level overrides is sufficient.**
Global alignment overrides are a consumer concern, not an engine
concern.

---

## What This Is Not (Scope Boundaries)

### Not metatensor

`alknet-typedef` is the binary struct *engine*. Metatensor is a
*format* (8-byte header length + JSON header + binary data) that uses
the typedef engine for its offset computation and tensor access. The
metatensor format also includes: the 8-byte header, the JSON header
parsing, the mmap integration, the QUIC stream mapping, the ujsx
authoring layer, and the wgpu buffer bridge. typedef is the engine;
metatensor builds on it.

### Not a Value system

TypeBox's `Value.Diff`, `Value.Migrate`, `Value.Convert` — schema
evolution — is out of scope for v1. The typedef engine reads and
writes bytes at computed offsets; it does not diff schemas or migrate
data between schema versions. Schema evolution is a higher-level
concern (metatensor format versioning, call protocol versioning). The
engine should not do anything that explicitly blocks adding a Value
system later (e.g., don't bake in assumptions about schema
immutability).

### Not a code generator

typebox-rs's `codegen/` module (Rust/TypeScript/WGSL code generation
via Handlebars) is a separate concern. The typedef engine consumes
schemas; it does not generate them. Schemas are authored in TypeBox
(JS) or hand-written JSON.

### Not a schema builder

The typedef engine does not provide a fluent API for constructing
schemas. Schemas are plain JSON — authored in TypeBox, generated by
ujsx components, or hand-written. The engine only consumes them.

### Not a serialization framework

The typedef engine is not a general-purpose serde replacement. It
operates on raw byte buffers at computed offsets — no intermediate
`Value` tree, no reflection, no dynamic dispatch per field. It is
closer to `#[repr(C)]` struct field access than to serde. For JSON data,
use serde. For binary data with a known schema, use typedef.

---

## Consumers

| Consumer | Schema describes | Engine provides |
|----------|-----------------|-----------------|
| russh-sftp | 29 packet structs + Packet union (byte discriminator) | Read/write SFTP frames from bytes |
| metatensor | Model layout (ConvNet struct, tensor refs) | Offset map for mmap'd tensor access |
| binary call frames | `call.requested` / `call.responded` / etc. structs | Read/write binary call frames |
| TTY negotiation | `NegotiateRequest` / `NegotiateResponse` structs | Read/write TTY control frames |
| channels wire | `ChunkHeader { channel_id, length }` | Already trivial (8 bytes, no schema needed) |

The russh-sftp case is the most instructive and the highest-value POC
target. The `Packet` enum's `TryFrom<&mut Bytes>` impl is a
hand-written dispatch on a type byte followed by serde
deserialization. Under typedef, the dispatch is `TUnion` with a
byte-offset discriminator — the schema says "byte 0 is the
discriminator, bytes 1..N are the variant struct." The engine reads the
discriminator, looks up the variant schema, computes offsets, reads
fields. Same result, no per-packet-type code.

### Relationship to the call protocol

The call protocol's `OperationSpec.input_schema` and
`OperationSpec.output_schema` are already JSON Schemas. With typedef,
those schemas can describe binary payloads — not just JSON validation
shapes. An op with `channel_open: Some(...)` and a typedef schema for
its input/output is a binary-stream op whose wire format is
schema-driven. The handler doesn't write a serde struct; it writes
bytes at schema-computed offsets. The schema is the format.

This closes the loop on "channels is call with a binary data plane"
from the call-channels-unification findings: the binary data plane's
wire format is the call protocol's own schema system, just
binary-encoded. The `channel_open` marker says "use binary framing";
the typedef engine says "here's how to read/write the binary payload."

---

## POC Results

POCs live in the global workspace (`/workspace/alknet-typedef-poc/`),
not in the alknet repo. The research doc references them; the code is
disposable.

### POC 1: Core offset computation — COMPLETE

**Target:** `/workspace/alknet-typedef-poc/`

**What was built:**
- `OffsetMap` with natural alignment padding for fixed-size fields
- Read/write for u8, u16, u32, u64, f32 at computed offsets
- Read/write for length-prefixed strings and byte arrays
- Nested struct support with dotted field paths (`header.version`)
- Endianness support (little-endian default, big-endian via `"endian":
  "big"`)
- jsonschema custom keyword validators for all 16 `TypeDef:*` kinds
  (Float32, Float64, Int8/16/32, Uint8/16/32, Boolean, String, Struct,
  Array, Enum, Union, Record, Timestamp)
- `TypedefEngine` struct combining offset map + validator

**Key findings:**
- `serde_json` needs the `preserve_order` feature — field order matters
  for binary layouts
- Nested struct handling must happen inline in `compute_struct_offsets`
  to propagate the field path prefix correctly
- The `jsonschema` crate's `with_keyword` API works cleanly — each
  factory is ~5 lines
- Alignment padding between fields (e.g., u8 → u32 inserts 3 bytes) is
  correct for mmap-friendly layouts but wrong for protocol wire formats

**Code size:** ~1,268 lines (lib + offset + validate), ~174 lines of
integration tests. 16 unit tests + 3 integration tests, all passing.

### POC 2: russh-sftp packet round-trip — COMPLETE

**What was built:**
- 3 SFTP packet types as typedef schemas: `Read` (id: u32, handle:
  string, offset: u64, len: u32), `Write` (id: u32, handle: string,
  offset: u64, data: bytes), `Status` (id: u32, status_code: u32,
  error_message: string, language_tag: string)
- `Packet` union schema with byte-offset discriminator (byte 0, u8,
  big-endian)
- `LayoutBuilder` — packed sequential layout for protocol wire formats
  (no alignment padding, variable-length fields shift subsequent fields)
- `SequentialReader` — walks a buffer field-by-field according to the
  schema, reading length prefixes to determine variable-length data
  positions
- `parse_union_discriminator` + `read_union_discriminator` for TUnion
  dispatch
- Byte-identical round-trip tests against russh-sftp's own serialization

**Key findings:**
- **Two layout modes are required.** The aligned `OffsetMap` from POC 1
  is correct for mmap-friendly formats (metatensor) but wrong for
  protocol wire formats (SFTP, channels, TTY). Protocols pack fields
  sequentially with no alignment padding. This is the most important
  architectural finding from the POCs.
- **Variable-length fields shift all subsequent fields in protocol
  layouts.** The `LayoutBuilder` takes actual data sizes to compute
  correct positions. The `SequentialReader` walks the buffer
  sequentially, reading length prefixes to find each field's data.
- **TUnion with byte-offset discriminator works correctly.** The SFTP
  type byte (5=Read, 6=Write, 101=Status) dispatches to the correct
  variant schema. The mapping keys are stringified integers (`"5"`,
  `"6"`, `"101"`).
- **Endianness is per-schema.** SFTP uses `"endian": "big"`; the engine
  reads/writes correctly in both little and big endian.
- **The `jsonschema` crate's `serde_json::Value`-based validation works
  for protocol schemas.** The same schema validates both the JSON
  representation and (via the typedef engine) the binary representation.

**Code size:** ~347 lines of integration tests. 7 tests, all passing
(5 round-trip against russh-sftp + 1 union dispatch + 1 schema
validation).

### POC 3: Metatensor header parse + mmap access — NOT STARTED

Deferred. The aligned `OffsetMap` from POC 1 is the correct engine for
this; the remaining work is the metatensor format layer (8-byte header,
JSON header parsing, mmap integration).

### POC 4: WASM compilation — NOT STARTED

Deferred. The `jsonschema` crate's docs confirm WASM support with
`default-features = false`. The typedef engine's core (offset
computation, read/write) operates on `&[u8]` slices with no platform
dependencies.

### Summary of architectural findings

| Finding | Impact |
|---------|--------|
| Two layout modes needed (packed vs aligned) | The engine must support both. `LayoutBuilder`/`SequentialReader` for protocols; `OffsetMap` for mmap formats. |
| `serde_json` needs `preserve_order` | Field order is load-bearing for binary layouts. |
| jsonschema custom keywords work cleanly | 16 `TypeDef:*` kinds registered, all passing. No need for a custom schema engine. |
| TUnion byte-offset discriminator works | SFTP type byte dispatch confirmed. Same pattern works for call protocol event types. |
| Endianness is per-schema | `"endian": "big"` / `"endian": "little"` annotation. Default little-endian. |
| Variable-length fields shift subsequent fields in protocol layouts | `LayoutBuilder` takes actual sizes; `SequentialReader` walks sequentially. |
| Nested structs work with dotted field paths | `header.version` → recursive offset computation with prefix propagation. |

---

## Open Questions

### 1. Variable-length encoding annotation shape

The `"encoding": "length-prefixed"` vs `"encoding": "offset-indirect"`
annotation needs a concrete JSON shape. Options:
- A string enum on the type keyword: `"TypeDef:String": { "encoding":
  "length-prefixed" }` (requires the keyword value to be an object, not
  `true`)
- A separate keyword: `"TypeDef:String": true, "typedef:encoding":
  "length-prefixed"`
- A default with override: length-prefixed is the default; only
  offset-indirect needs explicit annotation

**Recommendation:** The keyword value is an object with an `encoding`
field. `"TypeDef:String": { "encoding": "length-prefixed" }` or
`"TypeDef:String": { "encoding": "offset-indirect" }`. `true` is a
shorthand for the default (length-prefixed). This keeps the common case
concise and the override explicit.

### 2. TUnion discriminator JSON shape

The `"discriminator"` field needs a concrete shape that covers both
byte-offset and field-name variants. Proposed:

```json
// Byte-offset discriminator
"discriminator": {
  "kind": "byte",
  "offset": 0,
  "type": "TypeDef:Uint8"
}

// Field-name discriminator
"discriminator": {
  "kind": "field",
  "name": "type"
}
```

The `mapping` keys are strings in both cases (integer values are
stringified: `"1"`, `"3"`, `"101"`). The engine parses the key to
match the discriminator value.

**Open sub-question:** Should the mapping values be full schemas or
`$ref` pointers? `$ref` is cleaner for large unions (29 SFTP variants)
but requires a `$defs` section. Inline schemas are simpler for small
unions (5 call protocol event types). Both should work.

### 3. Alignment annotation shape

The `"align"` annotation needs a concrete shape. Options:
- A top-level schema property: `{ "TypeDef:Struct": true, "align": 256,
  "properties": {...} }`
- A per-field property: `{ "weight": { "TypeDef:Float32": true, "align":
  256 } }`
- Both (field overrides struct default)

**Recommendation:** Both. Struct-level `align` sets the default for all
fields; field-level `align` overrides. The engine applies the most
specific annotation.

### 4. Endianness

safetensors is little-endian. SFTP is big-endian. The typedef engine
needs to know which to use. Options:
- Schema-level annotation: `{ "endian": "little" }` or `{ "endian":
  "big" }`
- Per-type annotation (overkill — mixed endianness in one schema is
  pathological)
- Consumer-level configuration (the engine is endian-agnostic; the
  consumer specifies)

**Recommendation:** Schema-level annotation with a default of
little-endian (matching safetensors and wgpu). The SFTP consumer
specifies `"endian": "big"`. The engine reads the annotation and
byte-swaps accordingly.

### 5. Arrays of variable-length-element structs

Deferred for v1. The SFTP `Name` packet has `Vec<File>` where `File`
contains strings — but SFTP serializes this as a sequence of
length-prefixed strings (the serde `SeqAccess` pattern), not as an
array of fixed-stride structs. The typedef engine can handle this by
treating the array as a sequence of length-prefixed elements rather
than a fixed-stride array. The exact mechanism (schema annotation for
"this array has variable-length elements, walk sequentially") is a v2
concern.

### 6. `no_std` compatibility

The `jsonschema` crate requires `alloc` (for `String`, `Vec`,
`HashMap`) but not `std`. The typedef engine's offset computation and
read/write functions operate on `&[u8]` slices — no allocation needed.
The question is whether to target `no_std` + `alloc` or just `std`.
For WASM, `std` is available via `wasm-bindgen`. For embedded, `no_std`
+ `alloc` would be needed.

**Recommendation:** Target `std` for v1. The WASM target has `std`
available. If embedded use cases emerge, `no_std` + `alloc` can be
added as a feature gate later — the engine's core (offset computation,
read/write) is already allocation-free.

### 7. Schema validation at load time vs access time

The jsonschema validator is built once at schema load time
(`validator_for(&schema)?`) and then called repeatedly
(`validator.is_valid(&instance)`). The typedef engine should follow the
same pattern: parse the schema once, build the offset map and validator
once, then use them for repeated read/write/validate operations. The
`TypedefEngine` struct is the compiled form of a schema.

### 8. Error handling strategy

The engine needs clear error types for:
- Schema parsing errors (invalid JSON, missing required keywords,
  unknown TypeDef kinds)
- Offset computation errors (field not found, type not supported for
  offset computation)
- Read/write errors (buffer too short, invalid UTF-8, value out of
  range)
- Validation errors (delegated to jsonschema's `ValidationError`)

**Recommendation:** A single `TypedefError` enum with variants for each
category. The jsonschema `ValidationError` is wrapped as
`TypedefError::Validation(ValidationError)`. Read/write errors carry
the field path for debugging.

---

## References

- **typedef.ts (binary layout types):** `/workspace/@alkdev/typebox/example/typedef/typedef.ts`
  (619 lines) — `TFloat32`, `TInt32`, `TStruct`, `TUnion`, `TEnum` with
  `TypeRegistry.Set<...>` custom validators
- **typebox-rs (prior attempt, to be replaced):** `/workspace/@alkimiadev/typebox-rs/`
  (~8,400 lines) — full TypeBox port with its own jsonschema engine;
  `layout.rs` (193 lines) is the salvageable offset computation
- **alktype (prior attempt, to be replaced):** `/workspace/@alkimiadev/alktype/`
  (~5,600 lines) — handler-registry approach, also built its own
  validation; only intrinsics implemented
- **jsonschema crate (Rust validation):** `/workspace/jsonschema/`
  (v0.46.5, Draft 2020-12 default) — `validator_for(&schema)?`,
  `with_keyword("TypeDef:...", factory)` for custom kinds,
  `with_format("...", validator)` for custom formats
- **russh-sftp protocol packets:** `/workspace/russh-sftp/src/protocol/`
  — 29 packet structs, `Packet` enum with u8 type byte dispatch, custom
  serde Serializer/Deserializer; the primary POC 2 target
- **metatensor format:** `docs/research/alknet-tensor/metatensor-format.md`
  — the format that builds on typedef; 8-byte header + JSON header +
  binary data; flat/struct/blob tensor kinds
- **call-channels-unification findings:** `docs/research/call-channels-unification/findings.md`
  §"alknet-typedef: JSON Schema as the binary struct engine" — the
  origin of this research thread; the `channel_open` marker + typedef
  engine = schema-driven binary data plane
- **stream-unification findings:** `docs/research/stream-unification/findings.md`
  — the research-then-sync pattern precedent; channels as pure channel
  multiplexing (8-byte header, handler-owns-sub-multiplexing)
