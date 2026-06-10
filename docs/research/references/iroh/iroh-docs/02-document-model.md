# iroh-docs: Document Model and CRDT Details

## Core Data Model

### Namespace (Document Identity)

A **Namespace** is the identity of a document. It consists of:

- **`NamespaceSecret`** — An Ed25519 signing key (32 bytes) that grants write capability
- **`NamespacePublicKey`** — The corresponding verifying key (32 bytes)
- **`NamespaceId`** — A `[u8; 32]` that is the byte representation of the public key; this serves as the unique identifier for a document/replica

```
NamespaceSecret (signing key) ──derives──▶ NamespacePublicKey (verifying key)
                                           ──into─────▶ NamespaceId ([u8; 32])
```

### Author (Writer Identity)

An **Author** represents a writer identity within a document. Multiple authors can write to the same namespace.

- **`Author`** — An Ed25519 signing key (32 bytes)
- **`AuthorPublicKey`** — The corresponding verifying key (32 bytes)
- **`AuthorId`** — A `[u8; 32]` byte representation of the public key

Authors are application-defined: an application might create one author per device, per user, or per session.

### Capability

Access to a document is controlled through a `Capability`:

```rust
pub enum Capability {
    Write(NamespaceSecret),  // Full read-write access
    Read(NamespaceId),        // Read-only access (can sync but not insert)
}
```

Capabilities can be **merged** — a `Read` capability can be upgraded to `Write` if a matching `Write` is presented:

```rust
capability.merge(other_capability)  // Read + Write → Write
```

The raw representation is `(u8, [u8; 32])` — a kind byte followed by 32 bytes of key material.

### Entry (The Fundamental Record)

An **`Entry`** is the core data unit, consisting of:

```rust
pub struct Entry {
    id: RecordIdentifier,  // (namespace, author, key)
    record: Record,         // (hash, len, timestamp)
}
```

#### RecordIdentifier

```rust
pub struct RecordIdentifier(Bytes);  // namespace[0..32] || author[32..64] || key[64..]
```

The key is a variable-length byte sequence. `RecordIdentifier` implements `Ord` by comparing namespace first, then author, then key — this ordering is critical for the range-based sync algorithm.

#### Record

```rust
pub struct Record {
    len: u64,         // byte length of the content
    hash: Hash,       // BLAKE3 hash of the content (32 bytes)
    timestamp: u64,   // microseconds since Unix epoch
}
```

The `Record` comparison uses `(timestamp, hash)` ordering — this is the **Last-Writer-Wins** rule for same-key entries. When two records for the same key exist, the one with the higher timestamp wins; if timestamps are equal, the higher hash wins as a tiebreaker.

### SignedEntry (Entry with Proofs)

```rust
pub struct SignedEntry {
    signature: EntrySignature,  // dual Ed25519 signatures
    entry: Entry,
}
```

#### EntrySignature

```rust
pub struct EntrySignature {
    author_signature: Signature,    // 64-byte Ed25519 signature
    namespace_signature: Signature,  // 64-byte Ed25519 signature
}
```

Both signatures cover the canonical byte encoding of the `Entry` (id + record). This means:
- The **namespace signature** proves write authorization (only holders of `NamespaceSecret` can produce valid entries)
- The **author signature** proves authorship (provides attribution and non-repudiation)

#### Verification

```rust
fn verify<S: PublicKeyStore>(&self, store: &S) -> Result<(), SignatureError>
```

Verification requires both the `NamespacePublicKey` and `AuthorPublicKey`, which are derived from the entry's namespace and author IDs. The `PublicKeyStore` trait provides caching for these expanded keys.

### Empty Entries (Tombstones / Prefix Deletion)

An entry is **empty** when `hash == Hash::EMPTY && len == 0`. Empty entries serve as **deletion markers**:

- **Key deletion**: Inserting an empty entry with the exact key removes the previous entry for that key
- **Prefix deletion**: Inserting an empty entry with key "foo" removes all entries whose keys start with "foo" (prefix deletion)

```rust
pub async fn delete_prefix(&mut self, prefix: impl AsRef<[u8]>, author: &Author) -> Result<usize, InsertError>
```

### Insert Semantics (CRDT Rules)

When a `SignedEntry` is inserted into a replica via `Store::put()` (the ranger store trait):

1. **Check prefixes**: Look up all existing entries whose key is a **prefix** of the new entry's key. If any prefix entry has a value `>=` the new entry's value, the new entry is **rejected** (`InsertOutcome::NotInserted`).

2. **Remove dominated entries**: Remove all existing entries whose key **starts with** the new entry's key (i.e., the new key is a prefix of theirs) AND whose value is `<=` the new entry's value.

3. **Insert**: If not rejected, the new entry is stored.

This implements a **prefix-aware last-writer-wins** CRDT:
- Newer entries for the same (namespace, author, key) tuple replace older ones
- A new entry at key "/foo" can delete all entries under "/foo/*" if it's newer
- Different authors can coexist on the same key — each author's latest entry is kept

### Timestamp and Future Shift

Timestamps are in **microseconds since Unix epoch**. There is a maximum allowed future shift:

```rust
pub const MAX_TIMESTAMP_FUTURE_SHIFT: u64 = 10 * 60 * Duration::from_secs(1).as_millis() as u64;
```

Entries with timestamps more than 10 minutes in the future of the local clock are rejected during validation.

### Content Status

Each entry's content has an availability status:

```rust
pub enum ContentStatus {
    Complete,    // Content blob is fully available locally
    Incomplete,  // Partially available
    Missing,     // Not available
}
```

This status is communicated during sync to help peers decide whether to download content.

### AuthorHeads (Efficient Sync Optimization)

`AuthorHeads` tracks the latest timestamp for each author in a document:

```rust
pub struct AuthorHeads {
    heads: BTreeMap<AuthorId, Timestamp>,
}
```

This enables a quick check: `has_news_for(other)` — comparing local and remote heads to determine whether sync would yield any new entries. If all timestamps are at least as recent locally, no sync is needed.

`AuthorHeads` can be serialized with a size limit, dropping the oldest entries when the limit is exceeded.

## Event System

Replicas emit events through a subscription system:

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
        should_download: bool,          // based on download policy
        remote_content_status: ContentStatus,
    },
}
```

Subscribers use `async_channel` for non-blocking notification delivery. The `ReplicaInfo::subscribe()` method registers a sender, and events are fanned out to all subscribers.

## Validation

Entry validation during insertion checks:

1. **Namespace match**: The entry's namespace must match the replica's namespace
2. **Signature verification**: For non-local entries, both namespace and author signatures are verified
3. **Timestamp check**: The entry must not be more than `MAX_TIMESTAMP_FUTURE_SHIFT` in the future
4. **Empty entry check**: An empty entry must have `hash == EMPTY && len == 0`, and a non-empty entry must have `len != 0`