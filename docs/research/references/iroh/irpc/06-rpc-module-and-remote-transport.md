# irpc: RPC Module and Remote Transport

The `rpc` module (enabled by the `rpc` feature) contains all cross-process RPC functionality: QUIC stream handling, connection management, serialization, and server-side request processing.

## Module Structure

```rust
pub mod rpc {
    pub const MAX_MESSAGE_SIZE: u64 = 1024 * 1024 * 16;
    pub const ERROR_CODE_MAX_MESSAGE_SIZE_EXCEEDED: u32 = 1;
    pub const ERROR_CODE_INVALID_POSTCARD: u32 = 2;

    pub enum WriteError { Noq, MaxMessageSizeExceeded, Io }
    pub trait RemoteConnection: Send + Sync + Debug + 'static { ... }
    pub struct RemoteSender<S>(SendStream, RecvStream, PhantomData<S>);
    pub type Handler<R> = Arc<dyn Fn(R, RecvStream, SendStream) -> BoxFuture<Result<(), SendError>> + Send + Sync>;
    pub trait RemoteService: Service + Sized { ... }
    pub async fn listen<R>(endpoint, handler);
    pub async fn handle_connection<R>(connection, handler) -> io::Result<()>;
    pub async fn read_request<S: RemoteService>(connection) -> io::Result<Option<S::Message>>;
    pub async fn read_request_raw<R>(connection) -> io::Result<Option<(R, RecvStream, SendStream)>>;
}
```

## RemoteConnection Implementations

### NoqLazyRemoteConnection

The default remote connection for noq (QUIC-by-socket-address):

```rust
struct NoqLazyRemoteConnection(Arc<NoqLazyRemoteConnectionInner>);

struct NoqLazyRemoteConnectionInner {
    endpoint: noq::Endpoint,
    addr: SocketAddr,
    connection: Mutex<Option<noq::Connection>>,
}
```

**Behavior:**
- `open_bi()`: 
  1. Locks the `Mutex<Option<Connection>>`
  2. If a cached connection exists, tries `conn.open_bi()`
  3. If that fails, clears the cache and establishes a new connection
  4. If no cached connection, establishes a new one
  5. Returns `(SendStream, RecvStream)` pair
