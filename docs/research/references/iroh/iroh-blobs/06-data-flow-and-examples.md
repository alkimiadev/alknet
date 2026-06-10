# iroh-blobs: Data Flow and Complete Example

## Complete Data Flow: Provider Side

```
                          QUIC Connection Arrives
                                    │
                                    ▼
                    handle_connection(conn, store, events)
                                    │
                         ┌──────────┴──────────┐
                         │  Accept QUIC BIDI    │
                         │  streams in loop     │
                         └──────────┬──────────┘
                                    │
                          handle_stream(pair, store)
                                    │
                         ┌──────────┴──────────┐
                         │  Read Request type   │
                         │  byte + deserialize  │
                         └──────────┬──────────┘
                                    │
              ┌─────────────┬───────┼───────┬──────────────┐
              │             │       │       │              │
         handle_get   handle_get  handle  handle      (reserved)
                       _many    _observe  _push
              │             │       │       │
              ▼             ▼       ▼       ▼
         ┌─────────────────────────────────────────────────┐
         │  For each (offset, ranges) in request.ranges:   │
         │                                                   │
         │  if offset == 0:                                  │
         │    send_blob(store, 0, hash, ranges, writer)      │
         │  else:                                            │
         │    lookup hash in HashSeq[offset-1]               │
         │    send_blob(store, offset, child_hash, ranges, writer) │
         │                                                   │
         │  send_blob:                                       │
         │    store.export_bao(hash, ranges)                 │
         │      .write_with_progress(writer, ctx, &hash, idx) │
         └─────────────────────────────────────────────────┘
```

## Complete Data Flow: Requester Side (Get FSM)

```
                    Create GetRequest
                          │
                          ▼
               fsm::start(connection, request, counters)
                          │
                          ▼
                    AtInitial.next()
                          │ (open_bi, send request)
                          ▼
                    AtConnected.next()
                          │
              ┌───────────┼───────────┐
              │           │           │
        StartRoot    StartChild    Closing
        (offset=0)   (offset>0)    (empty)
              │           │           │
              ▼           ▼           ▼
        AtBlobHeader  AtBlobHeader  AtClosing
        .next()       .next(hash)   .next()
              │           │           │
              ▼           ▼           ▼
        (size, AtBlobContent)     Stats
              │
     ┌────────┴────────┐
     │                 │
  More(item)          Done
  (loop back to       (AtEndBlob)
  AtBlobContent)           │
                    ┌─────┼─────┐
                    │           │
              MoreChildren  Closing
              (AtStartChild) (AtClosing)
                    │           │
                    └───────────┘
```

### Blob Content Items

During `AtBlobContent`, items arrive as `BaoContentItem`:

```rust
pub enum BaoContentItem {
    Parent(ParentNode),  // (node, (left_hash, right_hash)) — 64 bytes
    Leaf(Leaf),          // { offset: u64, data: Bytes } — actual data
}
```

- **Parent nodes** contain BLAKE3 hash pairs for tree verification. They're overhead (~64 bytes per internal node).
- **Leaf nodes** contain actual data chunks. Each leaf's data is at most `IROH_BLOCK_SIZE` bytes (16 KiB).

Verification is automatic: the `ResponseDecoder` from `bao-tree` validates each chunk against the expected hash tree rooted at the request hash.

## Blob Verification and BaoTree Encoding

### How BLAKE3 Verified Streaming Works

