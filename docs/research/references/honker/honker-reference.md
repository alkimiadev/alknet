# Research: Honker — SQLite Pub/Sub, Queue, and Notification Extension

## Key Findings

- **Honker is a Rust-based SQLite extension** that adds Postgres-style NOTIFY/LISTEN semantics plus durable pub/sub, task queues, and event streams entirely within SQLite. It eliminates the need for a separate message broker (Redis, Kafka) when SQLite is the primary datastore.
- **Three core primitives**: `notify/listen` (ephemeral pub/sub), `stream` (durable pub/sub with per-consumer offsets), and `queue` (at-least-once work queue with retries, priority, delayed jobs, and dead-letter handling). All three are SQL INSERTs inside your transaction — business write and side-effect commit or roll back together.
- **Wake mechanism**: Uses `PRAGMA data_version` polling at 1ms granularity to detect commits, achieving ~1-2ms median cross-process wake latency without requiring a daemon or broker. A single thread per database fans out to N subscribers via bounded channels.
- **Single-machine, single-writer model**: Designed for self-hosted deployments. Not distributed — no multi-node replication. This maps perfectly to alknet's per-node architecture where domain events are internal to a service boundary (ADR-032).
- **Comprehensive SQL API**: 30+ SQL scalar functions (`honker_enqueue`, `honker_claim_batch`, `honker_ack_batch`, `honker_stream_publish`, `honker_stream_read_since`, `honker_stream_save_offset`, `notify`, `honker_lock_acquire`, `honker_rate_limit_try`, `honker_scheduler_register`, etc.) registered as a loadable SQLite extension. Any language that can `SELECT load_extension('honker')` gets the same features.
- **Rust core (`honker-core`)**: All SQL implementations live in a shared Rust crate consumed by the loadable extension, PyO3 Python binding, napi-rs Node binding, and other language wrappers. One source of truth for the SQL — no behavioral drift across bindings.
- **License**: Apache 2.0 / MIT dual-license. Fully permissive for integration.

**Recommendation**: Adopt honker's patterns directly in `alknet-storage`. The `honker` crate (or `honker-core` for a Rust-native integration) should be a dependency of `alknet-storage`. Honker's single-node model aligns with alknet's event boundary discipline — domain events stay within the service boundary, and cross-node events go through the call protocol. For production deployments that use Postgres instead of SQLite, the same patterns (queue/claim, stream/subscribe, notify/listen) can be replicated using Postgres features, but honker's built-in retry, visibility timeout, and scheduling would need to be reimplemented.

---

## 1. Architecture

### What Is Honker?

Honker is a **SQLite extension + language bindings** that adds Postgres-style `NOTIFY`/`LISTEN` semantics to SQLite, with built-in durable pub/sub, task queues, and event streams — without requiring a client-polling loop, a daemon, or a separate broker.

**Core idea**: If SQLite is your primary datastore, your queue should live in the same file. `INSERT INTO orders` and `queue.enqueue(...)` commit in the same transaction. Rollback drops both.

**Implementation language**: Rust. The shared engine is `honker-core`, a plain Rust `rlib` crate. Language bindings (Python via PyO3, Node via napi-rs, Go via CGo, Ruby via C extension, .NET via P/Invoke, JVM via JNI, Kotlin wrapper, Elixir via NIF, C++ via header-only wrapper) are thin wrappers around the loadable extension's SQL functions.

**How it works as a SQLite extension**: The `honker-extension` crate compiles to `libhonker_ext.{so,dylib,dll}`. Any SQLite 3.9+ client loads it:

```sql
.load ./libhonker_ext
SELECT honker_bootstrap();
```

This creates the schema tables (`_honker_live`, `_honker_dead`, `_honker_notifications`, `_honker_stream`, `_honker_stream_consumers`, `_honker_locks`, `_honker_rate_limits`, `_honker_scheduler_tasks`, `_honker_results`) and registers all SQL scalar functions. The extension and Python/binding tables are shared, so a Python worker can claim jobs any other language pushed via the extension.

### Crate Structure

```
honker-core/              # Rust rlib shared across all bindings (published on crates.io)
honker-extension/         # SQLite loadable extension (cdylib, published on crates.io)
packages/
  honker/                 # Python package (PyO3 cdylib + Queue/Stream/Outbox/Scheduler)
  honker-node/            # napi-rs Node.js binding
  honker-rs/              # Ergonomic Rust wrapper
  honker-go/              # Go binding
  honker-ruby/            # Ruby binding
  honker-bun/             # Bun binding
  honker-ex/              # Elixir binding
  honker-cpp/             # C++ binding
  honker-dotnet/          # .NET / C# binding
  honker-jvm/             # JVM / Java-compatible binding
  honker-kotlin/          # Kotlin convenience wrapper
```

### Wake Path Architecture

The fundamental challenge for any SQLite-based pub/sub system: SQLite has no wire protocol or server-push. Consumers must initiate reads. Honker solves this with a **single-digit-microsecond `PRAGMA data_version` read**:

1. **One PRAGMA-poll thread per `Database`** queries `data_version` every 1ms
2. Counter change → fan out a tick to each subscriber's bounded channel (capacity 1 — coalesces redundant wakes)
3. Each subscriber runs `SELECT … WHERE id > last_seen` against a partial index, yields rows, returns to wait
4. 100 subscribers = 1 poll thread. Idle listeners run zero SQL queries.

Idle cost: ~3.5µs per `PRAGMA data_version` query, ~3.5ms/sec total at 1kHz. A 5-second paranoia poll exists as a fallback only if the update watcher cannot fire.

**Three backend options** (controlled by `WatcherBackend` enum):
- **Polling** (default, stable): `PRAGMA data_version` every 1ms. Correct on all platforms.
- **Kernel** (experimental, `kernel-watcher` Cargo feature): Uses `notify-rs` filesystem events. Fires on every filesystem write. May produce spurious/missed wakes. Dead-man's switch for file replacement.
- **SHM fast path** (experimental, `shm-fast-path` Cargo feature): Memory-maps the `-shm` WAL index file and reads `iChange` at offset 8 at ~100µs cadence. WAL-mode only. Dead-man's switch for file replacement.

**Dead-man's switch**: All backends check file identity `(dev, ino)` / `(volume_serial, file_index)` every ~100ms. If the database file is replaced (atomic rename, litestream restore, volume remount), the watcher panics with a clear error message. Subscribers see an error from `update_events()` instead of hanging silently.

### SharedUpdateWatcher

```rust
pub struct SharedUpdateWatcher {
    watcher: Mutex<Option<UpdateWatcher>>,  // background poll thread
    senders: Arc<Mutex<HashMap<u64, SyncSender<()>>>>,  // fan-out channels
    next_id: AtomicU64,
}
```

