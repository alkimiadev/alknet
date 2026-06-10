# iroh-gossip: Utility Data Structures & Wire Format

## IndexSet (`proto::util::IndexSet`)

A wrapper around `indexmap::IndexSet` that provides random selection capabilities needed by HyParView:

```rust
pub(crate) struct IndexSet<T> {
    inner: indexmap::IndexSet<T>,
}
```

### Key Operations

| Method | Purpose |
|--------|---------|
| `insert(value)` | Add element (returns false if already present) |
| `remove(value)` | Remove by value (swap-remove, O(1)) |
| `remove_index(index)` | Remove by index (swap-remove) |
| `remove_random(rng)` | Remove a random element |
| `pick_random(rng)` | Get reference to random element |
| `pick_random_without(exclude, rng)` | Random element excluding certain elements |
| `pick_random_index(rng)` | Random index |
| `shuffled(rng)` | All elements in random order |
| `shuffled_and_capped(len, rng)` | First `len` elements after shuffle |
| `shuffled_without(exclude, rng)` | Random order excluding certain elements |
| `shuffled_without_and_capped(exclude, len, rng)` | Capped shuffle excluding elements |
| `iter_without(value)` | Iterator skipping a specific element |

These operations are critical for HyParView's random walks, shuffle exchanges, and passive view management.

## TimerMap (`proto::util::TimerMap`)

A priority queue of timer entries sorted by `Instant`, with stable ordering via a sequence counter:

```rust
pub struct TimerMap<T> {
    heap: BinaryHeap<TimerMapEntry<T>>,
    seq: u64,
}
```

Used by the protocol state machine for scheduling future events (shuffles, graft timeouts, cache eviction). The networking layer wraps this in an async-friendly `Timers` type that can `wait_next()`.

### Key Operations

| Method | Purpose |
|--------|---------|
| `insert(instant, item)` | Schedule a timer |
| `pop_before(limit)` | Pop the earliest entry if it's before `limit` |
| `drain_until(from)` | Drain all entries up to a time |
| `first()` | Get reference to earliest entry |

## TimeBoundCache (`proto::util::TimeBoundCache`)

A `HashMap` where entries expire after a specified `Instant`:

```rust
pub struct TimeBoundCache<K, V> {
    map: HashMap<K, (Instant, V)>,
    expiry: TimerMap<K>,
}
```

Used by PlumTree for:
- `received_messages: TimeBoundCache<MessageId, ()>` — deduplication
- `cache: TimeBoundCache<MessageId, Gossip>` — message payload storage for Graft replies

### Key Operations

| Method | Purpose |
|--------|---------|
| `insert(key, value, expires)` | Insert with expiration |
| `contains_key(key)` | Check existence |
| `get(key)` | Get value |
| `expires(key)` | Get expiration time |
| `expire_until(instant)` | Remove all expired entries, returns count |
| `len()` / `is_empty()` | Size queries |

The `expire_until` method correctly handles re-insertions: if a key is re-inserted with a later expiration time after being added to the expiry queue, the old expiry entry is ignored (not removed from the map).

## Wire Format

### Frame Encoding

Messages are encoded using `postcard` (a `no_std`-friendly, `serde`-compatible format) and sent as length-prefixed frames:

```
┌──────────────┬──────────────┬─────────────────┐
│ Length (u32) │ TopicHeader  │ Message Payload  │
│   big-endian │  postcard    │   postcard       │
└──────────────┴──────────────┴─────────────────┘
```

### Stream Protocol

Each QUIC unidirectional stream is dedicated to a single topic. The stream begins with a `StreamHeader`:

```rust
pub(crate) struct StreamHeader {
    pub(crate) topic_id: TopicId,
}
```

All subsequent frames on that stream carry messages for that topic. When a `Disconnect` message is sent, the stream is closed (via `finish()`).

### Message Types on Wire

```rust
pub enum Message<PI> {
    Swarm(hyparview::Message<PI>),   // Membership messages
    Gossip(plumtree::Message),        // Broadcast messages
}
```

Where `PI` is `PublicKey` (32-byte ed25519 public key) in the networking layer.

The `MessageKind` classification is used for metrics:

| Kind | Message Types |
|------|--------------|
| `Data` | `Gossip` messages (actual content) |
| `Control` | All Swarm messages, plus `Prune`, `Graft`, `IHave` |

### Message Size Limits

- Default max message size: 4096 bytes (minimum: 512)
- The header size is computed at compile time via `postcard::experimental::serialized_size`
- Actual payload capacity = `max_message_size - header_size`

The `SendLoop` checks message size before writing and returns `WriteError::TooLarge` if exceeded.

## PeerData & Address Propagation

The `PeerData` type is an opaque `Bytes` wrapper used in HyParView messages. In the `net` layer, it carries addressing information:

```rust
struct AddrInfo {
    relay_url: Option<RelayUrl>,
    direct_addresses: BTreeSet<SocketAddr>,
}
```

This is serialized with `postcard` and passed as `PeerData` in `Join`, `ForwardJoin`, and `Neighbor` messages. When received, the `AddrInfo` is decoded and fed into `GossipAddressLookup`, which implements iroh's `AddressLookup` trait, allowing gossip-discovered addresses to be used for future connections.

## GossipAddressLookup

```rust
pub(crate) struct GossipAddressLookup {
    endpoints: NodeMap,                       // Arc<RwLock<BTreeMap<EndpointId, StoredEndpointInfo>>>
    _task_handle: Arc<AbortOnDropHandle<()>>, // Background eviction task
}
```

Key behaviors:
- **Merging**: When adding addresses for an already-known endpoint, new addresses are merged (union of direct addresses, relay URL is overwritten)
- **Expiration**: Entries expire after 5 minutes, with eviction checks every 30 seconds
- **Integration**: Implements `iroh::address_lookup::AddressLookup`, returning data with provenance "gossip"

## Dialer

```rust
struct Dialer {
    endpoint: Endpoint,
    pending: JoinSet<(EndpointId, Option<Result<Connection, ConnectError>>)>,
    pending_dials: HashMap<EndpointId, CancellationToken>,
}
```

The `Dialer` manages outgoing connection attempts:
- Queues a dial via `queue_dial(endpoint_id, alpn)`
- Checks for pending dials to avoid duplicate connections
- Supports cancellation of in-progress dials
- Returns completed connections via `next_conn()`

When a dial succeeds, the connection is passed to `handle_connection()`. When a dial fails and the peer is not already active, a `PeerDisconnected` event is injected into the protocol state.