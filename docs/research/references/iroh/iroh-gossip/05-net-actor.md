# iroh-gossip: Networking Layer & Actor Model

## Overview

The `net` module (`src/net.rs` and submodules) provides the async runtime layer that connects the IO-free protocol state machine to real network IO via iroh QUIC connections. It is built around a **single Actor** that manages all topics and connections.

## ALPN Protocol

```rust
pub const GOSSIP_ALPN: &[u8] = b"/iroh-gossip/1";
```

This ALPN identifier is used when establishing QUIC connections through iroh.

## Gossip Handle (`net::Gossip`)

```rust
#[derive(Debug, Clone)]
pub struct Gossip {
    pub(crate) inner: Arc<Inner>,
}
```

`Gossip` is the primary public handle. It derefs to `GossipApi`, providing the user-facing interface:

```rust
// Subscribe to a topic
let (sender, receiver) = gossip.subscribe(topic_id, bootstrap_peers).await?.split();

// Subscribe and wait for at least one connection
let topic = gossip.subscribe_and_join(topic_id, bootstrap_peers).await?;

// Broadcast a message
sender.broadcast(b"hello world".to_vec().into()).await?;

// Broadcast to neighbors only
sender.broadcast_neighbors(b"local announcement".to_vec().into()).await?;

// Join additional peers
sender.join_peers(vec![peer_id]).await?;
```

### Builder Pattern

```rust
let gossip = Gossip::builder()
    .max_message_size(8192)            // Default: 4096
    .membership_config(hyparview_config)  // HyParView settings
    .broadcast_config(plumtree_config)    // PlumTree settings
    .alpn(b"/custom-alpn")               // Custom ALPN (must match across network)
    .spawn(endpoint);
```

## Architecture: The Actor

The core of the networking layer is the `Actor` struct, which runs as a single async task:

```rust
struct Actor {
    alpn: Bytes,
    state: proto::State<PublicKey, StdRng>,     // Protocol state machine
    endpoint: Endpoint,                          // iroh endpoint for connections
    dialer: Dialer,                              // Manages outgoing connections
    rpc_rx: mpsc::Receiver<RpcMessage>,          // API commands
    local_rx: mpsc::Receiver<LocalActorMessage>, // Local commands (connections, shutdown)
    in_event_tx: mpsc::Sender<InEvent>,          // Protocol input channel
    in_event_rx: mpsc::Receiver<InEvent>,        // Protocol input channel (receiver)
    timers: Timers<Timer>,                        // Scheduled timers
    topics: HashMap<TopicId, TopicState>,         // Per-topic subscription state
    peers: HashMap<EndpointId, PeerState>,        // Per-peer connection state
    command_rx: stream_group::Keyed<TopicCommandStream>,  // Per-topic command streams
    quit_queue: VecDeque<TopicId>,               // Topics pending unsubscription
    connection_tasks: JoinSet<...>,               // Running connection loop tasks
    metrics: Arc<Metrics>,
    topic_event_forwarders: JoinSet<TopicId>,    // Tasks forwarding events to subscribers
    address_lookup: GossipAddressLookup,          // Address discovery integration
}
```

### Event Loop

The actor's `run()` method calls `event_loop()` in a loop. Each iteration uses `tokio::select!` to handle:

| Source | Action |
|--------|--------|
| `local_rx` (local messages) | Handle incoming connections or shutdown |
| `rpc_rx` (RPC messages) | Process `Join` requests from the API |
| `command_rx` (per-topic commands) | Process `Broadcast`, `BroadcastNeighbors`, `JoinPeers`, or stream closure |
| `addr_updates` (endpoint addr changes) | Update our `PeerData` in the protocol state |
| `dialer` (connection establishment) | Handle successful/failed outgoing connections |
| `in_event_rx` (protocol events from connections) | Feed events to the protocol state machine |
| `timers` (scheduled timers) | Feed timer expirations to the protocol state machine |
| `connection_tasks` (connection task completions) | Handle peer disconnections |
| `topic_event_forwarders` (subscription tasks) | Handle topic cleanup when all subscribers drop |

### Processing InEvents

When an `InEvent` is processed, the actor calls `self.state.handle(event, now, metrics)`, which returns `Vec<OutEvent>`. For each `OutEvent`:

| OutEvent | Action |
|----------|--------|
| `SendMessage(peer, message)` | Send via peer's active connection or queue for pending connection |
| `EmitEvent(topic, event)` | Forward to topic's `broadcast::Sender` → subscribers |
| `ScheduleTimer(delay, timer)` | Schedule timer via `Timers` data structure |
| `DisconnectPeer(peer)` | Drop the peer's send channel, removing from `peers` map |
| `PeerData(endpoint_id, data)` | Decode `AddrInfo` from `PeerData`, add to `GossipAddressLookup` |