- `subscribe()` → `(u64, Receiver<()>)` — register a channel; capacity 1
- `unsubscribe(id)` — remove channel; receiver sees `Err(RecvError)` 
- `close()` — join the poll thread, clear all subscribers
- Wakes are idempotent "go re-read state" signals. Dropped redundant wakes never lose data.

---

## 2. Core Capabilities

### 2.1 Notify/Listen — Ephemeral Pub/Sub

**What it is**: Fire-and-forget notifications to channel subscribers. Like `pg_notify` but with table-backed persistence until explicitly pruned.

**How it works**:
- `notify(channel, payload)` is a SQL scalar function that INSERTs into `_honker_notifications` and returns the row id. Runs inside the caller's open transaction — rollbacks drop the notification.
- `db.listen(channel)` or `db.updateEvents()` in Node — registers a subscriber that wakes on any database commit, then filters by channel in the `SELECT` path.
- Listeners attach at current `MAX(id)`; **history is not replayed**. This is the key distinction from streams.

**Schema**:
```sql
CREATE TABLE _honker_notifications (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  channel TEXT NOT NULL,
  payload TEXT NOT NULL,
  created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX _honker_notifications_recent ON _honker_notifications(channel, id);
```

**Key characteristics**:
- Not auto-pruned. Call `db.prune_notifications(older_than_s=…, max_keep=…)` from a scheduled task.
- Over-triggering is by design: a `data_version` change wakes every subscriber on that database, not just the matching channel. Each wasted wake = one indexed SELECT (microseconds). A missed wake = a silent correctness bug.
- Payload must be valid JSON for cross-language compatibility.

### 2.2 Queue — At-Least-Once Work Queue

**What it is**: Durable, at-least-once delivery work queue with retries, priority, delayed jobs, task expiration, dead-letter handling, named locks, and rate-limiting.

**Schema (single-table hybrid)**:

```sql
CREATE TABLE _honker_live (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  queue TEXT NOT NULL,
  payload TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT 'pending',      -- 'pending' | 'processing'
  priority INTEGER NOT NULL DEFAULT 0,
  run_at INTEGER NOT NULL DEFAULT (unixepoch()), -- for delayed jobs
  worker_id TEXT,
  claim_expires_at INTEGER,                    -- visibility timeout
  attempts INTEGER NOT NULL DEFAULT 0,
  max_attempts INTEGER NOT NULL DEFAULT 3,
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  expires_at INTEGER                            -- job expiration
);

CREATE INDEX _honker_live_claim
  ON _honker_live(queue, priority DESC, run_at, id)
  WHERE state IN ('pending', 'processing');

CREATE TABLE _honker_dead (
  id INTEGER PRIMARY KEY,
  queue TEXT NOT NULL,
  payload TEXT NOT NULL,
  priority INTEGER NOT NULL DEFAULT 0,
  run_at INTEGER NOT NULL DEFAULT 0,
  attempts INTEGER NOT NULL DEFAULT 0,
  max_attempts INTEGER NOT NULL DEFAULT 0,
  last_error TEXT,
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  died_at INTEGER NOT NULL DEFAULT (unixepoch())
);
```

**Claim/ack/nack model**:

| Operation | SQL | Notes |
|-----------|-----|-------|
| Enqueue | `INSERT INTO _honker_live (queue, payload, run_at, priority, max_attempts, expires_at) VALUES (…)` | Returns auto-increment id |
| Claim | `UPDATE _honker_live SET state='processing', worker_id=?, claim_expires_at=unixepoch()+?, attempts=attempts+1 WHERE id IN (SELECT id FROM _honker_live WHERE queue=? AND state IN ('pending','processing') AND (expires_at IS NULL OR expires_at > unixepoch()) AND ((state='pending' AND run_at <= unixepoch()) OR (state='processing' AND claim_expires_at < unixepoch())) ORDER BY priority DESC, run_at ASC, id ASC LIMIT ?) RETURNING …` | One `UPDATE … RETURNING` via partial index |
| Ack | `DELETE FROM _honker_live WHERE id=? AND worker_id=? AND claim_expires_at >= unixepoch() RETURNING id` | Returns 1 if claim still valid, 0 if expired |
| Retry | `UPDATE _honker_live SET state='pending', run_at=unixepoch()+?, worker_id=NULL, claim_expires_at=NULL WHERE id=?` + notify on queue channel | If `attempts >= max_attempts`, DELETE from `_honker_live` and INSERT into `_honker_dead` |
| Fail | `DELETE FROM _honker_live WHERE id=? AND worker_id=? AND claim_expires_at >= unixepoch() RETURNING …` + `INSERT INTO _honker_dead` | Unconditionally move to dead letter |
| Heartbeat | `UPDATE _honker_live SET claim_expires_at=unixepoch()+? WHERE id=? AND worker_id=? AND state='processing'` | Extend claim for long-running handlers |
| Cancel | `DELETE FROM _honker_live WHERE id=? AND state IN ('pending', 'processing')` | Idempotent |

**Visibility timeout**: Default 300 seconds (`claim_expires_at = unixepoch() + 300`). If a worker crashes mid-job, the claim expires and another worker reclaims. `attempts` increments. After `max_attempts` (default 3), the row moves to `_honker_dead`.

**Priority**: Higher `priority` value = claimed first. The partial index on `(queue, priority DESC, run_at, id)` ensures claim path is bounded by working-set size, not history size.

**Delayed jobs**: Set `run_at` to a future timestamp. Workers only claim rows where `run_at <= unixepoch()`. The `run_at` deadline also wakes sleeping workers through `honker_queue_next_claim_at()`.

**Task expiration**: Set `expires_at` on enqueue. Expired jobs are filtered from the claim path. Call `queue.sweep_expired()` to move them to `_honker_dead` with `last_error='expired'`.

**Named locks**: `honker_lock_acquire(name, owner, ttl_s)` → 1 (got it) or 0 (held). `honker_lock_release(name, owner)` → 1 (released) or 0 (not yours). Uses `_honker_locks` table with TTL-based expiration. Primary use case: cron tasks that shouldn't overlap (leader election).

**Rate limiting**: `honker_rate_limit_try(name, limit, per)` → 1 (under limit) or 0 (at limit). Fixed-window counter. Rejected calls don't inflate the count.

**Batch operations**: `honker_claim_batch(queue, worker_id, n, timeout_s)` returns a JSON array of claimed jobs. `honker_ack_batch('[1,2,3]', worker_id)` acks multiple jobs. Ack is per-transaction for batch — honest bool return.