1. **The hash is the root** of a binary Merkle tree
2. **Internal nodes** store `(left_child_hash, right_child_hash)` — 64 bytes each
3. **Leaf nodes** store the actual data chunks (up to 1024 bytes each in standard BLAKE3, or 16 KiB in iroh's block size)
4. **Chunk groups** (16 chunks = 16 KiB) are the minimum verification unit in iroh-blobs

For a request with specific ranges:
- The provider traverses the tree, yielding only nodes needed to verify the requested ranges
- The requester can verify each chunk group independently after receiving its parent hash pair
- Maximum undetected corruption: 16 KiB (one chunk group)

### Outboard Storage

The **outboard** is the BLAKE3 hash tree stored separately from the data. For the provider:
- Small blobs (≤16 KiB): outboard is empty (not needed, single chunk group)
- Large blobs: outboard stored as `PreOrderMemOutboard` (in-memory) or as a file (filesystem store)

For the requester, the outboard is built incrementally as data arrives.

## Import and Export Flows

### Import Bytes (Local Data)

```
add_bytes(data) / add_slice(data)
       │
       ▼
ImportBytesRequest { data, format, scope }
       │
       ▼
Actor::import_bytes()
  │ 1. Send AddProgressItem::Size(len)
  │ 2. Send AddProgressItem::CopyDone
  │ 3. Compute outboard: PreOrderMemOutboard::create(&data, IROH_BLOCK_SIZE)
  │ 4. Return ImportEntry { data, outboard, scope, format, tx }
       │
       ▼
Actor::finish_import()
  │ 1. Get hash from outboard.root()
  │ 2. Get or create BaoFileHandle for hash
  │ 3. Transition BaoFileStorage::Partial → Complete
  │ 4. Create TempTag for the hash_and_format
  │ 5. Send AddProgressItem::Done(temp_tag)
```

### Import BAO Stream (Remote Data)

```
import_bao_bytes(hash, ranges, data) / import_bao_reader(hash, ranges, reader)
       │
       ▼
ImportBaoRequest { hash, size }
       │
       ▼
Actor::import_bao()
  │ 1. Set size on partial entry
  │ 2. Create BaoTree for the size
  │ 3. For each BaoContentItem from stream:
  │    - Parent: write hash pair to outboard
  │    - Leaf: write data to storage, update bitfield
  │    - If bitfield becomes complete: transition Partial → Complete
  │ 4. Send result
```

### Export BAO

```
export_bao(hash, ranges) → ExportBao
       │
       ▼
Actor::export_bao()
  │ 1. Look up BaoFileHandle for hash
  │ 2. If not found: send EncodeError::NotFound and return
  │ 3. Create BaoTreeSender from data + outboard readers
  │ 4. Call traverse_ranges_validated(data, outboard, &ranges, tx)
  │    → streams validated BAO items to the sender
```

### Export Path (To Filesystem)

```
export(hash, target_path) → ExportPath
       │
       ▼
Actor::export_path()
  │ 1. Look up BaoFileHandle for hash
  │ 2. Create parent directories if needed
  │ 3. Create file at target_path
  │ 4. Send ExportProgressItem::Size(total_size)
  │ 5. Read data from store in 64 KiB chunks
  │ 6. Write to file, yielding ExportProgressItem::CopyProgress(offset)
  │ 7. Send ExportProgressItem::Done
```

## Observe Protocol Detail

```
Requester                          Provider
    │                                  │
    │  ObserveRequest {hash, ranges}   │
    │─────────────────────────────────►│
    │                                  │
    │  ObserveItem {size, ranges}      │  (initial state)
    │◄─────────────────────────────────│
    │                                  │
    │  ... (time passes, more data      │
    │       becomes available)          │
    │                                  │
    │  ObserveItem {size, ranges}      │  (delta update)
    │◄─────────────────────────────────│
    │                                  │
    │  ... (continue until             │
    │       requester stops            │
    │       or connection closes)       │
    │                                  │
    │  STOP_STREAM                     │
    │─────────────────────────────────►│
```

The observe protocol uses `Bitfield::diff()` to send only the new chunks since the last update, minimizing bandwidth.

## Full Working Example

```rust
use iroh::{protocol::Router, Endpoint, endpoint::presets};
use iroh_blobs::{store::mem::MemStore, BlobsProtocol, ticket::BlobTicket, BlobFormat};

// === Provider Side ===
async fn provider() -> anyhow::Result<()> {
    let endpoint = Endpoint::bind(presets::N0).await?;
    let store = MemStore::new();
    
    // Add some data
    let tag = store.add_slice(b"Hello, iroh-blobs!").await?;
    
    let _ = endpoint.online().await;
    let addr = endpoint.addr();
    
    // Create ticket for sharing
    let ticket = BlobTicket::new(addr, tag.hash, BlobFormat::Raw);
    println!("Ticket: {ticket}");
    
    // Start serving
    let blobs = BlobsProtocol::new(&store, None);
    let router = Router::builder(endpoint)
        .accept(iroh_blobs::ALPN, blobs)
        .spawn();
    
    tokio::signal::ctrl_c().await?;
    router.shutdown().await?;
    Ok(())
}

// === Requester Side ===
async fn requester(ticket: BlobTicket) -> anyhow::Result<()> {
    let (addr, hash, format) = ticket.into_parts();
    
    let endpoint = Endpoint::bind(presets::N0).await?;
    let conn = endpoint.connect(addr, iroh_blobs::ALPN).await?;
    
    // Build request based on format
    let request = match format {
        BlobFormat::Raw => iroh_blobs::protocol::GetRequest::blob(hash),
        BlobFormat::HashSeq => iroh_blobs::protocol::GetRequest::all(hash),
    };
    
    // Use the get FSM
    let start = iroh_blobs::get::fsm::start(conn, request, Default::default());
    let connected = start.next().await?;
    let connected = connected.next().await?;
    
    match connected {
        iroh_blobs::get::fsm::ConnectedNext::StartRoot(at_root) => {
            let (at_content, size) = at_root.next().next().await?;
            let (at_end, data) = at_content.concatenate_into_vec().await?;
            println!("Got {} bytes: {:?}", size, data);
            // ...
        }
        iroh_blobs::get::fsm::ConnectedNext::StartChild(at_child) => {
            // Need to know the child hash
        }
        iroh_blobs::get::fsm::ConnectedNext::Closing(at_closing) => {
            println!("Empty response");
        }
    }
    
    Ok(())
}
```

## Simplified Fetch (Using Store + Remote)

```rust
// The simplest way to download data
let store = MemStore::new();
let remote = store.remote();

// Fetch with automatic local availability checking
let result = remote.fetch(connection, hash, format, &store).await?;
// Result includes Stats with transfer metrics
```

## Key Error Types

| Error Type | Location | Purpose |
|------------|----------|---------|
| `GetError` | `get::error` | Errors during get FSM |
| `ExportBaoError` | `api` | Errors during BAO export |
| `RequestError` | `api` | Store command errors |
| `DecodeError` | `get::fsm` | BAO stream decode errors |
| `ProgressError` | `provider::events` | Provider event errors |