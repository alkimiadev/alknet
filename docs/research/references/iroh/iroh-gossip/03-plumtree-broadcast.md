# iroh-gossip: PlumTree Broadcast Protocol

## Overview

The PlumTree (Epidemic Broadcast Trees) protocol provides **efficient message broadcasting** across all peers in a topic's swarm. It builds on top of HyParView's membership layer, using the active view as its peer set.

It is implemented in `src/proto/plumtree.rs`.

## Core Concept: Eager vs Lazy Push

Each peer maintains two subsets of its HyParView active view:

| Set | Description | Behavior |
|-----|-------------|----------|
| **Eager push peers** | Peers to whom full messages are sent immediately | Messages are pushed eagerly (full content) |
| **Lazy push peers** | Peers to whom only message IDs (hashes) are sent | `IHave` announcements are sent, requesting content only if needed |

When a peer broadcasts a message:
1. The **full message** is pushed to all **eager** peers.
2. The **message ID** (a blake3 hash) is pushed to all **lazy** peers (after a short delay for batching).

This creates an **optimized broadcast tree**: eager peers form a spanning tree for low-latency delivery, while lazy peers provide redundancy through timeout-based recovery.

## Configuration (`plumtree::Config`)

```rust
pub struct Config {
    pub graft_timeout_1: Duration,           // Default: 80ms
    pub graft_timeout_2: Duration,           // Default: 40ms
    pub dispatch_timeout: Duration,          // Default: 5ms
    pub optimization_threshold: Round,        // Default: Round(7)
    pub message_cache_retention: Duration,   // Default: 30s
    pub message_id_retention: Duration,       // Default: 90s
    pub cache_evict_interval: Duration,       // Default: 1s
}
```

### Timeout Semantics

- **`graft_timeout_1`**: After receiving an `IHave`, wait this long for the full message from an eager peer. If it doesn't arrive, send a `Graft` to the `IHave` sender.
- **`graft_timeout_2`**: After sending a `Graft`, wait this shorter timeout for the reply. If no reply, try the next `IHave` sender.
- **`dispatch_timeout`**: Delay before batching and sending `IHave` messages. This allows multiple announcements to be aggregated into a single message.
- **`optimization_threshold`**: Number of hops difference required to trigger tree optimization (see below).

### Cache Settings

- **`message_cache_retention`**: How long to keep full message payloads in cache. This enables replying to `Graft` requests from peers who missed the eager push.
- **`message_id_retention`**: How long to remember that we've already seen a message ID. This prevents re-delivering duplicate messages.
- **`cache_evict_interval`**: How often to check and evict expired entries.

## State Structure

```rust
pub struct State<PI> {
    me: PI,                                        // Our peer identity
    config: Config,                                 // Protocol configuration

    pub eager_push_peers: BTreeSet<PI>,            // Full message delivery peers
    pub lazy_push_peers: BTreeSet<PI>,             // Message-ID-only delivery peers

    lazy_push_queue: BTreeMap<PI, Vec<IHave>>,     // Pending IHave announcements (batched)

    missing_messages: HashMap<MessageId, VecDeque<(PI, Round)>>,  // IHave senders awaiting delivery
    received_messages: TimeBoundCache<MessageId, ()>,              // Seen message IDs
    cache: TimeBoundCache<MessageId, Gossip>,                      // Full message payloads

    graft_timer_scheduled: HashSet<MessageId>,     // Active graft timers
    dispatch_timer_scheduled: bool,                // Whether IHave dispatch is pending

    init: bool,                                    // Whether first event was processed
    stats: Stats,                                  // Message counters
    max_message_size: usize,                        // Maximum allowed message size
}
```

## Message Types (`plumtree::Message`)

| Message | Direction | Purpose |
|---------|-----------|---------|
| `Gossip(Gossip)` | Eager push | Full message content, broadcast to eager peers |
| `Prune` | Bidirectional | Sent when moving a peer from eager to lazy set |
| `Graft(Graft)` | Lazy → Eager upgrade | Request to become an eager peer; may include a message ID to request re-delivery |
| `IHave(Vec<IHave>)` | Lazy push | Announcement: "I have these messages" (batched, sent after `dispatch_timeout`) |

### Gossip Message Structure

```rust
pub struct Gossip {
    id: MessageId,                        // blake3 hash of content
    content: Bytes,                        // The actual message payload
    scope: DeliveryScope,                  // Swarm(round) or Neighbors
}
```

The `DeliveryScope` tracks how many hops the message has traveled:

```rust
pub enum DeliveryScope {
    Swarm(Round),      // Delivered via the swarm; Round = hop count from origin
    Neighbors,         // Delivered only to direct neighbors (not forwarded further)
}
```

Each time a `Gossip` message is forwarded, its `Round` is incremented via `next_round()`. `Neighbors`-scope messages are not forwarded at all.

