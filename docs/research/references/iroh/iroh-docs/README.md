# iroh-docs Reference Documentation

> Version: 0.98.0  
> Repository: https://github.com/n0-computer/iroh-docs  
> License: MIT/Apache-2.0  
> Based on: [Range-Based Set Reconciliation (Meyer, 2022)](https://arxiv.org/abs/2212.13567)

## Document Index

| # | File | Topic |
|---|------|-------|
| 01 | [Overview and Architecture](01-overview-and-architecture.md) | High-level architecture, module layout, dependencies, feature flags |
| 02 | [Document Model](02-document-model.md) | CRDT data model: namespaces, authors, entries, signatures, prefix deletion, timestamps |
| 03 | [Sync Protocol](03-sync-protocol.md) | Range-based set reconciliation algorithm, fingerprints, message format, Store trait |
| 04 | [Store and Persistence](04-store-and-persistence.md) | redb table schema, transaction model, queries, download policies, PublicKeyStore |
| 05 | [Engine and Live Sync](05-engine-and-live-sync.md) | Engine, LiveActor, GossipState, content download, event system, DefaultAuthor |
| 06 | [Network Protocol](06-network-protocol.md) | ALPN, wire format, Alice/Bob protocol flow, error types, gossip integration |
| 07 | [API and Data Flow](07-api-and-data-flow.md) | RPC API, DocsApi, protocol messages, data flow diagrams |
| 08 | [Key Types Reference](08-key-types-reference.md) | All public types, constants, and their relationships |

## Quick Reference

### Core Concepts

- **Namespace**: A document identity. Identified by `NamespaceId` (32 bytes), backed by an Ed25519 keypair (`NamespaceSecret`).
- **Author**: A writer identity. Identified by `AuthorId` (32 bytes), backed by an Ed25519 keypair (`Author`).
- **Entry**: A record identified by (namespace, author, key) with a value of (hash, len, timestamp).
- **SignedEntry**: An entry with dual Ed25519 signatures (namespace + author) proving authorization and authorship.
- **Replica**: A local instance of a document, holding entries in a store.
- **Capability**: Either `Write(NamespaceSecret)` or `Read(NamespaceId)` — controls whether entries can be inserted.
- **Store**: A `redb`-backed persistent store managing authors, namespaces, entries, and peer caches.
- **Engine**: Coordinates sync actors, gossip, and content downloads for live synchronization.

### Key Algorithms

1. **Range-based set reconciliation**: Efficiently compute the union of two entry sets over a network by comparing fingerprints of partitions, subdividing when fingerprints differ.
2. **Prefix deletion**: An entry at key "foo" acts as a tombstone for all entries whose key starts with "foo/".
3. **Last-writer-wins**: When entries conflict on the same (namespace, author, key), the one with the higher (timestamp, hash) wins.
4. **XOR fingerprints**: Fingerprint of a set is the XOR of individual entry fingerprints (BLAKE3 hashes of key data).

### Data Flow

```
Application → DocsApi → Engine → LiveActor → GossipState → iroh-gossip
                                ↓                           ↓
                          SyncHandle → Actor → Store (redb) ← QUIC streams (iroh)
                                ↓
                          iroh-blobs (content transfer)
```

### Dependencies

- `iroh` — QUIC networking
- `iroh-blobs` — Content-addressed blob storage and transfer
- `iroh-gossip` — Gossip protocol for live updates
- `redb` — Embedded key-value store
- `ed25519-dalek` — Ed25519 signatures
- `blake3` — Hashing
- `postcard` — Serialization