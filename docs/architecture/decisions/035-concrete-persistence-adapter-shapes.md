# ADR-035: Concrete Persistence Adapter Shapes — Read/Write Split, honker+SQLite

## Status

Accepted. Resolves OQ-36. Refines ADR-031 §1 (`put`/`delete` → async;
`CredentialStoreError` → `StoreError`). Commits the concrete adapter
shape deferred by ADR-033 §"What this does NOT do."

## Context

ADR-033 committed the repo/adapter pattern — core defines repo traits
+ in-memory defaults; persistence adapters are separate crates; the
assembly layer wires the adapter. ADR-030 (`PeerEntry` /
`ConfigIdentityProvider`) and ADR-031 (`CredentialStore` /
`InMemoryCredentialStore`) committed the two trait shapes and their
in-memory defaults. **OQ-36 left the concrete persistence adapter
shapes open**: table schemas, backend choice, indexing, caching, the
honker+SQLite design. This ADR resolves OQ-36 by committing the
concrete shape.

Two constraints drive the design:

### Constraint 1: The hot-path read trait is sync

`IdentityProvider::resolve_from_fingerprint` is called on **every
incoming QUIC connection** in the accept loop (and on every
`call.requested` for the per-request identity path). It is synchronous
— `fn resolve_from_fingerprint(&self, fingerprint: &str) ->
Option<Identity>`, no `async`, no `.await`. This is a deliberate design
choice locked by ADR-004/011: the hot path can't block on a DB query,
and threading `.await` through the accept loop and every handler's
auth resolution would be a rewrite of the dispatch surface.

A SQLite-backed adapter cannot run a SQL query inside a sync call.
Therefore a persistence-backed `IdentityProvider` must hold an
**in-memory index** and serve sync reads from it. The question this
ADR answers: how does that index stay fresh when the DB changes,
without a restart?

### Constraint 2: Auth changes must take effect without a restart

The project explicitly fixed an early issue where changing auth
required restarting the server. `ConfigIdentityProvider` solves this
via `ArcSwap<DynamicConfig>` — a config reload atomically swaps the
config, and the next `resolve_from_fingerprint` call reads the new
state. Live resolution changes, no restart.

A persistence adapter needs the **same property**. A CLI admin tool
(`alknet peer add`), an admin call-protocol operation, or another
node writes a new `PeerEntry` to the SQLite DB. The running alknet
process's in-memory index must reflect that write **without a
restart** and **without polling** (polling re-introduces the staleness
window the `ArcSwap` pattern removed).

### The honker mechanism

[honker](https://github.com/nicholasgriffintn/honker) is a SQLite
extension + language bindings that adds Postgres-style `NOTIFY`/
`LISTEN` to SQLite, with single-digit-millisecond cross-process
wake-up and no polling. It works by watching `PRAGMA data_version`
(every 1ms, single-digit-µs read) and waking listeners on committed
updates.

This is **exactly the cache-invalidation mechanism** the
sync-hot-path + SQLite-backend combination needs:

1. A write hits SQLite (`INSERT`/`UPDATE`/`DELETE` on the `peers`
   table) and commits.
2. The write emits a honker `NOTIFY` alongside the business write.
   honker is designed so the notify is tied to the committed state —
   a rollback drops the notify (no spurious wake for work that didn't
   land). The honker docs describe the `PRAGMA data_version` watch as
   waking on committed updates and ignoring rolled-back work, which is
   the property this design relies on. **Corner case:** a process
   crash *between* the SQLite commit and the local `LISTEN` wake
   leaves the DB updated but the crashed process's index stale until
   its next restart (it reloads from SQLite on boot, so it converges
   — just not via the live-notify path). Other live processes still
   wake normally. This is acceptable for the single-process-failure
   assumption; a multi-process crash-durability guarantee would need
   WAL + checkpoint engineering beyond this ADR's scope.
3. The running alknet process's `LISTEN` wakes in single-digit ms.
4. The process reloads its in-memory index from SQLite (one `SELECT
   * FROM peers`) and atomically swaps it (same `ArcSwap` pattern as
   `ConfigIdentityProvider`).
5. The next `resolve_from_fingerprint` call reads the new index. Live
   resolution changes, no restart, no polling.

honker is therefore not "an optional backend choice" — it is the
mechanism that makes the sync-read + cached-index + no-restart
combination work. Without it, the persistence adapter either polls
(stale window) or requires a restart to pick up changes (the bug the
project already fixed once).

### The keypal reference

The keypal TypeScript library (`/workspace/keypal`) demonstrates the
repo pattern: a `Storage` interface with an in-memory default adapter
and backend adapters for Redis, Drizzle, Prisma, Kysely, Convex. The
core logic is backend-agnostic; storage is a trait; the consumer picks
the adapter at wiring time. The alknet adaptation follows the same
shape (core trait + in-memory default + separate adapter crates) but
diverges from keypal in three places, recorded here so a future reader
doesn't wonder why:

- **Two trait families, not one `Storage<T>`.** keypal stores one
  kind of thing (API key records), so one trait fits. alknet has two
  distinct aggregates — `PeerEntry` (identity + ACL, hot-path read on
  every connection) and `EncryptedData` blobs (credentials, read once
  at startup into `Capabilities`). Different shapes, different
  read/write profiles, different hot-path criticality. ADR-033 §4
  already committed to one trait per concern; this ADR keeps that.
- **Read/write trait split.** keypal's `Storage` is uniform (all
  methods async). alknet's hot path is sync, so the read trait is sync
  and the write trait is a separate async extension. keypal doesn't
  face this because JS/TS has no sync-hot-path constraint.