**Task result storage**: `honker_enqueue()` returns the job id. Workers can persist return values via `honker_result_save(id, value, ttl_s)`. Callers await results with `queue.wait_result(id, timeout)`. Opt-in (default `save_result=False`).

**Claim iterator pattern**:
```python
async for job in q.claim("worker-1"):
    try:
        send(job.payload)
        job.ack()
    except Exception as e:
        job.retry(delay_s=60, error=str(e))
```

Each iteration is `claim_batch(worker_id, 1)`. Wakes on database update from any process, or when the next `run_at` / reclaim deadline arrives. 5-second paranoia poll is the only fallback.

**Queue notifications**: Each enqueue also fires a notification on `honker:<queue>` channel so workers wake immediately without waiting for the next poll cycle.

### 2.3 Stream — Durable Pub/Sub with Per-Consumer Offsets

**What it is**: Durable event stream where each named consumer tracks its own offset. Events persist until explicitly pruned. At-least-once delivery with configurable offset flush cadence.

**Schema**:
```sql
CREATE TABLE _honker_stream (
  offset INTEGER PRIMARY KEY AUTOINCREMENT,
  topic TEXT NOT NULL,
  key TEXT,
  payload TEXT NOT NULL,
  created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX _honker_stream_topic
  ON _honker_stream(topic, offset);

CREATE TABLE _honker_stream_consumers (
  name TEXT NOT NULL,
  topic TEXT NOT NULL,
  offset INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (name, topic)
);
```

**API**:

| Function | Returns | Notes |
|----------|---------|-------|
| `honker_stream_publish(topic, key_or_null, payload_json)` | `offset` | INSERTs into `_honker_stream` + fires notification on `honker:stream:<topic>` |
| `honker_stream_read_since(topic, offset, limit)` | JSON array | Reads rows where `offset > ?` ordered by offset |
| `honker_stream_save_offset(consumer, topic, offset)` | 1 or 0 | Monotonic upsert — never rewinds. 1 = advanced, 0 = existing offset ≥ new |
| `honker_stream_get_offset(consumer, topic)` | offset or 0 | Returns saved offset for consumer/topic pair |

**Python binding**:
```python
stream = db.stream("user-events")
stream.publish({"user_id": uid, "change": "name"}, tx=tx)
async for event in stream.subscribe(consumer="dashboard"):
    await push_to_browser(event)
```

**Subscribe behavior**:
1. Replay rows past `offset > saved_offset` in batches (default 1000 rows)
2. Transition to live delivery on commit wake
3. Auto-save offset at most every 1000 events or every 1 second (whichever first)
4. At-least-once: a crash re-delivers in-flight events up to the last flushed offset
5. Override auto-save with `save_every_n=` / `save_every_s=`; set both to 0 for manual control

**Transaction coupling**: `stream.publish(payload, tx=tx)` inserts into `_honker_stream` inside the caller's transaction. Rollback drops the event. This is the transactional outbox pattern without a separate dispatch table.

### 2.4 Scheduler — Time-Triggered Cron Tasks

**Schema**:
```sql
CREATE TABLE _honker_scheduler_tasks (
  name TEXT PRIMARY KEY,
  queue TEXT NOT NULL,
  cron_expr TEXT NOT NULL,
  payload TEXT NOT NULL,
  priority INTEGER NOT NULL DEFAULT 0,
  expires_s INTEGER,
  next_fire_at INTEGER NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1
);
```

**API**:
```sql
SELECT honker_scheduler_register('nightly', 'backups', '0 3 * * *', '"go"', 0, NULL);
SELECT honker_scheduler_tick(unixepoch());      -- JSON: fires due
SELECT honker_scheduler_soonest();                -- min next_fire_at
SELECT honker_scheduler_unregister('nightly');    -- 1 = deleted
SELECT honker_scheduler_pause('nightly');        -- 1 = paused
SELECT honker_scheduler_resume('nightly');       -- 1 = resumed
SELECT honker_scheduler_list();                   -- JSON array of all schedules
SELECT honker_scheduler_update('nightly', '0 4 * * *', NULL, NULL, NULL, 0);
```

Supports: 5-field cron, 6-field cron (with seconds), `@every <n><unit>` interval expressions.

**Leader election via named lock**: `db.lock('honker-scheduler', ttl=60)`. Two scheduler processes can't both fire. The lock is heartbeat-refreshed every 30s.

**Missed-fire catch-up**: If the scheduler was down for 4 hours with an hourly schedule, the first iteration fires all 4 missed boundaries (with `expires=` to drop stale ones).

**Fires = enqueue**: The scheduler never runs handlers. It enqueues into the task queue. Regular workers consume.

### 2.5 Outbox Pattern

The `outbox` is a convenience wrapper around the `Queue` primitive:

```python
db.outbox("emails", delivery=send_email)
db.outbox("emails").enqueue({"to": "alice@example.com"}, tx=tx)
db.outbox("emails").run_worker("worker-1")
```

Failures retry with exponential backoff (`base_backoff_s * 2^(attempts-1)`) up to `max_attempts`, then land in `_honker_dead`.

---

## 3. Persistence and Reliability

### Durability Guarantees

- **Atomic commit**: Business write + side-effect enqueue/event/notify commit together or roll back together. This is SQLite ACID — the transactional outbox pattern is built into the primitives, not bolted on.
- **SIGKILL safety**: Verified in `tests/test_crash_recovery.py`. Subprocess killed pre-COMMIT → `PRAGMA integrity_check == 'ok'`, zero in-flight rows, no stale write lock, queue round-trip works post-crash.
- **Worker crash recovery**: If a worker crashes mid-job, the claim expires after `visibility_timeout_s` (default 300s) and another worker reclaims. `attempts` increments on each claim. After `max_attempts` (default 3), the row moves to `_honker_dead`.
- **Stream at-least-once**: Offsets auto-flush every 1000 events or 1 second. A crash re-delivers in-flight events up to the last flushed offset. The crash window is bounded by the flush thresholds.
- **Notify has no replay**: Listeners attach at `MAX(id)`. Pruned events are gone. For durable replay, use streams.

### WAL Mode

Recommended default (`journal_mode = WAL`). Gives concurrent readers with one writer and efficient fsync batching (`wal_autocheckpoint = 10000`). Other journal modes work but lose WAL's concurrent-read-while-writing property. Wake detection (`PRAGMA data_version`) works in all journal modes.

### What Happens on Crash

| Scenario | Result |
|----------|--------|
| Process SIGKILL mid-TRANSACTION | SQLite atomic-commit rollback. In-flight write did not land. Fresh process can acquire write lock immediately. |
| Worker process crash mid-job | Claim expires after visibility_timeout. Another worker reclaims. `attempts` increments. |
| Stream consumer crash | Resumes from last auto-saved offset (at-least-once). Pending offset is lost. |
| Database file replaced (litestream restore) | Watcher panics with clear error message. All subscribers see error from update_events(). Must reopen database. |

