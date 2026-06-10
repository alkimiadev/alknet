# iroh-blobs: Key Types and Data Structures

## Hash

```rust
// src/hash.rs
pub struct Hash(blake3::Hash);  // 32-byte BLAKE3 hash, wraps blake3::Hash
```

The fundamental content-address. Created via `Hash::new(data)` or `Hash::from_bytes([u8; 32])`. Has a constant `Hash::EMPTY` for the empty blob. Supports hex display, serde (compact binary for non-human-readable), and is stored as a 32-byte fixed array in redb.

Wire format: 32 raw bytes (postcard serialization). No framing overhead.

## BlobFormat

```rust
pub enum BlobFormat {
    Raw,       // A single blob
    HashSeq,   // A sequence of BLAKE3 hashes
}
```

Distinguishes between a raw binary blob and a hash sequence. Wire format: single byte (0 = Raw, 1 = HashSeq).

## HashAndFormat

```rust
pub struct HashAndFormat {
    pub hash: Hash,
    pub format: BlobFormat,
}
```

Pairs a hash with its format. Wire format: 33 bytes (32 for hash + 1 for format). Display format: hex string, optionally prefixed with 's' for HashSeq.

## HashSeq

```rust
// src/hashseq.rs
pub struct HashSeq(Bytes);  // Wrapper around Bytes, length must be multiple of 32
```

A blob interpreted as a sequence of 32-byte BLAKE3 hashes. Created from `Bytes` via `HashSeq::new(bytes)` (returns `None` if length is not a multiple of 32). Iterable, supports `get(index)`, `pop_front()`.

Used extensively: collections are stored as a HashSeq where the first child is metadata and subsequent children are data blobs.

## Bitfield

```rust
// src/api/proto/bitfield.rs
pub struct Bitfield {
    pub size: u64,          // Total size of the blob in bytes
    pub ranges: ChunkRanges, // Which chunks are verified/present
}
```

Tracks which chunks of a blob are present and verified. Key methods:
- `is_complete()` — all chunks present
- `validated_size()` — how many bytes are verified
- `diff(&other)` — compute the delta between two bitfields

Used by the observe protocol and internal state tracking.

## Tag

```rust
// src/store/util.rs
pub struct Tag(pub Bytes);  // Named reference, arbitrary bytes, typically UTF-8
```

A persistent named reference to content in the store. Tags protect content from garbage collection. Auto-generated tags use the format `"auto-2026-01-15T12:34:56.789Z"`. Tags are stored in the store's database and can be listed, created, renamed, and deleted.

## TempTag

```rust
// src/util/temp_tag.rs
pub struct TempTag {
    inner: HashAndFormat,
    on_drop: Option<Weak<dyn TagDrop>>,  // Callback when dropped
}
```

An ephemeral, in-memory tag. While a `TempTag` exists, its referenced content is protected from garbage collection. When dropped, the `TagDrop` callback notifies the store to unprotect. Can be `leak()`ed to make the protection permanent for the process lifetime.

Scopes: `TempTagScope` manages groups of temp tags. `Scope::GLOBAL` is the default scope. Batches of operations can create scoped temp tags that are cleaned up together.

## BlobTicket

```rust
// src/ticket.rs
pub struct BlobTicket {
    addr: EndpointAddr,  // How to reach the provider (includes EndpointId, relay URL, direct addresses)
    format: BlobFormat,  // Raw or HashSeq
    hash: Hash,          // What to retrieve
}
```

A shareable token containing everything needed to retrieve a blob from a provider. Serialized via `iroh_tickets::Ticket` trait (base32-encoded with "blob" prefix). Wire format uses postcard with a variant discriminator.

```rust
// Creating a ticket
let ticket = BlobTicket::new(addr, hash, BlobFormat::Raw);

// From a ticket string
let ticket: BlobTicket = ticket_str.parse()?;
```

## ChunkRanges and ChunkRangesSeq

### ChunkRanges

```rust
pub type ChunkRanges = RangeSet2<ChunkNum>;  // From range_collections crate
```

