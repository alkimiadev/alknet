# irpc: Protocol and Message Flow

## Wire Protocol

When the `rpc` feature is enabled, irpc uses the following wire format over QUIC streams:

### Message Framing

Every message on the wire is **length-prefixed using postcard varints** (LEB128 encoding):

```
┌─────────────────┬──────────────────────┐
│ varint length   │ postcard-serialized  │
│ (1-10 bytes)    │ message data          │
└─────────────────┴──────────────────────┘
```

- **Length prefix**: LEB128 varint encoding of `u64` length. Each byte uses 7 bits for the value and the MSB as a continuation bit. Maximum 10 bytes for a full `u64`.
- **Payload**: Postcard-encoded (compact, no-schema serde format) Rust message.

### Maximum Message Size

`MAX_MESSAGE_SIZE = 16 MiB (16 * 1024 * 1024)`

Messages exceeding this limit are rejected:
- **Send side**: The sender checks `postcard::experimental::serialized_size()` before sending. If exceeded, the stream is reset with error code `1` (`ERROR_CODE_MAX_MESSAGE_SIZE_EXCEEDED`).
- **Receive side**: After reading the varint length, if it exceeds `MAX_MESSAGE_SIZE`, the stream is stopped with error code `1`.

### Error Codes

| Code | Constant | Meaning |
|---|---|---|
| `1` | `ERROR_CODE_MAX_MESSAGE_SIZE_EXCEEDED` | Message larger than 16 MiB |
| `2` | `ERROR_CODE_INVALID_POSTCARD` | Postcard serialization failed |

These are used as QUIC stream reset/stop error codes.

### Connection Closure

Error code `0` on the QUIC connection means "clean close" — the remote side intentionally shut down. This is distinguished from actual errors.

## Message Flow: Local Path

```
Client                              Actor
  │                                    │
  │  Client::rpc(Get { key: "x" })    │
  │                                    │
  │  1. Create oneshot channel pair    │
  │     (tx, rx) = oneshot::channel() │
  │                                    │
  │  2. Wrap into WithChannels         │
  │     WithChannels {                 │
  │       inner: Get { key: "x" },     │
  │       tx: oneshot::Sender<Res>,    │
  │       rx: NoReceiver,              │
  │       span: current_span,          │
  │     }                              │
  │                                    │
  │  3. Convert to Message enum         │
  │     StorageMessage::Get(wc)        │
  │                                    │
  │  4. Send over mpsc channel ────────►│
  │                                    │
  │  5. Await on oneshot receiver       │
  │     rx.await ◄─────────────────────│
  │                          tx.send(res)│
  │                                    │
  │  Result: res                       │
```

For bidirectional streaming:
```
Client                              Actor
  │                                    │
  │  Client::bidi_streaming(Sum, 4, 4) │
  │                                    │
  │  1. Create channel pairs           │
  │     (update_tx, update_rx)          │
  │     (res_tx, res_rx)               │
  │                                    │
  │  2. WithChannels {                  │
  │       inner: Sum,                  │
  │       tx: mpsc::Sender<i64>,       │
  │       rx: mpsc::Receiver<i64>,     │
  │     }                              │
  │                                    │
  │  3. Send message ──────────────────►│
  │                                    │
  │  4. Use update_tx.send(val) ───────►│
  │     Use res_rx.recv()    ◄─────────│
  │                          res_tx.send(val)
  │                                    │
```

## Message Flow: Remote Path

```
Client                              Server
  │                                    │
  │  Client::rpc(Get { key: "x" })    │
  │                                    │
  │  1. open_bi() → (SendStream, RecvStream)
  │                                    │
  │  2. Serialize StorageProtocol::Get(Get { key: "x" })
  │     with postcard + varint prefix  │
  │                                    │
  │  3. Write to SendStream ───────────►│
  │                                    │
  │                                    │  4. Accept bi stream
  │                                    │  5. Read varint + deserialize
  │                                    │  6. RemoteService::with_remote_channels()
  │                                    │     → WithChannels { inner, tx, rx }
  │                                    │  7. Forward to local actor
  │                                    │
  │                                    │  Actor processes, sends response
  │                                    │  on the SendStream (which is the
  │                                    │  oneshot::Sender<T> backed by QUIC)
  │                                    │
  │  8. Read from RecvStream ◄──────────│
  │  9. Deserialize response           │
  │                                    │
  │  Result: res                       │
```

For bidirectional streaming over remote:
```
Client                              Server
  │                                    │
  │  Client::bidi_streaming(Sum, 4, 4) │
  │                                    │
  │  open_bi() → (SendStream, RecvStream)
  │                                    │
  │  SendStream → mpsc::Sender<Update> │  RecvStream → mpsc::Receiver<Update>
  │  RecvStream → oneshot::Receiver<Res>│  SendStream → oneshot::Sender<Res>
  │  (or mpsc::Receiver<Res> for       │
  │   server-streaming with mpsc tx)   │
  │                                    │
  │  The initial message is sent on    │
  │  SendStream with varint prefix.    │
  │                                    │
  │  Subsequent updates are sent on    │
  │  the same SendStream as varint-    │
  │  prefixed postcard messages.       │
  │                                    │
  │  The response stream is read from  │
  │  the RecvStream as varint-prefixed │
  │  postcard messages.               │
```

