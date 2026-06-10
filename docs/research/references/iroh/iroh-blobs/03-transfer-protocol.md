# iroh-blobs: Transfer Protocol

## Overview

The transfer protocol is a **request-response** protocol operating over QUIC streams (via iroh). The ALPN is `b"/iroh-bytes/4"`.

The requester opens a bidirectional QUIC stream, sends a request, and the provider responds with BLAKE3-verified streaming data on the same stream.

**Key properties**:
- Data integrity is verified in-stream — every 16 KiB chunk group can be independently verified against the BLAKE3 hash tree
- No upper limit on blob or collection size — streaming design avoids buffering entire transfers
- Zero round-trip overhead for multiple small blobs (via HashSeq/GetManyRequest)
- Range requests supported at chunk granularity

## Request Types

```rust
pub enum Request {
    Get(GetRequest),
    Observe(ObserveRequest),
    Slot2, Slot3, Slot4, Slot5, Slot6, Slot7,  // Reserved
    Push(PushRequest),
    GetMany(GetManyRequest),
}
```

Wire format: 1-byte discriminator (postcard-encoded `RequestType` enum), followed by postcard-serialized request body.

### GetRequest

```rust
pub struct GetRequest {
    pub hash: Hash,              // BLAKE3 hash of the root blob
    pub ranges: ChunkRangesSeq,  // What ranges to request
}
```

The most common request type. The `ranges` field uses `ChunkRangesSeq` to express which parts of the root blob and its children to request.

**Common patterns**:

```rust
// Request an entire single blob
let req = GetRequest::blob(hash);
// -> ChunkRangesSeq with a single element: all chunks of the root

// Request a HashSeq (root + all children)
let req = GetRequest::all(hash);
// -> ChunkRangesSeq::all() - infinite sequence of "all chunks"

// Request parts of a single blob
let req = GetRequest::builder()
    .root(ChunkRanges::bytes(0..1000))
    .build(hash);

// Request a HashSeq with specific child ranges
let req = GetRequest::builder()
    .root(ChunkRanges::all())            // full root (the hash seq)
    .child(1, ChunkRanges::bytes(0..100)) // partial child 1
    .next(ChunkRanges::all())             // full remaining children
    .build_open(hash);                    // build_open = last range repeats forever
```

### GetManyRequest

```rust
pub struct GetManyRequest {
    pub hashes: Vec<Hash>,       // Sorted, deduplicated list of hashes
    pub ranges: ChunkRangesSeq,  // Ranges for each hash (no root entry)
}
```

Like a `GetRequest` for a HashSeq, but the hashes are provided by the requester instead of looked up from the provider. This avoids the provider needing to have a pre-existing HashSeq blob.

```rust
let req = GetManyRequest::builder()
    .hash(hash1, ChunkRanges::all())
    .hash(hash2, ChunkRanges::all())
    .build();
// Deduplicates and sorts hashes automatically
```

### PushRequest

```rust
pub struct PushRequest(GetRequest);  // Wraps a GetRequest
```

The inverse of a GetRequest — the requester pushes data to the provider. The request describes what will be sent, followed by the actual data stream. Providers may reject push requests (disabled by default via `EventMask`).

### ObserveRequest

```rust
pub struct ObserveRequest {
    pub hash: Hash,
    pub ranges: RangeSpec,  // Which ranges to observe
}
```

Subscribes to availability changes for a blob's bitfield. The provider sends `ObserveItem` updates as chunks become available.

## Response Format

### For Get/GetMany/Push

The response is BLAKE3-verified streaming data (bao-tree format). For each blob in the request:

1. **8-byte size header** (little-endian u64) — the total size of the blob
2. **BLAKE3 verified stream** — encoded data for the requested ranges, using bao-tree's mixed encoding:
   - `BaoContentItem::Parent(node, (left_hash, right_hash))` — internal hash tree nodes (64 bytes each)
   - `BaoContentItem::Leaf(Leaf { offset, data })` — actual data chunks

The data is sent in order: ascending chunks for each blob, blobs in HashSeq order.