### IHave Structure

```rust
pub struct IHave {
    id: MessageId,      // The blake3 hash of the message content
    round: Round,        // The hop count at which the sender received this message
}
```

### Graft Structure

```rust
pub struct Graft {
    id: Option<MessageId>,  // If set, also reply with full message content
    round: Round,           // The round from the IHave that triggered this graft
}
```

### Message ID

```rust
pub struct MessageId([u8; 32]);  // blake3 hash of message content

impl MessageId {
    pub fn from_content(message: &[u8]) -> Self {
        Self::from(blake3::hash(message))
    }
}
```

Messages are validated: when receiving a `Gossip`, the receiver checks that `MessageId::from_content(&content) == id`. Spoofed messages (where the hash doesn't match the content) are silently discarded.

## Broadcast Flow

### Sending a Message

```
1. Compute MessageId = blake3(content)
2. Create Gossip { id, content, scope: Swarm(Round(0)) or Neighbors }
3. If Swarm scope:
   a. Add to received_messages and cache
   b. Queue IHave for lazy peers (dispatched after dispatch_timeout)
4. Eager-push Gossip to all eager peers (except self and sender)
```

### Receiving a Gossip Message

```
1. Validate: message.id == blake3(message.content) → discard if invalid
2. If already received (in received_messages):
   → Send Prune to sender (move sender to lazy set)
   → Return (don't re-broadcast)
3. If Swarm scope:
   a. Add to received_messages
   b. Increment round (next_round)
   c. Add to cache (for Graft replies)
   d. Eager-push to all eager peers (except sender)
   e. Lazy-push IHave to all lazy peers (except sender)
   f. Check if any prior IHave senders had a shorter path → optimize tree
4. Emit Received event to application
```

### Receiving an IHave

```
For each IHave entry:
  If message ID not in received_messages:
    Add (sender, round) to missing_messages[message_id]
    If no graft timer scheduled for this message:
      Schedule SendGraft timer (graft_timeout_1)
```

### Graft Timer Expiry (Two-Phase)

**Phase 1 (`graft_timeout_1`):**
```
If message already received → no-op (cancel)
Otherwise:
  Pop first (peer, round) from missing_messages[message_id]
  Move peer to eager set
  Send Graft { id: Some(message_id), round } to that peer
  Schedule another SendGraft timer (graft_timeout_2) for fallback
```

**Phase 2 (`graft_timeout_2`):**
```
If message already received → no-op
Otherwise:
  Pop next (peer, round) from missing_messages[message_id]
  Move that peer to eager set
  Send Graft { id: Some(message_id), round }
  Schedule another SendGraft timer (graft_timeout_2)
  (continues until the message is received or senders are exhausted)
```

### Receiving a Graft

```
1. Move sender to eager set
2. If Graft contains a message ID:
   Look up message in cache
   If found: send Gossip(message) to the requesting peer
```

### Receiving a Prune

```
Move sender from eager set to lazy set
```

## Tree Optimization

The PlumTree self-optimizes based on latency. When a `Gossip` message is received, if we previously received an `IHave` for the same message from a different peer, we check whether the IHave path was significantly shorter:

```
if (ihave_round < gossip_round) && (gossip_round - ihave_round) >= optimization_threshold:
    Graft the IHave sender (move to eager)
    Prune the Gossip sender (move to lazy)
```

This means if a peer consistently has a shorter path to the message origin, they are promoted to eager, and the longer-path peer is demoted. The `optimization_threshold` (default: 7 hops) prevents thrashing from minor latency differences.

## Neighbor Events

PlumTree receives neighbor events from HyParView:

- **`NeighborUp(peer)`**: Add peer to eager set (all new neighbors start as eager)
- **`NeighborDown(peer)`**: Remove from both eager and lazy sets; clean up any `IHave` entries from this peer in `missing_messages`

## Neighbor-Only Broadcast

The `Scope::Neighbors` broadcast scope sends a message only to directly connected peers (the active view), without any forwarding:

```rust
pub enum Scope {
    Swarm,       // Broadcast to all peers in the swarm
    Neighbors,   // Broadcast only to immediate neighbors
}
```

Neighbor-scoped messages are useful for localized communication and are not cached or re-broadcast.

## Cache Management

The PlumTree maintains two time-bounded caches:

1. **`cache`** (`TimeBoundCache<MessageId, Gossip>`): Stores full message payloads for `message_cache_retention` (default 30s). This enables replying to `Graft` requests for recently-broadcast messages.

2. **`received_messages`** (`TimeBoundCache<MessageId, ()>`): Tracks which messages have been seen for `message_id_retention` (default 90s). This prevents duplicate delivery.

Both caches are periodically evicted (every `cache_evict_interval`, default 1s) via the `EvictCache` timer.