# iroh-docs: Key Types Reference

## Cryptographic Keys

### NamespaceSecret

```rust
pub struct NamespaceSecret {
    signing_key: SigningKey,  // ed25519_dalek::SigningKey (32 bytes)
}
```

- The write capability for a document
- Can sign entries (namespace signature)
- Derives `NamespacePublicKey` and `NamespaceId`
- Serialized as 32 bytes

### NamespacePublicKey

```rust
pub struct NamespacePublicKey(VerifyingKey);  // ed25519_dalek::VerifyingKey
```

- The verifying key corresponding to `NamespaceSecret`
- Can verify namespace signatures on entries
- Serialized as 32 bytes

### NamespaceId

```rust
pub struct NamespaceId([u8; 32]);
```

- The byte representation of `NamespacePublicKey`
- Serves as the unique identifier for a document
- Can be converted back to `NamespacePublicKey` via `PublicKeyStore` (handles invalid curve points)

### Author

```rust
pub struct Author {
    signing_key: SigningKey,  // ed25519_dalek::SigningKey (32 bytes)
}
```

- A writer identity within a document
- Can sign entries (author signature)
- Derives `AuthorPublicKey` and `AuthorId`
- Created randomly with `Author::new(&mut rng)`
- Stored persistently in the redb authors table

### AuthorPublicKey

```rust
pub struct AuthorPublicKey(VerifyingKey);
```

- The verifying key corresponding to an `Author`
- Can verify author signatures on entries
- Serialized as 32 bytes

### AuthorId

```rust
pub struct AuthorId([u8; 32]);
```

- Byte representation of `AuthorPublicKey`
- Used as a component of `RecordIdentifier`
- Has `fmt_short()` for human-readable display (first 10 hex chars)

## Entry Types

### RecordIdentifier

```rust
pub struct RecordIdentifier(Bytes);
// Layout: [NamespaceId(32) | AuthorId(32) | Key(variable)]
```

- The composite key for an entry
- Byte layout: 32 bytes namespace + 32 bytes author + variable-length key
- Ordering: namespace → author → key (lexicographic)
- This ordering is critical for the range-based sync algorithm

### Record

```rust
pub struct Record {
    len: u64,           // Byte length of content
    hash: Hash,          // BLAKE3 hash of content (32 bytes)
    timestamp: u64,      // Microseconds since Unix epoch
}
```

- The value portion of an entry
- Ordering: timestamp first, then hash (Last-Writer-Wins)
- `Record::empty(timestamp)` creates a tombstone (hash=EMPTY, len=0)
- `Record::new_current(hash, len)` uses current system time

### Entry

```rust
pub struct Entry {
    id: RecordIdentifier,
    record: Record,
}
```

- Combines key and value
- `Entry::new(id, record)` constructor
- `Entry::new_empty(id)` creates a tombstone with current timestamp
- `entry.sign(namespace, author)` produces a `SignedEntry`

### SignedEntry

```rust
pub struct SignedEntry {
    signature: EntrySignature,  // Dual Ed25519 signatures
    entry: Entry,
}
```

- An entry with cryptographic proof of authorization and authorship
- `SignedEntry::from_entry(entry, namespace, author)` — create from entry
- `signed_entry.verify(store)` — verify both signatures using a `PublicKeyStore`
- Implements `RangeEntry` for the sync algorithm

### EntrySignature

```rust
pub struct EntrySignature {
    author_signature: Signature,    // 64-byte Ed25519 signature
    namespace_signature: Signature,  // 64-byte Ed25519 signature
}
```

- Created by signing the canonical byte encoding of the `Entry`
- Both signatures cover the same message bytes
- Verification requires both `NamespacePublicKey` and `AuthorPublicKey`

## Sync Types

### SyncOutcome

```rust
pub struct SyncOutcome {
    pub heads_received: AuthorHeads,
    pub num_recv: usize,
    pub num_sent: usize,
}
```

- Tracks the result of a sync session
- `heads_received` accumulates the latest timestamp seen from each author on the remote side

### ProtocolMessage

```rust
pub type ProtocolMessage = ranger::Message<SignedEntry>;
```

- The wire type for sync protocol messages
- Contains `Vec<MessagePart<SignedEntry>>`

### ContentStatus

