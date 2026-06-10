# iroh-docs: Engine and Live Sync

## Overview

The `Engine` is the top-level coordinator for live document synchronization. It brings together:

1. **SyncHandle/Actor** — Single-threaded actor for all store and replica operations
2. **LiveActor** — Async event loop coordinating sync, gossip, and content downloads
3. **GossipState** — Integration with `iroh-gossip` for broadcasting updates
4. **Blobs/Downloader** — Integration with `iroh-blobs` for content transfer

## Engine

```rust
pub struct Engine {
    pub endpoint: Endpoint,
    pub sync: SyncHandle,
    pub default_author: DefaultAuthor,
    to_live_actor: mpsc::Sender<ToLiveActor>,
    actor_handle: AbortOnDropHandle<()>,
    content_status_cb: ContentStatusCallback,
    blob_store: iroh_blobs::api::Store,
    _gc_protect_task: AbortOnDropHandle<()>,
}
```

### Initialization

```rust
Engine::spawn(
    endpoint,           // iroh Endpoint for QUIC connections
    gossip,             // iroh-gossip instance
    replica_store,      // Store for document data
    bao_store,          // iroh-blobs Store for content blobs
    downloader,         // Downloader for fetching blobs
    default_author_storage,  // Where to persist the default author
    protect_cb,         // Optional GC protection callback
) -> Result<Self>
```

During spawn:
1. A `ContentStatusCallback` is created that checks blob availability in `iroh-blobs`
2. A `SyncHandle` actor is spawned on a dedicated thread
3. A `LiveActor` is spawned as a tokio task
4. The default author is loaded or created
5. A GC protection task is started (if callback provided)

### Key Engine Methods

```rust
// Start syncing a document with given peers
async fn start_sync(&self, namespace: NamespaceId, peers: Vec<EndpointAddr>) -> Result<()>

// Stop syncing and leave gossip swarm
async fn leave(&self, namespace: NamespaceId, kill_subscribers: bool) -> Result<()>

// Subscribe to document events
async fn subscribe(&self, namespace: NamespaceId) -> Result<impl Stream<Item = Result<LiveEvent>>>

// Handle incoming QUIC connections
async fn handle_connection(&self, conn: Connection) -> Result<()>

// Shutdown the engine
async fn shutdown(&self) -> Result<()>
```

### GC Protection

The `ProtectCallbackHandler` bridges iroh-docs with iroh-blobs' garbage collection:

```rust
let (handler, protect_cb) = ProtectCallbackHandler::new();
// protect_cb goes into iroh-blobs GC config
// handler goes into Engine::spawn
```

When iroh-blobs runs GC, it calls `protect_cb` which queries the docs store for all content hashes, ensuring blobs referenced by document entries are not garbage-collected.

## SyncHandle / Actor

The `SyncHandle` is a handle to a single-threaded actor that processes all store and replica operations sequentially:

```rust
pub struct SyncHandle {
    tx: async_channel::Sender<Action>,
    join_handle: Arc<Option<std::thread::JoinHandle<()>>>,
    metrics: Arc<Metrics>,
}
```

### Actor Architecture

```
External Code ──async──▶ SyncHandle ──channel──▶ Actor Thread
                                                    │
                                              Store (redb)
                                              Replica operations
                                              Flush on timeout (500ms)
```

The actor runs on a **dedicated OS thread** (not a tokio task), using `tokio::runtime::Builder::new_current_thread()` internally. This ensures store operations are never concurrent.

### Action Types

```rust
enum Action {
    ImportAuthor { author, reply },
    ExportAuthor { author, reply },
    DeleteAuthor { author, reply },
    ImportNamespace { capability, reply },
    ListAuthors { reply },
    ListReplicas { reply },
    ContentHashes { reply },
    FlushStore { reply },
    Replica(NamespaceId, ReplicaAction),
    Shutdown { reply },
}

enum ReplicaAction {
    Open { reply, opts },
    Close { reply },
    GetState { reply },
    SetSync { sync, reply },
    Subscribe { sender, reply },
    Unsubscribe { sender, reply },
    InsertLocal { author, key, hash, len, reply },
    DeletePrefix { author, key, reply },
    InsertRemote { entry, from, content_status, reply },
    SyncInitialMessage { reply },
    SyncProcessMessage { message, from, state, reply },
    GetSyncPeers { reply },
    RegisterUsefulPeer { peer, reply },
    GetExact { author, key, include_empty, reply },
    GetMany { query, reply },
    DropReplica { reply },
    ExportSecretKey { reply },
    HasNewsForUs { heads, reply },
    SetDownloadPolicy { policy, reply },
    GetDownloadPolicy { reply },
}
```

### Replica Opening

When a replica is opened via the actor, an `OpenReplica` struct is created:

```rust
struct OpenReplica {
    info: ReplicaInfo,    // Capability, subscribers, content status callback
    sync: bool,           // Whether to accept sync requests
    handles: usize,       // Reference count for open handles
}
```

Multiple handles to the same replica are supported via reference counting.

## LiveActor

The `LiveActor` is the central async coordinator:

```rust
pub struct LiveActor {
    inbox: mpsc::Receiver<ToLiveActor>,
    sync: SyncHandle,
    endpoint: Endpoint,
    bao_store: Store,
    downloader: Downloader,
    memory_lookup: MemoryLookup,
    replica_events_tx: async_channel::Sender<Event>,
    replica_events_rx: async_channel::Receiver<Event>,
    sync_actor_tx: mpsc::Sender<ToLiveActor>,
    gossip: GossipState,
    running_sync_connect: JoinSet<SyncConnectRes>,
    running_sync_accept: JoinSet<SyncAcceptRes>,
    download_tasks: JoinSet<DownloadRes>,
    missing_hashes: HashSet<Hash>,
    queued_hashes: QueuedHashes,
    hash_providers: ProviderNodes,
    subscribers: SubscribersMap,
    state: NamespaceStates,
    metrics: Arc<Metrics>,
}
```

### Event Loop

The `LiveActor::run_inner()` loop uses `tokio::select!` with biased polling:

```rust
tokio::select! {
    biased;
    msg = self.inbox.recv() => { /* handle actor messages */ }
    event = self.replica_events_rx.recv() => { /* handle replica insert events */ }
    res = self.running_sync_connect.join_next() => { /* sync connect finished */ }
    res = self.running_sync_accept.join_next() => { /* sync accept finished */ }
    res = self.download_tasks.join_next() => { /* download completed */ }
    res = self.gossip.progress() => { /* gossip task progress */ }
}
```

### ToLiveActor Messages

```rust
pub enum ToLiveActor {
    StartSync { namespace, peers, reply },
    Leave { namespace, kill_subscribers, reply },
    Shutdown { reply },
    Subscribe { namespace, sender, reply },
    HandleConnection { conn },
    AcceptSyncRequest { namespace, peer, reply },
    IncomingSyncReport { from, report },
    NeighborContentReady { namespace, node, hash },
    NeighborUp { namespace, peer },
    NeighborDown { namespace, peer },
}
```

### Gossip Operations (Op)

```rust
pub enum Op {
    Put(SignedEntry),           // New entry inserted
    ContentReady(Hash),         // Content blob now available
    SyncReport(SyncReport),     // Heads summary after sync
}
```

Gossip broadcasts `Op` messages to all swarm participants. When a `Put` is received, the entry is inserted into the local replica. When a `ContentReady` is received, peers know they can download the blob. When a `SyncReport` is received, peers check `has_news_for_us()` to decide if they should sync.

### Content Download Flow

1. When a `RemoteInsert` event occurs with `should_download: true`, the entry's content hash is queued for download
2. The `LiveActor` uses `iroh_blobs::downloader::Downloader` to fetch the blob
3. Known providers (peers who had `ContentStatus::Complete`) are used as download sources
4. On download completion, a `LiveEvent::ContentReady` event is emitted

### LiveEvent (Public API)

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

`SyncEvent` wraps `SyncFinished`:

```rust
pub struct SyncFinished {
    pub namespace: NamespaceId,
    pub peer: PublicKey,
    pub outcome: SyncOutcome,
    pub timings: Timings,
}
```

## NamespaceStates

```rust
pub struct NamespaceStates(BTreeMap<NamespaceId, NamespaceState>);

struct NamespaceState {
    nodes: BTreeMap<EndpointId, PeerState>,
    may_emit_ready: bool,
}
```

Each peer has a `PeerState` tracking sync progress:

```rust
struct PeerState {
    state: SyncState,         // Idle or Running
    resync_requested: bool,   // Whether a resync was requested during active sync
    last_sync: Option<(Instant, Result<SyncFinished>)>,
}
```

This state machine prevents concurrent syncs with the same peer for the same namespace and queues resync requests when needed.

## DefaultAuthor

```rust
pub struct DefaultAuthor {
    value: RwLock<AuthorId>,
    storage: DefaultAuthorStorage,
}
```

- `DefaultAuthorStorage::Mem` — Ephemeral, creates a new author each time
- `DefaultAuthorStorage::Persistent(path)` — Stores the author ID as hex in a file, loads it on startup

The default author provides a convenient "current user" identity for applications.

## Docs Protocol Handler

```rust
pub struct Docs {
    engine: Arc<Engine>,
    api: DocsApi,
}
```

`Docs` implements `ProtocolHandler` for integration with iroh's `Router`:

```rust
impl ProtocolHandler for Docs {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> { ... }
    async fn shutdown(&self) { ... }
}
```

The `Builder` pattern configures storage:

```rust
let docs = Docs::memory()
    .spawn(endpoint, blobs, gossip)
    .await?;
// or
let docs = Docs::persistent(path)
    .protect_handler(handler)
    .spawn(endpoint, blobs, gossip)
    .await?;
```

## DocTicket

```rust
pub struct DocTicket {
    pub capability: Capability,
    pub nodes: Vec<EndpointAddr>,
}
```

A `DocTicket` encapsulates everything needed to join a document:
- A `Capability` (Read or Write) — provides the namespace key
- A list of `EndpointAddr` — bootstrap peers to connect to

Tickets are serialized as base32-encoded postcard data with a `"doc"` prefix, using the `iroh_tickets::Ticket` trait.