**Verification**: The requester validates each chunk group against the expected BLAKE3 hash tree. Invalid data is detected within at most 16 KiB of reception. Missing data (provider doesn't have a chunk) causes the provider to close the stream at the point where data becomes unavailable.

### For Observe

The provider sends length-prefixed `ObserveItem` messages:

```rust
pub struct ObserveItem {
    pub size: u64,          // Blob size
    pub ranges: ChunkRanges, // Available chunks
}
```

Updates are sent as deltas — only the new chunks that have become available since the last update.

## Error Handling

Error codes for stream/connection closure:

| Code | Name | Meaning |
|------|------|---------|
| 0 | StreamDropped | RecvStream was dropped |
| 1 | ProviderTerminating | Provider is shutting down |
| 2 | RequestReceived | Only one request per stream allowed |
| 1 (application) | ERR_PERMISSION | Permission denied |
| 2 (application) | ERR_LIMIT | Rate limited |
| 3 (application) | ERR_INTERNAL | Internal error |

## Client-Side FSM (Get)

The `get::fsm` module implements the get request as a **finite state machine** for maximum control:

```
AtInitial
  │ (open QUIC stream)
  ▼
AtConnected
  │ (send request, drop writer)
  ▼
ConnectedNext ─┬─ StartRoot(hash, ranges)     // offset 0 = root blob
               ├─ StartChild(offset, ranges)  // offset > 0 = child blob
               └─ Closing                    // empty request
                    │
AtStartRoot / AtStartChild
  │ (determine hash for child)
  ▼
AtBlobHeader
  │ (read 8-byte size)
  ▼
AtBlobContent
  │ (stream BLAKE3-verified items)
  ├─ More(content_item) → AtBlobContent  // loop
  └─ Done → AtEndBlob
                    │
AtEndBlob
  │ (iterate to next blob in sequence)
  ├─ MoreChildren(AtStartChild)
  └─ Closing
       │ (drain remaining bytes)
       ▼
     Stats (transfer statistics)
```

Each state transition is explicit. The FSM gives the consumer full control:
- `AtBlobContent::next()` returns `BlobContentNext::More((content, item))` or `BlobContentNext::Done(end)`
- `AtBlobHeader::next()` reads the size header and creates a `ResponseDecoder`
- `AtStartChild::next(hash)` requires the caller to supply the hash (from the HashSeq)

### Stats Tracking

```rust
pub struct Stats {
    pub payload_bytes_read: u64,    // Actual data bytes
    pub other_bytes_read: u64,       // Hash pairs, headers
    pub payload_bytes_written: u64,  // For push
    pub other_bytes_written: u64,    // For push
    pub elapsed: Duration,
}
```

## Provider-Side Handling

```rust
pub async fn handle_connection(connection: Connection, store: Store, events: EventSender);
```

The provider accepts QUIC streams on a connection. For each stream:
1. Read the request type byte
2. Deserialize the request
3. Dispatch to `handle_get`, `handle_get_many`, `handle_observe`, or `handle_push`
4. For `handle_get`: iterate over the `ChunkRangesSeq`, streaming each blob via `store.export_bao(hash, ranges)`
5. For HashSeq requests: load the root blob, parse it as `HashSeq`, then stream each requested child

### Event System

The provider can emit events for monitoring and access control:

```rust
pub struct EventMask {
    pub connected: ConnectMode,   // None, Notify, Intercept
    pub get: RequestMode,          // None, Notify, Intercept, NotifyLog, InterceptLog, Disabled
    pub get_many: RequestMode,
    pub push: RequestMode,         // Disabled by default!
    pub observe: ObserveMode,
    pub throttle: ThrottleMode,    // None, Intercept
}
```

- **None**: No events, requests processed normally
- **Notify**: Events sent but cannot block requests
- **Intercept**: Events sent as RPC requests; handler can reject with `AbortReason`
- **Disabled**: All requests of this type rejected

Progress events: `TransferStarted`, `TransferProgress`, `TransferCompleted`, `TransferAborted`.

## Collection Format

```rust
pub struct Collection {
    blobs: Vec<(String, Hash)>,  // Named references to child blobs
}
```

Wire format (as a HashSeq blob):
1. First child blob: `CollectionMeta` serialized with postcard
2. Remaining children: the actual data blobs

```rust
pub struct CollectionMeta {
    header: [u8; 13],  // Must be b"CollectionV0."
    names: Vec<String>, // Names for each child blob
}
```

The header `b"CollectionV0."` is a magic number for format identification. The meta blob's hash becomes the first entry in the HashSeq, followed by the hashes of each data blob. Names correspond 1:1 with data blobs (excluding the meta entry).