- **No adapter factory.** keypal's `adapter-factory` is a runtime
  generic over column mapping and type coercion — a TS/JS affordance
  (dynamic objects, runtime schema introspection). In Rust, each
  adapter is a concrete type implementing the trait; column mapping is
  done at adapter build time with concrete types. The *intent*
  ("adapters only implement the backend-specific query, cross-cutting
  concerns are shared") is achieved in Rust by a shared helper module
  (e.g., `alknet-store-sqlite` has a `schema` module both adapters
  use) and by the trait itself defining the contract. The factory
  pattern is intentionally not ported.

## Decision

### 1. Read trait stays sync; persistence adapters cache in memory

`IdentityProvider` and `CredentialStore::get` are **sync** and
**unchanged**. A persistence-backed adapter serves sync reads from an
in-memory index (`HashMap<fingerprint, PeerEntry>` for identity;
`HashMap<String, EncryptedData>` for credentials), loaded from the
backend at construction and refreshed on honker `NOTIFY`. This is the
same `ArcSwap`-backed "load full state, atomically swap" pattern
`ConfigIdentityProvider` uses for config reload — generalized from
"config file is the source of truth" to "SQLite is the source of
truth, honker signals when it changed."

The in-memory index is a **full reload**, not a delta apply, on each
`NOTIFY`. Peer/credential counts are small (10s–100s, per ADR-030
Assumption 4); a `SELECT *` + `HashMap` rebuild is cheap and avoids
the correctness hazards of incremental cache updates (missed deletes,
partial updates). This is the same posture as `ConfigIdentityProvider`
(reload the whole `DynamicConfig`, not a patch).

### 2. Add `IdentityStore` — the async write trait for peer management

`IdentityProvider` is read-only today and stays read-only. Peer
**mutations** (add/update/remove a `PeerEntry`) go through a new
async trait that extends `IdentityProvider`:

```rust
/// Read trait — hot path, sync, unchanged (ADR-004). ConfigIdentityProvider
/// and SqliteIdentityProvider both implement this. The SQLite adapter serves
/// from an in-memory index refreshed by honker LISTEN.
pub trait IdentityProvider: Send + Sync + 'static {
    fn resolve_from_fingerprint(&self, fingerprint: &str) -> Option<Identity>;
    fn resolve_from_token(&self, token: &AuthToken) -> Option<Identity>;
}

/// Write trait — management path, async. ConfigIdentityProvider does NOT
/// implement this (config reload is its write path). SqliteIdentityProvider
/// does: writes hit SQLite, emit honker NOTIFY, and the local LISTEN
/// refreshes the in-memory read index.
#[async_trait]
pub trait IdentityStore: IdentityProvider {
    async fn put_peer(&self, peer: &PeerEntry) -> Result<(), StoreError>;
    async fn update_peer(&self, peer_id: &str, peer: &PeerEntry) -> Result<(), StoreError>;
    async fn remove_peer(&self, peer_id: &str) -> Result<(), StoreError>;
}
```

- `put_peer` — insert or replace a `PeerEntry` (upsert by `peer_id`).
- `update_peer` — update an existing `PeerEntry` (error if `peer_id`
  not found; for upsert semantics use `put_peer`).
- `remove_peer` — delete a `PeerEntry` by `peer_id`.

Why a separate trait, not async methods on `IdentityProvider`:
- The hot-path read trait is consumed by the accept loop and every
  handler — those call sites are sync and must not gain `.await`. If
  `put_peer` were on `IdentityProvider`, every consumer would see the
  async method even though only the management path calls it. A
  separate `IdentityStore: IdentityProvider` supertrait keeps the read
  surface lean and makes the write surface opt-in.
- `ConfigIdentityProvider` does **not** implement `IdentityStore`.
  Its write path is config reload (`ConfigReloadHandle::reload`), not
  a method call. This preserves the config-is-source-of-truth model
  for the in-memory default while the SQLite adapter gains a method-
  call write path.

**Cache coherence on the writer's own process:** when a `SqliteIdentityProvider::put_peer` commits, the write's own honker `NOTIFY` wakes the local `LISTEN` and the local index refreshes — the writer's own read index is consistent with the write without special handling. There is no "write-through to local cache" shortcut; the NOTIFY path is the single source of truth for index freshness, on the writer's process and on every other process listening to the same DB. This keeps one mechanism instead of two (write-through for local + NOTIFY for remote), which is simpler and avoids the local/remote divergence bug.

### 3. `CredentialStore` write methods become async

ADR-031 sketched `CredentialStore::put`/`delete` as sync. This ADR
**refines that sketch**: `get` stays sync (cached read, same as
`IdentityProvider`), `put`/`delete` become **async** (they hit the
backend). The refinement is within the one-way door ADR-031 committed
("there IS a `CredentialStore` trait with `get`/`put`/`delete` keyed
by provider, persisting `EncryptedData`, never decrypting") — that
contract stands; the sync-vs-async of the write methods was an
unspecified detail in the sketch, and ADR-033 §"What this does NOT
do" explicitly deferred concrete adapter shapes to this work.

```rust
pub trait CredentialStore: Send + Sync {
    fn get(&self, provider: &str) -> Option<EncryptedData>;
    async fn put(&self, provider: &str, data: &EncryptedData) -> Result<(), StoreError>;
    async fn delete(&self, provider: &str) -> Result<(), StoreError>;
}
```

`InMemoryCredentialStore`'s `put`/`delete` are async with no `.await`
points (trivially satisfy an async trait) — no behavior change for the
in-memory default, just the signature. The SQLite adapter's
`put`/`delete` hit SQLite and emit honker `NOTIFY`, refreshing the
local and remote read caches.

`get` stays sync for the same reason `IdentityProvider` reads stay
sync: the credential load happens at startup into `Capabilities`
(ADR-031), and a cached sync read serves it. A runtime `get` (e.g., a
handler fetching a newly-stored credential without restart) hits the
in-memory index, which honker keeps fresh.

### 4. `alknet-store-sqlite` — the first concrete adapter crate

A single crate, `alknet-store-sqlite`, implementing **both**
`IdentityStore` and `CredentialStore` against SQLite + honker. Two
adapters in one crate is fine because they share:

- The SQLite connection pool.
- The honker `LISTEN` loop (one listener, multiple channels —
  `peers_changed` and `credentials_changed`).
- The migration infrastructure (`CREATE TABLE IF NOT EXISTS` on
  first open; the schema is small enough that a hand-rolled
  idempotent bootstrap is simpler than pulling in a migration
  framework — see §6).

Splitting into `alknet-peer-store-sqlite` +
`alknet-credential-store-sqlite` later is a two-way door (additive) if
a use case forces it (e.g., a deployment that wants peer persistence
but not credential persistence). The default is one crate, both
adapters, shared infra.

```rust
// alknet-store-sqlite — the concrete adapter

pub struct SqliteIdentityProvider {
    // SQLite connection (writes) + honker listener handle
    conn: Arc<SqliteConn>,
    // In-memory read index, atomically swapped on honker NOTIFY
    index: Arc<ArcSwap<PeerIndex>>,
}

impl IdentityProvider for SqliteIdentityProvider {
    fn resolve_from_fingerprint(&self, fp: &str) -> Option<Identity> {
        let idx = self.index.load();
        idx.resolve_from_fingerprint(fp)
    }
    fn resolve_from_token(&self, token: &AuthToken) -> Option<Identity> {
        let idx = self.index.load();
        idx.resolve_from_token(token)
    }
}

#[async_trait]
impl IdentityStore for SqliteIdentityProvider {
    async fn put_peer(&self, peer: &PeerEntry) -> Result<(), StoreError> {
        // 1. INSERT/UPDATE peers row in SQLite (transactional)
        // 2. NOTIFY 'peers_changed' (same transaction — atomic)
        // 3. The local+remote LISTEN loops wake, reload the index
        self.conn.put_peer(peer).await
    }
    // update_peer, remove_peer — same shape
}

pub struct SqliteCredentialStore {
    conn: Arc<SqliteConn>,           // shared with the identity adapter
    index: Arc<ArcSwap<HashMap<String, EncryptedData>>>,
}

impl CredentialStore for SqliteCredentialStore {
    fn get(&self, provider: &str) -> Option<EncryptedData> {
        self.index.load().get(provider).cloned()
    }
    async fn put(&self, provider: &str, data: &EncryptedData) -> Result<(), StoreError> {
        // INSERT/UPDATE credentials row + NOTIFY 'credentials_changed'
        self.conn.put_credential(provider, data).await
    }
    async fn delete(&self, provider: &str) -> Result<(), StoreError> {
        // DELETE credentials row + NOTIFY 'credentials_changed'
        self.conn.delete_credential(provider).await
    }
}
```

The `PeerIndex` is the in-memory structure that makes
`resolve_from_fingerprint` and `resolve_from_token` O(1) —
`HashMap<fingerprint, &PeerEntry>` + `HashMap<auth_token_hash,
&PeerEntry>`, built once per reload from `SELECT * FROM peers`. This
is the secondary-index pattern keypal's `memory.ts` uses
(`hashIndex`, `ownerIndex`, `tagIndex`); alknet needs the fingerprint
and auth-token-hash indexes. (Implementation note: the index owns
the `Vec<PeerEntry>` loaded from SQLite; the secondary maps borrow
with a lifetime tied to the index struct — self-referential, so the
index is built in one pass and held behind the `ArcSwap` as a single
`Arc<PeerIndex>`. Cloning entries to avoid self-reference is a
two-way-door implementation choice; the trait is agnostic.)

### 5. honker is the cache-invalidation mechanism — a hard dependency of the SQLite adapter

`alknet-store-sqlite` depends on `honker` (the Rust crate,
`honker-core`/`honker-extension`). This is **not** a core dependency
— `alknet-core` stays honker-free (ADR-033's "core has no backend
dependency" is preserved). The honker dependency lives in the adapter
crate, alongside the `rusqlite` (or `sqlx`) dependency.

The honker `LISTEN` loop is spawned by the adapter at construction
(`SqliteIdentityProvider::new` starts a tokio task that `LISTEN`s on
`peers_changed` and reloads the index on wake). The loop is
cancellation-safe (dropping the adapter cancels the task). The
listener uses honker's `PRAGMA data_version` watch — single-digit-ms
wake, no polling, no daemon.

**Why honker is load-bearing, not optional:** without it, the
sync-read + cached-index + no-restart combination breaks down into
either (a) polling (re-introduces the staleness window the project
already fixed), or (b) restart-on-change (the bug the project already
fixed). A SQLite adapter without honker would be a strictly worse
`ConfigIdentityProvider` (config reload does the same thing, simpler).
honker is what makes the SQLite adapter worth building: it adds
persistence *and* preserves the no-restart property *and* keeps the
hot path sync.

### 6. Schema (the commitment, not the DDL)

The ADR commits to the **table shape**, not the exact DDL. The DDL is
an implementation-detail two-way door (it lives in the adapter crate's
own code/tests, not an ADR); the shape is the one-way door because it
determines what the trait can express and what indexes the adapter
builds.

**`peers` table** — one row per `PeerEntry`:

| Column | Type | Notes |
|--------|------|-------|
| `peer_id` | TEXT PK | Stable logical id (`"worker-a"`) |
| `fingerprints` | TEXT (JSON array) | `["ed25519:...","SHA256:..."]` |
| `auth_token_hash` | TEXT NULL | SHA-256 of bearer token, or NULL |
| `scopes` | TEXT (JSON array) | `["relay:connect"]` |
| `resources` | TEXT (JSON object) | `{"service":["gitea","registry"]}` |
| `display_name` | TEXT NULL | |
| `enabled` | INTEGER (0/1) | Boolean |

The `PeerIndex` rebuilds from `SELECT * FROM peers` on each honker
wake. The fingerprint index is built by iterating rows and expanding
the `fingerprints` JSON array. The auth-token-hash index is built
from the non-NULL `auth_token_hash` rows.

**`credentials` table** — one row per `EncryptedData` blob:

| Column | Type | Notes |
|--------|------|-------|
| `provider` | TEXT PK | `"openai"`, `"anthropic"`, etc. |
| `key_version` | INTEGER | From `EncryptedData` (ADR-020) |
| `salt` | BLOB | Wire-format compat (OQ-20); unused in v2. Writers echo the vault's `EncryptedData.salt` field (even if unused in v2) so the row round-trips through the core `EncryptedData` mirror without loss; v2 may write a zero-length salt but must not drop the field. |
| `iv` | BLOB | AES-GCM IV (OsRng-generated, ADR-020) |
| `data` | BLOB | Ciphertext |

The `EncryptedData` core mirror (ADR-031 §3) round-trips through these
columns. The store never decrypts (ADR-025); the vault does.

**Migrations:** the adapter bootstraps with `CREATE TABLE IF NOT
EXISTS` on first open. The schema is small and stable (locked by
ADR-020/030); a migration framework (sqlx migrations, refinery) is
not pulled in for v1. If a future schema change requires a real
migration, that's additive (a `migrations/` dir + a migration runner
— two-way door). This is recorded so a future reader doesn't assume
migrations were forgotten.

### 7. The `StoreError` type (renames `CredentialStoreError`)

A shared error enum for both adapters, `#[non_exhaustive]` +
`thiserror::Error`. **This renames the `CredentialStoreError` sketched
in ADR-031 §1 to `StoreError`** — a single shared type for both the
identity and credential store traits, so both adapters and all
consumers reference one error type. The rename is within ADR-031's
one-way door (the contract was "a `#[non_exhaustive]` error enum for
store failures"; the *name* was unspecified detail). ADR-031's sketch
is amended to use `StoreError` by this rename.

```rust
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("backend error: {message}")]
    Backend { message: String },
    #[error("not found: {entity}")]
    NotFound { entity: String },
    #[error("serialization error: {message}")]
    Serialization { message: String },
}
```

`Backend` covers SQLite errors (constraint failures, disk I/O,
corruption). `NotFound` is for `update_peer`/`remove_peer` on a
missing `peer_id`. `#[non_exhaustive]` lets the adapter add variants
without breaking downstream match arms. A `Duplicate` variant is
**not** in v1: `put_peer` is upsert (insert-or-replace), so
`peer_id` collisions are a replace, not an error; a strict-insert
mode that would return `Duplicate` is a future addition (with its own
method or flag, added non-breakingly). The error type lives in
`alknet-core` (where the traits live) so both adapters and consumers
reference one type; the adapter crate may add a wrapper error for
backend-specific failures it surfaces as `Backend { message }`.

## What this does NOT change

- **`IdentityProvider` trait shape (ADR-004/030)** — unchanged. The
  read methods stay sync. `IdentityStore` is a new supertrait, not a
  modification.
- **`CredentialStore` contract (ADR-031)** — `get`/`put`/`delete`
  keyed by provider, persisting `EncryptedData`, never decrypting:
  unchanged. The *signature* of `put`/`delete` changes sync→async (a
  breaking change to `impl`s and call sites — acknowledged in
  Consequences), which is within the one-way door ADR-031 committed
  because ADR-031's sketch left sync-vs-async unspecified and ADR-033
  §"What this does NOT do" explicitly deferred concrete adapter shapes
  to this work. The error type is renamed `CredentialStoreError` →
  `StoreError` (§7), within the same one-way door (the contract was "a
  `#[non_exhaustive]` error enum"; the name was unspecified).
- **`PeerEntry` struct (ADR-030)** — unchanged.
- **`EncryptedData` core mirror (ADR-031 §3)** — unchanged.
- **`ConfigIdentityProvider`** — unchanged, still read-only, still
  config-backed. It does not implement `IdentityStore`.
- **`InMemoryCredentialStore`** — unchanged behavior; `put`/`delete`
  become async-with-no-awaits (signature change only).
- **alknet-core has no backend dependency** — ADR-033's commitment is
  preserved. `honker` and `rusqlite`/`sqlx` are dependencies of
  `alknet-store-sqlite`, not `alknet-core`.
- **The no-env-vars invariant (ADR-014)** — unaffected. The
  `CredentialStore` path is the persistence layer for encrypted
  blobs; the assembly layer still loads them into `Capabilities`; no
  `std::env::var` path exists.

## Consequences

**Positive:**
- OQ-36 is resolved. The concrete persistence adapter shape is
  committed: read-sync / write-async / honker-NOTIFY-for-cache-
  invalidation, against SQLite, in a separate `alknet-store-sqlite`
  crate.
- The no-restart-on-auth-change property is preserved across the
  config-backed and SQLite-backed deployments. `ConfigIdentityProvider`
  uses `ArcSwap` + config reload; `SqliteIdentityProvider` uses
  `ArcSwap` + honker NOTIFY. Same property, same mechanism shape,
  different source of truth.
- The hot path stays sync. No `.await` in the accept loop or handler
  auth resolution. The SQLite adapter caches in memory and serves
  reads from the cache, with honker keeping the cache fresh.
- The keypal-style repo pattern lands in Rust with the adaptations
  the alknet constraints require (two trait families, read/write
  split, no adapter factory). The pattern is now concrete, not
  aspirational.
- A third-party / downstream user can implement `IdentityProvider` or
  `CredentialStore` against any backend (Postgres, Redis, a remote
  service) by implementing the trait; the `IdentityStore` write
  extension is opt-in. The trait shapes are the public contract.

**Negative:**
- `alknet-core` gains the `IdentityStore` trait and the `StoreError`
  type. Small surface, but it's a new public trait — downstream
  consumers see it. The trade is that peer management (CLI tools,
  admin ops) gets a typed write surface instead of each adapter
  rolling its own.
- `InMemoryCredentialStore::put`/`delete` change signature (sync →
  async). Callers (the assembly layer, tests) add `.await`. This is
  within the ADR-031 sketch's unspecified detail; the in-memory
  adapter's behavior is unchanged.
- The SQLite adapter has a hard `honker` dependency. A deployment
  that wants SQLite persistence but not honker would need a separate
  adapter (`alknet-store-sqlite-polling` or similar) that polls or
  requires restart — strictly worse, and not built. This is the
  trade for the no-restart property; it's explicit.
- The full-reload-on-NOTIFY strategy is O(rows) per wake. At
  expected scale (10s–100s of peers) this is cheap; at thousands it
  would matter. The peer/credential counts are small by design
  (ADR-030 Assumption 4); if a future use case pushes to thousands,
  a delta-apply strategy is a two-way-door optimization (additive,
  behind the same trait).

## Assumptions

1. **The hot path must stay sync.** `IdentityProvider::resolve_from_
   fingerprint` and `CredentialStore::get` are called in contexts
   that cannot `.await` (accept loop, per-request dispatch). This is
   locked by ADR-004/011 and is the one-way door that drives the
   read/write split. Reverting to async reads would require rewriting
   the dispatch surface — not planned.

2. **Peer/credential counts are small (10s–100s).** The full-reload-
   on-NOTIFY strategy is cheap at this scale (ADR-030 Assumption 4).
   A delta-apply strategy is a future two-way-door optimization if
   scale forces it.

3. **honker is the cache-invalidation mechanism for the SQLite
   adapter.** It is a hard dependency of `alknet-store-sqlite`, not
   of `alknet-core`. A non-honker SQLite adapter is possible but
   would poll or require restart — strictly worse and not built.

4. **`ConfigIdentityProvider` does not implement `IdentityStore`.**
   Config reload is its write path; the trait stays read-only. This
   preserves the config-is-source-of-truth model. A deployment that
   wants method-call peer management uses the SQLite adapter, not the
   config adapter.

5. **The concrete SQL DDL is an implementation-detail two-way door.**
   The table *shape* (one row per `PeerEntry`, one row per
   `EncryptedData`, JSON columns for arrays/objects) is the one-way
   door this ADR commits; the exact DDL, migration tooling, and
   column naming live in the adapter crate.

6. **Redis / Postgres / on-chain adapters are not needed for the
   current scope.** The trait shapes make them possible; the adapter
   crates get built when a concrete use case forces them. This is a
   scoping judgment, not a deferral — the SQLite adapter is the
   committed build; the others are not designed here.

7. **One crate (`alknet-store-sqlite`) for both adapters.** Splitting
   into `alknet-peer-store-sqlite` + `alknet-credential-store-sqlite`
   is a two-way door (additive) if a use case forces it; the default
   is one crate, shared infra.

## References

- OQ-36 (resolved by this ADR) — concrete persistence adapter shapes
- [ADR-003](003-crate-decomposition.md) — crate decomposition (core is
  lean, adapters are separate crates, the assembly layer wires — the
  rules this ADR's adapter-crate structure follows)
- [ADR-009](009-one-way-door-decision-framework.md) — one-way door
  decision framework (the door-type vocabulary used throughout)
- [ADR-004](004-auth-as-shared-core.md) — `IdentityProvider` (the
  read trait this ADR keeps sync)
- [ADR-011](011-authcontext-structure.md) — AuthContext resolution
  flow (where the sync read is called from)
- [ADR-014](014-secret-material-flow-and-capability-injection.md) —
  no-env-vars invariant (the `CredentialStore` path supports it)
- [ADR-020](020-hd-derivation-for-encryption-keys.md) —
  `EncryptedData` shape (the `credentials` table row shape)
- [ADR-025](025-vault-local-only-dispatch.md) — vault is the sole
  decryption boundary; `CredentialStore` never decrypts
- [ADR-030](030-peerentry-and-identity-id-decoupling.md) — `PeerEntry`
  model (the `peers` table row shape); Assumption 4 (small peer
  counts → full-reload is cheap)
- [ADR-031](031-credentialstore-repo-trait.md) — `CredentialStore`
  trait (this ADR refines `put`/`delete` to async, within the
  one-way door ADR-031 committed)
- [ADR-033](033-storage-boundary-and-repo-adapter-pattern.md) — the
  repo/adapter pattern (this ADR commits the concrete adapter shape
  ADR-033 §"What this does NOT do" deferred)
- `docs/research/alknet-storage-strategy/findings.md` — the
  SQLite+honker foundation and the repo/adapter pattern research
- `/workspace/keypal` — TypeScript repo-pattern reference (the
  `Storage` interface + adapters pattern; the in-memory secondary-
  index pattern in `memory.ts`; the adapter-factory intent this ADR
  does not port to Rust)
- `/workspace/honker` — honker: SQLite NOTIFY/LISTEN, the
  cache-invalidation mechanism the SQLite adapter depends on