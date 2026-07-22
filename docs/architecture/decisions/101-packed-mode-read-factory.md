# ADR-101: Packed-Mode Read API — Engine as SequentialReader Factory

## Status
Accepted

## Context

`TypedefEngine` stores a `SequentialReader` inside its `Layout::Packed`
variant. The engine exposes it via
`engine.sequential_reader() -> Option<&SequentialReader>`.

The problem: `SequentialReader`'s read methods (`read_next`,
`read_field`, `reset`) all take `&mut self` — they mutate the reader's
internal cursor (`field_index`, `position`). But the engine hands out
`&SequentialReader` (a shared reference), which cannot be used to call
`&mut self` methods. The accessor can only give the consumer
`position()` and `endian()` (the `&self` methods) — the actual read
API is unreachable.

This makes the engine's packed read-side dead API. A consumer that
wants to read a packed buffer must construct their own
`SequentialReader::new(&schema)` from the schema, bypassing the engine
entirely. The stored reader is dead weight.

Three options were considered:
1. **Factory method** — the engine provides a method that returns an
   owned fresh `SequentialReader` (reconstructed from the stored
   schema). The consumer owns the reader and drives it with `&mut self`.
2. **Interior mutability** — wrap the reader in `Mutex` or `RefCell`
   so `&SequentialReader` can be upgraded to `&mut`. Adds overhead and
   complexity for mutable cursor state that the consumer legitimately
   wants to own.
3. **`sequential_reader_mut()`** — return `&mut SequentialReader`.
   Requires `&mut self` on the engine, which is overly restrictive
   (the consumer may share the engine across threads or hold it behind
   an `Arc`).

## Decision

**The engine is a `SequentialReader` factory.** Replace
`sequential_reader() -> Option<&SequentialReader>` with
`sequential_reader() -> Option<SequentialReader>` — the method returns
an owned fresh reader, reconstructed from the stored schema.

```rust
impl TypedefEngine {
    /// Construct a fresh SequentialReader for packed-mode reads.
    /// Returns None if compiled in aligned mode.
    pub fn sequential_reader(&self) -> Option<SequentialReader>;
}
```

Each call returns a new reader with the cursor at position 0. The
consumer owns the reader and calls `read_next`/`read_field`/`reset` on
it directly. The engine still stores its own reader (used for schema
validation during construction), but no longer exposes it by
reference.

The same applies to `LayoutBuilder`: `layout_builder()` returns
`Option<&LayoutBuilder>` which is fine — `LayoutBuilder::build` takes
`&self`, so the shared reference is usable. No change needed for the
write-side.

### Cost

`SequentialReader::new` clones the top-level struct's field schemas (a
`Vec<(String, Value)>` of the `properties` entries) and clones the
schema itself. This is cheap — a struct has a small number of fields
(SFTP's largest packet has 5). The construction cost is negligible
compared to the cost of reading a buffer.

## Consequences

### Positive

- **The packed read API is now usable.** A consumer calls
  `engine.sequential_reader()` to get an owned reader and drives it
  directly. No dead API.
- **No interior mutability overhead.** The reader's mutable cursor
  state is owned by the consumer, not shared through a lock.
- **Thread-safe engine.** The engine remains `Send + Sync` (it only
  exposes `&self` methods). The reader is owned by the calling thread.
- **Simple.** One method signature change. The stored reader in
  `Layout::Packed` can be removed (it was only used for schema
  validation during construction, which is done by the time the
  consumer calls `sequential_reader()`).

### Negative

- **Each call to `sequential_reader()` allocates a new reader.** The
  cost is a `Vec` of field schemas + a schema clone. Acceptable for
  the use case (one reader per buffer read).
- **The engine no longer holds a live reader.** If a future use case
  needs to share a reader's cursor state across calls, the consumer
  must manage that themselves. This is the correct separation — cursor
  state is consumer-owned, not engine-owned.

## References

- [ADR-096](096-two-layout-modes-packed-vs-aligned.md) — packed
  sequential mode (`SequentialReader` as the read-side)
- `docs/architecture/crates/typedef/data-access.md` §"Higher-level
  read/write" — the `SequentialReader` API