### What Honker Does NOT Provide

- **Multi-writer replication**: SQLite's locking is for single-host. Two servers writing one `.db` over NFS will corrupt it. Shard by file or switch to Postgres.
- **In-memory database support**: `:memory:` creates a separate database per connection, splitting writer/readers/watchers. Use temp file-backed `.db` for tests.
- **Cross-node distribution**: Honker is single-machine. No built-in mechanism for distributing events across nodes. (This is intentional — see alknet relevance below.)
- **Task pipelines/chains/groups/chords**. Deliberately not built.
- **Workflow orchestration with DAGs**. Deliberately not built.
- **Ordering guarantees across queues**. Each queue is independent.
- **Exactly-once delivery**. Honker provides at-least-once. Idempotent handlers are the user's responsibility.

---

## 4. Performance

### Benchmarks (M-series, release build, median of 3)

| Operation | Throughput |
|-----------|-----------|
| enqueue (1/tx) | ~8,000/sec |
| enqueue (100/tx) | ~110,000/sec |
| claim + ack (individual) | ~4,500/sec |
| claim_batch + ack_batch (32) | ~75,000/sec |
| claim_batch + ack_batch (128) | ~110,000/sec |
| async iter end-to-end | ~6,500/sec |
| stream replay | ~1,000,000/sec |
| stream live e2e p50 | 0.24ms |
| stream live e2e p99 | 8ms |

### Cross-Process Wake Latency

Median ~1-2ms on M-series, bounded by the 1ms `PRAGMA data_version` poll cadence. 600-second soak test under sustained ~75 commits/sec showed zero missed wakes, zero drift, `PRAGMA integrity_check = ok`.

### Claim Performance at Scale

With 100,000 dead rows in `_honker_dead`:

| Operation | Claim+ack |
|-----------|-----------|
| 0 dead rows (fresh DB) | ~4,000/sec |
| 100k dead rows | ~3,500/sec |

The partial index `(queue, priority DESC, run_at, id) WHERE state IN ('pending','processing')` keeps the claim hot path bounded by working-set size, not history size.

### How It Compares to Polling

Prior to honker's wake mechanism, the alternative would be application-level polling (e.g., `SELECT … WHERE id > last_seen` every N seconds). Honker replaces this with a single-digit-microsecond PRAGMA read. 100 subscribers still = 1 poll thread. The over-triggering trade-off (waking all subscribers on any commit) is explicitly chosen over potentially missing a wake.

---

## 5. SQLite Integration

### Loading the Extension

```sql
-- Any SQLite 3.9+ client
.load ./libhonker_ext
SELECT honker_bootstrap();
```

`honker_bootstrap()` is idempotent — it runs `CREATE TABLE IF NOT EXISTS` and `CREATE INDEX IF NOT EXISTS` for all schema tables.

### Compile/Load Flags

For Rust integration via `rusqlite`:

```toml
[dependencies]
honker-core = "0.2.3"
rusqlite = { version = "0.39.0", features = ["functions", "hooks"] }
```

Then in Rust:
```rust
use honker_core::{attach_notify, attach_honker_functions, bootstrap_honker_schema, open_conn};

let conn = open_conn("app.db", true)?;  // true = install notify
attach_honker_functions(&conn)?;
bootstrap_honker_schema(&conn)?;
```

For the loadable extension:
```bash
cargo build --release -p honker-extension
# Produces: target/release/libhonker_ext.so (or .dylib, .dll)
```

### Rust Crate Usage

```rust
use honker_core::SharedUpdateWatcher;

let watcher = SharedUpdateWatcher::new(db_path.clone());
let (sub_id, rx) = watcher.subscribe();

// In a loop:
match rx.recv_timeout(Duration::from_secs(5)) {
    Ok(()) => { /* re-read state from SQLite */ },
    Err(RecvTimeoutError::Timeout) => { /* paranoia poll */ },
    Err(RecvTimeoutError::Disconnected) => { /* watcher died, reopen */ },
}

watcher.unsubscribe(sub_id);
watcher.close()?;
```

### Using with ORM Connections

Load `libhonker_ext` on the ORM's connection and call `honker_bootstrap()` inside the ORM's transaction:

```python
# SQLAlchemy
@event.listens_for(engine, "connect")
def _load_honker(conn, _):
    honker.load_extension(conn)
    conn.execute("SELECT honker_bootstrap()")

with Session(engine) as s, s.begin():
    s.add(Order(user_id=42))
    s.execute(text("SELECT honker_enqueue(:q, :p, NULL, NULL, 0, 3, NULL)"),
              {"q": "emails", "p": '{"to":"alice"}'})
```

### PRAGMA Defaults

