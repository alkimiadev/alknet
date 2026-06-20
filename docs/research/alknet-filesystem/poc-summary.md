# alknet-filesystem: POC Research Summary

**Status:** Research complete on the two highest-leverage unknowns (path-tree layer + write path); the approach is viable enough to spec. Remaining unknowns are implementation-scope, not feasibility.
**Date:** 2026-06-20
**Scope:** Captures what the POC proved, what unknowns it closed, what remains open, and the architectural direction it establishes. Source material for the eventual `alknet-filesystem` crate spec.

---

## Executive Summary

A POC (`alknet-filesystem-poc`, `/workspace/alknet-filesystem-poc`) was completed that resolves the two largest sources of feasibility uncertainty around building a content-addressed, branch-aware, mountable filesystem from three orthogonal layers: SQLite (path tree + application file format), iroh-blobs (content-addressed blob store), and honker (durable pub/sub + queue + locks inside the same SQLite file).

The POC was built in two iterations:

1. **Path-tree layer** (Tier 1) — proved that a SQLite-backed path tree over an iroh-blobs `MemStore` gives Fossil-style branching with free content dedup, honker notify-on-commit inside the same transaction as path-tree mutations, and free multi-tenant isolation via a `bucket_id` column. 8 tests.
2. **Write path** — proved that "branch on write, merge on close" reconciles the fundamental mismatch between content-addressed storage (BLAKE3 must hash the complete file) and filesystem write semantics (chunks arrive incrementally, possibly out of order). A concurrent reader sees the old version until `close()` commits atomically; crash/abort leaves the old version intact. 7 tests.

**15/15 tests pass.** All deps are published crates.io versions (no workspace path deps), so the POC is portable.

The three layers compose cleanly for both the read path *and* the write path. The remaining unknowns — FsStore/redb vs SQLite, actual SFTP wiring, network distribution, GC/tag management — are implementation details rather than architectural risks.

---

## Background: The Insight

The POC originated from a correction on X: SQLite is not just a database, it's a legitimate [application file format](https://sqlite.org/appfileformat.html). The key observations from that link:

- **BLOBs < ~100KB are faster inline in SQLite than as filesystem files.** This inverts the usual "databases are slow for big binaries" assumption at exactly the granularity iroh-blobs already cares about (16 KiB chunk groups).
- **Atomic transactions** over path-tree metadata, independent of content blobs.
- **The schema is the documentation.** An SQL schema defines the file format more concisely than a custom format spec.

iroh-blobs had already discovered this independently: its `FsStore` uses a hybrid approach (`DESIGN.md`, `/workspace/iroh-blobs/DESIGN.md`) with redb (an embedded KV DB, SQLite-shaped philosophy) for small blobs and filesystem files for large blobs, with four metadata tables (`blobs`, `tags`, `inline_data`, `inline_outboard` — see `src/store/fs/meta/tables.rs`). The path-tree layer is the missing piece iroh-blobs deliberately doesn't provide: iroh answers *"given a BLAKE3 hash, where are the bytes?"* but not *"given a path, which hash does it resolve to?"*

The architectural insight: **SQLite for path edges, iroh for content, honker for coordination.** Each layer does what it's best at, and the boundaries are clean.

---

## The Three Layers

### Layer 1: iroh-blobs — content-addressed blob storage

**Crate:** `iroh-blobs` 0.103 from crates.io, `MemStore` backend (no redb, no fsync rabbit hole — deliberate POC scope choice).
**Source reviewed:** `/workspace/iroh-blobs` (v0.100 local checkout) + published 0.103 source in cargo cache.

iroh-blobs provides content-addressed blob storage with BLAKE3 verified streaming. A blob is a sequence of bytes of arbitrary size, identified by its 32-byte BLAKE3 hash. Content is dedup'd by definition: same hash = same bytes. This is the layer that gives us the "many agents / many forks share most content" property — content is shared across branches because it lives under its hash, not under its path.

