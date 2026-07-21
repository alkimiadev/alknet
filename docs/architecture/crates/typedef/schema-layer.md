---
status: draft
last_updated: 2026-07-20
---

# alknet-typedef — Schema Layer

The schema layer: the 17 `TypeDef:*` custom type kinds, their mapping to
Rust types and byte sizes, the `jsonschema` custom keyword integration,
TypeBox interop, and the concrete JSON shapes for schema-level annotations.

## The 17 TypeDef Kinds

These are the custom schema kinds defined in TypeBox's `typedef.ts`
(`/workspace/@alkdev/typebox/example/typedef/typedef.ts`, 619 lines) and
ported to Rust via `jsonschema` custom keywords. Each kind carries binary
layout semantics — a known byte size (for fixed-size types) or a known
encoding strategy (for variable-length types).

| Kind | TypeBox key | Rust type | Size | Category |
|------|-------------|-----------|------|----------|
| `TFloat32` | `TypeDef:Float32` | `f32` | 4 | fixed |
| `TFloat64` | `TypeDef:Float64` | `f64` | 8 | fixed |
| `TInt8` | `TypeDef:Int8` | `i8` | 1 | fixed |
| `TInt16` | `TypeDef:Int16` | `i16` | 2 | fixed |
| `TInt32` | `TypeDef:Int32` | `i32` | 4 | fixed |
| `TUint8` | `TypeDef:Uint8` | `u8` | 1 | fixed |
| `TUint16` | `TypeDef:Uint16` | `u16` | 2 | fixed |
| `TUint32` | `TypeDef:Uint32` | `u32` | 4 | fixed |
| `TBoolean` | `TypeDef:Boolean` | `bool` (0x00=false, 0x01=true) | 1 | fixed |
| `TString` | `TypeDef:String` | length-prefixed UTF-8 | variable | variable |
| `TBytes` | `TypeDef:Bytes` | length-prefixed raw bytes | variable | variable |
| `TStruct` | `TypeDef:Struct` | record of fields | sum of field sizes | composite |
| `TUnion` | `TypeDef:Union` | tagged union | discriminator + variant | composite |
| `TArray` | `TypeDef:Array` | repeated element | count × element size | composite |
| `TEnum` | `TypeDef:Enum` | u32 index into enum values | 4 (fixed) | fixed |
| `TRecord` | `TypeDef:Record` | count-prefixed sequence of (key, value) pairs | variable | variable |
| `TTimestamp` | `TypeDef:Timestamp` | length-prefixed RFC 3339 string | variable | variable |

### Fixed-size types

`TFloat32`, `TFloat64`, `TInt8`, `TInt16`, `TInt32`, `TUint8`, `TUint16`,
`TUint32`, `TBoolean`, and `TEnum` have known byte sizes. The offset
computation uses these sizes directly. Read/write is zero-copy pointer
cast for these types.

**`TBoolean` byte representation:** `0x00` = false, `0x01` = true. Other
values are invalid and produce a `TypedefError::Access` on read.

**`TEnum` binary representation:** A `u32` index into the enum's declared
values, in declaration order. The first declared value is index 0, the
second is index 1, etc. The enum's values are declared via the standard
JSON Schema `"enum"` keyword (e.g., `"enum": ["read", "write", "execute"]`).
The `TypeDef:Enum` custom keyword signals that the type is an enum for
layout purposes; the built-in `enum` keyword provides the value list.

