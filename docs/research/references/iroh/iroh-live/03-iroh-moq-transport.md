# iroh-moq: MoQ Transport Layer

## Overview

`iroh-moq` is the transport bridge between iroh's QUIC endpoint and the moq-lite broadcast protocol. It manages connections, session lifecycle, broadcast routing, and subscription handling. This is the only crate in the workspace that directly interacts with QUIC transport — everything above uses `Moq`/`MoqSession` as the interface.

**ALPN:** `moq-lite-03`

## Core Types

### `Moq` — Transport Manager

The top-level transport entry point. Wraps an iroh `Endpoint` and runs an internal actor (`Actor`) that handles all connection and broadcast management.

```rust
let moq = Moq::new(endpoint);
```

**Internal architecture:**

`Moq` holds an `mpsc::Sender<ActorMessage>` to communicate with a spawned actor task. The actor manages:
- A `HashMap<EndpointId, MoqSession>` of active sessions
- A `HashMap<BroadcastName, BroadcastProducer>` of locally published broadcasts
- A `JoinSet` of session tasks (each tracks session lifetime)
- A `FuturesUnordered` of pending connect tasks
- A `broadcast::Sender<MoqSession>` for incoming session notifications

**Key methods:**

| Method | Description |
|--------|-------------|
| `new(endpoint)` | Creates transport and spawns the actor |
| `protocol_handler()` | Returns `MoqProtocolHandler` for Router registration |
| `publish(name, producer)` | Register a broadcast for all current and future sessions |
| `connect(remote)` | Connect to remote peer, deduplicating existing connections |
| `incoming_sessions()` | Get stream of incoming sessions |
| `published_broadcasts()` | List currently published broadcast names |
| `shutdown()` | Cancel the shutdown token, closing all sessions |

### `MoqProtocolHandler`

Implements iroh's `ProtocolHandler` trait. When the Router receives an incoming connection with the `moq-lite-03` ALPN:

1. Accepts the raw QUIC `Connection`
2. Wraps it in a `web_transport_iroh::Session::raw(connection)`
3. Completes the moq-lite server handshake: `MoqSession::session_accept(wt_session)`
4. Sends the session to the actor via `ActorMessage::HandleSession`

### `MoqSession` — Single Peer Connection

Represents a MoQ connection with one remote peer. Created via:
- `Moq::connect()` (outbound, client role)
- `IncomingSession::accept()` (inbound, server role)

```rust
// Outbound
let session = moq.connect(remote_addr).await?;

// Inbound
let incoming = incoming_session.next().await?;
let session = incoming.accept();  // or incoming.reject()
```

**Internal structure:**

```rust
pub struct MoqSession {
    wt_session: web_transport_iroh::Session,
    _moq_session: Arc<moq_lite::Session>,
    publish: OriginProducer,   // For announcing local broadcasts
    subscribe: OriginConsumer, // For consuming remote broadcasts
}
```

The `OriginProducer`/`OriginConsumer` pair comes from moq-lite. The session creates them before the handshake:

- **Client (connect):** Creates `OriginProducer` for publish and `OriginConsumer` for subscribe, then `Client::new().with_publish(...).with_consume(...).connect(session)`
- **Server (accept):** Same pattern with `Server::new().with_publish(...).with_consume(...).accept(session)`

**Key methods:**

| Method | Description |
|--------|-------------|
| `subscribe(name)` | Wait for remote to announce broadcast, return `BroadcastConsumer` |
| `publish(name, consumer)` | Make a broadcast available to remote peer |
| `conn()` | Reference to underlying QUIC `Connection` (for stats) |
| `remote_id()` | Remote peer's `EndpointId` |
| `close(code, reason)` | Close the session |
| `closed()` | Wait for session to close, returns `SessionError` |
| `origin_producer()` | Direct access to moq-lite publish origin |
| `origin_consumer()` | Direct access to moq-lite subscribe origin |

### `IncomingSession` / `IncomingSessionStream`

`IncomingSession` wraps a `MoqSession` that has completed the handshake. Provides:
- `remote_id()` — the connecting peer's identity
- `accept()` — returns the `MoqSession`
- `reject()` — closes with error code 1

`IncomingSessionStream` is an async stream that yields `IncomingSession` values. Uses a `broadcast::Receiver<MoqSession>` internally, handling lag by skipping missed sessions.

## Actor Internals

The `Actor` is the core event loop for the `Moq` transport:

```
loop {
    select! {
        msg = inbox.recv()           → handle_message(msg)
        session_closed               → remove session, log
        broadcast_closed             → remove from publishing map
        connect_completed            → handle_session or reply to caller
    }
}
```

### Message Types

```rust
enum ActorMessage {
    HandleSession { session: Box<MoqSession> },
    LocalBroadcast { broadcast_name: String, producer: BroadcastProducer },
    Connect { remote: EndpointAddr, reply: oneshot::Sender<...> },
    GetPublished { reply: oneshot::Sender<Vec<String>> },
}
```

### Connection Deduplication

When `Connect` is received for a peer that already has an active session, the existing session is returned immediately. If a connection attempt is already in progress, the oneshot reply is queued and notified when the connection completes.

### Broadcast Fan-out

When a `LocalBroadcast` is published via `Moq::publish()`:
1. The actor stores the `BroadcastProducer` in its `publishing` map
2. It immediately announces the broadcast to all existing sessions by calling `session.publish(name, producer.consume())` on each
3. For future sessions, the actor iterates `publishing` entries and announces each one
4. A `FuturesUnordered` tracks when each broadcast closes, removing it from the map

### Session Lifecycle

When a session is established (either incoming or outgoing):
1. All currently published broadcasts are announced to it
2. It's stored in `sessions` by `EndpointId`
3. A session task is spawned that waits for the session to close
4. If there were pending connect requests for this peer, they're fulfilled

## Error Types

```rust
enum Error {
    Connect(ConnectError),           // iroh connection failure
    Moq(moq_lite::Error),            // MoQ protocol error
    Server(web_transport_iroh::ServerError),  // WebTransport server error
    InternalConsistencyError(LiveActorDiedError), // Actor died
    Request(WriteError),             // QUIC write error
}

enum SubscribeError {
    NotAnnounced,                    // Track was not announced
    Closed,                          // Track was closed
    SessionClosed(SessionError),      // Session closed
}
```