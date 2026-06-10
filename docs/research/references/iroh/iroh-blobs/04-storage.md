# iroh-blobs: Storage Architecture

## Overview

iroh-blobs provides three store implementations sharing a common `Store` API surface:

| Store | Location | Mutable | Use Case |
|-------|----------|---------|----------|
| `MemStore` | In-memory | ✅ | Small data, testing, WASM |
| `FsStore` | Filesystem + redb | ✅ | Production, large data |
| `ReadonlyMemStore` | In-memory | ❌ | Static data serving |

All stores implement the same RPC-based command protocol (`Command` enum), allowing both local in-process and remote RPC access through the same `Store` type.

## Store API Surface

The `Store` type (from `api::Store`) is the primary interface. It's accessed via typed sub-APIs:

```rust
let store: Store = /* ... */;

// Blob operations
store.blobs()          // → Blobs API (add, export, read, delete, observe, etc.)
store.tags()           // → Tags API (create, list, set, delete, rename)

// Direct operations
store.add_bytes(data)  // → AddProgress
store.add_slice(data)  // → TempTag (convenience)
store.get_bytes(hash)  // → Result<Bytes>
store.has(hash)        // → bool
store.shutdown()       // Clean shutdown
store.wait_idle()      // Wait for all tasks to complete
store.sync_db()        // Sync database to disk (FsStore)
```

## Blobs API

```rust
let blobs = store.blobs();

// Import
blobs.add_slice(data)                          // → AddProgress (raw format)
blobs.add_bytes(data)                           // → AddProgress (raw format)
blobs.add_bytes_with_opts(AddBytesOptions{..})   // → AddProgress (with format)
blobs.import_byte_stream(format)                // → streaming import

// Export
blobs.reader(hash)                              // → BlobReader (AsyncRead + AsyncSeek)
blobs.export(hash, path)                        // → export to filesystem
blobs.export_bao(hash, ranges)                  // → ExportBao (BLAKE3 verified stream)
blobs.export_ranges(hash, ranges)                // → ExportRanges (raw data ranges)

// Observe (subscribe to chunk availability)
blobs.observe(hash)                             // → ObserveAt (bitfield stream)

// Status
blobs.status(hash)                              // → BlobStatus (NotFound/Partial/Complete)

// Import BAO-encoded data
blobs.import_bao_bytes(hash, ranges, data)      // → import verified BAO stream
blobs.import_bao_reader(hash, ranges, reader)   // → import from async reader

// Batch operations (scoped temp tags)
blobs.batch()                                   // → Batch (auto-cleanup scope)

// Delete
blobs.delete(hashes)                            // → force delete (use GC normally)
```

## Tags API

```rust
let tags = store.tags();

tags.set(name, value)            // Set a persistent tag
tags.create(value)               // Auto-generate a tag name, return Tag
tags.get(name)                   // → Option<TagInfo>
tags.list()                      // → Stream<TagInfo>
tags.list_hash_seq()             // → Stream<TagInfo> (only HashSeq format)
tags.delete(name)                // Delete a tag
tags.delete_range(range)         // Delete tags in range
tags.delete_prefix(prefix)       // Delete tags with prefix
tags.rename(from, to)            // Atomically rename a tag
tags.temp_tag(value)              // → TempTag (ephemeral protection)
```

## MemStore Architecture

The in-memory store uses a simple actor pattern:

```
MemStore (ApiClient)
  │
  └── Actor (tokio task)
        ├── State
        │   ├── data: HashMap<Hash, BaoFileHandle>  // All blob data
        │   ├── tags: BTreeMap<Tag, HashAndFormat>    // Persistent tags
        │   └── empty_hash: BaoFileHandle             // Special entry for empty blob
        ├── tasks: JoinSet<TaskResult>                // Spawned import/export tasks
        ├── temp_tags: TempTags                       // Ephemeral protection
        ├── protected: HashSet<Hash>                  // GC-protected hashes
        └── idle_waiters: Vec<oneshot::Sender<()>>     // Wait-idle notifications
```

### BaoFileHandle / BaoFileStorage

```rust
pub enum BaoFileStorage {
    Partial(PartialMemStorage),   // Still downloading
    Complete(CompleteStorage),     // Fully available
}

pub struct PartialMemStorage {
    data: SparseMemFile,           // Sparse byte array for data
    outboard: SparseMemFile,       // Sparse byte array for BLAKE3 hash tree
    size: SizeInfo,                 // Known/estimated size
    bitfield: Bitfield,            // Which chunks are verified
}

pub struct CompleteStorage {
    data: Bytes,                    // Complete data
    outboard: Bytes,               // Complete outboard (hash tree)
}
```

The `watch::Sender<BaoFileStorage>` pattern allows subscribers to observe state changes (for the `observe` API).