Applied on every connection opened via `open_conn`:

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;         -- fsync WAL at checkpoint, not every commit
PRAGMA busy_timeout = 5000;          -- wait up to 5s for writer lock
PRAGMA foreign_keys = ON;
PRAGMA cache_size = -32000;          -- 32MB page cache (default was 2MB)
PRAGMA temp_store = MEMORY;          -- temp B-trees in RAM
PRAGMA wal_autocheckpoint = 10000;   -- fsync every 10k WAL pages
```

---

## 6. Complete API Surface

### Notification Functions

| Function | Signature | Returns | Notes |
|----------|-----------|---------|-------|
| `notify(channel, payload)` | Scalar, 2 args | `rowid` | INSERTs into `_honker_notifications`, returns auto-generated id |

### Queue Functions

| Function | Signature | Returns | Notes |
|----------|-----------|---------|-------|
| `honker_bootstrap()` | 0 args | `1` | Creates all schema tables/indexes. Idempotent. |
| `honker_enqueue(queue, payload, run_at_or_null, delay_or_null, priority, max_attempts, expires_or_null)` | 7 args | `id` | INSERTs job. Delay overrides run_at. |
| `honker_claim_batch(queue, worker_id, n, timeout_s)` | 4 args | JSON array | Claims up to `n` jobs. Each gets `claim_expires_at = now + timeout_s`. |
| `honker_ack_batch(ids_json, worker_id)` | 2 args | `count` | ACKs (DELETEs) claimed jobs. `ids_json` is `[1,2,3]`. |
| `honker_ack(job_id, worker_id)` | 2 args | `1` or `0` | Single-job ack. Returns 0 if claim expired. |
| `honker_retry(job_id, worker_id, delay_s, error)` | 4 args | `1` or `0` | Retries (flips back to pending) or fails to dead if `attempts >= max_attempts`. |
| `honker_fail(job_id, worker_id, error)` | 3 args | `1` or `0` | Unconditionally moves to `_honker_dead`. |
| `honker_heartbeat(job_id, worker_id, extend_s)` | 3 args | `1` or `0` | Extends claim for long-running handlers. |
| `honker_cancel(job_id)` | 1 arg | `1` or `0` | Removes pending/processing row. Idempotent. |
| `honker_get_job(job_id)` | 1 arg | JSON or `""` | Read job state. Pure read. |
| `honker_sweep_expired(queue)` | 1 arg | `count` | Moves expired pending jobs to `_honker_dead`. |
| `honker_queue_next_claim_at(queue)` | 1 arg | `unix_ts` or `0` | Earliest future deadline (run_at or claim_expires_at + 1). |

### Stream Functions

| Function | Signature | Returns | Notes |
|----------|-----------|---------|-------|
| `honker_stream_publish(topic, key_or_null, payload_json)` | 3 args | `offset` | INSERTs event + fires notification |
| `honker_stream_read_since(topic, offset, limit)` | 3 args | JSON array | Reads events with `offset > ?` |
| `honker_stream_save_offset(consumer, topic, offset)` | 3 args | `1` or `0` | Monotonic upsert. 0 = existing offset ≥ new |
| `honker_stream_get_offset(consumer, topic)` | 2 args | `offset` or `0` | Returns saved offset |

### Lock Functions

| Function | Signature | Returns | Notes |
|----------|-----------|---------|-------|
| `honker_lock_acquire(name, owner, ttl_s)` | 3 args | `1` or `0` | 1 = acquired, 0 = held |
| `honker_lock_release(name, owner)` | 2 args | `1` or `0` | 1 = released, 0 = not yours |

### Rate Limit Functions

| Function | Signature | Returns | Notes |
|----------|-----------|---------|-------|
| `honker_rate_limit_try(name, limit, per)` | 3 args | `1` or `0` | 1 = under limit, 0 = at limit |
| `honker_rate_limit_sweep(older_than_s)` | 1 arg | `count` | Prunes expired windows |

### Scheduler Functions

| Function | Signature | Returns | Notes |
|----------|-----------|---------|-------|
| `honker_scheduler_register(name, queue, cron_expr, payload, priority, expires_s_or_null)` | 6 args | `1` | Upserts task. Computes next_fire_at. |
| `honker_scheduler_unregister(name)` | 1 arg | `0` or `1` | Deletes task. |
| `honker_scheduler_tick(now_unix)` | 1 arg | JSON array | Fires due tasks, enqueues payloads, advances next_fire_at. |
| `honker_scheduler_soonest()` | 0 args | `unix_ts` or `0` | Earliest next_fire_at for sleep duration calculation. |
| `honker_scheduler_pause(name)` | 1 arg | `0` or `1` | Toggles `enabled = 0`. |
| `honker_scheduler_resume(name)` | 1 arg | `0` or `1` | Toggles `enabled = 1`. |
| `honker_scheduler_list()` | 0 args | JSON array | Returns all schedules with state. |
| `honker_scheduler_update(name, cron_expr_or_null, payload_or_null, priority_or_null, expires_s_or_null, touch_expires)` | 6 args | `0` or `1` | Mutates schedule fields. Recomputes next_fire_at if cron_expr changed. |
| `honker_cron_next_after(expr, from_unix)` | 2 args | `unix_ts` | Pure deterministic function. 5-field, 6-field, or `@every <n><unit>`. |

### Result Functions

| Function | Signature | Returns | Notes |
|----------|-----------|---------|-------|
| `honker_result_save(job_id, value_json, ttl_s)` | 3 args | `1` | UPSERTs result. `ttl_s=0` = no expiration. |
| `honker_result_get(job_id)` | 1 arg | `value` or `NULL` | Returns result or NULL if expired/missing. |
| `honker_result_sweep()` | 0 args | `count` | Prunes expired result rows. |

### Watcher Functions (Extension ABI)

| Function | Signature | Returns | Notes |
|----------|-----------|---------|-------|
| `honker_update_watcher_open(db_path, backend)` | 2 SQL args | `id` | Opens a watcher handle. For Elixir and extension consumers. |
| `honker_update_watcher_wait(id, timeout_ms)` | 2 SQL args | `1`/`0`/`-1` | 1 = update observed, 0 = timeout, -1 = disconnected |
| `honker_update_watcher_close(id)` | 1 SQL arg | `1` | Closes watcher handle. |

C ABI (for Go, .NET, C++, Ruby bindings that route through the extension):

| Function | Signature | Returns | Notes |
|----------|-----------|---------|-------|
| `honker_watcher_open(db_path, backend, err_buf, err_buf_len)` | C ABI | `*mut HonkerWatcherHandle` | Opens a core-backed update watcher. |
| `honker_watcher_wait(handle, timeout_ms)` | C ABI | `1`/`0`/`-1`/`-2` | 1 = update, 0 = timeout, -1 = closed, -2 = panic |
| `honker_watcher_close(handle)` | C ABI | void | Closes and frees the handle. |

### Tables

| Table | Purpose |
|-------|---------|
| `_honker_live` | Pending + processing jobs. Partial index for fast claims. |
| `_honker_dead` | Terminal jobs (retry-exhausted or explicitly failed). Never scanned by claim path. |
| `_honker_notifications` | Ephemeral notify/listen messages. Not auto-pruned. |
| `_honker_stream` | Durable stream events with auto-incrementing offsets. |
| `_honker_stream_consumers` | Per-consumer stream offsets. Monotonic upsert. |
| `_honker_locks` | Named advisory locks with TTL expiration. |
| `_honker_rate_limits` | Fixed-window rate limit counters. |
| `_honker_scheduler_tasks` | Cron/schedule task definitions with next_fire_at. |
| `_honker_results` | Task result storage with TTL expiration. |

---

## 7. Comparison to Postgres (pg_notify)

| Feature | Honker | pg_notify |
|---------|--------|-----------|
| **Delivery model** | Table-backed `INSERT` in transaction | In-memory NOTIFY with LISTEN callback |
| **Persistence** | Rows survive restart. Not auto-pruned. | Ephemeral — lost on restart, not replayed. |
| **Transactional coupling** | `notify(channel, payload)` inside `BEGIN IMMEDIATE; INSERT; COMMIT` — atomic with business write | NOTIFY fires at COMMIT inside the same transaction. Atomic with business write. |
| **Retry / visibility timeout** | Queue has `claim_expires_at`, `attempts`, `max_attempts`, dead-letter. | No retry. No visibility timeout. |
| **Delayed delivery** | `run_at` for scheduled delivery. Jobs only claimable after deadline. | No scheduling. |
| **Cross-process wake** | `PRAGMA data_version` polling at ~1ms cadence. SharedUpdateWatcher fans out to N subscribers. | Postgres notifies listeners via its inter-process communication. |
| **Priority** | Queue priority via partial index `(queue, priority DESC, run_at, id)`. | No priority. |
| **Rate limiting** | Built-in fixed-window `rate_limit_try`. | No rate limiting. |
| **Named locks** | TTL-based advisory locks in `_honker_locks`. | `pg_advisory_lock` (similar concept, different implementation). |
| **Cron scheduling** | Built-in scheduler with 5-field/6-field cron + `@every` intervals. | Needs pg-boss/Oban/cron extension. |
| **Stream offsets** | Per-consumer tracked offsets with monotonic upsert. | No built-in stream offsets. |
| **Multi-process** | Single-machine, single-writer. | Multi-process, multi-writer natively. |
| **Durability** | SQLite ACID. WAL mode for concurrent readers. | Postgres ACID. Full write-ahead logging. |

**What honker gives you that pg_notify alone doesn't**:
1. **Retry with exponential backoff** — automatic re-delivery on failure
2. **Visibility timeout** — crashed workers don't permanently lose messages
3. **Dead-letter queue** — exhausted retries land in `_honker_dead` for inspection
4. **Delayed jobs** — `run_at` for future delivery
5. **Prioritization** — `priority` column in claim index
6. **Transactional outbox** — business write + enqueue/event in one transaction, without adding Redis/Celery
7. **Task result storage** — workers can persist return values; callers can await results
8. **Durable streams** — per-consumer offsets with at-least-once delivery
9. **Cron scheduling** — built-in periodic tasks with leader election
10. **Named locks and rate limiting** — built-in coordination primitives

**What you'd need to add if you used Postgres instead**: pg-boss, Oban, or similar PgBoss-style packages provide many of these features, but they require Postgres as the database. Honker exists for the case where SQLite is already the primary datastore.

---

## 8. Comparison to Other Message Systems

| Feature | Honker | Redis Pub/Sub | NATS | Kafka |
|---------|--------|--------------|------|-------|
| **Persistence** | SQLite tables (disk) | In-memory only (unless RDB/AOF) | In-memory (JetStream adds persistence) | Persistent log |
| **Transactional coupling** | Business write + enqueue in one tx | Not atomic with business data | Not atomic with business data | Not atomic with business data |
| **Delivery guarantee** | At-least-once | At-most-once (fire-and-forget) | At-most-once (core); at-least-once (JetStream) | At-least-once with consumer offsets |
| **Retry/visibility** | Built-in (claim timeout, retry, dead-letter) | None (messages disappear if no consumer) | None (core); redelivery (JetStream) | Consumer group offsets |
| **Priority** | Yes (partial index) | No | No | No |
| **Delayed delivery** | Yes (`run_at`) | No (requires sorted sets hack) | No | No (requires time-based logic) |
| **Single-node complexity** | Zero — just a `.db` file | Requires Redis server | Requires NATS server | Requires Kafka cluster |
| **Cross-process wake latency** | 1-2ms | ~0.1ms | ~0.1ms | ~1-5ms |
| **Cross-node distribution** | None (single machine) | Pub/Sub is fan-out to connected clients | JetStream supports clustering | Built for distributed |
| **Dependency** | SQLite (already in your stack) | Additional server | Additional server | Additional cluster |
| **Schema coupling** | Same file as business data — dual-write impossible | Separate system — dual-write risk | Separate system — dual-write risk | Separate system — dual-write risk |
| **Language support** | Python, Node, Rust, Go, Ruby, Bun, Elixir, C++, .NET, JVM, Kotlin | Many (but protocol, not SQL) | 40+ client libraries | Many client libraries |
| **Dead-letter queue** | Built-in `_honker_dead` | None | JetStream has DLQ | DLQ via configuration |

**When honker is the right choice**: SQLite is already your primary datastore, and you need pub/sub + queue + scheduling without introducing Redis/Celery/NATS. The dual-write problem between your business tables and the queue disappears.

**When honker is NOT the right choice**: Multi-node deployments, multi-writer sharding, need for cross-datacenter replication, or workloads exceeding single-machine throughput.

---

## 9. Relevance to Alknet

### 9.1 Alignment with Event Boundary Discipline (ADR-032)

ADR-032 defines three communication layers:

```
Call Protocol (Layer 3, external, JSON)
    └── irpc Service (Layer 3, internal, postcard)
            └── Honker Streams (Domain events, within service boundary)
