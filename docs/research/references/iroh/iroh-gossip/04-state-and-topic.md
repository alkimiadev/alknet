# iroh-gossip: Protocol State & Topic Coordination

## Overview

The `state` module (`src/proto/state.rs`) provides the **top-level protocol state machine** that manages multiple topics. The `topic` module (`src/proto/topic.rs`) coordinates the HyParView and PlumTree state machines for a single topic.

## Multi-Topic State (`state::State`)

```rust
pub struct State<PI, R> {
    me: PI,                                    // Our peer identity
    me_data: PeerData,                         // Our opaque peer data
    config: Config,                            // Protocol configuration
    rng: R,                                    // Random number generator
    states: HashMap<TopicId, topic::State<PI, R>>,  // Per-topic state
    outbox: Outbox<PI>,                        // Buffered output events
    peer_topics: ConnsMap<PI>,                 // Maps peer → set of shared topics
}
```

The `State` acts as a **multiplexer** — it routes events to the correct topic's state and collects output events. It also tracks which topics are shared with each peer (in `peer_topics`), which is used to determine when a peer connection can safely be closed (only when no topic still needs it).

### TopicId

```rust
#[derive(Clone, Copy, Eq, PartialEq, Hash, Serialize, Ord, PartialOrd, Deserialize)]
pub struct TopicId([u8; 32]);
```

A 32-byte identifier for a topic. Typically created as `blake3::hash(topic_name)` or from raw bytes. Each topic is an independent swarm and broadcast scope.

### Wire Message Format

```rust
pub struct Message<PI> {
    pub topic: TopicId,
    pub message: topic::Message<PI>,
}
```

Every wire message carries the `TopicId` prefix, allowing multiplexing of multiple topics over a single connection.

### Event Routing

`InEvent` is mapped to either a topic-specific event or a global event:

| InEvent | Routing |
|---------|---------|
| `RecvMessage(from, Message{topic, message})` | → Topic-specific: `topic::InEvent::RecvMessage` |
| `Command(topic, command)` | → Topic-specific: `topic::InEvent::Command` |
| `TimerExpired(Timer{topic, timer})` | → Topic-specific: `topic::InEvent::TimerExpired` |
| `PeerDisconnected(peer)` | → Broadcast to ALL topics |
| `UpdatePeerData(data)` | → Broadcast to ALL topics |

### Topic Lifecycle

When a `Command::Join(peers)` is received for a topic that doesn't yet have state, a new `topic::State` is automatically created. When `Command::Quit` is received, the topic's state is removed after processing the quit event.

### Connection Management

When a `topic::OutEvent::DisconnectPeer(peer)` is emitted, the state module checks `peer_topics` to see if any other topic still needs a connection to that peer. Only when no topic needs the peer anymore is `OutEvent::DisconnectPeer(peer)` emitted at the top level.

## Topic State (`topic::State`)

```rust
pub struct State<PI, R> {
    me: PI,
    pub swarm: hyparview::State<PI, R>,    // HyParView membership
    pub gossip: plumtree::State<PI>,          // PlumTree broadcast
    outbox: VecDeque<OutEvent<PI>>,
    stats: Stats,
}
```

The topic state **composes** HyParView and PlumTree, bridging them together:

### Event Forwarding

When `topic::State::handle()` is called:

1. **HyParView events** are processed first (membership layer).
2. **NeighborUp/NeighborDown events** emitted by HyParView are forwarded to PlumTree:
   - `NeighborUp(peer)` → `plumtree::InEvent::NeighborUp(peer)` — adds peer to eager set
   - `NeighborDown(peer)` → `plumtree::InEvent::NeighborDown(peer)` — removes peer from both sets
3. All output events from both layers are collected and returned.

### Command Handling

| Command | Action |
|---------|--------|
| `Join(peers)` | Sends `RequestJoin(peer)` to HyParView for each peer in the list |
| `Broadcast(data, scope)` | Sends `Broadcast(data, scope)` to PlumTree |
| `Quit` | Sends `Quit` to HyParView (which sends `Disconnect` to all active peers) |

### Message Routing

When a topic message is received:

```rust
match message {
    Message::Swarm(message) => hyparview.handle(RecvMessage(from, message)),
    Message::Gossip(message) => plumtree.handle(RecvMessage(from, message)),
}
```

### Timer Routing

```rust
match timer {
    Timer::Swarm(timer) => hyparview.handle(TimerExpired(timer)),
    Timer::Gossip(timer) => plumtree.handle(TimerExpired(timer)),
}
```

## Topic Messages (`topic::Message`)

```rust
pub enum Message<PI> {
    Swarm(hyparview::Message<PI>),   // Membership messages
    Gossip(plumtree::Message),        // Broadcast messages
}
```

The message kind is used for metrics tracking:

```rust
pub fn kind(&self) -> MessageKind {
    match self {
        Message::Swarm(_) => MessageKind::Control,
        Message::Gossip(message) => match message {
            plumtree::Message::Gossip(_) => MessageKind::Data,
            _ => MessageKind::Control,
        },
    }
}
```

## Topic Events (`topic::Event`)

```rust
pub enum Event<PI> {
    NeighborUp(PI),           // From HyParView: new active neighbor
    NeighborDown(PI),         // From HyParView: lost active neighbor
    Received(GossipEvent<PI>), // From PlumTree: received a gossip message
}
```

The `Received` event contains:

```rust
pub struct GossipEvent<PI> {
    pub content: Bytes,              // Message payload
    pub delivered_from: PI,          // Peer that delivered the message to us
    pub scope: DeliveryScope,        // Swarm(round) or Neighbors
}
```

## Topic Configuration

```rust
pub struct Config {
    pub membership: hyparview::Config,    // HyParView configuration
    pub broadcast: plumtree::Config,       // PlumTree configuration
    pub max_message_size: usize,           // Maximum wire message size (default: 4096)
}
```

The `max_message_size` is the total wire-level message size including headers. The actual payload capacity is computed as `max_message_size - postcard_header_size`, where the header size accounts for the topic ID and message envelope overhead.

## Statistics

Each topic tracks:
```rust
pub struct Stats {
    pub messages_sent: usize,
    pub messages_received: usize,
}
```

The PlumTree layer also tracks:
```rust
pub struct Stats {
    pub payload_messages_received: u64,
    pub control_messages_received: u64,
    pub max_last_delivery_hop: u16,
}
```