Key API surface used by the POC:
- `MemStore::new()` — in-memory store, no persistence
- `store.blobs().add_slice(bytes).with_tag().await` → `TagInfo` (returns the hash + creates a persistent named tag protecting the blob from GC)
- `store.blobs().reader(hash)` → `BlobReader` (implements `AsyncRead + AsyncSeek`)
- `store.blobs().has(hash)` → `bool`

**One sharp edge worth recording:** `Hash::new(buf)` *computes* a BLAKE3 hash of the input; `Hash::from_bytes(bytes)` *wraps* 32 raw bytes as a `Hash`. Round-tripping a hash through hex requires `from_bytes`, not `new`. This cost ~20 minutes of debugging and is worth documenting for anyone touching the iroh API.

### Layer 2: SQLite — path tree + application file format

**Crate:** `rusqlite` 0.39 (bundled), pinned to match `honker-core`'s rusqlite version.
**Source:** `src/schema.rs`, `src/fs.rs`

SQLite stores the path tree: a mapping from `(bucket, branch, path)` to an irob `Link` (64-hex BLAKE3 hash). The schema has four core tables:

- `buckets` — top-level isolation unit (multi-tenant). One row per tenant.
- `branches` — Fossil-style named snapshots with parent pointers. A branch starts empty and inherits everything from its parent.
- `paths` — one row per `(bucket, branch, path)` entry that has been *overridden or created* on this branch (only the delta from the parent). `link` is the BLAKE3 hash; `size` is cached so `fstat` is a single indexed lookup.
- `tombstones` — deletion markers. A path may exist on a parent branch but be deleted on a child; the tombstone stops the chain walk from inheriting.

Plus two write-path tables (see Write Path below):
- `write_sessions` — active write sessions (one per open file handle)
- `write_chunks` — chunks for active write sessions (one row per `write()` call, stored inline as BLOBs)

The core read operation is a recursive CTE that walks the branch parent chain:

```sql
WITH RECURSIVE chain(branch_id, parent_id, depth) AS (
    SELECT b.id, b.parent_id, 0 FROM branches b
      JOIN buckets bk ON bk.id = b.bucket_id
     WHERE bk.name = ?1 AND b.name = ?2
    UNION ALL
    SELECT b.id, b.parent_id, c.depth + 1 FROM chain c
      JOIN branches b ON b.id = c.parent_id
)
SELECT p.kind, p.link, p.symlink_to, p.size
  FROM chain c JOIN paths p ON p.branch_id = c.branch_id
  LEFT JOIN tombstones t ON t.branch_id IN (SELECT branch_id FROM chain WHERE depth <= c.depth)
                                    AND t.path = p.path
 WHERE p.path = ?3 AND t.id IS NULL
 ORDER BY c.depth ASC LIMIT 1
```

This returns the *first* matching path row that isn't tombstoned on a closer branch. The `ORDER BY c.depth ASC LIMIT 1` correctly picks the closest branch's override. Even on the POC's naive schema, resolves are sub-millisecond in-memory.

### Layer 3: honker — durable pub/sub + queue + locks inside the SQLite file

**Crate:** `honker-core` 0.2.4 (not `honker` — see below), `bundled-sqlite` feature.
**Source:** `src/schema.rs` (bootstrap), `src/watch.rs` (listener)

honker provides pub/sub, durable queues, named locks, rate limits, and a scheduler — all as SQL functions registered on *your own* rusqlite connection. The key integration point:

```rust
honker_core::apply_default_pragmas(conn)?;      // WAL, synchronous=NORMAL, ...
honker_core::attach_notify(conn)?;              // _honker_notifications table + notify() SQL function
honker_core::attach_honker_functions(conn)?;    // enqueue, claim, lock_acquire, stream_publish, cron, ...
honker_core::bootstrap_honker_schema(conn)?;   // queue/stream/scheduler tables
```

This registers `notify(channel, payload)` as a SQL scalar function on the same connection that owns the path-tree tables. The critical property: calling `SELECT notify(...)` inside the same transaction as a path-tree mutation means the event is atomic with the data change. A watcher wakes on commit, not on poll. A rolled-back mutation produces no event. This is the transactional-outbox pattern, built in.