- `zero_rtt_accepted()`: Always returns `true` (noq doesn't have 0-RTT concept in this context)
- `clone_boxed()`: Clones the `Arc`, sharing the same connection cache

### Direct noq::Connection

```rust
impl RemoteConnection for noq::Connection {
    fn open_bi(&self) -> BoxFuture<Result<(SendStream, RecvStream), RequestError>> {
        // Directly opens a bidirectional stream on the connection
    }
    fn zero_rtt_accepted(&self) -> BoxFuture<bool> { Box::pin(async { true }) }
}
```

## RemoteSender

```rust
pub struct RemoteSender<S>(noq::SendStream, noq::RecvStream, PhantomData<S>);
```

Created by `Client::request()` when the client is remote. Holds both sides of a QUIC bidirectional stream.

### Key Methods

```rust
impl<S: Service> RemoteSender<S> {
    pub fn new(send: SendStream, recv: RecvStream) -> Self;

    pub async fn write(self, msg: impl Into<S>) -> Result<(SendStream, RecvStream), WriteError> {
        let buf = prepare_write(msg)?;
        self.write_raw(&buf).await
    }

    // Internal: writes pre-serialized buffer
    pub(crate) async fn write_raw(self, buf: &[u8]) -> Result<(SendStream, RecvStream), WriteError>;
}
```

The `write()` method:
1. Converts `msg` into the protocol enum `S` via `Into`
2. Checks serialized size against `MAX_MESSAGE_SIZE`
3. Length-prefixes with varint + postcard serialization
4. Writes to the `SendStream`
5. Returns the stream pair (now usable for response channels)

The `write_raw()` method is used for 0-RTT where the message is pre-serialized to allow re-sending without re-serialization.

### prepare_write

```rust
fn prepare_write<S: Service>(msg: impl Into<S>) -> Result<SmallVec<[u8; 128]>, WriteError> {
    let msg = msg.into();
    if postcard::experimental::serialized_size(&msg)? as u64 > MAX_MESSAGE_SIZE {
        return Err(WriteError::MaxMessageSizeExceeded);
    }
    let mut buf = SmallVec::<[u8; 128]>::new();
    buf.write_length_prefixed(&msg)?;
    Ok(buf)
}
```

Uses `SmallVec<[u8; 128]>` to avoid heap allocation for small messages.

## Stream-to-Channel Conversions

When a QUIC stream pair is received on the server side, it needs to be converted into typed channels. The `From` implementations handle this:

### SendStream → Channel Tx

```rust
// NoSender: drop the stream
impl From<SendStream> for NoSender { ... }

// Oneshot: serialize and send single value, then done
impl<T: RpcMessage> From<SendStream> for oneshot::Sender<T> { ... }

// MPSC: repeatedly serialize and send values
impl<T: RpcMessage> From<SendStream> for mpsc::Sender<T> { ... }
```

### RecvStream → Channel Rx

```rust
// NoReceiver: drop the stream
impl From<RecvStream> for NoReceiver { ... }

// Oneshot: read single length-prefixed value
impl<T: DeserializeOwned> From<RecvStream> for oneshot::Receiver<T> { ... }

// MPSC: repeatedly read length-prefixed values
impl<T: RpcMessage> From<RecvStream> for mpsc::Receiver<T> { ... }
```

## Server-Side Request Processing

### read_request_raw

```rust
pub async fn read_request_raw<R: DeserializeOwned + 'static>(
    connection: &noq::Connection,
) -> io::Result<Option<(R, RecvStream, SendStream)>>
```

1. Calls `connection.accept_bi()` to accept an incoming bidirectional stream
2. If `ApplicationClosed(0)`, returns `Ok(None)` (clean shutdown)
3. Reads a varint length prefix from the `RecvStream`
4. Checks against `MAX_MESSAGE_SIZE`
5. Reads `length` bytes from the stream
6. Deserializes with `postcard::from_bytes::<R>()`
7. Returns `(deserialized_message, RecvStream, SendStream)`

### read_request (typed)

```rust
pub async fn read_request<S: RemoteService>(
    connection: &noq::Connection,
) -> io::Result<Option<S::Message>>
```

Calls `read_request_raw()` and then applies `S::with_remote_channels()` to convert the raw protocol message + stream pair into a `WithChannels`-wrapped `Message`.

### handle_connection

```rust
pub async fn handle_connection<R: DeserializeOwned + 'static>(
    connection: noq::Connection,
    handler: Handler<R>,
) -> io::Result<()>
```

Loops:
1. Calls `read_request_raw()` to get the next request
2. If `None`, returns `Ok(())` (connection closed)
3. Invokes `handler(msg, rx, tx)` to process the request
4. Continues until the connection closes or an error occurs

Each connection is handled in a separate task (spawned by `listen()`).

### listen

```rust
pub async fn listen<R: DeserializeOwned + 'static>(
    endpoint: noq::Endpoint,
    handler: Handler<R>,
)
```

The top-level server loop:
1. Accepts incoming connections from the `noq::Endpoint`
2. Spawns a task for each connection
3. Each task calls `handle_connection()`
4. Uses a `JoinSet` to manage and clean up completed tasks

## The Handler and Local Forwarding

The typical handler is created by `Protocol::remote_handler(local_sender)`:

```rust
fn remote_handler(local_sender: LocalSender<Self>) -> Handler<Self> {
    Arc::new(move |msg, rx, tx| {
        let msg = Self::with_remote_channels(msg, rx, tx);
        Box::pin(local_sender.send_raw(msg))
    })
}
```

This converts the raw (deserialized protocol message, RecvStream, SendStream) tuple into a typed `WithChannels` message and forwards it to the local actor via the mpsc channel. The local actor can then use the typed channels without knowing whether they're local or remote.

## Full Request Lifecycle (Remote)

```
  CLIENT                                    SERVER
    │                                          │
    │  1. Client::request()                    │
    │     → open_bi() on connection            │
    │                                          │
    │  2. RemoteSender::write(protocol_msg)    │
    │     → serialize + send on SendStream ────►│
    │                                          │  3. accept_bi()
    │                                          │  4. read_request_raw()
    │                                          │     → read varint + data
    │                                          │     → deserialize protocol_msg
    │                                          │
    │                                          │  5. RemoteService::with_remote_channels()
    │                                          │     → creates WithChannels
    │                                          │     → SendStream → tx channel
    │                                          │     → RecvStream → rx channel
    │                                          │
    │                                          │  6. handler(msg, rx, tx)
    │                                          │     → local_sender.send_raw(message)
    │                                          │     → message goes to actor
    │                                          │
    │                                          │  7. Actor processes:
    │                                          │     match message {
    │                                          │       Msg::Get(wc) => {
    │                                          │         let res = db.get(wc.inner.key);
    │                                          │         wc.tx.send(res).await;
    │                                          │         // tx.send() writes to SendStream
    │                                          │       }
    │                                          │     }
    │                                          │
    │  8. RecvStream reads response ◄───────────│
    │  9. Deserialize response                  │
    │  10. Return to caller                    │
```

## 0-RTT Flow

```
  CLIENT                                    SERVER
    │                                          │
    │  1. Serialize message into buffer        │
    │     (prepare_write)                      │
    │                                          │
    │  2. Open 0-RTT connection               │
    │     → write buffer immediately ─────────►│
    │                                          │
    │  3. Check zero_rtt_accepted()            │
    │     → If true: done, read response       │
    │     → If false:                          │
    │       4. Open new (full) connection       │
    │       5. Re-send same buffer ────────────►│
    │                                          │
    │  6. Read response ◄──────────────────────│
```

The key insight: the message buffer is pre-serialized so it can be re-sent without re-serialization if 0-RTT is rejected.