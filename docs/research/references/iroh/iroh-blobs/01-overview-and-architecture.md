# iroh-blobs: Overview and Architecture

**Version**: 0.100.0  
**Repository**: https://github.com/n0-computer/iroh-blobs  
**License**: MIT OR Apache-2.0  
**Rust Edition**: 2021  
**MSRV**: 1.89

## What It Is

`iroh-blobs` is a Rust crate for content-addressed blob transfer over QUIC connections, built on top of [iroh](https://docs.rs/iroh). It implements a request-response protocol for streaming BLAKE3-verified data between peers, along with store implementations for persisting blobs locally.

The core value proposition: transfer arbitrary-sized data with **cryptographic integrity guaranteed in-stream** — every 16 KiB chunk group can be verified against the BLAKE3 hash tree as it arrives, without waiting for the complete transfer.

## Core Concepts

| Concept | Description |
|---------|-------------|
| **Blob** | A sequence of bytes of arbitrary size, identified by its BLAKE3 hash. No metadata. |
| **Link** | A 32-byte BLAKE3 hash of a blob — the content address. |
| **HashSeq** | A blob whose content is a sequence of BLAKE3 hashes (each 32 bytes). Length must be a multiple of 32. |
| **Provider** | The side serving data. Waits for incoming requests and responds. |
| **Requester** | The side requesting data. Initiates connections and sends requests. |
| **Tag** | A persistent named reference to a `HashAndFormat`, protecting blobs from garbage collection. |
| **TempTag** | An ephemeral in-memory reference that protects content while the process runs. |
| **Chunk** | The fundamental BLAKE3 unit: 1024 bytes. |
| **Chunk Group** | Iroh's grouping of 16 chunks (16 KiB), the minimum granularity for range requests and verification. |

## Architecture Diagram

```
┌─────────────────────────────────────────────────────┐
│                    Application                       │
│                                                      │
│  ┌──────────┐  ┌──────────┐  ┌──────────────────┐  │
│  │  Blobs   │  │  Tags    │  │    Downloader    │  │
│  │   API    │  │   API    │  │      API         │  │
│  └────┬─────┘  └────┬─────┘  └───────┬──────────┘  │
│       │              │                │              │
│       └──────────────┴────────────────┘              │
│                      │                               │
│              ┌───────┴───────┐                       │
│              │  Store (API)  │  ← Actor-based, RPC   │
│              │    Commands   │    message passing     │
│              └───────┬───────┘                       │
│                      │                               │
│        ┌─────────────┼─────────────┐                │
│        │             │             │                 │
│  ┌─────┴─────┐ ┌────┴────┐ ┌─────┴─────┐          │
│  │  MemStore  │ │ FsStore │ │ Readonly  │          │
│  │            │ │ (redb +  │ │  MemStore │          │
│  │            │ │  fs)     │ │           │          │
│  └────────────┘ └─────────┘ └───────────┘          │
└─────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────┐
│                 Network Layer                        │
│                                                      │
│  ┌──────────────────┐    ┌──────────────────────┐    │
│  │  BlobsProtocol   │    │   Remote (Client)    │    │
│  │  (Provider side) │    │   (Requester side)   │    │
│  │                  │    │                      │    │
│  │  handle_conn()   │    │  Remote::fetch()     │    │
│  │  handle_stream() │    │  Remote::local()    │    │
│  └────────┬─────────┘    └──────────┬───────────┘    │
│           │                          │                │
│           └──────── iroh QUIC ───────┘                │
│              ALPN: /iroh-bytes/4                     │
└─────────────────────────────────────────────────────┘
```

## Module Structure

```
iroh-blobs/src/
├── lib.rs              # Crate root, re-exports
├── hash.rs             # Hash, BlobFormat, HashAndFormat
├── hashseq.rs          # HashSeq type
├── format.rs           # Format module (Collection)
│   └── collection.rs   # Collection type with metadata
├── protocol.rs         # Wire protocol types (GetRequest, etc.)
│   └── range_spec.rs   # ChunkRangesSeq, RangeSpec wire encoding
├── net_protocol.rs     # BlobsProtocol (iroh ProtocolHandler)
├── provider.rs         # Server-side request handling
│   └── events.rs       # Event system (connect/disconnect/progress)
├── get.rs              # Client-side FSM for getting data
│   ├── error.rs        # GetError, GetResult types
│   └── request.rs      # Request execution helpers
├── api/                # High-level store API
│   ├── blobs.rs        # Blob operations (add, export, read, etc.)
│   │   └── reader.rs   # BlobReader (AsyncRead + AsyncSeek)
│   ├── downloader.rs   # Multi-source download coordinator
│   ├── remote.rs       # Remote peer interaction (fetch, observe)
│   ├── tags.rs         # Tag management API
│   ├── proto.rs        # Store command protocol (RPC messages)
│   └── proto/          # Proto sub-modules
│       └── bitfield.rs # Bitfield type for chunk tracking
├── store/              # Storage implementations
│   ├── mod.rs           # IROH_BLOCK_SIZE, GcConfig
│   ├── mem.rs           # MemStore (in-memory, mutable)
│   ├── fs.rs            # FsStore (filesystem + redb hybrid)
│   ├── readonly_mem.rs  # Read-only memory store
│   ├── gc.rs            # Garbage collection
│   ├── util.rs          # Shared utilities (Tag, SparseMemFile, etc.)
│   └── test.rs          # Test utilities
├── ticket.rs           # BlobTicket (shareable connection info)
├── metrics.rs          # Prometheus metrics definitions
└── util/               # Utilities
    ├── channel.rs       # Channel helpers
    ├── connection_pool.rs # Connection pooling
    ├── stream.rs        # Stream abstractions
    └── temp_tag.rs      # TempTag, TagCounter, TempTags scope management
```

## Key Dependencies

| Dependency | Purpose |
|------------|---------|
| `bao-tree` | BLAKE3 verified streaming, outboard storage, BaoTree encoding/decoding |
| `iroh` | QUIC networking, endpoint, router |
| `irpc` | RPC framework for store commands |
| `postcard` | Wire serialization (compact, no-schema) |
| `redb` | Embedded key-value database (fs-store feature) |
| `range-collections` | RangeSet2 / ChunkRanges for chunk tracking |
| `bytes` | Efficient byte buffer handling |

## Feature Flags

| Feature | Default | Description |
|---------|---------|-------------|
| `fs-store` | ✅ | Filesystem-based store with redb + file hybrid |
| `rpc` | ✅ | RPC support via `noq` / `irpc` |
| `metrics` | ❌ | Prometheus metrics |
| `hide-proto-docs` | ✅ | Hides protocol docs from rustdocs |

## BLAKE3 Block Size

The crate uses a fixed block size of `IROH_BLOCK_SIZE = BlockSize::from_chunk_log(4)`, which means each chunk group is 2^4 = 16 chunks = 16 × 1024 = 16,384 bytes (16 KiB). This is the minimum granularity for range requests and verification.