**Why `honker-core` not `honker`:** The `honker` crate opens its *own* SQLite connection and manages its own database handle. To get the SQL functions on *your* connection — the whole point of the transactional-outbox property — you need `honker-core`, which exposes `attach_honker_functions(conn)` for any rusqlite connection. The `honker` crate is the ergonomic Rust wrapper for the "honker manages everything" use case; `honker-core` is the foundation for the "I own the connection" use case. The POC is the latter.

honker is single-machine, file-backed (explicitly: "two servers writing the same .db over NFS is not a Honker deployment strategy"). That's fine for the local-VFS layer; the distributed part is iroh's job at the blob layer. The split is clean: honker coordinates local state + local workers; iroh coordinates cross-node content. They don't compete.

---

## POC Iteration 1: Path-Tree Layer (Tier 1)

**Scope:** SQLite path-tree + iroh-blobs MemStore + honker notify, no SFTP, no FsStore, no network. The goal was to validate the SQLite+iroh seam and the branching model cheaply.

### What it proved

**1. SQLite is a workable path-tree layer over content-addressed blobs.** Path rows store BLAKE3 hashes; bytes live in iroh. `put`/`get` roundtrips cleanly. Atomic path-tree mutations (`rename`, `mkdir`, `unlink`) compose with iroh blob add/delete. `rename` is O(1) on path edges regardless of file size — content stays in iroh under its hash, not under its path. This is the property git also gets right. *(Test: `put_get_roundtrip`, `rename_is_o1_on_path_edges`)*

**2. Fossil-style branching gives free content sharing across forks.** A child branch inherits parent files via the recursive-CTE chain walk. Same content hashes to the same link (free dedup via content addressing). Writes on a child are invisible on the parent. Tombstones hide parent files on child branches. *(Tests: `branch_inherits_parent_content`, `branch_modifications_do_not_leak_to_parent`, `tombstone_hides_parent_file_on_child`, `content_is_deduped_across_branches`)*

**3. honker notify-on-commit works inside the same transaction.** `notify()` is called inside the path-tree mutation; watcher receives the event on commit. *(Test: `watch_fires_on_commit`)*

**4. Buckets (multi-tenancy) are free.** A `bucket_id` column on every row = isolation. Alpha files don't leak into Beta. Auth is an adapter problem (which connection sees which buckets). *(Test: `multi_bucket_isolation`)*

### The `share-check` demo

Running `cargo run -- share-check` demonstrates the content-sharing property end-to-end:

```
== content sharing across branches ==
main  shared.txt = "hello from main\n"
agent-a shared.txt = "hello from main\n"    ← inherited from parent, same hash
agent-a agent-a.txt = "agent a working here\n"
agent-a main-only.txt resolved = false     ← tombstoned on agent-a

== dir listing on agent-a (parent chain walk) ==
  agent-a.txt      kind=File link=f7a5aa575a40…
  shared.txt       kind=File link=b9d5a428d102…
```

`shared.txt` is byte-identical across `main` and `agent-a` (content shared by hash). `main-only.txt` is hidden on `agent-a` by the tombstone. `agent-a.txt` is only on the branch.

---

## POC Iteration 2: Write Path — "Branch on Write, Merge on Close"

**Scope:** Chunked writes (SFTP-style `open → write at offset × N → close`), crash/abort semantics, concurrent-reader isolation. The deal-breaker question: can content-addressed storage serve as the backend for a mountable filesystem's write path, or is the BLAKE3-must-hash-the-whole-thing constraint a fundamental mismatch?

### The problem

BLAKE3 must hash the *complete* file to produce the content address. But filesystem writes arrive as chunks — SFTP writes in ~32KB chunks, possibly pipelined and out of order. You can't hash until you have all the bytes. Where do partial writes live? What does a concurrent reader see? What happens on crash?

### The solution: "branch on write, merge on close"

A write session *is* a short-lived branch:

- **`open(path, WRITE)`** creates a temp child branch of the target branch. Inserts a `write_sessions` row.
- **`write(offset, chunk)`** inserts a row in `write_chunks` — one transaction per chunk, crash-safe. Chunks may arrive out of order (SFTP pipelines writes). Chunk-sized BLOBs (~32KB) are SQLite's sweet spot per the appfileformat paper's "BLOBs < 100KB faster inline" finding. The offset is the unique key — two writes to the same offset overwrite (last-write-wins within a session, matching POSIX overlapping-write semantics).
- **Reads on the *target* branch during the session** see the *old* version via the parent chain walk. This is POSIX "concurrent readers see old version until close commits", for free, from the branching model — no separate snapshot mechanism needed.
- **`close()`** assembles chunks in offset order → BLAKE3 hash → iroh `add_bytes` → updates the path row on the target branch → `notify()` → marks session closed. All the SQLite parts in one transaction. The new version becomes visible atomically.
- **`abort()` or crash:** the session row stays `open`/`aborted`, the target branch is untouched, the old version is still readable. Orphaned sessions can be found and cleaned up later.

### What it proved

| Test | What it proves |
|---|---|
| `chunked_write_close_produces_correct_hash` | Assembling chunks + BLAKE3 hashing produces the same hash as a whole-file write. Content addressing works through the chunk boundary. |
| `chunked_write_out_of_order` | SFTP-style pipelined writes (offset 6 before offset 0) assemble correctly. The offset is the key, not arrival order. |
| `concurrent_reader_sees_old_version_during_write` | **The key POSIX property.** During a write session, reads return the old version. After `close()`, reads return the new version. For free, from the branching model. |
| `abort_leaves_old_version_untouched` | Simulated crash/abort: old version survives, chunks discarded. |
| `abort_on_new_file_leaves_no_trace` | Aborting a write to a new path doesn't create a phantom entry. |
| `large_chunked_file_writes_correctly` | 1MB file in 32KB chunks assembles, hashes, and reads back correctly. Matches whole-file hash. |
| `multiple_concurrent_write_sessions_same_path` | Two sessions on the same path don't corrupt each other's chunks (session-scoped). Last close wins. |

### The bug that was found and fixed

The initial `chunk_idx = offset / 32768` computation was wrong — two writes at offsets 0 and 6 (both < 32768) collided on `chunk_idx = 0`, causing the `INSERT OR REPLACE` to overwrite the first chunk with the second. The out-of-order test caught this immediately. Fix: use the offset itself as the unique key. This is the kind of thing the POC exists to catch — a spec written without the POC would have shipped this bug.

### Why SQLite + honker wins for the write path specifically

The write path is where the SQLite-vs-redb-vs-filesystem decision matters most, and SQLite wins for three reasons that all show up here:

1. **Chunk-sized BLOBs are SQLite's sweet spot.** SFTP writes in ~32KB chunks. The appfileformat page's "BLOBs < 100KB are faster inline in SQLite than as filesystem files" finding is *exactly* this case. redb could do it too, but now you have two databases for one transaction.