**Design note:** TypeBox's `TEnum` is a string enum (variable-length). The
typedef engine uses a `u32` index instead — a deliberate deviation from
TypeBox fidelity in favor of binary efficiency. Most enums have a small
number of variants (e.g., the call protocol's 5 event types); a `u32`
index is compact, fixed-size, and sufficient for any realistic enum. The
JSON representation (for validation) remains a string; the binary
representation is the `u32` index.
The `u32` index follows the schema's endianness annotation (ADR-097), like
all other fixed-size types. In little-endian mode the index is
`u32::from_le_bytes`; in big-endian mode it is `u32::from_be_bytes`.

### Variable-length types

`TString`, `TBytes`, `TRecord`, and `TTimestamp` have variable byte sizes.
The typedef engine supports three strategies for handling variable-length
types in binary layouts, selected by the `encoding` annotation and the
standard JSON Schema `maxLength` keyword:

| Strategy | Encoding annotation | Layout behavior | Use case |
|----------|-------------------|-----------------|----------|
| **Inline length-prefixed** | `"length-prefixed"` (default) | `[length: u32][data]`; shifts subsequent fields in packed mode | Protocol wire formats (SFTP, channels, TTY) |
| **Fixed-size reservation** | (none — uses `maxLength`) | `[data: maxLength bytes]`, zero-padded; fixed offset in aligned mode | mmap-friendly formats where max size is known (database `VARCHAR(N)` pattern) |
| **Offset indirection** | `"offset-indirect"` | `{offset: u32, length: u32}` pointing into a separate data region | Blob tensors, metatensor variable-length data (the blob tensor pattern) |

**Strategy 1: Inline length-prefixing (default).** The field's fixed
portion is a 4-byte length prefix at a computed offset. The variable data
follows immediately after. In packed sequential mode, the length prefix
determines the position of subsequent fields. In aligned static mode, the
length prefix is at a known offset; the variable data is not included in
the static layout. This is the universal pattern used by channels, SFTP,
TTY, and most binary protocols.

**Strategy 2: Fixed-size reservation.** When a variable-length field
declares `maxLength` (a standard JSON Schema keyword), the engine reserves
`maxLength` bytes at a fixed offset in aligned static mode. Data shorter
than `maxLength` is zero-padded; data longer than `maxLength` is a
validation error. This makes the field fixed-size from the layout
perspective — subsequent fields have known, unchanging offsets. This is
the database `VARCHAR(N)` pattern and the metatensor struct-tensor
pattern for fields with known maximum sizes.

In packed sequential mode, `maxLength` is a validation constraint only —
the engine still uses inline length-prefixing (strategy 1) because
protocols don't benefit from fixed-size reservation.

**Strategy 3: Offset indirection.** The field is a struct
`{offset: u32, length: u32}` at a known position. The consumer provides
the data region separately; the engine reads the offset and length, then
slices the data region. This is the metatensor blob tensor pattern — the
index struct lives in one region, the blob data lives in another. Enables
mmap-friendly random access to variable-length data without parsing
length prefixes and without reserving worst-case space.

**Default strategy selection:**
- In packed sequential mode: always strategy 1 (inline length-prefixing).
  `maxLength` is a validation constraint only.
- In aligned static mode: strategy 2 (fixed-size reservation) if
  `maxLength` is declared; strategy 3 (offset indirection) if
  `"encoding": "offset-indirect"` is declared; strategy 1 (inline
  length-prefixing) otherwise.

**Length prefix endianness:** The 4-byte length prefix (strategies 1 and 3)
respects the schema's `"endian"` annotation (ADR-097). In little-endian
mode, the length is `u32::from_le_bytes`. In big-endian mode, the length
is `u32::from_be_bytes`. This ensures SFTP consumers (big-endian) have
consistent byte order for both field values and length prefixes.

**`TBytes`:** Raw bytes — no UTF-8 constraint. The payload is `&[u8]`.
Otherwise identical to `TString` in layout (same three strategies).

**Design note:** `TypeDef:Bytes` is an alknet-typedef addition — it does
not exist in TypeBox's `typedef.ts` (which defines 16 kinds). It is
included because raw byte arrays are a common binary protocol primitive
(SFTP data payloads, channels payloads, tensor data) and are semantically
distinct from UTF-8 strings. In the binary representation, TBytes is raw
bytes with no encoding (not base64, not hex). In the JSON representation
(for validation), TBytes is a string (JSON has no native byte type).

**`TRecord`:** A string-keyed map. Binary layout is a count-prefixed
sequence of `(key, value)` pairs: `[count: u32][key_len: u32][key_bytes]
[value_len: u32][value_bytes]...`. The count is the number of entries.
Each key is a length-prefixed UTF-8 string. Each value is the record's
declared value type (specified via the `"values"` property in the schema,
e.g., `"values": { "TypeDef:Float32": true }`). The count prefix respects
the schema's endianness. In aligned static mode with `maxLength`, the
entire record is reserved at `maxLength` bytes (zero-padded).

**`TTimestamp`:** An RFC 3339 timestamp string (the internet profile of
ISO 8601). Stored as a length-prefixed UTF-8 string (strategy 1) or
fixed-size reservation (strategy 2 with `maxLength`). The data-access
layer treats timestamps as opaque length-prefixed strings — it does not
parse or validate the timestamp format. The jsonschema custom keyword
validator checks RFC 3339 conformance at the JSON level (see
[validation.md](validation.md)).

`TArray` is variable-length when the element type is variable-length or
when the count is not known at schema time. For fixed-size element arrays
with a known count, the size is `element_size × count`.

**`TArray` count declaration:** The array count is declared via the
standard JSON Schema `"minItems"` and `"maxItems"` keywords. When
`minItems == maxItems`, the array has a fixed count known at schema time.
When they differ or are absent, the count is variable and the array uses
a length-prefixed encoding: `[count: u32][element_0]...[element_N]`.
The count prefix respects the schema's endianness.

### Composite types

`TStruct` and `TUnion` are composite — their size is the sum of their
fields' sizes (plus alignment padding in aligned static mode). The offset
computation recurses into their properties.

## jsonschema Custom Keyword Integration

The `jsonschema` crate (v0.46.5, Draft 2020-12) supports custom keywords
via the `with_keyword` API. Each `TypeDef:*` kind is registered as a
custom keyword:

```rust
let validator = jsonschema::options()
    .with_keyword("TypeDef:Float32", factory)
    .with_keyword("TypeDef:Int32", factory)
    .with_keyword("TypeDef:Struct", factory)
    // ... all 17 kinds
    .build(&schema)?;
```

The factory closure receives the parent schema object, the keyword's
value, and the schema path — enabling cross-keyword awareness. The
`TypeDef:Struct` validator, for example, inspects the parent's
`properties` to validate each field against its declared `TypeDef:*` kind.

Each custom keyword implementation is ~10 lines. The `jsonschema` crate
handles all structural validation (object properties, required fields,
array items, enum values) — the custom keywords only need to validate
the leaf type constraints. See [validation.md](validation.md) for the
validator implementations.

This is the same pattern as TypeBox's `TypeRegistry.Set` on the JS side.
Same semantics, different language, same JSON Schema wire format. A
TypeBox schema serialized to JSON feeds into the typedef engine after a
single pre-processing step: normalizing `$ref` values (see below).

## TypeBox Interop

TypeBox modules render to standard JSON Schema under `$defs`. A TypeBox
schema like:

```typescript
const TensorRef = Type.Object({
  dtype: Type.Union([Type.Literal("F32"), Type.Literal("I16")]),
  shape: Type.Array(Type.Number()),
  data_offsets: Type.Tuple([Type.Number(), Type.Number()])
});
```

serialized to JSON is a standard JSON Schema with `type: "object"`,
`properties`, and `required`. That JSON feeds into the typedef engine
after `$ref` normalization. The `TypeDef:*` custom keywords are added by
TypeBox's `TypeRegistry.Set` — they appear in the serialized JSON as
additional properties on the schema object.

### `$ref` normalization

TypeBox generates bare-name `$ref` values (e.g., `"$ref": "Read"`),
referencing sibling definitions within the same `$defs` block. The
`jsonschema` crate requires full JSON Pointer paths (e.g.,
`"$ref": "#/$defs/Read"`). The typedef engine normalizes TypeBox-style
refs at schema load time:

```rust
fn normalize_refs(schema: &mut Value) {
    // Walk the schema tree. For every "$ref" whose value is a bare name
    // (no "#" prefix), rewrite it to "#/$defs/<name>".
    // "$ref": "Read"  →  "$ref": "#/$defs/Read"
}
```

This is a ~20-line recursive walk of the schema JSON. It runs once at
load time, before the schema is passed to `jsonschema::validator_for`
or the offset computation. The normalization is idempotent — full JSON
Pointer refs pass through unchanged.

**Verification:** The jsonschema crate (v0.46.5) rejects bare-name refs
with `Resource 'Read' is not present in a registry`. Full JSON Pointer
refs (`#/$defs/Read`) resolve correctly. The normalization step bridges
the gap between TypeBox's output and jsonschema's input.

The typedef engine does not depend on TypeBox or any JS toolchain. It
consumes JSON — whether that JSON was authored in TypeBox, generated by
a ujsx component, or hand-written. The schema is the interface.

## Schema Annotations

Schema-level annotations control binary layout behavior. These are
decided in [ADR-097](../../decisions/097-schema-annotations.md).

### Endianness

Schema-level annotation with a default of little-endian:

```json
{ "TypeDef:Struct": true, "endian": "big", "properties": { ... } }
```

- `"endian": "little"` (default) — read/write in little-endian byte order.
- `"endian": "big"` — read/write in big-endian byte order.
- Applies to the entire schema and all nested types.

### Alignment

Both struct-level and field-level, with field-level overriding:

```json
{
  "TypeDef:Struct": true,
  "align": 256,
  "properties": {
    "weight": { "TypeDef:Float32": true, "align": 16 }
  }
}
```

- Struct-level `"align"` sets the default for all fields.
- Field-level `"align"` overrides the struct default.
- Default alignment: 1 for u8/bool, 2 for u16/i16, 4 for u32/i32/f32,
  8 for u64/i64/f64, max field alignment for structs.
- Only meaningful in aligned static mode (ADR-096). Ignored in packed
  sequential mode.

### Variable-length encoding

The typedef engine supports three strategies for variable-length types
(see §Variable-length types above for full details). The strategy is
selected by the `encoding` annotation and the standard JSON Schema
`maxLength` keyword:

```json
// Strategy 1: Inline length-prefixing (default, shorthand)
{ "TypeDef:String": true }

// Strategy 1: Explicit inline length-prefixing
{ "TypeDef:String": { "encoding": "length-prefixed" } }

// Strategy 2: Fixed-size reservation (uses standard maxLength)
{ "TypeDef:String": true, "maxLength": 256 }

// Strategy 3: Offset indirection (opt-in)
{ "TypeDef:String": { "encoding": "offset-indirect" } }
```

- `"encoding": "length-prefixed"` (default) — 4-byte length prefix at
  computed offset, variable data follows immediately. Used by protocol
  wire formats.
- `maxLength` (standard JSON Schema keyword) — in aligned static mode,
  reserves `maxLength` bytes at a fixed offset (zero-padded). Makes the
  field fixed-size from the layout perspective. In packed sequential
  mode, `maxLength` is a validation constraint only.
- `"encoding": "offset-indirect"` — field is a struct
  `{offset: u32, length: u32}` pointing into a separate data region.
  The consumer provides the data region separately. Used by metatensor
  blob tensors.
- Applies to all variable-length types: `TypeDef:String`, `TypeDef:Bytes`,
  `TypeDef:Array`, `TypeDef:Record`, `TypeDef:Timestamp`.

### TUnion discriminators

Two discriminator kinds: byte-offset (protocol dispatch) and field-name
(typedef.ts pattern).

**Byte-offset discriminator** (SFTP type bytes, call protocol event types):

```json
{
  "TypeDef:Union": true,
  "discriminator": {
    "kind": "byte",
    "offset": 0,
    "type": "TypeDef:Uint8"
  },
  "mapping": {
    "5": { "$ref": "#/$defs/Read" },
    "6": { "$ref": "#/$defs/Write" },
    "101": { "$ref": "#/$defs/Status" }
  }
}
```

- `"offset"` — byte position of the discriminator.
- `"type"` — the `TypeDef:*` kind of the discriminator (typically
  `TypeDef:Uint8`).
- Mapping keys are stringified integers. The variant struct starts at
  `offset + discriminator_size`.

**Field-name discriminator** (typedef.ts pattern):

```json
{
  "TypeDef:Union": true,
  "discriminator": {
    "kind": "field",
    "name": "type"
  },
  "mapping": {
    "read": { "$ref": "#/$defs/Read" },
    "write": { "$ref": "#/$defs/Write" }
  }
}
```

- `"name"` — the field name holding the discriminator value.
- Mapping keys are string values matching the discriminator field's value.
- The discriminator field is just another field in the struct.

Mapping values may be either inline schemas or `$ref` pointers. Both work.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| Schema annotations | [ADR-097](../../decisions/097-schema-annotations.md) | Concrete JSON shapes for endianness, alignment, encoding, and TUnion discriminators |
| Purpose and scope | [ADR-095](../../decisions/095-alknet-typedef-purpose-scope-jsonschema-engine.md) | Why jsonschema not a custom engine; "schema is the format" principle |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-071** (deferred(scope)): Builder API for schema construction.

## References

- `/workspace/@alkdev/typebox/example/typedef/typedef.ts` — the TypeBox
  schema kinds (619 lines)
- `/workspace/jsonschema/` — the jsonschema crate (v0.46.5, Draft 2020-12)
- [ADR-097](../../decisions/097-schema-annotations.md) — schema
  annotation shapes
- [validation.md](validation.md) — custom keyword validator implementations