```

**Honker's single-machine model is exactly right for the bottom layer.** Domain events in alknet are internal to the service that owns that data — `nodes:created`, `edges:deleted`, `accounts:updated`. These never cross the service boundary without projection into a call protocol `EventEnvelope`.

The integration plan (Phase 2.2) explicitly lists honker integration patterns for alknet-storage:

| Feature | Use Case |
|---------|----------|
| `stream_publish` / `subscribe` | Durable pub/sub for node/edge/membership changes |
| `notify` / `listen` | Ephemeral pub/sub for real-time control channel events |
| `queue` / `claim` / `ack` | Task queue for async operations |

### 9.2 Patterns from Honker for alknet-storage Adoption

**Map honker's primitives to alknet-storage's internal events**:

| Alknet Domain Event | Honker Primitive | Stream Name |
|---------------------|------------------|-------------|
| Node created | `stream.publish("nodes:created", ...)` | `nodes:created` |
| Node updated | `stream.publish("nodes:updated", ...)` | `nodes:updated` |
| Node deleted | `stream.publish("nodes:deleted", ...)` | `nodes:deleted` |
| Edge created | `stream.publish("edges:created", ...)` | `edges:created` |
| Account updated | `stream.publish("accounts:updated", ...)` | `accounts:updated` |
| ACL rule changed | `stream.publish("acl:changed", ...)` | `acl:changed` |

**Map honker's task queue to alknet's async operations**:

| Alknet Async Task | Honker Queue |
|-------------------|-------------|
| Key rotation | `queue("key-rotation")` |
| Certificate renewal | `queue("cert-renewal")` |
| Audit log archival | `queue("audit-archival")` |
| Node encryption/decryption | `queue("node-crypto")` |

**Map honker's notify/listen to real-time events**:

| Alknet Real-Time Event | Honker Channel |
|------------------------|---------------|
| SSH connection opened | `notify("ssh:connected", ...)` |
| Config reload triggered | `notify("config:reload", ...)` |
| Forwarding rule activated | `notify("forwarding:activated", ...)` |

### 9.3 Replicating Honker Patterns with Postgres for Production

If alknet-storage is backed by Postgres in production deployments (the storage spec mentions `rusqlite` but leaves room for alternative backends), the following Postgres equivalents would be needed:

| Honker Primitive | Postgres Equivalent | What's Lost |
|-----------------|---------------------|-------------|
| `notify/listen` | `pg_notify` + `LISTEN` | Postgres NOTIFY is ephemeral (lost on restart). Honker's table-backed notifications persist. Need to add a `_notifications` table and polling. |
| `stream_publish/subscribe` | `pg_notify` + consumer offset table | No built-in per-consumer offset tracking. Would need a `_stream_consumers` table and polling/cursor logic. |
| `queue/claim/ack` | pg-boss / Oban | These exist and are production-quality. Honker's simplicity (one table, partial index) is lost. Need a dependency on Oban or pg-boss. |
| `run_at` (delayed jobs) | Oban's `scheduled_at` / pg-boss's `startAfter` | Available in both. |
| `claim_expires_at` (visibility timeout) | Oban's `attempted_at` + `max_attempts` | Available in both. |
| `honker_lock_acquire/release` | `pg_advisory_lock` | Built-in, similar concept. |
| `honker_rate_limit_try` | Custom table or Redis | Postgres has no built-in rate limiting. |
| Transactional coupling | Same tx | Naturally available: `INSERT INTO orders ...; INSERT INTO _honker_live ...;` both in the same Postgres tx. |
| Scheduler | pg-boss `schedule()` or Oban's `Oban.insert(CronWorker, ...)` | Available in both. |

**What would be lost switching to Postgres + pg-boss/Oban**:
- **Schema simplicity**: Honker uses 2 tables for 90% of queue operations. pg-boss uses more tables. Oban uses per-queue tables.
- **Zero-dependency**: Honker is a SQLite extension. No Redis, no Celery, no broker. pg-boss requires Postgres. Oban requires Postgres + Elixir.
- **Cross-language transparency**: Any SQLite client can `SELECT load_extension('honker')` and get the same features. Postgres requires language-specific client libraries.
- **File-based deployment**: Copy the `.db` file. Done. Postgres requires a server.

**Recommendation for alknet-storage**: Start with honker on SQLite for self-hosted/edge deployments. For production Postgres deployments, create an abstraction layer in `alknet-storage` that implements the same `EventStream`, `TaskQueue`, and `NotificationChannel` traits against both backends. The honker-on-SQLite implementation is the reference; the Postgres implementation uses `pg_notify` + offset tables + Oban/pg-boss.

### 9.4 Honker's Queue/Claim Model and alknet's Call Protocol

The call protocol's `EventEnvelope` frames are the integration boundary (ADR-033). When a domain event needs to cross node boundaries, it must be projected:

```
Honker stream event (internal)
    → Projection function
        → EventEnvelope frame (external, call protocol)
            → Transported over SSH/QUIC/DNS
                → Received by remote node
                    → May trigger local Honker stream event on remote node
