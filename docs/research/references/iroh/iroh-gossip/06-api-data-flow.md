# iroh-gossip: Public API & Data Flow

## Public API Types

### Gossip (Main Handle)

The `Gossip` struct is the main entry point, created via a `Builder`:

```rust
let gossip = Gossip::builder()
    .max_message_size(8192)
    .membership_config(HyparviewConfig { ... })
    .broadcast_config(PlumtreeConfig { ... })
    .alpn(b"/custom-alpn")
    .spawn(endpoint);
```

It derefs to `GossipApi`, which provides:

| Method | Description |
|--------|-------------|
| `subscribe(topic_id, bootstrap)` | Join a topic with default options |
| `subscribe_and_join(topic_id, bootstrap)` | Join and wait for at least one connection |
| `subscribe_with_opts(topic_id, opts)` | Join with custom `JoinOptions` |
| `handle_connection(conn)` | Handle an incoming QUIC connection |
| `shutdown()` | Gracefully leave all topics and stop |
| `max_message_size()` | Get configured max message size |
| `metrics()` | Get metrics handle |

### GossipTopic (Subscription Handle)

Returned by `subscribe()`, it is a `Stream<Item = Result<Event, ApiError>>`:

```rust
let topic: GossipTopic = gossip.subscribe(topic_id, peers).await?;
topic.broadcast(b"hello".to_vec().into()).await?;
topic.broadcast_neighbors(b"local".to_vec().into()).await?;
topic.joined().await?;  // Wait for first connection
```

Can be split into sender and receiver:

```rust
let (sender, receiver) = topic.split();
// sender: GossipSender   - can broadcast and join peers
// receiver: GossipReceiver - can receive events and check neighbors
```

### GossipSender

```rust
pub struct GossipSender(mpsc::Sender<Command>);

impl GossipSender {
    pub async fn broadcast(&self, message: Bytes) -> Result<(), ApiError>;
    pub async fn broadcast_neighbors(&self, message: Bytes) -> Result<(), ApiError>;
    pub async fn join_peers(&self, peers: Vec<EndpointId>) -> Result<(), ApiError>;
}
```

### GossipReceiver

```rust
pub struct GossipReceiver {
    stream: Pin<Box<dyn Stream<Item = Result<Event, ApiError>> + Send + Sync + 'static>>,
    neighbors: HashSet<EndpointId>,
}

impl GossipReceiver {
    pub fn neighbors(&self) -> impl Iterator<Item = EndpointId> + '_;
    pub async fn joined(&mut self) -> Result<(), ApiError>;
    pub fn is_joined(&self) -> bool;
}
```

The `GossipReceiver` tracks the neighbor set internally by processing `NeighborUp` and `NeighborDown` events.

### Event Types

```rust
pub enum Event {
    NeighborUp(EndpointId),     // New direct neighbor connected
    NeighborDown(EndpointId),   // Direct neighbor disconnected
    Received(Message),          // Gossip message received
    Lagged,                      // Internal channel lagged (messages dropped)
}

pub struct Message {
    pub content: Bytes,              // Message content
    pub scope: DeliveryScope,        // Swarm(round) or Neighbors
    pub delivered_from: EndpointId,  // Peer that delivered the message to us
}
```

### Command Types

```rust
pub enum Command {
    Broadcast(Bytes),                    // Broadcast to all in swarm
    BroadcastNeighbors(Bytes),           // Broadcast to direct neighbors only
    JoinPeers(Vec<EndpointId>),         // Join additional peers
}
```

### JoinOptions

```rust
pub struct JoinOptions {
    pub bootstrap: BTreeSet<EndpointId>,    // Initial peers to connect to
    pub subscription_capacity: usize,        // Event channel capacity (default: 2048)
}
```

### DeliveryScope

```rust
pub enum DeliveryScope {
    Swarm(Round),     // Message traveled `Round` hops from origin
    Neighbors,        // Direct neighbor message (not forwarded)
}
```

`DeliveryScope::Swarm(Round(0))` means the message was sent by a direct neighbor. `Round(n)` means the message traveled n hops.

## Data Flow Diagrams

### Joining a Topic

```
User Code                    GossipApi                Actor                  Proto State
   |                            |                       |                        |
   |-- subscribe(topic, peers)->|                       |                        |
   |                            |-- JoinRequest ------->|                        |
   |                            |                       |-- Command::Join ------>|
   |                            |                       |                        |-- RequestJoin(peers)
   |                            |                       |                        |-- SendMessage(peer, Join)
   |                            |                       |                        |-- ...
   |                            |<-- NeighborUp events--|<-- EmitEvent(NeighborUp)|
   |<-- Event::NeighborUp ------|                       |                        |
```

### Broadcasting a Message

```
User Code          GossipSender       Actor            Proto State         Network
   |                    |                |                  |                  |
   |-- broadcast(msg) ->|                |                  |                  |
   |                    |-- Command:: --> |                  |                  |
   |                    |   Broadcast    |                  |                  |
   |                    |                |-- Broadcast ---->|                  |
   |                    |                |                  |-- eager_push --->|
   |                    |                |                  |   (Gossip msgs)  |
   |                    |                |                  |-- lazy_push ----->|
   |                    |                |                  |   (IHave msgs)   |
   |                    |                |                  |                  |
   |   (other peer receives Gossip)    |                  |                  |
   |                    |                |                  |<-- RecvMessage --|
   |                    |                |<-- InEvent -------|                  |
   |                    |                |                  |  (validates ID)  |
   |                    |                |                  |  (forwards)       |
   |<-- Received(msg) -|<-- EmitEvent -|                  |                  |
```

