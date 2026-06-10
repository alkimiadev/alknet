# iroh-docs: Store and Persistence

## Store Architecture

The store is implemented in `store::fs::Store` using `redb`, an embedded key-value database. It supports two modes:

- **In-memory**: `Store::memory()` — backed by a `Vec<u8>` via `redb::backends::InMemoryBackend`
- **Persistent**: `Store::persistent(path)` — backed by a single file on disk

Both modes use the same `redb` table structure.

## redb Table Schema

### Authors Table
```
Table: "authors-1"
Key:   [u8; 32]        (AuthorId)
Value: [u8; 32]        (Author secret key bytes)
```

### Namespaces Table
```
Table: "namespaces-2"
Key:   [u8; 32]                  (NamespaceId)
Value: (u8, [u8; 32])            (CapabilityKind, key bytes)
```

The `CapabilityKind` discriminates between `Write = 1` (full key stored) and `Read = 2` (only the public key / namespace ID stored).

### Records Table (Primary)
```
Table: "records-1"
Key:   (NamespaceId, AuthorId, key_bytes)     = ([u8; 32], [u8; 32], &[u8])
Value: (timestamp, namespace_sig, author_sig, len, hash) = (u64, &[u8; 64], &[u8; 64], u64, &[u8; 32])
```

This is the main table storing all document entries. The key layout `(namespace, author, key)` enables efficient range queries for the sync algorithm.

### Latest-Per-Author Table
```
Table: "latest-by-author-1"
Key:   (NamespaceId, AuthorId)     = (&[u8; 32], &[u8; 32])
Value: (timestamp, key_bytes)      = (u64, &[u8])
```

Used to quickly determine the latest entry timestamp for each author, supporting `AuthorHeads` computation and `has_news_for_us()` checks.

### Records-By-Key Table (Index)
```
Table: "records-by-key-1"
Key:   (NamespaceId, key_bytes, AuthorId)     = (&[u8; 32], &[u8], &[u8; 32])
Value: ()
```

An index table that enables efficient queries by key prefix, supporting `Query::key_prefix()` and `Query::key_exact()` lookups.

### Namespace Peers Table (Multimap)
```
MultimapTable: "sync-peers-1"
Key:   &[u8; 32]                   (NamespaceId)
Value: (Nanos, &PeerIdBytes)       (timestamp_nanos, peer_id)
```

Stores up to 5 (`PEERS_PER_DOC_CACHE_SIZE`) recently-useful peers per namespace. This is an LRU cache: when full, the oldest peer is evicted when a new one is registered.

### Download Policy Table
```
Table: "download-policy-1"
Key:   &[u8; 32]               (NamespaceId)
Value: &[u8]                   (postcard-encoded DownloadPolicy)
```

Per-namespace download policies controlling which content blobs to automatically download.

## Store Operations

### Transaction Model

The `Store` uses a "current transaction" approach:

```rust
enum CurrentTransaction {
    None,
    Read(ReadOnlyTables),
    Write(TransactionAndTables),
}
```

- Read operations obtain a read snapshot
- Write operations batch into a write transaction
- Transactions older than `MAX_COMMIT_DELAY` (500ms) are automatically committed
- `flush()` commits any pending write transaction

### Core Methods

```rust
// Create/open/close replicas
fn new_replica(&mut self, namespace: NamespaceSecret) -> Result<Replica<'_>>;
fn open_replica(&mut self, namespace_id: &NamespaceId) -> Result<Replica<'_>>;
fn close_replica(&mut self, id: NamespaceId);
fn import_namespace(&mut self, capability: Capability) -> Result<ImportNamespaceOutcome>;

// Author management
fn new_author<R: CryptoRng>(&mut self, rng: &mut R) -> Result<Author>;
fn import_author(&mut self, author: Author) -> Result<()>;
fn get_author(&mut self, author_id: &AuthorId) -> Result<Option<Author>>;
fn delete_author(&mut self, author: AuthorId) -> Result<()>;

// Queries
fn get_many(&mut self, namespace: NamespaceId, query: impl Into<Query>) -> Result<QueryIterator>;
fn get_exact(&mut self, namespace: NamespaceId, author: AuthorId, key: impl AsRef<[u8]>, include_empty: bool) -> Result<Option<SignedEntry>>;
fn get_latest_for_each_author(&mut self, namespace: NamespaceId) -> Result<LatestIterator<'_>>;

// Sync support
fn has_news_for_us(&mut self, namespace: NamespaceId, heads: &AuthorHeads) -> Result<Option<NonZeroU64>>;
fn get_sync_peers(&mut self, namespace: &NamespaceId) -> Result<Option<PeersIter>>;
fn register_useful_peer(&mut self, namespace: NamespaceId, peer: PeerIdBytes) -> Result<()>;

// Content
fn content_hashes(&mut self) -> Result<ContentHashesIterator>;
```

