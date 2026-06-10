# iroh-docs: Range-Based Set Reconciliation (Ranger)

## Overview

The sync protocol in iroh-docs is based on **Range-Based Set Reconciliation**, implementing the algorithm described in [Aljoscha Meyer's paper (arXiv:2212.13567)](https://arxiv.org/abs/2212.13567).

The core idea: two peers can efficiently compute the union of their entry sets by recursively partitioning the sets and comparing **fingerprints** (hashes) of partitions. When fingerprints match, no further work is needed. When they differ, the partition is subdivided until the difference can be resolved by sending the actual entries.

## Key Abstractions

### RangeEntry Trait

```rust
pub trait RangeEntry: Debug + Clone {
    type Key: RangeKey;
    type Value: RangeValue;
    
    fn key(&self) -> &Self::Key;
    fn value(&self) -> &Self::Value;
    fn as_fingerprint(&self) -> Fingerprint;
}
```

`SignedEntry` implements `RangeEntry`:
- `Key` = `RecordIdentifier` (namespace || author || key bytes)
- `Value` = `Record` (timestamp, hash, len)
- Fingerprint = BLAKE3 hash of (namespace || author || key || timestamp || content_hash)

### RangeKey Trait

```rust
pub trait RangeKey: Sized + Debug + Ord + PartialEq + Clone + 'static {
    fn is_prefix_of(&self, other: &Self) -> bool;  // test-only
}
```

`RecordIdentifier` implements this via byte-level prefix matching: `(namespace, author, key)` where key prefix matching supports the hierarchical deletion semantics.

### RangeValue Trait

```rust
pub trait RangeValue: Sized + Debug + Ord + PartialEq + Clone + 'static {}
```

`Record` implements `RangeValue` with ordering by `(timestamp, hash)` — the Last-Writer-Wins ordering.

### Fingerprint

```rust
pub struct Fingerprint(pub [u8; 32]);  // BLAKE3 hash
```

Fingerprints are computed by XOR-ing the individual entry fingerprints within a range. This means:
- The fingerprint of the empty set is `BLAKE3([])` (the hash of nothing)
- Adding/removing an entry toggles its contribution via XOR
- Equal sets produce equal fingerprints

## Range Concept

A `Range<K>` represents a half-open interval `[x, y)` in the key space, with special semantics:

```rust
pub(crate) struct Range<K> {
    x: K,
    y: K,
}
```

- `x == y`: The entire set (all elements)
- `x < y`: Standard half-open interval `[x, y)` — includes `x`, excludes `y`
- `x > y`: Wrapping range — elements from `x` to end + beginning to `y`

This wrapping range concept allows the algorithm to work with circular key spaces where the "first" element might be anywhere.

## Protocol Messages

```rust
pub type ProtocolMessage = crate::ranger::Message<SignedEntry>;
```

### Message Structure

```rust
pub struct Message<E: RangeEntry> {
    parts: Vec<MessagePart<E>>,
}

pub enum MessagePart<E: RangeEntry> {
    RangeFingerprint(RangeFingerprint<E::Key>),  // "Here's a fingerprint for this range"
    RangeItem(RangeItem<E>),                      // "Here are the entries in this range"
}

pub struct RangeFingerprint<K> {
    range: Range<K>,
    fingerprint: Fingerprint,
}

pub struct RangeItem<E: RangeEntry> {
    range: Range<E::Key>,
    values: Vec<(E, ContentStatus)>,
    have_local: bool,  // If true, sender already has these entries
}
```

The `have_local` flag is an optimization: when a peer sends entries AND indicates it already has them locally, the receiver doesn't need to send its own entries in that range back.

### Wire Format

Messages are serialized using `postcard` (a compact serde format) and framed with a 4-byte big-endian length prefix via `SyncCodec`:

```
┌─────────────────┬──────────────────────────────┐
│  u32 BE length  │  postcard-encoded Message     │
└─────────────────┴──────────────────────────────┘
```

Max message size: 1 GiB (`MAX_MESSAGE_SIZE = 1024 * 1024 * 1024`).

## Sync Algorithm Walkthrough

### 1. Initiation (Alice → Bob)

Alice generates the initial message:

```rust
fn init<S: Store<E>>(store: &mut S) -> Result<Self, S::Error> {
    let x = store.get_first()?;            // First key, or default
    let range = Range::new(x.clone(), x);  // "All elements" range
    let fingerprint = store.get_fingerprint(&range)?;
    Ok(Message { parts: vec![RangeFingerprint { range, fingerprint }] })
}
```

This sends a single fingerprint covering the entire set.

### 2. Processing (Bob processes Alice's message)

For each part in the message:

**Case 1: RangeFingerprint matches local fingerprint** → Nothing to do, sets are equal in this range.

**Case 2: RangeFingerprint is empty OR range has ≤ 1 local entry** → Send all entries in the range as a `RangeItem`.

**Case 3: Recurse** → Split the range into `split_factor` partitions, compute fingerprints, and send either `RangeFingerprint` (if partition is large) or `RangeItem` (if partition is small enough, ≤ `max_set_size`).

### 3. Processing RangeItem

When a peer receives a `RangeItem`:

1. **Validate** each incoming entry using `validate_cb`
2. **Insert** valid entries via `Store::put()` (which handles prefix deletion)
3. **Notify** via `on_insert_cb` for actually-inserted entries
4. If `have_local` is false, compute the **diff** — entries in the local range not present in the received set — and send them back

### Configuration

```rust
struct SyncConfig {
    max_set_size: usize,    // Default: 1 — entries to send before using fingerprints
    split_factor: usize,    // Default: 2 — number of partitions per recursion step
}
```

With `max_set_size = 1` and `split_factor = 2`, the algorithm behaves like a binary search: each fingerprint mismatch splits the range in two and sends fingerprints for both halves.

## Store Trait

The `Store` trait provides the interface that the reconciliation algorithm needs:

```rust
pub trait Store<E: RangeEntry>: Sized {
    type Error: Debug + Send + Sync + Into<anyhow::Error> + 'static;
    type RangeIterator<'a>: Iterator<Item = Result<E, Self::Error>> where Self: 'a, E: 'a;
    type ParentIterator<'a>: Iterator<Item = Result<E, Self::Error>> where Self: 'a, E: 'a;

    fn get_first(&mut self) -> Result<E::Key, Self::Error>;
    fn get_fingerprint(&mut self, range: &Range<E::Key>) -> Result<Fingerprint, Self::Error>;
    fn entry_put(&mut self, entry: E) -> Result<(), Self::Error>;
    fn get_range(&mut self, range: Range<E::Key>) -> Result<Self::RangeIterator<'_>, Self::Error>;
    fn prefixes_of(&mut self, key: &E::Key) -> Result<Self::ParentIterator<'_>, Self::Error>;
    fn remove_prefix_filtered(&mut self, prefix: &E::Key, predicate: impl Fn(&E::Value) -> bool) -> Result<usize, Self::Error>;
    fn initial_message(&mut self) -> Result<Message<E>, Self::Error>;
    async fn process_message<F, F2, F3>(...) -> Result<Option<Message<E>>, Self::Error>;
    fn put(&mut self, entry: E) -> Result<InsertOutcome, Self::Error>;
}
```

### Insert Semantics in `Store::put()`

The `put` method implements the CRDT insert logic:

```rust
fn put(&mut self, entry: E) -> Result<InsertOutcome, Self::Error> {
    // 1. Check prefix entries — if any parent entry has value >= new entry, reject
    for prefix_entry in self.prefixes_of(entry.key())? {
        if entry.value() <= prefix_entry.value() {
            return Ok(InsertOutcome::NotInserted);
        }
    }
    
    // 2. Remove entries whose key is prefixed by new entry's key AND whose value is <=
    let removed = self.remove_prefix_filtered(entry.key(), |v| entry.value() >= v)?;
    
    // 3. Insert the new entry
    self.entry_put(entry)?;
    Ok(InsertOutcome::Inserted { removed })
}
```

### InsertOutcome

```rust
enum InsertOutcome {
    NotInserted,                              // A newer or equal entry already exists
    Inserted { removed: usize },             // Successfully inserted; reports removed entries
}
```

## Sync Flow at the Protocol Level

The `Replica` type provides the sync interface:

```rust
// Create initial message for sync
fn sync_initial_message(&mut self) -> anyhow::Result<ProtocolMessage>

// Process an incoming message and produce optional reply
async fn sync_process_message(
    &mut self,
    message: ProtocolMessage,
    from_peer: PeerIdBytes,
    state: &mut SyncOutcome,
) -> Result<Option<ProtocolMessage>, anyhow::Error>
```

### SyncOutcome

Tracks the result of a sync session:

```rust
pub struct SyncOutcome {
    pub heads_received: AuthorHeads,  // Latest timestamps per author from remote
    pub num_recv: usize,               // Number of entries received
    pub num_sent: usize,               // Number of entries sent
}
```

## Network Protocol (Codec)

The sync protocol operates over a QUIC bidirectional stream:

1. **Alice** (initiator) sends `Message::Init { namespace, message }`
2. **Bob** (responder) validates the namespace and either:
   - Accepts and processes the initial message
   - Rejects with `Message::Abort { reason }`
3. Both peers exchange `Message::Sync(message)` rounds until one side has no reply (convergence reached)

The `BobState` manages the responder side, tracking namespace and `SyncOutcome` progress across message rounds.

### Abort Reasons

```rust
pub enum AbortReason {
    NotFound,           // Namespace not available
    AlreadySyncing,     // Already syncing this namespace
    InternalServerError,
}
```

### Concurrent Sync Prevention

When both peers try to sync with each other simultaneously, the system uses a deterministic tiebreaker based on comparing `EndpointId` bytes — the peer with the larger ID accepts, the other connects.