### Receiving and Processing IHave/Graft

```
Time →

Peer A                  Our Node                    Peer B
  |                        |                          |
  |-- IHave(id, round) --->|                          |
  |                        | Schedule graft_timeout_1  |
  |                        | (wait for eager push)    |
  |                        |                          |
  |   [timeout expires]    |                          |
  |                        |-- Graft(id, round) ----->|  (Peer B sent IHave)
  |                        |                          |
  |                        |<-- Gossip(content) -------|  (Peer B replies)
  |                        |                          |
  |                        |-- Prune ----------------->|  (maybe, if optimization)
```

### HyParView Join Flow

```
New Node           Contact Node          Active Peers of Contact
   |                    |                        |
   |-- Join(me_data) -->|                        |
   |                    |-- add_active(new)       |
   |                    |-- Neighbor(High) ----->| (to new node)
   |                    |-- ForwardJoin ------->| (to each active peer)
   |                    |                        |-- add_active or add_passive
   |                    |                        |-- Neighbor(Low/High) -> (to new node)
   |                    |                        |-- ForwardJoin -> (random peer)
   |                    |                        |
   |<-- Neighbor(High) -|                        |
   |<-- Neighbor(Low/High) ----------------------|
   |                        |                        |
```

### Shuffle Periodic Operation

```
Node A                Node B                   Random Node
  |                      |                        |
  |-- Shuffle ---------->|                        |
  |   (origin=A, nodes,  |                        |
  |    TTL=6)            |                        |
  |                      |-- Shuffle ------------>|
  |                      |   (origin=A, nodes,    |
  |                      |    TTL=5)              |
  |                      |                        |-- ...
  |                      |                        |-- (TTL reaches 0)
  |                      |                        |
  |<-- ShuffleReply ----|<-- ShuffleReply --------|
  |   (random nodes)    |   (random nodes)       |
  |                      |                        |
  |-- add_passive(nodes from reply)               |
```

## RPC Support (Optional Feature)

When the `rpc` feature is enabled, `GossipApi` can also operate remotely:

```rust
// Server side
gossip.listen(rpc_endpoint).await;

// Client side
let api = GossipApi::connect(rpc_endpoint, addr);
let topic = api.subscribe_and_join(topic_id, bootstrap).await?;
```

This uses the `irpc`/`noq` crates for bidirectional streaming RPC. The `Join` request establishes a bidirectional stream:
- Client → Server: `Command` messages (Broadcast, BroadcastNeighbors, JoinPeers)
- Server → Client: `Event` messages (NeighborUp, NeighborDown, Received, Lagged)

## Channel Architecture

```
                    ┌─────────────────────────────────────────────────┐
                    │                   Actor                        │
                    │                                                 │
  RPC/Local  ──────►│  rpc_rx ◄─────────────────────────────────────│
  Commands          │  local_rx ◄── HandleConnection, Shutdown     │
                    │                                                 │
                    │  in_event_tx ──► in_event_rx ────────────────│──► proto::State::handle()
                    │                                                 │     │
                    │  ◄── OutEvent ────────────────────────────────│◄──── │
                    │      │                                          │
                    │      ├──► SendMessage ──► peer.send_tx        │
                    │      ├──► EmitEvent ──► topic.event_sender   │
                    │      ├──► ScheduleTimer ──► timers            │
                    │      ├──► DisconnectPeer ──► drop peer       │
                    │      └──► PeerData ──► address_lookup         │
                    │                                                 │
                    │  topic.event_sender ──► broadcast channel ────│──► GossipReceiver
                    │                                                 │
                    │  command_rx ◄─── per-topic command streams ──│◄── GossipSender
                    │                                                 │
                    └─────────────────────────────────────────────────┘
```

## Configuration Defaults Summary

| Parameter | Default | Source |
|-----------|---------|--------|
| Active view capacity | 5 | HyParView paper (p9) |
| Passive view capacity | 30 | HyParView paper (p9) |
| Active random walk length | 6 | HyParView paper (p9) |
| Passive random walk length | 3 | HyParView paper (p9) |
| Shuffle random walk length | 6 | HyParView paper (p9) |
| Shuffle active view count | 3 | HyParView paper (p9) |
| Shuffle passive view count | 4 | HyParView paper (p9) |
| Shuffle interval | 60s | Implementation choice |
| Neighbor request timeout | 500ms | Implementation choice |
| Graft timeout 1 | 80ms | Implementation choice |
| Graft timeout 2 | 40ms | Implementation choice |
| Dispatch timeout | 5ms | Implementation choice |
| Optimization threshold | 7 hops | PlumTree paper (p12) |
| Message cache retention | 30s | Implementation choice |
| Message ID retention | 90s | Implementation choice |
| Cache evict interval | 1s | Implementation choice |
| Max message size | 4096 bytes | Implementation choice |
| Send queue capacity | 64 messages | Implementation choice |
| To-actor channel capacity | 64 messages | Implementation choice |
| In-event channel capacity | 1024 messages | Implementation choice |
| Topic event channel capacity | 256 events | Implementation choice |
| Topic events default capacity | 2048 events | Implementation choice |
| Topic commands channel capacity | 64 commands | Implementation choice |