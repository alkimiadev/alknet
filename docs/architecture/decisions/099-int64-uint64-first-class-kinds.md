# ADR-099: Int64/Uint64 as First-Class Kinds

## Status
Accepted

## Context

The typedef engine's kind set (ADR-095, ADR-097) tops out at 32-bit
integers. The POC included `u64` read/write primitives, and the
call-channels-unification research's own SFTP schema example uses
`"TypeDef:Uint64"` for the `offset` field (`Read`/`Write` packets have
`offset: u64`). Metatensor/safetensors `data_offsets` are also `u64`.

A `TypeDef:Uint64` variant was added to the `TypeDefKind` enum during
implementation (the task decomposition correctly identified the gap),
but without an ADR the addition was half-finished: `type_size()`
returned `None`, the layout engines couldn't compute offsets for it, and
the validator didn't register a `TypeDef:Uint64` keyword. The variant
was then removed (commit `14d9cf2`) on the grounds that it was
unintended and a latent panic — but the underlying gap is real: SFTP and
metatensor, the two primary POC targets, both require 64-bit integers.

The presumed reason 64-bit integers were left out of the original
specification is a JSON-level concern: `serde_json::Number` loses
precision past 2^53 when parsing from JSON text. This is a
*validation-layer* caveat, not a *layout-layer* one — the binary layout
is 8 raw bytes, and `from_le_bytes`/`from_be_bytes` work correctly for
the full `u64`/`i64` range. The validation concern is handled by
accepting integer-form JSON values (the `jsonschema` crate's
`as_i64`/`as_u64` methods handle the common range; values past 2^53 are
a JSON representation limitation, not a typedef limitation).

## Decision

**Add `TypeDef:Int64` and `TypeDef:Uint64` as first-class kinds.**

Both are fixed-size (8 bytes), with natural alignment 8. They follow
the schema's endianness annotation like all other fixed-size types.
Read/write is via `data_access::read_i64`/`write_i64`/`read_u64`/
`write_u64` (endian-aware, 8 bytes).

### Kind table additions

| Kind | TypeBox key | Rust type | Size | Alignment |
|------|-------------|-----------|------|-----------|
| `TInt64` | `TypeDef:Int64` | `i64` | 8 | 8 |
| `TUint64` | `TypeDef:Uint64` | `u64` | 8 | 8 |

### Validation

The custom keyword validators check:
- `TypeDef:Int64`: value must be an integer in `i64::MIN..=i64::MAX`
  (`-9223372036854775808` to `9223372036854775807`).
- `TypeDef:Uint64`: value must be a non-negative integer in
  `0..=u64::MAX` (`0` to `18446744073709551615`).

The `jsonschema` crate's `as_i64`/`as_u64` handle the common range.
JSON numbers past 2^53 lose precision in the JSON representation —
this is a JSON limitation, not a typedef limitation. The binary
representation (8 raw bytes) is always exact. A consumer that needs
to validate the full 64-bit range from JSON should provide the value
as a JSON integer (which `serde_json` preserves for values up to
`u64::MAX`/`i64::MIN` when the `arbitrary_precision` feature is
enabled, or when the value fits in `i64`/`u64` without the feature).

### `FieldValue` additions

`FieldValue::I64(i64)` and `FieldValue::U64(u64)` are added to the
unified return type. The `SequentialReader`, `TypedefEngine::read_field`,
and `TypedefEngine::write_field` dispatch on the new kinds.

### Kind count

The engine now has **19** first-class kinds (17 + Int64 + Uint64).
`TypeDefKind::is_fixed_size()` returns `true` for both new kinds.
`type_size()` returns `Some(8)`. `natural_alignment()` returns `8`.
`needs_endian()` returns `true`.

## Consequences

### Positive

- **Unblocks the two primary POC targets.** SFTP `Read`/`Write` packets
  (`offset: u64`) and metatensor `data_offsets` (`u64`) are now
  expressible in typedef schemas.
- **Completes the half-finished addition.** The `TypeDefKind` enum,
  `data_access` primitives, and `FieldValue` variants for 64-bit
  integers now have matching layout, validator, and engine support.
- **No new design surface.** Int64/Uint64 are fixed-size types that
  follow all existing patterns (endianness, alignment, zero-copy
  read/write). They are mechanical additions.

### Negative

- **JSON precision caveat.** Values past 2^53 lose precision in the
  JSON representation (not in the binary representation). This is a
  JSON limitation, not a typedef limitation, but it means the
  validation layer cannot perfectly round-trip the full 64-bit range
  through JSON `Number` without `arbitrary_precision`. In practice,
  SFTP offsets and tensor data offsets are well within 2^53.
- **Two more kinds to maintain.** The kind table, validator
  registration, `FieldValue` enum, and dispatch arms all grow by two
  variants. This is the cost of completeness.

## References

- `docs/research/call-channels-unification/findings.md` §"russh-sftp" —
  the SFTP schema with `"offset": { "TypeDef:Uint64": true }`
- `docs/research/alknet-typedef/findings.md` §"POC 1" — the POC included
  u64 read/write
- [ADR-095](095-alknet-typedef-purpose-scope-jsonschema-engine.md) —
  purpose and scope (the kind set)
- [ADR-097](097-schema-annotations.md) — schema annotations
  (endianness applies to the new kinds)