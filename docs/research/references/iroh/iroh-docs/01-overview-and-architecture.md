# iroh-docs: Overview and Architecture

> Reference document for the `iroh-docs` crate (v0.98.0).  
> Source: `/workspace/iroh-docs`

## What Is iroh-docs?

`iroh-docs` is a Rust crate implementing **multi-dimensional key-value documents with an efficient synchronization protocol**. It provides:

1. **A CRDT-based document model** — Replicas (documents) hold entries identified by namespace + author + key, with content-addressed values (BLAKE3 hashes).
2. **Range-based set reconciliation** — An efficient sync protocol based on [Aljoscha Meyer's paper](https://arxiv.org/abs/2212.13567) for reconciling sets between peers.
3. **Live sync via gossip** — Real-time document updates propagated through an iroh-gossip swarm.
4. **Persistent storage** — A `redb`-backed store supporting both in-memory and file-based modes.

## High-Level Architecture

```
┌──────────────────────────────────────────────────────────────┐
│                        Docs (Protocol)                       │
│  ┌─────────────────────────────────────────────────────────┐ │
│  │                      Engine                              │ │
│  │  ┌──────────┐  ┌──────────────┐  ┌───────────────────┐ │ │
│  │  │ LiveActor│  │ GossipState  │  │  SyncHandle/Actor │ │ │
│  │  │ (events) │  │ (iroh-gossip)│  │  (store + sync)   │ │ │
│  │  └──────────┘  └──────────────┘  └───────────────────┘ │ │
│  └─────────────────────────────────────────────────────────┘ │
│                                                               │
│  ┌────────────────┐  ┌────────────────┐  ┌────────────────┐  │
│  │   Replica      │  │   SignedEntry  │  │    Author/      │  │
│  │   (sync.rs)    │  │   Entry/Record │  │  Namespace keys │  │
│  └────────────────┘  └────────────────┘  └────────────────┘  │
│                                                               │
│  ┌─────────────────────────────────────────────────────────┐ │
│  │                    Store (redb)                          │ │
│  │   Authors │ Namespaces │ Records │ RecordsByKey │ ...   │ │
│  └─────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────┘
```

### Module Layout

| Module | Purpose |
|--------|---------|
| `sync.rs` | Core types: `Replica`, `Entry`, `SignedEntry`, `Record`, `RecordIdentifier`, `Capability`, events |
| `keys.rs` | Cryptographic key types: `Author`, `NamespaceSecret`, `AuthorId`, `NamespaceId` |
| `ranger.rs` | Range-based set reconciliation algorithm implementation |
| `heads.rs` | `AuthorHeads` — latest timestamps per author for efficient sync decisions |
| `store/` | Storage abstraction and `redb`-backed persistent store |
| `store/fs.rs` | File-based `Store` implementation with redb tables |
| `store/pubkeys.rs` | `PublicKeyStore` trait for caching expanded ed25519 public keys |
| `actor.rs` | `SyncHandle` / Actor — single-threaded executor for store and replica operations |
| `engine/` | Live sync coordination: `Engine`, `LiveActor`, `GossipState`, `NamespaceStates` |
| `engine/live.rs` | The `LiveActor` event loop: handles sync, gossip, content download |
| `engine/gossip.rs` | Integration with `iroh-gossip` for broadcasting document operations |
| `engine/state.rs` | `NamespaceStates` — tracks per-namespace, per-peer sync state |
| `net/` | Network protocol: ALPN `/iroh-sync/1`, connection handling |
| `net/codec.rs` | Wire codec: length-prefixed postcard-serialized `Message` frames |
| `protocol.rs` | `Docs` struct (the `ProtocolHandler`) and `Builder` |
| `api/` | irpc-based RPC API for external access |
| `ticket.rs` | `DocTicket` — shareable document capability + peer addresses |

## Key Design Principles

1. **Two-key identity model**: Every entry is uniquely identified by (namespace, author, key). The namespace key provides write authorization; the author key provides attribution.

2. **Content-addressed values**: Entries store a BLAKE3 hash + length, not the actual content. Content blobs are handled separately by `iroh-blobs`.

3. **Prefix deletion**: An entry with key "foo" acts as a tombstone for all entries whose keys start with "foo/" (prefix deletion semantics). This enables hierarchical key structures.

4. **Last-writer-wins with per-author timestamps**: Entries are ordered by (timestamp, hash). Newer entries dominate older ones. Different authors can have entries for the same key simultaneously (multi-dimensional).

5. **Actor-based concurrency**: All store and replica mutations go through a single `SyncHandle` actor thread, eliminating the need for locks on the store.

6. **Event-driven live sync**: The `LiveActor` coordinates gossip, direct sync, and content downloads through a `tokio::select!` event loop.

## Dependencies

Key dependencies from `Cargo.toml`:

| Crate | Purpose |
|-------|---------|
| `iroh` | Networking: endpoints, connections, protocol routing |
| `iroh-blobs` | Content-addressed blob storage and transfer |
| `iroh-gossip` | Gossip protocol for broadcasting updates |
| `iroh-tickets` | Ticket-based sharing mechanism |
| `redb` | Embedded key-value store for persistence |
| `ed25519-dalek` | Ed25519 signatures for entries |
| `blake3` | Hashing (fingerprints + content hashes) |
| `postcard` | Serialization (wire format for sync protocol) |
| `irpc` / `noq` | RPC framework for API |

## Feature Flags

| Feature | Default | Description |
|---------|---------|-------------|
| `metrics` | Yes | Enables iroh-metrics instrumentation |
| `rpc` | Yes | Enables irpc-based RPC API (depends on `noq`) |
| `fs-store` | Yes | Enables persistent file-based store |