```

The **queue/claim model maps to async call protocol operations**:

1. **call.requested** → Honker `queue.enqueue({"operation": "/head/auth/verify", "input": {...}})`
2. **Worker claims the job** → Like a worker process picking up a call request
3. **job.ack()** → call.responded with the result
4. **job.retry()** → Call timeout / retry logic (but this is at the transport layer, not the queue)
5. **job fails → _honker_dead** → Dead letter equivalent for failed call protocol operations

The **key difference**: alknet's call protocol is synchronous request-response at the transport layer, while honker's queue is async at-least-once. They serve different purposes:
- **Call protocol**: "I need you to verify this pubkey NOW" (synchronous, cross-node)
- **Honker queue**: "Process this key rotation in the background" (asynchronous, within-node)

For **cross-node task distribution**, honker's queue should NOT be the transport. Instead:
1. A domain event (honker stream) in the storage service triggers a projection
2. The projection creates an `EventEnvelope` frame
3. The call protocol delivers it to remote nodes
4. Remote nodes may enqueue it into their own honker queues for local processing

### 9.5 Cross-Node Event Distribution

**Honker is single-node by design.** This is correct for alknet's architecture because:

1. **Domain events stay within the service boundary** (ADR-032). Honker streams are for internal state reconstruction, not cross-node distribution.
2. **Integration events cross boundaries via the call protocol.** When a domain event in the storage service needs to be communicated to another node, it's projected into an `EventEnvelope` frame and sent over the wire.
3. **Each node has its own `.db` file** with its own honker streams. This is a feature, not a limitation — it enforces the event boundary discipline.

The bridge pattern:

```
Node A (storage service):
  1. Business write INSERTs into SQLite
  2. stream.publish("nodes:created", {node_id: 42}) in same tx
  3. A local subscriber detects the event
  4. Projects it into EventEnvelope {operation: "/head/nodes/created", data: {node_id: 42}}
  5. Sends via call protocol over SSH/QUIC/DNS to Node B

Node B (receiver):
  1. Receives EventEnvelope via call protocol
  2. Enqueues locally: queue("incoming-events").enqueue({source: "node-A", event: ...})
  3. Or publishes locally: stream.publish("remote:nodes:created", {node_id: 42})
```

This preserves the three-layer model while respecting honker's single-machine design.

### 9.6 Honker Patterns and Integration Plan Mapping

The integration plan (Phase 2.2, alknet-storage) references these honker patterns. Here's the direct mapping:

| Plan Reference | Honker Primitive | Implementation Notes |
|---------------|-----------------|---------------------|
| `stream_publish/subscribe` | `db.stream("topic").publish(data, tx=tx)` + `async for event in stream.subscribe(consumer="name")` | Used for domain events within alknet-storage. Each metagraph change (node/edge created/updated/deleted) publishes to a stream. Consumers (local reactive logic, SSE endpoints) subscribe. |
| `notify/listen` | `tx.notify("channel", data)` + `async for n in db.listen("channel")` | Used for ephemeral real-time signals. SSH connection events, config reload triggers, forwarding rule activation. No persistence needed. |
| `queue/claim` | `queue.enqueue(data, tx=tx)` + `async for job in queue.claim(worker_id)` | Used for background tasks. Key rotation, certificate renewal, audit log archival, batch operations. The `tx=tx` parameter ensures atomicity with business writes. |

**Implementation approach for alknet-storage (Rust)**:

Use `honker-core` directly (not the Python binding). The Rust crate exposes:
- `open_conn(path, install_notify)` — open a connection with PRAGMA defaults
- `attach_honker_functions(&conn)` — register all SQL functions
- `bootstrap_honker_schema(&conn)` — create tables
- `SharedUpdateWatcher::new(db_path)` — the wake listener

Or load the extension via `rusqlite`:
```rust
use rusqlite::Connection;

