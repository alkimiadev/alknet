# iroh-docs: API and RPC

## DocsApi

The `DocsApi` provides an RPC-based interface to the docs engine, implemented via `irpc`:

```rust
#[derive(Debug, Clone)]
pub struct DocsApi {
    inner: Client<DocsProtocol>,
}
```

### Methods (via irpc)

The API exposes document operations through an RPC protocol defined in `api/protocol.rs`:

| Method | Request | Response | Description |
|--------|---------|----------|-------------|
| `Open` | `OpenRequest { doc_id }` | `OpenResponse` | Open a document for operations |
| `Close` | `CloseRequest { doc_id }` | `CloseResponse` | Close a document |
| `Status` | `StatusRequest { doc_id }` | `StatusResponse { status: OpenState }` | Get document open state |
| `List` | `ListRequest` | Stream of `ListResponse { id, capability }` | List all documents |
| `Create` | `CreateRequest` | `CreateResponse { id }` | Create a new document |
| `Drop` | `DropRequest { doc_id }` | `DropResponse` | Remove a document |
| `Import` | `ImportRequest { capability }` | `ImportResponse { doc_id }` | Import a document by capability |
| `Set` | `SetRequest { doc_id, author_id, key, value }` | `SetResponse { entry }` | Set a key-value pair |
| `SetHash` | `SetHashRequest { doc_id, author_id, key, hash, size }` | `SetHashResponse` | Set a key with pre-hashed content |
| `GetMany` | `GetManyRequest { doc_id, query }` | Stream of entries | Query entries |
| `GetExact` | `GetExactRequest { doc_id, key, author, include_empty }` | `GetExactResponse { entry }` | Get single entry |
| `Del` | `DelRequest { doc_id, author_id, key }` | `DelResponse { removed }` | Delete by key prefix |
| `Subscribe` | `SubscribeRequest { doc_id }` | Stream of `LiveEvent` | Subscribe to document events |
| `Share` | `ShareRequest { doc_id, mode, peers }` | `ShareResponse { ticket }` | Create a sharing ticket |
| `StartSync` | `StartSyncRequest { doc_id, peers }` | `StartSyncResponse` | Start live sync |
| `Leave` | `LeaveRequest { doc_id }` | `LeaveResponse` | Leave gossip swarm |
| `ImportFile` | `ImportFileRequest { ... }` | Stream of `ImportProgress` | Import file content and set key |
| `ExportFile` | `ExportFileRequest { ... }` | Stream of `ExportProgress` | Export content to file |
| `AuthorList` | `AuthorListRequest` | Stream of `AuthorListResponse` | List authors |
| `AuthorCreate` | `AuthorCreateRequest` | `AuthorCreateResponse { author_id }` | Create new author |
| `AuthorImport` | `AuthorImportRequest { author }` | `AuthorImportResponse { author_id }` | Import author key |
| `AuthorExport` | `AuthorExportRequest { author_id }` | `AuthorExportResponse { author }` | Export author key |
| `AuthorDelete` | `AuthorDeleteRequest { author_id }` | `AuthorDeleteResponse` | Delete author |
| `AuthorGetDefault` | `AuthorGetDefaultRequest` | `AuthorGetDefaultResponse { author_id }` | Get default author |
| `AuthorSetDefault` | `AuthorSetDefaultRequest { author_id }` | `AuthorSetDefaultResponse` | Set default author |
| `SetDownloadPolicy` | `SetDownloadPolicyRequest { doc_id, policy }` | `SetDownloadPolicyResponse` | Set download policy |
| `GetDownloadPolicy` | `GetDownloadPolicyRequest { doc_id }` | `GetDownloadPolicyResponse { policy }` | Get download policy |
| `GetSyncPeers` | `GetSyncPeersRequest { doc_id }` | `GetSyncPeersResponse { peers }` | Get known sync peers |

## RPC Implementation

The RPC is implemented via `irpc` (for local/remote procedure calls) and `noq` (for remote network access):

### Local API

`DocsApi::spawn(engine)` creates an `RpcActor` that processes requests against the engine directly:

```rust
impl DocsApi {
    pub fn spawn(engine: Arc<Engine>) -> Self {
        RpcActor::spawn(engine)
    }
}
```

### Remote API

When the `rpc` feature is enabled, `DocsApi::connect(endpoint, addr)` creates a remote client that sends requests over the network via `noq`.

### Protocol Dispatch

```rust
irpc::rpc::Handler<DocsProtocol> dispatches:
DocsProtocol::Open(msg) => local.send((msg, tx)).await
DocsProtocol::Set(msg) => local.send((msg, tx)).await
// ... etc
```

