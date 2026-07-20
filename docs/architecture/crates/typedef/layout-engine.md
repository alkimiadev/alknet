---
status: draft
last_updated: 2026-07-20
---

# alknet-typedef — Layout Engine

The layout engine: offset computation, the two layout modes (packed
sequential vs aligned static), alignment, endianness, and variable-length
field handling. This is the novel code — the recursive walk of the schema
JSON that computes byte positions for each field.

## The Two Layout Modes

The POCs surfaced that protocols and mmap-friendly formats need different
layout strategies. This is the most important architectural finding —
decided in [ADR-096](../../decisions/096-two-layout-modes-packed-vs-aligned.md).

### Mode 1: Packed sequential (protocol wire formats)

Fields are packed with no alignment padding. Variable-length fields shift
all subsequent fields. Used by SFTP, channels, TTY, and most binary
protocols.

**Components:**

- **`LayoutBuilder`** — takes a schema and actual data sizes for
  variable-length fields, computes byte positions for each field in a
  packed layout. Used at write time when the consumer knows the data
  sizes upfront.
- **`SequentialReader`** — walks a buffer field-by-field according to the
  schema, reading length prefixes to determine variable-length data
  positions. Used at read time when the consumer is parsing an incoming
  frame.

**How it works:**

For a struct with fields `[u8, u32, string]`:

```
LayoutBuilder (write):
  field[0] u8:     offset 0, size 1
  field[1] u32:    offset 1, size 4
  field[2] string: offset 5, size 4 (length prefix) + data_len
  total: 9 + data_len

SequentialReader (read):
  read u8 at offset 0
  read u32 at offset 1
  read u32 length prefix at offset 5 → data_len
  read string data at offset 9, length data_len
  next field at offset 9 + data_len
```

There is no alignment padding. The `u32` at offset 1 is unaligned — this
is correct for protocol wire formats, which pack fields tightly.

**Variable-length fields in packed mode:**

The `LayoutBuilder` takes actual data sizes for variable-length fields
to compute correct positions for subsequent fields. The consumer must
know the data sizes before writing — this is inherent to packed layouts.

The `SequentialReader` reads each field's length prefix to determine the
data extent and the position of the next field. The reader walks the
buffer sequentially; it cannot jump to field N without reading fields
0..N-1 first.

### Mode 2: Aligned static (mmap-friendly formats)

Fields have fixed positions with natural alignment padding.
Variable-length fields get a 4-byte length prefix at a known offset; the
variable data is not included in the static layout. Used by metatensor
and safetensors.

**Component:**

- **`OffsetMap`** — walks the schema once, computes fixed byte positions
  for each field based on type sizes and alignment. The output is a flat
  table of `(field_path, byte_range)` pairs. Used for both read and write
  at known offsets.

**How it works:**

For a struct with fields `[u8, u32, f32]` and natural alignment:

```
OffsetMap:
  field[0] u8:  offset 0, size 1
  field[1] u32: offset 4, size 4  (3 bytes padding after u8)
  field[2] f32: offset 8, size 4
  total: 12 (struct aligned to 4)
```

The `u32` is aligned to offset 4 (its natural alignment). The consumer
can read `field[1]` at offset 4 without reading `field[0]` first — random
access by field path.

**Variable-length fields in aligned mode:**

Variable-length fields get a 4-byte length prefix at a known offset. The
variable data lives outside the static layout — either immediately after
the fixed fields (inline length-prefixing) or in a separate data region
(offset indirection). The `OffsetMap` records the position of the length
prefix (or the `{offset, length}` pair for offset-indirect fields).

For inline length-prefixing, the variable data follows the fixed fields
but is not included in the `OffsetMap`'s field ranges. The consumer reads
the length prefix from the `OffsetMap`'s known offset, then slices the
data region.

For offset indirection, the field is a struct `{offset: u32, length: u32}`
at a known position in the `OffsetMap`. The consumer reads the offset and
length, then slices the separate data region.

## Offset Computation Algorithm

The offset computation is a recursive walk of the schema JSON. The
algorithm is the same for both modes; the difference is whether alignment
padding is inserted between fields.

### Fixed-size types

For each fixed-size type, the algorithm:
1. Determines the type's byte size from the `TypeDef:*` kind.
2. In aligned mode: inserts padding to satisfy the type's alignment
   (or the field's `align` annotation, or the struct's `align` default).
3. Records the field's `(start, end)` range.
4. Advances the current offset by the type's size.

### Composite types

**`TStruct`:** Recurse into the struct's `properties`. The inner fields
are computed relative to the struct's start offset. The struct's total
size is the sum of its fields' sizes (plus alignment padding in aligned
mode). The struct itself may have an `align` annotation that rounds up
its total size.

