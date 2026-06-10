# irpc: Key Types and Traits

## Core Traits

### `RpcMessage`

```rust
pub trait RpcMessage: Debug + Serialize + DeserializeOwned + Send + Sync + Unpin + 'static {}
```

A blanket trait implemented for all types that satisfy the bounds. Every message sent through irpc (both local and remote) must implement this. The `Serialize + DeserializeOwned` requirement exists even without the `rpc` feature because the same protocol definition should work in both modes.

### `Service`

```rust
pub trait Service: Serialize + DeserializeOwned + Send + Sync + Debug + 'static {
    type Message: Send + Unpin + 'static;
}
```

Implemented on the **protocol enum** (e.g., `StorageProtocol`). The `Message` associated type is the **message enum** — an enum with identical variant names but whose single field is `WithChannels<InnerType, Self>`.

The `Service` trait acts as a **scope** for channel type definitions, allowing the same inner request type to be used with multiple services.

### `Channels<S>`

```rust
pub trait Channels<S: Service>: Send + 'static {
    type Tx: Sender;
    type Rx: Receiver;
}
```

Implemented on each **request type** (e.g., `Get`, `Set`). Specifies what kind of channels accompany that request when sent through service `S`. The `Tx` type is the response channel (server → client); the `Rx` type is the update channel (client → server).

### `Sender` and `Receiver`

```rust
pub trait Sender: Debug + Sealed {}
pub trait Receiver: Debug + Sealed {}
```

Sealed marker traits. Only the types in `irpc::channel` implement these: `oneshot::Sender`, `oneshot::Receiver`, `mpsc::Sender`, `mpsc::Receiver`, `NoSender`, `NoReceiver`.

### `RemoteService` (rpc feature)

```rust
pub trait RemoteService: Service + Sized {
    fn with_remote_channels(self, rx: noq::RecvStream, tx: noq::SendStream) -> Self::Message;

    fn remote_handler(local_sender: LocalSender<Self>) -> Handler<Self> {
        // Default: convert deserialized protocol enum + streams → Message, send to local sender
    }
}
```

Implemented on the protocol enum. Maps a deserialized protocol variant + a pair of QUIC streams into a `WithChannels` message, which is then forwarded to the local actor.

### `RemoteConnection` (rpc feature)

```rust
pub trait RemoteConnection: Send + Sync + Debug + 'static {
    fn clone_boxed(&self) -> Box<dyn RemoteConnection>;
    fn open_bi(&self) -> BoxFuture<Result<(noq::SendStream, noq::RecvStream), RequestError>>;
    fn zero_rtt_accepted(&self) -> BoxFuture<bool>;
}
```

Abstraction over how to open a bidirectional QUIC stream. Implemented for:
- `noq::Connection` — direct noq connection
- `NoqLazyRemoteConnection` — lazy connection that caches the underlying QUIC connection
- `IrohRemoteConnection` — iroh connection (in `irpc-iroh`)
- `IrohLazyRemoteConnection` — lazy iroh connection (in `irpc-iroh`)
- `IrohZrttRemoteConnection` — 0-RTT iroh connection (in `irpc-iroh`)

## Key Structs

### `WithChannels<I, S>`

```rust
pub struct WithChannels<I: Channels<S>, S: Service> {
    pub inner: I,
    pub tx: <I as Channels<S>>::Tx,
    pub rx: <I as Channels<S>>::Rx,
    #[cfg(feature = "spans")]
    pub span: tracing::Span,
}
```

The central message wrapper. Wraps a request type `I` with its typed channels for service `S`. Implements `Deref` to `I` for convenient field access.

**Construction** via tuple conversions:
- `(inner, tx, rx)` → full channels
- `(inner, tx)` → when `Rx = NoReceiver` (most common for RPC/server-streaming)
- `(inner,)` → when `Tx = NoSender, Rx = NoReceiver` (notify)

### `Client<S>`

```rust
#[derive(Debug)]
pub struct Client<S: Service>(ClientInner<S::Message>, PhantomData<S>);
```

The primary client type. Generic over a service `S`. Can be either local or remote.

**Construction:**
- `Client::local(mpsc_sender)` — from a tokio mpsc sender
- `Client::noq(endpoint, addr)` — from a noq endpoint + address (rpc feature)
- `Client::boxed(remote_connection)` — from any `RemoteConnection` impl

**Key methods** (all handle both local and remote transparently):

