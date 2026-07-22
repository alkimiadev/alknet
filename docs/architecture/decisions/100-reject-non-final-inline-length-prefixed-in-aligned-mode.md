# ADR-100: Reject Non-Final Inline Length-Prefixed Variable Fields in Aligned Mode

## Status
Accepted

## Context

The aligned static layout mode (ADR-096) is designed for mmap-friendly
formats: fields have fixed positions with natural alignment padding,
enabling random access by field path without parsing preceding fields.

The spec (layout-engine.md) says variable-length fields in aligned mode
get a 4-byte length prefix at a known offset, and "the variable data
lives outside the static layout — either immediately after the fixed
fields (inline length-prefixing) or in a separate data region (offset
indirection)."

The implementation has a bug: `OffsetMap::compute` reserves only 4 bytes
for an inline length-prefixed variable field (the length prefix), but
`TypedefEngine::write_field` for a `String`/`Bytes` field calls
`data_access::write_string` at `range.start`, which writes
`[4-byte length][data]` inline — clobbering every subsequent field. The
`read_field` path has the mirror behavior (reads inline), so the engine
is self-consistent but only works correctly when the variable field is
the last field in the struct (no subsequent field to clobber).

Concretely, `{name: String, id: Uint32}` in aligned mode maps
`name → 0..4`, `id → 4..8`. Writing `"hello"` to `name` writes
`[5,0,0,0,h,e,l,l,o]` at offset 0, overwriting `id`'s range with
`hello`. All existing tests happen to put the variable field last, so
the bug is latent.

The spec's "data region after fixed fields" model (where variable data
lives after all fixed fields) is the correct design for aligned mode,
but implementing it would require a two-region layout (fixed fields +
variable data region) with the `OffsetMap` tracking both the prefix
position and the data position. This is a significant design addition
for a use case that doesn't exist yet — real aligned-format consumers
(metatensor, safetensors) use `maxLength` reservation or
`offset-indirect` encoding for variable data, not inline
length-prefixing.

## Decision

**Reject non-final inline length-prefixed variable fields in aligned
static mode at `OffsetMap::compute` time.**

A variable-length field (`TypeDef:String`, `TypeDef:Bytes`,
`TypeDef:Timestamp`, `TypeDef:Record`) in aligned static mode that uses
the default inline length-prefixing strategy (no `maxLength`, no
`offset-indirect`) must be the last field in its struct. If a non-final
inline length-prefixed variable field is encountered,
`OffsetMap::compute` returns `TypedefError::Offset` with a message
explaining that non-final variable fields in aligned mode require
`maxLength` (fixed-size reservation) or `"encoding": "offset-indirect"`
(offset indirection).

This is a validation-time rejection (schema load time), not a runtime
check. The consumer learns about the problem when compiling the schema,
not when writing data.

### What is NOT rejected

- Inline length-prefixed variable fields that are the last field in
  their struct — these are fine (no subsequent field to clobber).
- `maxLength` reservation and `offset-indirect` encoding in any
  position — these make the field fixed-size from the layout
  perspective (known size at a known offset), so they don't clobber.
- Inline length-prefixed variable fields in packed sequential mode —
  packed mode doesn't have fixed offsets; variable fields shift
  subsequent fields by design.

## Consequences

### Positive

- **Eliminates a silent data-corruption bug.** A consumer that writes
  a non-final string in aligned mode currently clobbers subsequent
  fields with no error. After this fix, the schema is rejected at
  compile time.
- **Matches real aligned-format usage.** mmap-friendly formats use
  `maxLength` or `offset-indirect` for variable data; inline
  length-prefixing in aligned mode is only meaningful as the last
  field.
- **Simple to implement.** A single check in `compute_struct` (is this
  variable field non-final and using inline length-prefixing? → reject).
  No two-region layout needed.
- **Defers the two-region design without blocking consumers.** If a
  future consumer needs inline length-prefixing in non-final position
  in aligned mode, the two-region layout can be implemented then. The
  rejection is reversible (remove the check, add the two-region logic).

### Negative

- **A schema that worked before (silently corrupting data) now fails
  at compile time.** This is the correct behavior — the schema was
  always broken, it just wasn't caught.
- **The "data region after fixed fields" model from the spec is not
  implemented.** A consumer that wants inline variable data in a
  non-final position must use packed mode or wait for the two-region
  layout. This is acceptable for v1 — no current consumer needs it.

## References

- [ADR-096](096-two-layout-modes-packed-vs-aligned.md) — the two layout
  modes (aligned static mode's variable-length handling)
- [ADR-097](097-schema-annotations.md) — the three variable-length
  encoding strategies (`maxLength`, `offset-indirect`, inline
  length-prefixing)
- `docs/architecture/crates/typedef/layout-engine.md` §"Variable-length
  fields in aligned mode" — the spec's "data region after fixed fields"
  description