**`TUnion`:** The discriminator occupies `offset..offset + discriminator_size`
bytes. For byte-offset discriminators, the variant struct starts at
`offset + discriminator_size`. For field-name discriminators, the
discriminator is just another field — its offset is computed like any
other field, and the variant struct follows at the end of the
discriminator field.

In aligned static mode, the union's total size is `discriminator_size +
max(variant_sizes)`, where variant sizes are computed from the schema
(variable-length data lives outside the static layout).

In packed sequential mode, variant sizes depend on the actual sizes of
variable-length fields within each variant, which aren't known at schema
time. The `LayoutBuilder` takes the actual variant discriminator value
and data sizes at write time, computes the size of the selected variant,
and uses that for the union's total size. The `SequentialReader` reads
the discriminator first, looks up the variant schema, then reads the
variant struct sequentially — it doesn't need to know the union's total
size upfront.

**`TArray` of fixed-size elements:** Element stride = element size (plus
alignment padding in aligned mode). Element `i` starts at
`array_offset + i × stride`. The array's total size is `count × stride`.

**`TArray` of variable-length-element structs:** Deferred for v1
(OQ-069).

### Variable-length types

The typedef engine supports three strategies for variable-length types
(see [schema-layer.md](schema-layer.md) §Variable-length types and
[ADR-097](../../decisions/097-schema-annotations.md) §3 for the full
annotation shapes).

**Strategy 1: Inline length-prefixing (default).**
1. Records the position of the 4-byte length prefix.
2. In aligned mode: the length prefix is aligned; the variable data is
   not included in the static layout.
3. In packed mode: the `LayoutBuilder` takes the actual data size to
   compute the length prefix value and the position of subsequent fields.
   The `SequentialReader` reads the length prefix to determine the data
   extent and the position of the next field.

**Strategy 2: Fixed-size reservation (`maxLength`).**
1. In aligned static mode: reserves `maxLength` bytes at a fixed offset.
   Data shorter than `maxLength` is zero-padded. Subsequent fields have
   known, unchanging offsets — the field is fixed-size from the layout
   perspective. This is the database `VARCHAR(N)` pattern.
2. In packed sequential mode: `maxLength` is a validation constraint
   only. The engine uses strategy 1 (inline length-prefixing) because
   protocols don't benefit from fixed-size reservation.

**Strategy 3: Offset indirection (`"encoding": "offset-indirect"`).**
1. The field is a struct `{offset: u32, length: u32}`.
2. The `OffsetMap` records the position of this struct.
3. The consumer provides the data region separately. This is the
   metatensor blob tensor pattern — the index struct lives in one region,
   the blob data lives in another.

### Nested structs and field paths

Nested structs produce dotted field paths: `header.version`,
`header.magic`. The offset computation propagates the field path prefix
during recursion. The `OffsetMap` stores fully-qualified paths.

### Endianness

Endianness is per-schema (ADR-097). The offset computation is
endian-agnostic — it computes byte positions, not byte values. The
read/write functions apply endianness when converting between bytes and
typed values. The engine reads the `"endian"` annotation from the schema
and byte-swaps accordingly.

## Mode Selection

The consumer selects the mode at engine construction time. The choice is
determined by the use case, not by the schema:

- **Protocol consumer** (SFTP, binary call frames, TTY negotiation):
  uses `LayoutBuilder` for writing and `SequentialReader` for reading.
- **mmap consumer** (metatensor): uses `OffsetMap` for both reading and
  writing.

The same schema can be used in either mode. A schema describing an SFTP
packet can be consumed by a `SequentialReader` (for parsing incoming
frames) and a `LayoutBuilder` (for constructing outgoing frames). A schema
describing a metatensor layout can be consumed by an `OffsetMap` (for
mmap access).

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| Two layout modes | [ADR-096](../../decisions/096-two-layout-modes-packed-vs-aligned.md) | Packed sequential for protocols; aligned static for mmap formats |
| Schema annotations | [ADR-097](../../decisions/097-schema-annotations.md) | Endianness, alignment, encoding annotations that control layout behavior |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-069** (deferred(scope)): Arrays of variable-length-element structs
  — requires lazy walking logic; blocked on a concrete consumer that
  needs it.

## References

- `docs/research/alknet-typedef/findings.md` §"POC Results" — POC 1
  (aligned OffsetMap) and POC 2 (packed LayoutBuilder/SequentialReader)
- [ADR-096](../../decisions/096-two-layout-modes-packed-vs-aligned.md) —
  the two layout modes decision
- [ADR-097](../../decisions/097-schema-annotations.md) — schema
  annotations
- [schema-layer.md](schema-layer.md) — the 17 TypeDef kinds and their
  byte sizes
- [data-access.md](data-access.md) — read/write functions that use the
  computed offsets