## Stream Direction Convention

In irpc's QUIC stream model:
- **Client opens** a bidirectional stream (`open_bi()`)
- **SendStream** (client → server): carries the initial request message, plus any client-streaming updates
- **RecvStream** (server → client): carries the response(s) from the server

The `RemoteService::with_remote_channels()` method decides how to map streams to channels:

```rust
// For a simple RPC (tx=oneshot, rx=none):
fn with_remote_channels(self, rx: RecvStream, tx: SendStream) -> Self::Message {
    // rx stream is unused (NoReceiver), tx carries response
    WithChannels::from((msg, tx.into(), rx.into()))
    // tx → oneshot::Sender<Res> (or mpsc::Sender<Res>)
    // rx → NoReceiver
}
```

Wait — looking at the actual implementation more carefully:

The `RemoteService::with_remote_channels` method takes `(self, rx: RecvStream, tx: SendStream)` where:
- `rx` = the `RecvStream` from the bidirectional stream (client reads from this)
- `tx` = the `SendStream` from the bidirectional stream (client writes to this)

But for the **server side**, the `RecvStream` is what the server reads from (client updates), and `SendStream` is what the server writes to (server responses).

In the `with_remote_channels` generated code:
```rust
// For rpc(tx=oneshot::Sender<Res>, rx=mpsc::Receiver<Update>):
WithChannels::from((msg, tx.into(), rx.into()))
// tx (SendStream) → oneshot::Sender<Res> — server writes response
// rx (RecvStream) → mpsc::Receiver<Update> — server reads client updates
```

So the naming in `with_remote_channels` is from the **server's perspective**:
- `rx` parameter = RecvStream = what server receives (client → server updates)
- `tx` parameter = SendStream = what server sends (server → client responses)

## Connection Management

### NoqLazyRemoteConnection

```rust
struct NoqLazyRemoteConnection(Arc<NoqLazyRemoteConnectionInner>);

struct NoqLazyRemoteConnectionInner {
    endpoint: noq::Endpoint,
    addr: SocketAddr,
    connection: Mutex<Option<noq::Connection>>,
}
```

- Lazily establishes connection on first use
- Caches the `noq::Connection` inside a `Mutex<Option<...>>`
- On `open_bi()`: if cached connection exists, tries to reuse it; if it fails, clears cache and reconnects once
- Thread-safe via `Arc` + `Mutex`

### IrohLazyRemoteConnection (irpc-iroh)

Same pattern but for iroh endpoints, with an additional `alpn` field for protocol identification.

### 0-RTT Support

irpc supports QUIC 0-RTT for reduced latency on reconnections:

- `Client::rpc_0rtt()` — sends request immediately with 0-RTT data; if the server rejects 0-RTT, re-sends
- `Client::server_streaming_0rtt()` — same for server-streaming
- `Client::notify_0rtt()` — same for fire-and-forget

The 0-RTT flow:
1. Client serializes the message into a buffer (`prepare_write()`)
2. Sends the buffer over a 0-RTT connection
3. Awaits `zero_rtt_accepted()` to check if 0-RTT was accepted
4. If not accepted, opens a new connection and re-sends the same buffer

`RemoteConnection::zero_rtt_accepted()` returns `true` for regular connections and for lazy connections. For `IrohZrttRemoteConnection`, it checks the actual 0-RTT status via `handshake_completed()`.

## Server-Side: Accepting Connections

### Using noq (direct QUIC)

```rust
irpc::rpc::listen(endpoint, handler)
```

This function:
1. Loops on `endpoint.accept()` to accept incoming connections
2. For each connection, spawns a task running `handle_connection()`
3. `handle_connection()` loops on `read_request_raw()` to read requests from bidirectional streams
4. Each request is deserialized and passed to the `Handler`

### Using iroh

```rust
IrohProtocol::with_sender(local_sender)
```

This creates a `ProtocolHandler` that can be registered with `iroh::protocol::Router`. When a connection arrives, it calls `handle_connection()` from irpc-iroh, which handles the protocol handshake and reads requests.

For 0-RTT support:
```rust
Iroh0RttProtocol::with_sender(local_sender)
```

This implements `ProtocolHandler::on_accepting()` to handle 0-RTT connections.

### Handler Function

```rust
type Handler<R> = Arc<
    dyn Fn(R, noq::RecvStream, noq::SendStream) -> BoxFuture<Result<(), SendError>>
        + Send + Sync + 'static,
>;
```

The handler receives:
1. The deserialized protocol message (`R`)
2. The `RecvStream` (for client → server updates)
3. The `SendStream` (for server → client responses)

Typically created via `Protocol::remote_handler(local_sender)`, which converts streams to typed channels and forwards the `WithChannels` message to a local actor.