2. **The chunks, the path tree, and the honker notification are one transaction.** No dual-write between "where chunks live" and "where the path tree lives." No fsync ordering problem (the thing iroh's DESIGN.md spent the most words on — "files are hard," the bitfield/data/outboard write-ordering problem). One WAL, one commit boundary.

3. **honker coordinates the session.** Named locks on the path prevent concurrent writers from stomping each other (not yet wired in the POC — see Open Unknowns). The write session can be a honker job — enqueue on open, track progress, close completes the job, notify fires on merge. Crash leaves an orphaned job you can see and clean up.

---

## Architectural Direction (Established by the POC)

### The stack

```
┌─────────────────────────────────────────────────────────────────┐
│  SFTP / SSH (russh-sftp Handler trait) — not yet wired           │
│  open / read / write / close / readdir / rename / unlink / ...   │
│  maps 1:1 to PathTree + WriteSession API                         │
└────────────────────────────┬────────────────────────────────────┘
                             │
                             ▼
┌─────────────────────────────────────────────────────────────────┐
│  PathTree (SQLite)                                               │
│  buckets, branches, paths, tombstones                            │
│  recursive-CTE chain walk for branch-aware resolve               │
│  WriteSession: branch-on-write, merge-on-close                   │
│  honker notify() inside every mutation txn                       │
└────────────────────────────┬────────────────────────────────────┘
                             │
              ┌──────────────┴──────────────┐
              ▼                             ▼
┌─────────────────────────────┐  ┌────────────────────────────────┐
│  iroh-blobs (content store)  │  │  honker (in the SQLite file)   │
│  BLAKE3 content addressing   │  │  notify/listen (watch)        │
│  MemStore (POC) /            │  │  durable queues (sync/replicate)│
│  FsStore (production, redb)  │  │  named locks (writer coord)   │
│  Tags + GC (mark-sweep)      │  │  scheduler (maintenance cron)  │
└─────────────────────────────┘  └────────────────────────────────┘
```

### Layer separation

| Concern | Layer | Why |
|---|---|---|
| Path → content hash mapping | SQLite path tree | Hierarchical, indexed, transactional, schema-as-doc |
| Content bytes | iroh-blobs | Content-addressed, dedup, verified streaming, network transfer |
| Path-tree mutations | SQLite txn | Atomic, crash-safe, single WAL |
| Filesystem events (watch/inotify) | honker notify() | Atomic with the mutation, wake-on-commit not poll |
| Background sync/replication kicks | honker queues | Transactional outbox: enqueue in same txn as mutation |
| Writer coordination | honker named locks | Prevent concurrent writers on same path |
| Branching / forking | branches table + chain walk | Fossil-style, content shared by hash, only path edges diverge |
| Multi-tenancy | bucket_id column | Free — just a where-clause, auth is an adapter problem |
| Chunked writes | write_sessions + write_chunks | Branch-on-write, merge-on-close, crash-safe per-chunk |

### SFTP mapping (conceptual, not yet wired)

The russh-sftp `Handler` trait (`/workspace/russh-sftp/src/server/handler.rs`) is a near 1:1 mirror of POSIX FS syscalls translated to SFTP packets. Each maps onto SQL operations against the path-tree tables:

| SFTP op | PathTree operation |
|---|---|
| `open(path, O_RDONLY)` | `resolve(bucket, branch, path)` → `Link` → iroh `BlobReader` |
| `open(path, O_WRONLY)` | `WriteSession::open(bucket, branch, path)` |
| `write(handle, offset, data)` | `WriteSession::write_chunk(offset, data)` |
| `close(handle)` | `WriteSession::close()` → hash → merge → notify |
| `readdir(path)` | `list_dir(bucket, branch, path)` |
| `stat`/`lstat`/`fstat` | `resolve(...)` → entry size/kind (indexed lookup, cheaper than real FS fstat) |
| `rename(from, to)` | `PathTree::rename(...)` — O(1) on edges, content stays in iroh |
| `remove(path)` | `PathTree::unlink(...)` — tombstone, content GC'd later |
| `mkdir`/`rmdir` | `PathTree::mkdir`/`unlink` with `kind=dir` |
| `symlink`/`readlink` | `PathTree::symlink`/`resolve` with `kind=symlink` |
| `extended` | SFTP escape hatch — could expose iroh-specific ops (get ticket, resolve to hash) |

The russh-sftp client's `File` already implements `AsyncRead + AsyncSeek + AsyncWrite` with pipelined writes (`write_nowait` + ack window), and the iroh `BlobReader` already supports range reads. The two trait surfaces line up. `SeekFrom::End` (round-trip-costly in real SFTP — calls `fstat`) becomes a single indexed SQLite lookup, so it's actually *cheaper* than a real FS fstat.

### Multi-tenancy / buckets

The "bucket" concept maps to S3's bucket format (from the rudolfs reference, `/workspace/@alkdev/alknet/docs/research/references/gitlfs/rudolfs-reference.md`). In rudolfs, `StorageKey = (Namespace, Oid)` where `Namespace = (org, project)` — tenant isolation by URL path. In the POC, `bucket_id` on every row achieves the same isolation with a single where-clause. Auth from the SSH/SFTP connection determines which buckets are visible — it's an adapter problem, not a storage problem.

The rudolfs caching layer (LRU + disk cache → permanent storage, with `fanout()` to stream to both client and cache simultaneously) is a useful pattern for the production version: a local iroh-blobs `FsStore` as cache, a remote iroh node as permanent storage. The decorator composition pattern (`Verify ↔ Encrypted ↔ Cached ↔ Retrying(Disk → S3)`) translates directly: `Verify` becomes BLAKE3 verification (built into iroh), `Cached` becomes the local iroh store, `S3` becomes the remote iroh node.

---

## Open Unknowns (For Future POCs)

These are the unknowns that remain after the POC. None are feasibility blockers (the basic mechanics work); they are scope/work-quantity questions that affect spec sizing.

### 1. FsStore (redb + filesystem) vs SQLite — the two-database question (scoping, not feasibility)

The POC used `MemStore` deliberately — no redb, no fsync rabbit hole, no partial-file lifecycle. The production version needs `FsStore` for persistence of large blobs. The open question: is having *both* redb (iroh's metadata) and SQLite (our path tree) in one process a problem?

Two embedded databases means two WAL files, two fsync paths, two crash-recovery stories. The likely answer is "fine, they serve different purposes" — redb stores blob metadata + inline data, SQLite stores path trees + write chunks + honker tables. But it needs validation.

The alternative — forking iroh-blobs to use SQLite instead of redb — is a big maintenance commitment. The POC's write_chunks table proves SQLite can handle chunk-sized inline BLOBs at iroh's granularity, so the swap is mechanically possible. But it should only be done if the two-database coexistence proves problematic, not for aesthetics. A scoping probe would run `FsStore` alongside the SQLite path tree and measure: double-fsync overhead, WAL contention, operational confusion.

### 2. Incomplete blobs in a distributed context (design, not feasibility)

The "many agents" scenario has a second incomplete-blob problem: agent B tries to read a file whose hash is in the path tree (inherited from parent) but whose *content* hasn't been downloaded to B's local store yet. What does the read return? Does it block? Does it trigger a fetch? Does it return an error?

iroh's `BlobReader` errors on missing chunks — but a filesystem caller expects either data or `ENOENT`, not "try again later." This is the seam between "path tree says it exists" and "blob store has the bytes." A design probe would model the fetch-on-read path: resolve → miss → async fetch from a peer that has it → block or return `EIO` temporarily. The honker queue is the coordination mechanism for background fetching.

### 3. SFTP wiring (mechanical, not design)

The `Handler` trait maps 1:1 to the `PathTree` + `WriteSession` API (see SFTP mapping table above). Wiring it is straightforward but non-trivial: handle management (open files, open directories), error code mapping (SQLite errors → SFTP status codes), `fsync@openssh.com` extension negotiation, and the `extended` channel for iroh-specific ops. A POC would implement the `Handler` trait and test with an actual `sshfs` mount.

### 4. honker named locks for writer coordination (mechanical)

The POC's `multiple_concurrent_write_sessions_same_path` test shows that two sessions on the same path don't corrupt each other's chunks (session-scoped), and last-close wins. But a real filesystem needs explicit locking: `honker_lock_acquire('path:<bucket>:<branch>:<path>', writer_id, timeout)` to prevent concurrent writers from stomping each other. The lock is a SQL function already registered on the connection — it just needs to be called in `WriteSession::open`. Not wired in the POC; straightforward to add.

### 5. GC and tag management (design)

iroh-blobs uses tags + mark-sweep GC: blobs are protected from deletion by tags (persistent or temp), and GC walks all tags to find reachable hashes, then sweeps everything else. The path tree needs to manage tags: when a path row points to a `Link`, that blob needs a persistent tag so it survives GC. When a path is tombstoned (unlinked), the tag can be removed and the blob becomes eligible for GC. The mapping from path rows to tags is the design question. A POC would wire `tags.create()` / `tags.delete()` into `upsert_path` / `unlink` and verify GC reclaims orphaned content.

### 6. Branch chain depth and performance (perf, deferred)

The recursive CTE walks the full parent chain on every resolve. For shallow chains (2-3 levels, as in "main → agent-a → working-session") this is sub-millisecond. For deep chains (many nested forks, or long-lived agent workspaces with many snapshot branches), performance could degrade. A materialized view or a "resolved paths" cache table (updated on commit) would solve this if it becomes an issue. Worth a perf probe with realistic branch depths before spec.

### 7. Snapshot / commit semantics (design)

The POC has branches but no explicit "commit" or "snapshot" operation — a branch is just a name, and writes to it are immediate. A real filesystem (especially one backing git) needs snapshot points: "this branch was at this state at this time." The `branches` table has `created_at` but no snapshot history. The design question: is a snapshot a new branch (Fossil's model), or is it a recorded point-in-time within a branch (git's model)? Fossil's model maps more naturally to the existing recursive CTE.

---

## Test Coverage

```
running 15 tests
test abort_leaves_old_version_untouched ... ok
test abort_on_new_file_leaves_no_trace ... ok
test branch_inherits_parent_content ... ok
test branch_modifications_do_not_leak_to_parent ... ok
test chunked_write_close_produces_correct_hash ... ok
test chunked_write_out_of_order ... ok
test concurrent_reader_sees_old_version_during_write ... ok
test content_is_deduped_across_branches ... ok
test large_chunked_file_writes_correctly ... ok
test multi_bucket_isolation ... ok
test multiple_concurrent_write_sessions_same_path ... ok
test put_get_roundtrip ... ok
test rename_is_o1_on_path_edges ... ok
test tombstone_hides_parent_file_on_child ... ok
test watch_fires_on_commit ... ok
```

---

## POC Structure

```
src/
  main.rs           # CLI: mkfs, ls, put, get, rm, mv, branch, watch, share-check
  schema.rs         # SQLite schema (paths, branches, tombstones, write_sessions,
                    #   write_chunks) + honker bootstrap
  fs.rs             # PathTree: resolve, put_file, mkdir, symlink, unlink, rename,
                    #   list_dir, read_file, open_write
                    # recursive-CTE chain walk for branch-aware resolve
  blob_bridge.rs    # iroh MemStore adapter: put_bytes → Link, get_bytes → bytes
  branch.rs         # Branch listing / creation
  write_session.rs  # Chunked write: open, write_chunk, close, abort
                    # branch-on-write, merge-on-close
  watch.rs          # honker notify/listen wrapper for fs events
  lib.rs            # re-exports
tests/
  integration.rs    # 15 tests: 8 path-tree + 7 write-path
```

---

## References

- POC: `/workspace/alknet-filesystem-poc` — `Cargo.toml`, `src/`, `tests/integration.rs`
- SQLite appfileformat: https://sqlite.org/appfileformat.html
- iroh-blobs source (v0.100): `/workspace/iroh-blobs` — `DESIGN.md` (blob store tradeoffs, hybrid approach, files-are-hard), `src/store/fs/` (FsStore, redb tables, EntryState), `src/api/blobs.rs` (Blobs API)
- iroh-blobs published (v0.103): cargo cache — `store/mem.rs` (MemStore), `api/blobs.rs` (AddProgress, BlobReader)
- honker: https://honker.dev/docs/ — `honker-core` API: `attach_honker_functions`, `attach_notify`, `bootstrap_honker_schema`, `apply_default_pragmas`
- honker source: `~/.cargo/registry/src/*/honker-core-0.2.4/src/` — `lib.rs`, `honker_ops.rs`
- russh-sftp source: `/workspace/russh-sftp/src/` — `server/handler.rs` (Handler trait), `client/fs/file.rs` (pipelined AsyncWrite), `client/fs/dir.rs`
- rudolfs reference: `/workspace/@alkdev/alknet/docs/research/references/gitlfs/rudolfs-reference.md` — decorator pattern, LRU cache, namespace/bucket isolation, fanout streaming
- iroh-blobs research docs: `/workspace/@alkdev/alknet/docs/research/references/iroh/iroh-blobs/` — overview, storage, key types, transfer protocol
- russh-sftp research docs: `/workspace/@alkdev/alknet/docs/research/references/ssh/russh-sftp/` — overview, client API, server API, wire protocol