| Method | Pattern | Tx Type | Rx Type |
|---|---|---|---|
| `rpc()` | Unary RPC | `oneshot::Sender<Res>` | `NoReceiver` |
| `server_streaming()` | Server streaming | `mpsc::Sender<Res>` | `NoReceiver` |
| `client_streaming()` | Client streaming | `oneshot::Sender<Res>` | `mpsc::Receiver<Update>` |
| `bidi_streaming()` | Bidirectional | `mpsc::Sender<Res>` | `mpsc::Receiver<Update>` |
| `notify()` | Fire-and-forget | `NoSender` | `NoReceiver` |
| `rpc_0rtt()` | 0-RTT unary | `oneshot::Sender<Res>` | `NoReceiver` |
| `server_streaming_0rtt()` | 0-RTT server streaming | `mpsc::Sender<Res>` | `NoReceiver` |
| `notify_0rtt()` | 0-RTT fire-and-forget | `NoSender` | `NoReceiver` |

Each method creates the appropriate channel pair, wraps the message into `WithChannels`, and sends it.

### `LocalSender<S>`

```rust
#[repr(transparent)]
pub struct LocalSender<S: Service>(crate::channel::mpsc::Sender<S::Message>);
```

A thin wrapper around `mpsc::Sender<S::Message>` for sending messages to a local actor. Provides:

```rust
impl<S: Service> LocalSender<S> {
    pub fn send<T>(&self, value: impl Into<WithChannels<T, S>>) -> impl Future<Output = Result<(), SendError>>
    where
        T: Channels<S>,
        S::Message: From<WithChannels<T, S>>;

    pub fn send_raw(&self, value: S::Message) -> impl Future<Output = Result<(), SendError>>;
}
```

### `Request<L, R>`

```rust
pub enum Request<L, R> {
    Local(L),
    Remote(R),
}
```

A generic enum distinguishing local vs remote requests. `Client::request()` returns `Request<LocalSender<S>, RemoteSender<S>>`.

### `RemoteSender<S>` (rpc feature)

```rust
pub struct RemoteSender<S>(noq::SendStream, noq::RecvStream, PhantomData<S>);
```

Holds a QUIC stream pair after opening a bidirectional stream. The `write()` method serializes the protocol message with postcard + varint length prefix and sends it over the send stream.

### `Handler<R>` (rpc feature)

```rust
pub type Handler<R> = Arc<
    dyn Fn(R, noq::RecvStream, noq::SendStream) -> BoxFuture<Result<(), SendError>>
        + Send + Sync + 'static,
>;
```

A shared handler function that processes incoming remote requests. Typically created via `Protocol::remote_handler(local_sender)`.

## Error Types

### `RequestError`

```rust
pub enum RequestError {
    Connect { source: noq::ConnectError },   // Connection establishment failed
    Connection { source: noq::ConnectionError }, // Stream open failed
    Other { source: AnyError },               // Generic error for non-noq transports
}
```

### `SendError` (in `channel` module)

```rust
pub enum SendError {
    ReceiverClosed,                    // Local: receiver dropped
    MaxMessageSizeExceeded,            // Remote: message > 16 MiB
    Io { source: io::Error },          // Remote: network/serialization error
}
```

### `RecvError` (oneshot and mpsc variants)

```rust
// oneshot::RecvError
pub enum RecvError {
    SenderClosed,                      // Local: sender dropped
    MaxMessageSizeExceeded,            // Remote: message > 16 MiB
    Io { source: io::Error },          // Remote: network/deserialization error
}

// mpsc::RecvError
pub enum RecvError {
    MaxMessageSizeExceeded,            // Remote: message > 16 MiB
    Io { source: io::Error },          // Remote: network/deserialization error
}
```

Note: `mpsc::RecvError` does **not** have `SenderClosed` — mpsc receivers return `Ok(None)` when the sender is dropped.

### `WriteError` (rpc feature)

```rust
pub enum WriteError {
    Noq { source: noq::WriteError },   // QUIC stream write error
    MaxMessageSizeExceeded,            // Message > 16 MiB
    Io { source: io::Error },           // Serialization error
}
```

### `Error` (top-level umbrella)

```rust
pub enum Error {
    Request { source: RequestError },
    Send { source: SendError },
    MpscRecv { source: mpsc::RecvError },
    OneshotRecv { source: oneshot::RecvError },
    Write { source: rpc::WriteError },  // rpc feature only
}
```

All error types implement `From<Error>` for `io::Error`, allowing integration with `?` in `io::Result` contexts.