## RpcActor

The `RpcActor` (in `api/actor.rs`) bridges the RPC protocol to the `Engine`:

```rust
struct RpcActor {
    engine: Arc<Engine>,
}
```

It handles each request type by calling the corresponding `Engine`/`SyncHandle` method and returning the result through the RPC channel.

For streaming responses (like `GetMany`, `Subscribe`, `AuthorList`), the actor sends results through an `mpsc` channel that the RPC framework streams back to the client.

## Share Mode and Tickets

When sharing a document:

```rust
pub enum ShareMode {
    Read,    // Share with read-only capability
    Write,   // Share with full write capability
}
```

The `Share` RPC method:
1. Gets or creates the namespace capability
2. Creates a `DocTicket` with the capability and provided peer addresses
3. Starts sync with the provided peers
4. Returns the ticket for distribution

## Example: Basic Setup

```rust
use iroh::{endpoint::presets, protocol::Router, Endpoint};
use iroh_blobs::{BlobsProtocol, store::mem::MemStore, ALPN as BLOBS_ALPN};
use iroh_docs::{protocol::Docs, ALPN as DOCS_ALPN};
use iroh_gossip::{net::Gossip, ALPN as GOSSIP_ALPN};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let endpoint = Endpoint::bind(presets::N0).await?;
    let blobs = MemStore::default();
    let gossip = Gossip::builder().spawn(endpoint.clone());
    let docs = Docs::memory()
        .spawn(endpoint.clone(), (*blobs).clone(), gossip.clone())
        .await?;

    let router = Router::builder(endpoint.clone())
        .accept(BLOBS_ALPN, BlobsProtocol::new(&blobs, None))
        .accept(GOSSIP_ALPN, gossip)
        .accept(DOCS_ALPN, docs)
        .spawn();

    Ok(())
}
```

## Data Flow Summary

```
┌─────────────────────────────────────────────────────────────────┐
│                     Application / RPC                           │
│   DocsApi ──irpc──▶ RpcActor ──▶ Engine / SyncHandle           │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│                     Live Sync (per document)                     │
│                                                                  │
│   LiveActor event loop:                                          │
│   ┌────────────────┐  ┌─────────────────┐  ┌──────────────────┐ │
│   │ Actor Messages │  │ Replica Events  │  │ Gossip Events    │ │
│   │ (StartSync,   │  │ (LocalInsert,  │  │ (Put,            │ │
│   │  Subscribe,   │  │  RemoteInsert) │  │  ContentReady,   │ │
│   │  Leave, ...)  │  │                │  │  SyncReport)     │ │
│   └──────┬─────────┘  └───────┬────────┘  └──────┬──────────┘ │
│          │                     │                   │            │
│          ▼                     ▼                   ▼            │
│   ┌──────────────────────────────────────────────────────────┐  │
│   │              LiveActor::run_inner()                      │  │
│   │   tokio::select! { ... }                                │  │
│   │                                                          │  │
│   │   - Start/stop gossip subscriptions                     │  │
│   │   - Initiate outgoing syncs (connect_and_sync)           │  │
│   │   - Accept incoming syncs (handle_connection)            │  │
│   │   - Queue content downloads                              │  │
│   │   - Broadcast local inserts via gossip                   │  │
│   │   - Emit LiveEvent to subscribers                        │  │
│   └──────────────────────────────────────────────────────────┘  │
│                                                                  │
│   Running Tasks:                                                 │
│   ┌───────────────────┐  ┌───────────────────┐                 │
│   │ sync_connect tasks│  │ sync_accept tasks  │                 │
│   └───────────────────┘  └───────────────────┘                 │
│   ┌───────────────────┐  ┌───────────────────┐                 │
│   │ download tasks    │  │ gossip receive loop│                 │
│   └───────────────────┘  └───────────────────┘                 │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│                    Sync Actor (dedicated thread)                 │
│                                                                  │
│   ┌────────────┐   ┌─────────────────────────────────────────┐ │
│   │ Action     │   │ Replica Operations:                     │ │
│   │ Channel    │──▶│ Insert, Delete, Get, Query,             │ │
│   │ (bounded)  │   │ SyncInit, SyncProcess, Open, Close, ...│ │
│   └────────────┘   └─────────────────────────────────────────┘ │
│                                                                  │
│   Store (redb) ──▶ All reads/writes on this thread             │
└─────────────────────────────────────────────────────────────────┘
```