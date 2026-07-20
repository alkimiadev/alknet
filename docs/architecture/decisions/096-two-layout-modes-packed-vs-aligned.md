# ADR-096: Two Layout Modes — Packed Sequential vs Aligned Static

## Status
Accepted

## Context

The POCs surfaced that protocols and mmap-friendly formats need different
layout strategies. POC 1 built an aligned `OffsetMap` with natural
alignment padding — correct for mmap-friendly formats (metatensor) but
wrong for protocol wire formats (SFTP, channels, TTY). POC 2 built a
`LayoutBuilder` and `SequentialReader` for packed sequential layouts —
correct for protocol wire formats but wrong for mmap-friendly formats.

This is the most important architectural finding from the POCs. The
engine must support both modes; a single layout strategy cannot serve
both use cases.

### Packed sequential layout (protocol wire formats)

Protocols pack fields sequentially with no alignment padding.
Variable-length fields shift all subsequent fields. Writing requires
knowing actual data sizes upfront; reading walks the buffer sequentially,
reading length prefixes to determine positions.

This is the layout used by SFTP (all strings and byte arrays are
length-prefixed inline), channels (`[channel_id: u32][size: u32][payload]`),
TTY (`[stream_type: u8][length: u32][payload]`), and most binary protocols.

### Aligned static layout (mmap-friendly formats)

Fields have fixed positions with natural alignment padding.
Variable-length fields get a 4-byte length prefix at a known offset; the
variable data is not included in the static layout. This enables
mmap-friendly random access — the consumer can read field N at a known
offset without parsing the fields before it.

This is the layout used by metatensor (blob tensor pattern: index struct
in one region, blob data in another) and safetensors (header + aligned
tensor data).

## Decision

**The typedef engine supports two layout modes, selected by the consumer
at engine construction time:**

### Mode 1: Packed sequential (`LayoutBuilder` / `SequentialReader`)

For protocol wire formats. Fields are packed with no alignment padding.
Variable-length fields shift all subsequent fields.

- **LayoutBuilder** — takes a schema and actual data sizes for
  variable-length fields, computes byte positions for each field in a
  packed layout. Used at write time when the consumer knows the data
  sizes upfront.
- **SequentialReader** — walks a buffer field-by-field according to the
  schema, reading length prefixes to determine variable-length data
  positions. Used at read time when the consumer is parsing an incoming
  frame.

The `LayoutBuilder` and `SequentialReader` are the primary interface for
protocol consumers (SFTP, binary call frames, TTY negotiation).

### Mode 2: Aligned static (`OffsetMap`)

For mmap-friendly formats. Fields have fixed positions with natural
alignment padding. Variable-length fields get a 4-byte length prefix at
a known offset; the variable data is not included in the static layout.

- **OffsetMap** — walks the schema once, computes fixed byte positions
  for each field based on type sizes and alignment. The output is a flat
  table of `(field_path, byte_range)` pairs. Used for both read and write
  at known offsets.

The `OffsetMap` is the primary interface for mmap consumers (metatensor).

### Variable-length handling in each mode

**Packed sequential mode:** Variable-length fields are inline
length-prefixed by default (`[length: u32][data]`). The `LayoutBuilder`
takes the actual data size to compute the length prefix value and the
position of subsequent fields. The `SequentialReader` reads the length
prefix to determine the data extent and the position of the next field.

**Aligned static mode:** Variable-length fields get a 4-byte length
prefix at a known offset. The variable data lives outside the static
layout — either immediately after the fixed fields (inline
length-prefixing) or in a separate data region (offset indirection, the
metatensor blob tensor pattern). The `OffsetMap` records the position of
the length prefix (or the `{offset, length}` pair for offset-indirect
fields).

### Default for variable-length types

Inline length-prefixing (`[length: u32][data]`) is the default for all
variable-length types in both modes. This is the universal pattern used
by channels, SFTP, TTY, and most binary protocols. Offset indirection is
opt-in via the `encoding` annotation (see ADR-097).

## Consequences

### Positive

- **One engine, two modes.** The same schema can be used in either mode.
  A schema describing an SFTP packet can be consumed by a `SequentialReader`
  (for parsing incoming frames) and a `LayoutBuilder` (for constructing
  outgoing frames). A schema describing a metatensor layout can be
  consumed by an `OffsetMap` (for mmap access).
- **Correct for both use cases.** Packed sequential mode produces
  byte-identical output to hand-written protocol serialization (validated
  by POC 2's russh-sftp round-trip tests). Aligned static mode produces
  correct offsets for mmap-friendly access (validated by POC 1's
  alignment tests).
- **No mode confusion.** The consumer explicitly selects the mode at
  engine construction time. A protocol consumer never accidentally gets
  alignment padding; an mmap consumer never accidentally gets
  variable-length field shifting.

### Negative

- **Two APIs to learn.** Consumers must choose between
  `LayoutBuilder`/`SequentialReader` and `OffsetMap`. The choice is
  determined by the use case (protocol vs mmap), not by the schema.
- **Variable-length fields in packed mode require size foreknowledge.**
  The `LayoutBuilder` needs actual data sizes for variable-length fields
  to compute correct positions for subsequent fields. This is inherent
  to packed layouts — the consumer must know the data sizes before
  writing.

## References

- `docs/research/alknet-typedef/findings.md` §"POC Results" — POC 1
  (aligned OffsetMap) and POC 2 (packed LayoutBuilder/SequentialReader)
- [ADR-095](095-alknet-typedef-purpose-scope-jsonschema-engine.md) —
  purpose and scope
- [ADR-097](097-schema-annotations.md) — schema annotations including
  the `encoding` field for variable-length types
