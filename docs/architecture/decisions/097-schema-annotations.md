# ADR-097: Schema Annotations — Endianness, Alignment, Encoding, and TUnion Discriminators

## Status
Accepted

## Context

The typedef engine needs concrete JSON shapes for schema-level
annotations that control binary layout behavior. The POCs validated the
semantics; this ADR pins the shapes.

Four annotation categories need concrete shapes:

1. **Endianness** — safetensors is little-endian, SFTP is big-endian.
   The engine needs to know which to use.
2. **Alignment** — different backends have different alignment
   requirements (wgpu: 256-byte, protocols: natural, mmap: page).
3. **Variable-length encoding** — inline length-prefixing vs offset
   indirection for strings, byte arrays, and other variable-length types.
4. **TUnion discriminators** — byte-offset (protocol dispatch) vs
   field-name (typedef.ts pattern).

## Decision

### 1. Endianness

**Schema-level annotation with a default of little-endian.**

```json
{
  "TypeDef:Struct": true,
  "endian": "big",
  "properties": { ... }
}
```

- `"endian": "little"` (default) — read/write in little-endian byte order.
- `"endian": "big"` — read/write in big-endian byte order.
- The annotation applies to the entire schema and all nested types.
- Mixed endianness within one schema is not supported (pathological; no
  known protocol requires it).
- The default is little-endian, matching safetensors, wgpu, and most
  modern formats. SFTP consumers specify `"endian": "big"`.

### 2. Alignment

**Both struct-level and field-level, with field-level overriding
struct-level.**

```json
{
  "TypeDef:Struct": true,
  "align": 256,
  "properties": {
    "header": { "TypeDef:Struct": true, "properties": { ... } },
    "weight": { "TypeDef:Float32": true, "align": 16 }
  }
}
```

- Struct-level `"align"` sets the default alignment for all fields in
  that struct. The struct's total size is rounded up to this alignment.
- Field-level `"align"` overrides the struct default for that specific
  field.
- Default alignment (when no annotation is present): 1 for u8/bool, 2
  for u16/i16, 4 for u32/i32/f32, 8 for u64/i64/f64, max field alignment
  for structs.
- Alignment is only meaningful in aligned static mode (ADR-096). In
  packed sequential mode, alignment annotations are ignored — fields are
  packed with no padding.

### 3. Variable-length encoding

**Three strategies for variable-length types, selected by the `encoding`
annotation and the standard JSON Schema `maxLength` keyword.**

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
pattern for fields with known maximum sizes. In packed sequential mode,
`maxLength` is a validation constraint only — the engine still uses
inline length-prefixing (strategy 1).

**Strategy 3: Offset indirection.** The field is a struct
`{offset: u32, length: u32}` that points into a separate data region.
This is the metatensor blob tensor pattern — the index struct lives in
one region, the blob data lives in another. The consumer provides the
data region separately. Enables mmap-friendly random access to
variable-length data without parsing length prefixes and without
reserving worst-case space.

**Default strategy selection:**
- In packed sequential mode: always strategy 1 (inline length-prefixing).
  `maxLength` is a validation constraint only.
- In aligned static mode: strategy 2 (fixed-size reservation) if
  `maxLength` is declared; strategy 3 (offset indirection) if
  `"encoding": "offset-indirect"` is declared; strategy 1 (inline
  length-prefixing) otherwise.

- `true` is a shorthand for the default (length-prefixed). This keeps
  the common case concise and the override explicit.
- The `encoding` annotation and `maxLength` apply to all variable-length
  types: `TypeDef:String`, `TypeDef:Bytes`, `TypeDef:Array`,
  `TypeDef:Record`, `TypeDef:Timestamp`.

### 3a. TRecord value type

`TypeDef:Record` is a string-keyed map. The value type is declared via
the `"values"` property in the schema:

```json
{
  "TypeDef:Record": true,
  "values": { "TypeDef:Float32": true }
}
```

- `"values"` is a schema object declaring the `TypeDef:*` kind of all
  values in the record. All values share the same type.
- The binary layout is a count-prefixed sequence of `(key, value)` pairs:
  `[count: u32][key_len: u32][key_bytes][value_len: u32][value_bytes]...`.
- The count prefix respects the schema's endianness.
- In aligned static mode with `maxLength`, the entire record is reserved
  at `maxLength` bytes (zero-padded).

### 4. TUnion discriminators

**Two discriminator kinds: byte-offset (protocol dispatch) and
field-name (typedef.ts pattern).**

#### Kind A: Byte-offset discriminator

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
    "101": { "$ref": "#/$defs/Status" }
  }
}
```

- The discriminator is a fixed-size integer at a known byte offset.
- `"offset"` is the byte position of the discriminator within the union's
  buffer.
- `"type"` is the `TypeDef:*` kind of the discriminator (typically
  `TypeDef:Uint8` for protocol type bytes).
- The mapping keys are stringified integers (`"1"`, `"5"`, `"101"`).
  The engine parses the key to match the discriminator value.
- The variant struct starts at `offset + discriminator_size`.
- This is the SFTP `Packet` enum pattern and the call protocol's event
  type dispatch.

#### Kind B: Field-name discriminator

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

- The discriminator is a named field within the struct.
- `"name"` is the field name that holds the discriminator value.
- The mapping keys are string values matching the discriminator field's
  value.
- The discriminator field is just another field in the struct — its
  offset is computed like any other field.
- This is the typedef.ts `TUnion` pattern.

#### Mapping values

Mapping values may be either inline schemas or `$ref` pointers. `$ref`
is cleaner for large unions (29 SFTP variants) but requires a `$defs`
section. Inline schemas are simpler for small unions (5 call protocol
event types). Both work.

## Consequences

### Positive

- **Concrete, validated shapes.** All four annotation categories have
  concrete JSON shapes that were validated by the POCs.
- **Sensible defaults.** Little-endian, natural alignment, inline
  length-prefixing — the common case requires no annotations.
- **Explicit overrides.** Big-endian, custom alignment, offset
  indirection — the uncommon case is explicit and self-documenting.
- **TUnion covers both protocol and typedef.ts patterns.** The
  byte-offset discriminator handles SFTP type bytes and call protocol
  event types. The field-name discriminator handles the typedef.ts string
  pattern. No separate union type needed.

### Negative

- **Keyword value shape change.** `"TypeDef:String": true` (boolean) and
  `"TypeDef:String": { "encoding": "length-prefixed" }` (object) are both
  valid. The engine must handle both shapes. This is a minor parsing
  concern — the POC already handles it.
- **Alignment annotations are mode-specific.** Alignment is only
  meaningful in aligned static mode. In packed sequential mode, alignment
  annotations are ignored. This is documented, not enforced — a consumer
  that specifies alignment in packed mode gets no error, just no effect.

## References

- `docs/research/alknet-typedef/findings.md` §"Open Questions" — the
  annotation shape questions this ADR resolves
- [ADR-095](095-alknet-typedef-purpose-scope-jsonschema-engine.md) —
  purpose and scope
- [ADR-096](096-two-layout-modes-packed-vs-aligned.md) — the two layout
  modes (alignment only meaningful in aligned static mode)