let conn = Connection::open("alknet.db")?;
conn.load_extension("libhonker_ext", None)?;
conn.execute_batch("SELECT honker_bootstrap()")?;
```

### 9.7 Rust Integration: honker-core vs honker-rs

Two options for Rust integration in alknet-storage:

**Option A: Use `honker-core` directly**

The `honker-core` crate provides:
- `attach_notify(&conn)` — `_honker_notifications` table + `notify()` SQL function
- `attach_honker_functions(&conn)` — all `honker_*` SQL functions
- `bootstrap_honker_schema(&conn)` — all table/index creation
- `SharedUpdateWatcher` — the wake mechanism
- `open_conn(path, install_notify)` — connection factory with PRAGMA defaults

This gives you raw SQL access. You call `conn.query_row("SELECT honker_enqueue(…)")` etc. Maximum control, minimum abstraction.

**Option B: Use `honker-rs` ergonomic wrapper**

The `packages/honker-rs` crate provides:
- `Database::open(path)` — opens `system.db`
- `db.queue("name")` — `Queue` handle with `.enqueue()`, `.claim_batch()`, `.ack_batch()`
- `db.stream("name")` — `Stream` handle with `.publish()`, `.subscribe()`
- `db.listen("channel")` — async listener
- `db.outbox("name", delivery_fn)` — outbox pattern
- `db.lock("name", owner, ttl)` — named lock
- `db.scheduler()` — cron scheduler

**Recommendation**: Start with `honker-core` + direct SQL. The schema and functions are stable and well-tested. Wrap in application-level methods as needed. `honker-rs` may not expose all features (e.g., the scheduler pause/resume/list/update functions added in Phase Mantle). Using `honker-core` gives maximum flexibility while maintaining a single source of truth for SQL behavior.

---

## 10. Open Questions for Alknet

1. **Should alknet-storage bundle honker as a Rust crate dependency, or load the extension at runtime?**
   - Bundling `honker-core` gives compile-time verification. Loading the extension requires shipping `libhonker_ext.so/.dylib/.dll` alongside the binary.
   - Recommendation: Bundle `honker-core` as a crate dependency for the Rust implementation. Extension loading is for language bindings that can't link Rust code directly.

2. **Should the `alknet-storage` crate depend on `honker` (the Python package) or `honker-core` (the Rust rlib)?**
   - `honker-core` (Rust rlib) — correct choice for a Rust crate. `honker` is the Python binding.
   - The Crate dependency in storage.md currently lists `honker = "0.x"`. This should be `honker-core = "0.2"`.

3. **How does the Rust `SharedUpdateWatcher` integrate with tokio?**
   - `SharedUpdateWatcher::subscribe()` returns a `std::sync::mpsc::Receiver<()>`, which is blocking. For tokio integration, wrap in `tokio::task::spawn_blocking` or use `tokio::sync::mpsc` as a bridge.
   - Alternatively, use `UpdateWatcher::spawn()` directly and convert ticks to tokio notifications.

4. **Should alknet-storage abstract over honker-specific table names?**
   - Honker prefixes all internal tables with `_honker_` (e.g., `_honker_live`, `_honker_stream`). Alknet-storage should treat these as honker's internal schema and not directly query them for application logic.
   - Application-level tables (like `nodes`, `edges`, `accounts`) should use their own namespacing convention. Honker's tables coexist in the same `.db` file.

5. **Multi-tenant support**: Honker queues and streams are identified by name strings (e.g., `"emails"`, `"user-events"`). For alknet's multi-tenant model (system DB vs tenant DB), each tenant gets its own `.db` file with its own honker tables. Cross-tenant events must go through the call protocol — never by direct honker stream subscription across database files.

6. **Database file management**: Alknet-storage's system DB (`system.db`) and tenant DBs (`tenant-{orgId}.db`) should each have their own honker instance. The `SharedUpdateWatcher` is per-database, so 100 active tenants = 100 poll threads. This is fine for the expected alknet deployment size, but worth monitoring thread count in large deployments.

---

## 11. License and Maturity

- **License**: Apache 2.0 OR MIT (dual-licensed). Fully permissive for integration.
- **Maturity**: Alpha software (noted in README). Better than experimental but not beta-quality yet.
- **Status**: Active development. Regular commits. Cross-language interop tests. 180+ Python tests, 12+ Rust tests. Crash recovery verified. 600-second soak test under sustained writes.
- **Breaking changes risk**: The project is pre-1.0. Some table names still reference "joblite" and "litenotify" in the CHANGELOG (historical names). Current names use `_honker_` prefix. The API surface is stabilizing but may change.
- **Recommendation**: Pin to a specific `honker-core` version in `alknet-storage`'s `Cargo.toml`. The schema migration path (seen in `bootstrap_honker_schema`'s ALTER TABLE for `enabled` column) shows the project handles migrations.

---

## References

- [Honker GitHub Repository](https://github.com/russellromney/honker) — Primary source for all code and documentation
- [Honker README](https://github.com/russellromney/honker/blob/main/README.md) — Feature overview, quick start, architecture, performance
- [Honker BINDINGS.md](https://github.com/russellromney/honker/blob/main/BINDINGS.md) — Language binding support matrix
- [Honker ROADMAP.md](https://github.com/russellromney/honker/blob/main/ROADMAP.md) — Future work phases, planned features (singleton/dedup, state events, queue stats, per-queue config)
- [Honker CHANGELOG.md](https://github.com/russellromney/honker/blob/main/CHANGELOG.md) — Detailed history of all changes, performance passes, and architecture decisions
- [Honker honker-core/src/lib.rs](https://github.com/russellromney/honker/blob/main/honker-core/src/lib.rs) — Core Rust implementation: Writer, Readers, UpdateWatcher, SharedUpdateWatcher, schema, PRAGMAs
- [Honker honker-core/src/honker_ops.rs](https://github.com/russellromney/honker/blob/main/honker-core/src/honker_ops.rs) — All SQL function implementations: enqueue, claim, ack, retry, stream, lock, rate limit, scheduler
- [Honker honker-extension/src/lib.rs](https://github.com/russellromney/honker/blob/main/honker-extension/src/lib.rs) — Loadable extension entry point and C ABI for watcher
- [alknet ADR-032: Event Boundary Discipline](../../architecture/decisions/032-event-boundary-discipline.md) — Domain events stay within service boundary
- [alknet Integration Plan](../../research/integration-plan.md) — Phase 2.2: alknet-storage honker integration
- [alknet Storage Spec](../../architecture/storage.md) — alknet-storage crate design and honker integration table