# iroh-blobs Reference Documentation

This directory contains a comprehensive reference for the `iroh-blobs` crate (v0.100.0), a Rust library for content-addressed blob transfer over QUIC connections using BLAKE3 verified streaming.

## Documents

1. **[Overview and Architecture](01-overview-and-architecture.md)** — Core concepts, module structure, feature flags, and architecture diagram. Start here.

2. **[Key Types and Data Structures](02-key-types.md)** — Detailed reference for `Hash`, `BlobFormat`, `HashAndFormat`, `HashSeq`, `Bitfield`, `Tag`, `TempTag`, `BlobTicket`, `ChunkRanges`/`ChunkRangesSeq`/`RangeSpec`, and the store command protocol.

3. **[Transfer Protocol](03-transfer-protocol.md)** — Wire protocol specification: request types (`GetRequest`, `GetManyRequest`, `PushRequest`, `ObserveRequest`), response format (BLAKE3 verified streaming), the client-side FSM, provider handling, event system, and the Collection format.

4. **[Storage Architecture](04-storage.md)** — Store implementations: `MemStore` (in-memory), `FsStore` (hybrid redb + filesystem), `ReadonlyMemStore`. Covers the actor pattern, `BaoFileHandle`/`BaoFileStorage`, partial/complete states, the hybrid inline/file approach, entry states, blob lifecycle, and garbage collection.

5. **[Remote API and Downloader](05-remote-and-downloader.md)** — `Remote` API for fetching from/observing/pushing to peers, `Downloader` for multi-source downloads, connection pooling, and iroh integration via `BlobsProtocol`.

6. **[Data Flow and Examples](06-data-flow-and-examples.md)** — End-to-end data flow diagrams for provider and requester sides, BLAKE3 verification mechanics, import/export flows, observe protocol detail, and complete working examples.

## Quick Reference

### Creating a Provider

```rust
use iroh::{protocol::Router, Endpoint, endpoint::presets};
use iroh_blobs::{store::mem::MemStore, BlobsProtocol};

let endpoint = Endpoint::bind(presets::N0).await?;
let store = MemStore::new();
let tag = store.add_slice(b"data").await?;
let blobs = BlobsProtocol::new(&store, None);
let router = Router::builder(endpoint)
    .accept(iroh_blobs::ALPN, blobs)
    .spawn();
```

### Key Constants

| Constant | Value | Meaning |
|----------|-------|---------|
| `ALPN` | `b"/iroh-bytes/4"` | QUIC ALPN protocol identifier |
| `IROH_BLOCK_SIZE` | `BlockSize::from_chunk_log(4)` | 16 KiB chunk groups |
| `MAX_MESSAGE_SIZE` | `1 MiB` | Maximum request message size |
| `Hash::EMPTY` | BLAKE3 of `b""` | Hash of the empty blob |

### Core Crate Exports

```rust
pub use hash::{BlobFormat, Hash, HashAndFormat};
pub use hashseq::HashSeq;
pub use net_protocol::BlobsProtocol;
pub use protocol::ALPN;
pub mod api;        // Store API, Blobs, Tags, Downloader, Remote
pub mod format;     // Collection type
pub mod get;        // Client-side FSM
pub mod protocol;   // Wire protocol types (GetRequest, etc.)
pub mod provider;   // Server-side handling
pub mod store;      // Storage implementations
pub mod ticket;     // BlobTicket
pub mod util;       // Connection pool, temp tags, stream helpers
```