A set of non-overlapping chunk ranges. Supports boolean operations (union, intersection, difference). The fundamental unit is `ChunkNum` (a u64 newtype representing a 1024-byte BLAKE3 chunk).

Helper trait `ChunkRangesExt` provides:
- `ChunkRanges::all()` — all chunks
- `ChunkRanges::bytes(range)` — byte range rounded up to chunk boundaries
- `ChunkRanges::chunks(range)` — chunk range from u64 bounds
- `ChunkRanges::last_chunk()` — the very last chunk (for size verification)
- `ChunkRanges::chunk(n)` — a single chunk
- `ChunkRanges::offset(n)` — a single byte offset rounded to chunk

### ChunkRangesSeq

```rust
// src/protocol/range_spec.rs
pub struct ChunkRangesSeq(SmallVec<[(u64, ChunkRanges); 2]>);
```

A sequence of `ChunkRanges`, one per blob in a HashSeq. Uses run-length encoding: stores `(offset, ranges)` pairs, where offset is the first blob index with that range spec. Unspecified indices default to the most recent range (or empty for finite sequences).

Key methods:
- `ChunkRangesSeq::all()` — request everything (root + all children, forever)
- `ChunkRangesSeq::root()` — request only the root blob
- `ChunkRangesSeq::empty()` — request nothing
- `ChunkRangesSeq::from_ranges(ranges)` — from explicit iterator
- `ChunkRangesSeq::from_ranges_infinite(ranges)` — last range repeats forever
- `.iter_non_empty_infinite()` — iterate only non-empty ranges
- `.is_blob()` — true if requesting a single blob (offset 0 with one entry)

### RangeSpec (Wire Format)

```rust
pub struct RangeSpec(SmallVec<[u64; 2]>);
```

The on-wire encoding of `ChunkRanges`. Uses alternating spans: first span is deselected, second is selected, etc. SmallVec avoids allocation for the common case of a single range.

Examples:
- `[]` — empty (nothing selected)
- `[0]` — everything from chunk 0 selected (entire blob)
- `[2, 5, 3, 1]` — chunks 2-7 and 10-11 selected
- `[u64::MAX]` — only the last chunk (size proof)

### ChunkRangesSeq Wire Format

Serialized as `(SmallVec<[(u64, RangeSpec); 2]>)` where each element is `(delta_offset, rangespec)`. The `delta_offset` is the distance from the previous entry. Uses postcard varint encoding for compact transmission.

## Store Command Protocol

The store API uses an RPC-style command pattern via `irpc`. Each command has a `Command` enum variant with typed request/response channels:

```rust
#[rpc_requests(message = Command, alias = "Msg", rpc_feature = "rpc")]
pub enum Request {
    ListBlobs(ListRequest),
    Batch(BatchRequest),
    DeleteBlobs(BlobDeleteRequest),
    ImportBao(ImportBaoRequest),       // streaming: rx bao items, tx result
    ExportBao(ExportBaoRequest),        // streaming: tx encoded items
    ExportRanges(ExportRangesRequest), // streaming: tx range data
    Observe(ObserveRequest),           // streaming: tx bitfield updates
    BlobStatus(BlobStatusRequest),
    ImportBytes(ImportBytesRequest),
    ImportByteStream(ImportByteStreamRequest), // duplex streaming
    ImportPath(ImportPathRequest),
    ExportPath(ExportPathRequest),
    ListTags(ListTagsRequest),
    SetTag(SetTagRequest),
    DeleteTags(DeleteTagsRequest),
    RenameTag(RenameTagRequest),
    CreateTag(CreateTagRequest),
    CreateTempTag(CreateTempTagRequest),
    ListTempTags(ListTempTagsRequest),
    SyncDb(SyncDbRequest),
    WaitIdle(WaitIdleRequest),
    Shutdown(ShutdownRequest),
    ClearProtected(ClearProtectedRequest),
}
```

This allows both local (in-process) and remote (RPC) store access through the same API surface.