```rust
pub enum ContentStatus {
    Complete,    // Content blob fully available
    Incomplete,  // Partially available
    Missing,     // Not available
}
```

- Communicated alongside entries during sync
- Helps peers decide whether to download content

### InsertOrigin

```rust
pub enum InsertOrigin {
    Local,
    Sync {
        from: PeerIdBytes,           // [u8; 32] — the remote peer
        remote_content_status: ContentStatus,
    },
}
```

## Event Types

### Event (Internal)

```rust
pub enum Event {
    LocalInsert {
        namespace: NamespaceId,
        entry: SignedEntry,
    },
    RemoteInsert {
        namespace: NamespaceId,
        entry: SignedEntry,
        from: PeerIdBytes,
        should_download: bool,
        remote_content_status: ContentStatus,
    },
}
```

- Emitted by `Replica` via `ReplicaInfo` subscribers
- `should_download` is determined by the `DownloadPolicy`

### LiveEvent (Public)

```rust
pub enum LiveEvent {
    InsertLocal { entry: Entry },
    InsertRemote { from: PublicKey, entry: Entry, content_status: ContentStatus },
    ContentReady { hash: Hash },
    PendingContentReady,
    NeighborUp(PublicKey),
    NeighborDown(PublicKey),
    SyncFinished(SyncEvent),
}
```

- Emitted by the `Engine` through `subscribe()`
- `InsertLocal` / `InsertRemote` are derived from `Event` by stripping `SignedEntry` → `Entry`
- `ContentReady` is emitted when a blob download completes
- `SyncFinished` wraps `SyncFinished` from the network layer

## Store Types

### Store (store::fs::Store)

```rust
pub struct Store {
    db: Database,                          // redb database
    transaction: CurrentTransaction,       // Current read/write transaction
    open_replicas: HashSet<NamespaceId>,   // Track which replicas are open
    pubkeys: MemPublicKeyStore,            // Cache for expanded public keys
}
```

### Query

```rust
pub struct Query {
    kind: QueryKind,                    // Flat or SingleLatestPerKey
    filter_author: AuthorFilter,        // Any or Exact
    filter_key: KeyFilter,              // Any, Exact, or Prefix
    limit: Option<u64>,
    offset: u64,
    include_empty: bool,
    sort_direction: SortDirection,
}
```

### Capability

```rust
pub enum Capability {
    Write(NamespaceSecret),
    Read(NamespaceId),
}
```

- `Write` allows inserting entries and signing them
- `Read` allows syncing and reading but not inserting
- Can be serialized as `(u8, [u8; 32])` — kind byte + key bytes
- `merge()` can upgrade `Read` to `Write`

### DownloadPolicy

```rust
pub enum DownloadPolicy {
    NothingExcept(Vec<FilterKind>),      // Whitelist mode
    EverythingExcept(Vec<FilterKind>),   // Blacklist mode (default)
}
```

### DocTicket

```rust
pub struct DocTicket {
    pub capability: Capability,
    pub nodes: Vec<EndpointAddr>,
}
```

- Serializable as a base32 string with "doc" prefix
- Contains everything needed to join a document
- The wire format uses a versioned enum: `TicketWireFormat::Variant0(DocTicket)`

## OpenState

```rust
pub struct OpenState {
    pub sync: bool,         // Whether sync is enabled
    pub subscribers: usize,  // Number of event subscribers
    pub handles: usize,      // Number of open handles
}
```

Returned by the `Status` RPC method to report the state of an open document.

## Utility Constants

| Constant | Value | Purpose |
|----------|-------|---------|
| `MAX_TIMESTAMP_FUTURE_SHIFT` | 10 min in μs | Max future drift for entry timestamps |
| `MAX_COMMIT_DELAY` | 500ms | Auto-commit interval for store transactions |
| `ACTION_CAP` | 1024 | Bounded channel capacity for SyncHandle actions |
| `ACTOR_CHANNEL_CAP` | 64 | Channel capacity for LiveActor messages |
| `SUBSCRIBE_CHANNEL_CAP` | 256 | Channel capacity for event subscriptions |
| `PEERS_PER_DOC_CACHE_SIZE` | 5 | LRU cache size for sync peers per document |
| `MAX_MESSAGE_SIZE` | 1 GiB | Max wire message size |