### ImportNamespaceOutcome

```rust
pub enum ImportNamespaceOutcome {
    Inserted,   // New namespace created
    Upgraded,   // Existing namespace upgraded from Read to Write
    NoChange,   // Namespace already existed with same or higher capability
}
```

## Query System

The `Query` type supports flexible entry lookups:

```rust
pub struct Query {
    kind: QueryKind,
    filter_author: AuthorFilter,
    filter_key: KeyFilter,
    limit: Option<u64>,
    offset: u64,
    include_empty: bool,
    sort_direction: SortDirection,
}
```

### Query Kinds

```rust
enum QueryKind {
    Flat(FlatQuery),                    // Returns all matching entries
    SingleLatestPerKey(SingleLatestPerKeyQuery),  // Returns only latest entry per key
}
```

- **Flat**: Returns all entries matching the filters, sorted by `(namespace, author, key)` or `(namespace, key, author)` depending on `SortBy`
- **SingleLatestPerKey**: Groups by key and returns only the latest entry (by record value ordering) per key

### Filters

```rust
enum KeyFilter {
    Any,                  // Match all keys
    Exact(Bytes),         // Exact key match
    Prefix(Bytes),        // Key starts with prefix
}

enum AuthorFilter {
    Any,                  // Match all authors
    Exact(AuthorId),      // Match specific author
}
```

### Builder Pattern

```rust
// Get all entries
Query::all()

// Get entries by author
Query::author(author_id)

// Get entries by key prefix
Query::key_prefix(b"/path/")

// Get single latest entry per key
Query::single_latest_per_key()
    .key_prefix(b"/path/")
    .author(author_id)
```

## Download Policy

Controls which content blobs to automatically download after sync:

```rust
pub enum DownloadPolicy {
    NothingExcept(Vec<FilterKind>),    // Only download matching entries
    EverythingExcept(Vec<FilterKind>),  // Download all except matching (default)
}

pub enum FilterKind {
    Prefix(Bytes),   // Matches keys starting with bytes
    Exact(Bytes),     // Matches exact key
}
```

Default: `EverythingExcept(Vec::new())` — download everything.

## PublicKeyStore

The `PublicKeyStore` trait caches expanded `ed25519_dalek::VerifyingKey` objects to avoid repeated curve point decompression:

```rust
pub trait PublicKeyStore {
    fn public_key(&self, id: &[u8; 32]) -> Result<VerifyingKey, SignatureError>;
    fn namespace_key(&self, bytes: &NamespaceId) -> Result<NamespacePublicKey, SignatureError>;
    fn author_key(&self, bytes: &AuthorId) -> Result<AuthorPublicKey, SignatureError>;
}
```

The `MemPublicKeyStore` implementation uses `Arc<RwLock<HashMap<[u8; 32], VerifyingKey>>>` for thread-safe caching.

The `Store` itself implements `PublicKeyStore`, leveraging its redb tables for author storage and the in-memory cache for fast verification.

## StoreInstance

```rust
pub struct StoreInstance<'a> {
    namespace: NamespaceId,
    store: &'a mut Store,
}
```

A `StoreInstance` bundles a namespace ID with a mutable reference to the store, providing the `ranger::Store<SignedEntry>` implementation for the sync algorithm. This is what `Replica` uses internally to perform sync operations.

## Replica

```rust
pub struct Replica<'a, I = Box<ReplicaInfo>> {
    store: StoreInstance<'a>,
    info: I,
}
```

`Replica` is the primary user-facing type for document operations. It combines:
- A `StoreInstance` for data access
- `ReplicaInfo` for metadata (capability, subscribers, content status callback)

Key methods:
- `insert(key, author, hash, len)` — Insert a new entry
- `delete_prefix(prefix, author)` — Delete entries by key prefix
- `insert_remote_entry(entry, from, content_status)` — Insert from sync
- `hash_and_insert(key, author, data)` — Hash data and insert
- `sync_initial_message()` / `sync_process_message()` — Sync protocol operations