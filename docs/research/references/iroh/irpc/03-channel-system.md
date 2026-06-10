# irpc: Channel System

The channel system is the heart of irpc. It provides channel types that abstract over local (tokio) and remote (QUIC stream) communication, with the same API surface regardless of transport.

## Channel Kinds

irpc provides three kinds of channels, each with local and remote variants:

### Oneshot Channels (`channel::oneshot`)

Single-value, single-use channels for RPC responses.

| Type | Local Backend | Remote Backend |
|---|---|---|
| `oneshot::Sender<T>` | `tokio::sync::oneshot::Sender` | `BoxedSender<T>` (FnOnce over QUIC write) |
| `oneshot::Receiver<T>` | `FusedOneshotReceiver<T>` | `BoxedReceiver<T>` (boxed future over QUIC read) |

**Creation:** `oneshot::channel::<T>()` returns `(Sender<T>, Receiver<T>)`

**Sender behavior:**
- Local: `send(value)` is synchronous-ish, fails only if receiver dropped
- Remote: `send(value)` is async — serializes with postcard, length-prefixes with varint, writes to QUIC stream

**Receiver behavior:**
- Implements `Future<Output = Result<T, RecvError>>`
- Local: resolves to the value or `SenderClosed` error
- Remote: reads varint length prefix, reads that many bytes, deserializes with postcard

**Filtering/Mapping** (on `Sender<T>` where `T: Send + Sync + 'static`):
```rust
sender.with_filter(|v| v > 0)     // Drop messages failing predicate
sender.with_map(|v: U| v.into())  // Transform before sending
sender.with_filter_map(|v| ...)   // Combined filter + map
```

### MPSC Channels (`channel::mpsc`)

Multi-producer, single-consumer streaming channels for server-streaming, client-streaming, and bidirectional patterns.

| Type | Local Backend | Remote Backend |
|---|---|---|
| `mpsc::Sender<T>` | `tokio::sync::mpsc::Sender` | `Arc<DynSender<T>>` (NoqSender) |
| `mpsc::Receiver<T>` | `tokio::sync::mpsc::Receiver` | `Box<dyn DynReceiver<T>>` (NoqReceiver) |

**Creation:** `mpsc::channel::<T>(buffer)` returns `(Sender<T>, Receiver<T>)`

**Sender behavior:**
- `send(value).await` — sends, yielding if full (remote: serializes + writes to stream)
- `try_send(value).await` — non-blocking attempt; returns `Ok(false)` if would block
- `closed().await` — waits until all receivers are dropped
- `is_rpc()` — returns `true` for remote senders

**Receiver behavior:**
- `recv().await` → `Result<Option<T>, RecvError>` — `None` means sender closed/cleanly finished
- `filter(pred)`, `map(fn)`, `filter_map(fn)` — chainable transformations
- `into_stream()` (with `stream` feature) — converts to `Stream<Item = Result<T, RecvError>>`

**Cloning:** `mpsc::Sender<T>` implements `Clone`. Local senders clone the underlying tokio sender; remote senders clone the `Arc`.

### None Channels (`channel::none`)

Placeholder channels for when no communication is needed.

```rust
pub struct NoSender;  // Implements Sender, does nothing
pub struct NoReceiver; // Implements Receiver, does nothing
```

Used as defaults when `#[rpc(tx=...)]` or `#[rpc(rx=...)]` are omitted.

## Remote Channel Internals

### NoqSender<T>

```rust
struct NoqSender<T>(tokio::sync::Mutex<NoqSenderState<T>>);

enum NoqSenderState<T> {
    Open(NoqSenderInner<T>),
    Closed,
}

struct NoqSenderInner<T> {
    send: noq::SendStream,
    buffer: SmallVec<[u8; 128]>,  // Stack-allocated buffer for small messages
    _marker: PhantomData<T>,
}
```