## Connection Management

### Peer States

```rust
enum PeerState {
    Pending {
        queue: Vec<ProtoMessage>,   // Messages queued while connecting
    },
    Active {
        active_send_tx: mpsc::Sender<ProtoMessage>,  // Current active send channel
        active_conn_id: ConnId,                       // Stable ID of active connection
        other_conns: Vec<ConnId>,                     // Older connections still closing
    },
}
```

When a message needs to be sent to a peer:
- **Active**: Send immediately via `active_send_tx`
- **Pending**: Queue the message and initiate a dial

### Dialer

```rust
struct Dialer {
    endpoint: Endpoint,
    pending: JoinSet<(EndpointId, Option<Result<Connection, ConnectError>>)>,
    pending_dials: HashMap<EndpointId, CancellationToken>,
}
```

The `Dialer` manages outgoing connections. It:
1. Checks if a dial is already pending for a peer
2. Spawns an async connection task with cancellation support
3. Returns completed connections via `next_conn()`

### Connection Loop

Each peer connection runs a `connection_loop` task:

```rust
async fn connection_loop(
    from: PublicKey,           // Remote peer's public key
    conn: Connection,          // QUIC connection
    origin: ConnOrigin,        // Accept (incoming) or Dial (outgoing)
    send_rx: mpsc::Receiver<ProtoMessage>,  // Messages to send
    in_event_tx: mpsc::Sender<InEvent>,      // Channel to protocol
    max_message_size: usize,                   // Maximum message size
    queue: Vec<ProtoMessage>,                  // Queued messages to send first
) -> Result<(), ConnectionLoopError>
```

The connection loop:
1. First sends any queued messages
2. Runs a send loop and receive loop concurrently (`tokio::join!`)
3. Uses iroh QUIC bidirectional streams for communication

### Wire Protocol

Messages are serialized with `postcard` and sent as **length-prefixed frames** over QUIC unidirectional streams:

```
┌──────────────┐
│ Stream Header │  ── Contains TopicId (sent once per stream)
├──────────────┤
│ Frame (len)  │  ── u32 length prefix
│ Frame (data) │  ── postcard-encoded topic::Message<PublicKey>
├──────────────┤
│ Frame (len)  │  ── next message...
│ Frame (data) │
└──────────────┘
```

Each topic gets its own unidirectional stream. The stream header is sent once when the stream is opened. Disconnect messages close the stream after being sent.

The `SendLoop` manages per-topic streams within a connection:

```rust
struct SendLoop {
    conn: Connection,
    streams: HashMap<TopicId, SendStream>,  // One stream per topic
    buffer: Vec<u8>,
    max_message_size: usize,
    send_rx: mpsc::Receiver<ProtoMessage>,
}
```

When a disconnect message is sent for a topic, the stream for that topic is closed (via `finish()`).

## Topic State (Net Layer)

```rust
struct TopicState {
    neighbors: BTreeSet<EndpointId>,          // Current active neighbors (from protocol)
    event_sender: broadcast::Sender<ProtoEvent>,  // Broadcast channel to subscribers
    command_rx_keys: HashSet<stream_group::Key>,   // Active command stream keys
}
```

A topic is considered "still needed" if it has either:
- Active command receivers (publishers), or
- Active event subscribers (subscribers)

When neither exists, the topic is queued for quit/unsubscription.

## Address Lookup Integration

The `GossipAddressLookup` integrates with iroh's address discovery system:

```rust
pub(crate) struct GossipAddressLookup {
    endpoints: NodeMap,                      // BTreeMap<EndpointId, StoredEndpointInfo>
    _task_handle: Arc<AbortOnDropHandle<()>>, // Background eviction task
}
```

It implements iroh's `AddressLookup` trait, allowing gossip-discovered peer addresses to feed back into iroh's connection establishment. This means that when a peer shares its address information in `Join` or `ForwardJoin` messages, that information is used to help iroh connect to that peer.

Entries expire after 5 minutes (configurable via `RetentionOpts`), with eviction checks every 30 seconds.

## Metrics

The `Metrics` struct tracks various counters:

| Metric | Description |
|--------|-------------|
| `msgs_ctrl_sent` | Control messages sent |
| `msgs_ctrl_recv` | Control messages received |
| `msgs_data_sent` | Data messages sent |
| `msgs_data_recv` | Data messages received |
| `msgs_data_sent_size` | Total size of data messages sent |
| `msgs_data_recv_size` | Total size of data messages received |
| `msgs_ctrl_sent_size` | Total size of control messages sent |
| `msgs_ctrl_recv_size` | Total size of control messages received |
| `neighbor_up` | Neighbor connections established |
| `neighbor_down` | Neighbor connections lost |
| `actor_tick_*` | Various event loop tick counters |