# iroh-live: Rooms — Multi-Party Coordination

## Overview

The `rooms` module provides multi-party room coordination over iroh-gossip. Participants discover each other via a gossip topic, automatically connect and subscribe to each other's broadcasts, and receive `RoomEvent` notifications as peers join, publish, and leave.

## Core Types

### `Room`

The main room handle. Created via `Room::new(live, ticket)`. Spawns an internal actor that manages all peer coordination.

```rust
// Create a room (generates a random topic)
let ticket = RoomTicket::generate();
let room = Room::new(&live, ticket.clone()).await?;

// Or join an existing room
let room = Room::new(&live, existing_ticket).await?;
```

**Methods:**
- `recv()` — Wait for next `RoomEvent`
- `try_recv()` — Non-blocking event check
- `ticket()` — Get a ticket that includes this peer as a bootstrap node
- `split()` — Decompose into `(RoomEvents, RoomHandle)` for use in separate tasks
- `publish(name, &LocalBroadcast)` — Publish a broadcast to the room
- `set_chat_publisher(publisher)` — Register a chat publisher
- `send_chat(text)` — Send a chat message

### `RoomHandle`

Cloneable handle for publishing into a room. Obtained from `Room::split()`. Can be shared across tasks.

```rust
let (events, handle) = room.split();

// In one task: receive events
while let Some(event) = events.recv().await {
    match event { ... }
}

// In another task: publish
handle.publish("camera", &broadcast).await?;
handle.send_chat("Hello!").await?;
handle.set_display_name("Alice").await?;
```

### `RoomTicket`

```rust
pub struct RoomTicket {
    pub bootstrap: Vec<EndpointId>,  // Bootstrap peer IDs for gossip
    pub topic_id: TopicId,           // Gossip topic identifier
}
```

Serialized via `iroh_tickets` (binary format). Can be created from:
- `RoomTicket::generate()` — Random topic, no bootstrap
- `RoomTicket::new(topic_id, bootstrap)` — Specific topic and peers
- `RoomTicket::new_from_env()` — From `IROH_LIVE_ROOM` or `IROH_LIVE_TOPIC` env vars

### `RoomEvent`

```rust
pub enum RoomEvent {
    RemoteAnnounced {
        remote: EndpointId,
        broadcasts: Vec<String>,
    },
    BroadcastSubscribed {
        session: Box<MoqSession>,
        broadcast: Box<RemoteBroadcast>,
    },
    PeerJoined {
        remote: EndpointId,
        display_name: Option<String>,
    },
    PeerLeft {
        remote: EndpointId,
    },
    ChatReceived {
        remote: EndpointId,
        message: ChatMessage,
    },
}
```

## Room Actor — Internal Architecture

The room actor is a spawned task that manages the gossip KV subscription and coordinates all peer connections.

### State

```rust
struct Actor {
    me: EndpointId,
    _gossip: Gossip,
    live: Live,
    active_subscribe: HashSet<BroadcastId>,    // (EndpointId, name) pairs
    active_publish: HashSet<String>,            // Locally published broadcast names
    known_peers: HashMap<EndpointId, Option<String>>, // display names
    connecting: ConnectingFutures,             // In-flight subscribe attempts
    subscribe_closed: FuturesUnordered,        // Track subscription lifetimes
    publish_closed: FuturesUnordered,           // Track publish lifetimes
    chat_messages: FuturesUnordered,           // Active chat subscribers
    chat_publisher: Option<ChatPublisher>,
    display_name: Option<String>,
    event_tx: mpsc::Sender<RoomEvent>,
    kv: iroh_smol_kv::Client,                // Distributed KV for peer state
    kv_writer: WriteScope,                     // KV write access
}
```

### Gossip KV for Peer Discovery

The room uses `iroh-smol-kv` over gossip for peer state coordination. Each peer writes their `PeerState` to key `b"s"`:

```rust
struct PeerState {
    broadcasts: Vec<String>,
    display_name: Option<String>,
}
```

Serialized with postcard (binary format — **no `skip_serializing_if`** allowed since postcard is positional).

### Event Loop

```
loop {
    select! {
        update = gossip_kv_stream.next()     → handle_gossip_update
        msg = inbox.recv()                     → handle_api_message
        result = connecting.next()             → subscribe succeeded/failed
        broadcast_closed                       → remove from active, maybe emit PeerLeft
        publish_closed                         → remove from active_publish, update KV
        chat_message                           → emit ChatReceived
    }
}
```

### Peer Discovery Flow

1. Peer A publishes a broadcast via `handle.publish("camera", &broadcast)`
2. Actor publishes to MoQ AND updates gossip KV with `PeerState { broadcasts: ["camera"], display_name: ... }`
3. Peer B's gossip KV stream receives the update
4. Peer B's actor checks `known_peers` — if new, emits `PeerJoined`
5. Peer B's actor checks `active_subscribe` — if new broadcast, initiates `live.subscribe(remote, name)`
6. When subscription succeeds, Peer B emits `BroadcastSubscribed`
7. If the broadcast has a chat track, a chat subscriber is spawned

### Chat

Chat uses a dedicated MoQ track within each broadcast. Each message is a single MoQ group containing one frame of UTF-8 text. The sender identity comes from the broadcast context (peer ID), not the message payload.

### Connection Lifecycle

- When a broadcast closes (`subscribe_closed`), it's removed from `active_subscribe`
- If this was the last broadcast from that peer, `PeerLeft` is emitted
- When a publish closes (`publish_closed`), the KV is updated to remove that broadcast

### `RoomPublisherSync`

A convenience wrapper for the common pattern of publishing camera+audio and optionally screen share into a room:

```rust
let publisher = RoomPublisherSync::new(room_handle, audio_backend);
publisher.set_state(&PublishOpts::default())?;
```

Automatically publishes a "camera" broadcast and manages a "screen" broadcast when screen sharing is toggled on.

## API Messages

```rust
enum ApiMessage {
    Publish { name: String, producer: BroadcastProducer },
    SendChat { text: String },
    SetChatPublisher { publisher: ChatPublisher },
    SetDisplayName { name: String },
}
```

These are sent from `RoomHandle` to the actor via an mpsc channel.