Key behaviors:
- **Mutex-protected state**: The inner state is `Mutex`-protected because `DynSender::send()` takes `&self`. When a send fails, the state transitions to `Closed` and all subsequent sends return `BrokenPipe`.
- **Buffer reuse**: Uses `SmallVec<[u8; 128]>` to avoid heap allocation for messages that serialize to ≤128 bytes.
- **Serialization**: Each message is postcard-serialized with a varint length prefix. If serialization exceeds `MAX_MESSAGE_SIZE` (16 MiB), the stream is reset with error code `ERROR_CODE_MAX_MESSAGE_SIZE_EXCEEDED` (1).
- **Serialization errors**: If postcard serialization fails, the stream is reset with `ERROR_CODE_INVALID_POSTCARD` (2).

### NoqReceiver<T>

```rust
struct NoqReceiver<T> {
    recv: noq::RecvStream,
    _marker: PhantomData<T>,
}
```

Reads a varint length prefix, allocates a buffer of that size, reads the data, and deserializes with postcard. If the length exceeds `MAX_MESSAGE_SIZE`, stops the stream with the appropriate error code.

### Oneshot Remote Sender

For `oneshot::Sender<T>` over QUIC, the sender is a `BoxedSender<T>` — a `Box<dyn FnOnce(T) -> BoxFuture<Result<(), SendError>>>`. This captures the `noq::SendStream` and on invocation:
1. Computes `postcard::experimental::serialized_size(&value)`
2. Checks against `MAX_MESSAGE_SIZE`
3. Writes length-prefixed postcard data to the stream

### Oneshot Remote Receiver

For `oneshot::Receiver<T>` over QUIC, the receiver is constructed from a `noq::RecvStream`:
1. Reads a varint length prefix
2. Reads that many bytes
3. Deserializes with postcard
4. Returns the value

## Channel Conversion Table

When a QUIC stream pair `(SendStream, RecvStream)` is received for a request:

| Channel Kind | `Tx` (SendStream →) | `Rx` (RecvStream →) |
|---|---|---|
| `oneshot::Sender<T>` | Serialize + write, then finish | Read length-prefixed data |
| `mpsc::Sender<T>` | Repeatedly serialize + write | N/A |
| `oneshot::Receiver<T>` | N/A | Read single length-prefixed value |
| `mpsc::Receiver<T>` | N/A | Repeatedly read length-prefixed values |
| `NoSender` | Drop the stream | N/A |
| `NoReceiver` | N/A | Drop the stream |

The `From<noq::RecvStream>` and `From<noq::SendStream>` impls handle these conversions automatically based on the target type.

## DynSender and DynReceiver Traits

The `mpsc` module exposes traits for dynamic dispatch:

```rust
pub trait DynSender<T>: Debug + Send + Sync + 'static {
    fn send(&self, value: T) -> Pin<Box<dyn Future<Output = Result<(), SendError>> + Send + '_>>;
    fn try_send(&self, value: T) -> Pin<Box<dyn Future<Output = Result<bool, SendError>> + Send + '_>>;
    fn closed(&self) -> Pin<Box<dyn Future<Output = ()> + Send + Sync + '_>>;
    fn is_rpc(&self) -> bool;
}

pub trait DynReceiver<T>: Debug + Send + Sync + 'static {
    fn recv(&mut self) -> Pin<Box<dyn Future<Output = Result<Option<T>, RecvError>> + Send + Sync + '_>>;
}
```

These enable boxing of remote senders/receivers while keeping the local variants unboxed for zero overhead.

## FusedOneshotReceiver

A thin wrapper around `tokio::sync::oneshot::Receiver` that prevents panics when polling an already-completed receiver. It tracks completion state and returns `Poll::Pending` indefinitely after resolution, matching the `FusedFuture` pattern.

## Cancellation Safety

For remote `mpsc::Sender`:
- If a `send()` future is dropped before completion, the underlying QUIC stream is closed.
- All clones of the sender will receive `SendError::Io(BrokenPipe)` on subsequent send attempts.
- This is documented behavior: **always poll send futures to completion if you want to reuse the sender**.

For remote `oneshot::Sender`:
- Since it's `FnOnce`, dropping the future before sending simply means the value is never sent. The receiver will get `SenderClosed`.