### Data Flow (Import)

1. `add_bytes(data)` → compute outboard via `PreOrderMemOutboard::create()` → transition `Partial → Complete`
2. `import_bao(hash, size, stream)` → receive `BaoContentItem` stream → write to `PartialMemStorage` → update bitfield → transition to `Complete` when all chunks present

### Data Flow (Export)

1. `export_bao(hash, ranges)` → look up `BaoFileHandle` → `traverse_ranges_validated(data, outboard, &ranges, tx)` — streams validated BAO data

## FsStore Architecture (Hybrid Store)

The filesystem store uses a **hybrid approach** that stores small data inline in redb and large data as files on disk.

### Design Rationale (from DESIGN.md)

- **Databases** are good for small blobs (low per-entry overhead, fast random access)
- **Filesystems** are good for large blobs (OS-level caching, direct file access)
- **Neither alone** works well for both cases

### Layout

```
<data_dir>/
├── db/                          # redb database
│   ├── metadata table           # Hash → EntryState
│   ├── inline_data table        # Hash → Bytes (for small blobs)
│   ├── inline_outboard table    # Hash → Bytes (for small outboards)
│   └── tags table               # Tag → HashAndFormat
├── data/<hash>.data             # Large blob data files
├── data/<hash>.outboard         # Large outboard files
├── data/<hash>.sizes            # Size tracking for partial files
└── data/<hash>.bitfield         # Validated chunk tracking for partial files
```

### EntryState

```rust
// Simplified from src/store/fs/entry_state.rs
pub enum EntryState {
    Complete(CompleteEntryState),
    Partial(PartialEntryState),
}

pub struct CompleteEntryState {
    pub data: DataLocation,      // Inline, Owned (canonical path), or External (user path)
    pub outboard: OutboardLocation, // Inline, Owned, or NotNeeded
    pub size: u64,
}

pub enum DataLocation {
    Inline,           // Stored in redb inline_data table
    Owned,            // File at canonical path <hash>.data
    External(Vec<PathBuf>), // User-owned file paths
}

pub enum OutboardLocation {
    Inline,           // Stored in redb inline_outboard table
    Owned,            // File at canonical path <hash>.outboard
    NotNeeded,        // Data ≤ 16 KiB, no outboard needed
}

pub struct PartialEntryState {
    // Either we know the verified size, or we don't yet
    pub verified_size: Option<NonZeroU64>,
}
```

### Thresholds

- **Data inline threshold**: 16 KiB (default) — blobs smaller than this are stored entirely in redb
- **Outboard inline threshold**: 16 KiB (default) — outboards smaller than this are stored in redb
- Data ≤ 16 KiB has no outboard (not needed for verification of a single chunk group)

### Blob Lifecycle

**Adding a local file (known data, unknown hash)**:
1. Compute the full BLAKE3 hash and outboard
2. Atomically move the file into the store under the hash name
3. Apply inlining rules: small files → redb, large files → filesystem

**Syncing from remote (known hash, unknown data)**:
1. Start with no data — keep state in memory (not in database)
2. As chunks arrive, write incrementally to partial files
3. Once size is known to exceed the inline threshold, create database entry + filesystem files
4. On completion, transition to `Complete` state and apply inlining rules

**Deletion**:
- Tags protect content from GC
- `TempTag` provides ephemeral (process-lifetime) protection
- HashSeq tags protect the root blob AND all referenced child blobs
- GC is mark-and-sweep: mark all reachable content via tags → sweep (delete) everything else
- Explicit `force` deletion bypasses protection (emergency use only)

### FsStore Actor Architecture

```
FsStore (ApiClient)
  │
  └── MainActor (tokio task)
        ├── TaskContext { config, db_actor_sender }
        ├── EntityMap: HashMap<Hash, ActiveEntityState>  // Currently active entities
        ├── JoinSet<TaskResult>                          // Running tasks
        ├── TempTags                                    // Ephemeral protection
        ├── ProtectedSet                                // GC protection
        └── idle_waiters
```

The FsStore uses an **entity manager** pattern where each hash gets a `BaoFileHandle` (like MemStore) when active, and entries are cleaned up when tasks complete.

## Garbage Collection

```rust
pub struct GcConfig {
    pub interval: Duration,
    pub add_protected: Option<ProtectCb>,  // Optional callback to add more protected hashes
}
```

GC is a two-phase process:
1. **Mark**: Walk all tags (persistent + temp), collect reachable hashes. For HashSeq format, traverse the hash sequence to find all child hashes.
2. **Sweep**: Delete all blobs not in the reachable set, in batches of 100.

GC runs automatically at a configurable interval via `run_gc(store, config)`, or manually via `gc_run_once(store, live)`.