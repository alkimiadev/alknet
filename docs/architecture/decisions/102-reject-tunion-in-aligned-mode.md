# ADR-102: Reject TUnion in Aligned Mode for v1

## Status
Accepted

## Context

The aligned static layout mode (ADR-096) computes fixed byte positions
for each field, enabling random access by field path. `TUnion` in
aligned mode has three implementation problems:

1. **No variant field offsets.** Only the `__discriminator` byte range
   is recorded in the `OffsetMap`. Variant field offsets are not
   available anywhere in aligned mode — the consumer must recompute
   them by hand. This makes `TypedefEngine::read_field` on a union
   variant field impossible.

2. **`find_discriminator_field` takes the first variant's offset.** For
   a field-name discriminator, the code probes the first variant that
   contains the discriminator field and records that offset globally.
   If variants order fields differently, the discriminator sits at
   different offsets per variant and the recorded range is silently
   wrong. The code should validate that the offset is identical across
   all variants (or require the discriminator field to be first).

3. **Byte-discriminator union total misaligns the variant.** The union
   total is `disc_off + disc_size + variant_max_size`, but the variant
   was probed from offset 0 with alignment. A `u8` discriminator
   before a `u32`-bearing variant produces a variant region that
   starts at an unaligned offset in a mode whose entire purpose is
   alignment.

The real question is whether `TUnion` in aligned mode is even needed.
The two consumer profiles are:

- **Protocol consumers** (SFTP, call protocol event types): use packed
  sequential mode. `TUnion` with byte-offset discriminators is the
  core dispatch mechanism. This is well-supported.
- **mmap consumers** (metatensor, safetensors): use aligned static
  mode. These formats are structs and arrays of structs — they don't
  use tagged unions. A tensor file has a header struct with tensor
  descriptors, not a "which variant is this?" dispatch.

`TUnion` in aligned mode is a combination that no current or planned
consumer needs. Shipping broken semantics for an unused use case is
worse than rejecting it clearly.

## Decision

**Reject `TUnion` in aligned static mode for v1.**

`OffsetMap::compute` returns `TypedefError::Offset` when it encounters
a `TypeDef:Union` field, with a message explaining that unions are not
supported in aligned mode and the consumer should use packed mode (or
restructure as a struct with an explicit discriminator field).

This is a schema-load-time rejection. The consumer learns about the
problem when compiling the schema, not at runtime.

### What is NOT rejected

- `TUnion` in packed sequential mode — this is the core use case
  (SFTP `Packet` dispatch, call protocol event types) and is fully
  supported by `LayoutBuilder` and `SequentialReader`.
- `TStruct`, `TArray`, and all primitive kinds in aligned mode — these
  are the mmap-format primitives and are fully supported.

### Reversal

This is a two-way door. If a future mmap-format consumer needs tagged
unions in aligned mode, the rejection can be lifted and the three
implementation problems fixed. The fix would require:
- Recording per-variant field offsets in the `OffsetMap` (which
  variant's offsets to record when variants have different layouts?).
- Validating that field-name discriminators have identical offsets
  across all variants.
- Aligning the variant region correctly after the byte discriminator.

These are design questions that should be answered when the use case
arrives, not speculatively now.

## Consequences

### Positive

- **No broken semantics shipped.** The three implementation problems
  are removed from the API surface rather than silently producing
  wrong offsets.
- **Clear scope boundary.** Aligned mode is for structs and arrays;
  packed mode is for protocols (including union dispatch). The
  consumer chooses the mode based on the use case.
- **Reversible.** When a real consumer needs aligned-mode unions, the
  rejection is lifted and the design questions are worked through with
  a concrete use case.

### Negative

- **A schema with a `TUnion` field cannot be compiled in aligned
  mode.** A consumer that wants both aligned layout and union dispatch
  must use packed mode or restructure. No current consumer needs this.
- **The aligned-mode union code in `offset_map.rs` is dead.** It can
  be removed or left as a reference for when the rejection is lifted.
  Removing it is cleaner.

## References

- [ADR-096](096-two-layout-modes-packed-vs-aligned.md) — the two layout
  modes
- [ADR-097](097-schema-annotations.md) §4 — TUnion discriminators
- `docs/architecture/crates/typedef/layout-engine.md` §"TUnion" —
  aligned-mode union sizing