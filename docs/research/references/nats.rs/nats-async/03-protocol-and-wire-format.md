# async-nats: NATS Protocol & Wire Format

## Protocol Overview

NATS uses a simple, text-based protocol over TCP. Messages are terminated with `\r\n`. The protocol is symmetric for client and server operations.

### Client → Server Operations (`ClientOp`)

```rust
pub(crate) enum ClientOp {
    Publish { subject, payload, respond, headers },
    Subscribe { sid, subject, queue_group },
    Unsubscribe { sid, max },
    Ping,
    Pong,
    Connect(ConnectInfo),
}
```

### Server → Client Operations (`ServerOp`)

```rust
pub(crate) enum ServerOp {
    Ok,
    Info(Box<ServerInfo>),
    Ping,
    Pong,
    Error(ServerError),
    Message { sid, subject, reply, payload, headers, status, description, length },
}
```

## Wire Format: Client Operations

### CONNECT

Sent immediately after receiving the first `INFO` from the server:

```
CONNECT {"verbose":false,"pedantic":false,...}\r\n
```

The JSON payload is `ConnectInfo` serialized inline on the same line.

### PUB (Publish without headers)

```
PUB <subject> [reply-to] <payload-size>\r\n
<payload>\r\n
```

Example:
```
PUB events.data INBOX.67 11\r\n
Hello World\r\n
```

### HPUB (Publish with headers)

When headers are present and non-empty:

```
HPUB <subject> [reply-to] <header-size> <total-size>\r\n
<headers>\r\n
<payload>\r\n
```

The `<total-size>` = `<header-size>` + `<payload-size>`.

Header block format:
```
NATS/1.0\r\n
Header-Name: Header-Value\r\n
Another-Header: Another-Value\r\n
\r\n
```

The version line (`NATS/1.0`) may include a status code and description:
```
NATS/1.0 404 No Messages\r\n
\r\n
```

### SUB (Subscribe)

```
SUB <subject> [queue-group] <sid>\r\n
```

The `sid` (subscription ID) is a client-assigned u64, unique per connection.

### UNSUB (Unsubscribe)

```
UNSUB <sid> [max]\r\n
```

The optional `max` tells the server to auto-unsubscribe after `max` messages are delivered.

### PING / PONG

```
PING\r\n
PONG\r\n
```

Client sends PING periodically (default every 60s). If 2+ pings are pending without PONG, the connection is considered dead.

## Wire Format: Server Operations

### INFO

First message sent by the server on connection:

```
INFO {"server_id":"NATSxxx","version":"2.10"...}\r\n
```

Also sent asynchronously when cluster topology changes.

### MSG (Message without headers)

```
MSG <subject> <sid> [reply-to] <payload-size>\r\n
<payload>\r\n
```

### HMSG (Message with headers)

```
HMSG <subject> <sid> [reply-to] <header-size> <total-size>\r\n
<headers + payload>\r\n
```

### +OK / -ERR

```
+OK\r\n
-ERR <description>\r\n
```

Sent only when `verbose=true` in `CONNECT`. The client always sets `verbose=false`, so `+OK` is not expected.

## Protocol Parser

The `Connection` struct handles all protocol parsing and serialization:

### Read Path (`try_read_op`)

1. Search for `\r\n` in `read_buf` using `memchr::memmem::find`
2. Inspect the first bytes to determine the operation type:
   - `+OK` → `ServerOp::Ok`
   - `PING` → `ServerOp::Ping`
   - `PONG` → `ServerOp::Pong`
   - `-ERR` → `ServerOp::Error(...)` (description is `trim_matches('\'')`)
   - `INFO ` → `ServerOp::Info(...)` (serde_json deserialization)
   - `MSG ` → Parse subject/sid/reply/size, then read payload
   - `HMSG ` → Parse subject/sid/reply/header_len/total_len, then read headers + payload
3. For `MSG`/`HMSG`: if the full message body hasn't been read yet, return `None` (wait for more data)
4. For `HMSG`: parse the header block — extract version line (`NATS/1.0[ <status>[ <description>]]`), then key-value pairs (supports folded/multi-line header values)

### Write Path (`enqueue_write_op`)

Writes into a buffer strategy:
- **Small writes** (< 4096 bytes): flattened into `flattened_writes: BytesMut`
- **Large writes** (≥ 4096 bytes): appended as separate `Bytes` chunks in `write_buf: VecDeque<Bytes>`

This enables efficient vectored I/O when the underlying stream supports it.

### Write Flush Strategy

The `should_flush()` method returns:
- `Yes` — buffers empty but haven't flushed yet
- `May` — buffers not empty and haven't flushed
- `No` — already flushed or nothing to flush

The `ConnectionHandler` calls `poll_flush()` after processing commands, ensuring data is actually sent to the server.

## Vectored I/O

When `stream.is_write_vectored()` returns true, the connection uses `poll_write_vectored()` to write up to 64 `IoSlice`s at once. This is significantly more efficient for bursty publish patterns.

```rust
const WRITE_VECTORED_CHUNKS: usize = 64;
```

## WebSocket Transport

When the `websockets` feature is enabled, `WebSocketAdapter<T>` wraps `tokio_websockets::WebSocketStream<T>` to implement `AsyncRead + AsyncWrite`, making WebSocket connections transparent to the protocol layer.

```rust
#[cfg(feature = "websockets")]
pub(crate) struct WebSocketAdapter<T> {
    pub(crate) inner: WebSocketStream<T>,
    pub(crate) read_buf: BytesMut,
}
```

WebSocket connections use `ws://` or `wss://` scheme in the server URL. TLS for `wss://` is handled by the WebSocket library's built-in TLS support.

## Connection Lifecycle

### Initial Connection Flow

```
Client                                  Server
  │                                        │
  │──── TCP connect ────────────────────▶  │
  │◀──── INFO {server_id, nonce, ...} ─── │
  │──── CONNECT {auth, ...} ──────────▶  │
  │──── PING ─────────────────────────▶  │
  │◀──── PONG (or -ERR) ─────────────── │
  │                                        │
  │  [connected, ConnectionHandler runs]   │
```

If `tls_first` is enabled, TLS is established before reading INFO:

```
Client                                  Server
  │                                        │
  │──── TCP connect ────────────────────▶  │
  │──── TLS handshake ─────────────────▶  │
  │◀──── TLS handshake ──────────────── │
  │◀──── INFO {...} ──────────────────── │
  │──── CONNECT + PING ────────────────▶  │
  │◀──── PONG ────────────────────────── │
```

### Ping/Pong Keepalive

- Client sends PING every `ping_interval` (default 60s)
- Server responds with PONG
- If `pending_pings > MAX_PENDING_PINGS (2)`, connection is considered dead
- Any server operation resets the ping interval timer

### Reconnection Flow

On disconnect:
1. `handle_disconnect()` sends `Event::Disconnected` and sets state to `Disconnected`
2. `handle_reconnect()` calls `connector.connect()` which:
   - Shuffles servers (unless `retain_servers_order`)
   - Sorts by `failed_attempts` (ascending)
   - Iterates through servers with exponential backoff delay
   - On each server: DNS resolve → TCP connect → INFO → TLS (if needed) → CONNECT+PING → PONG
3. On success:
   - Sends `Event::Connected`, sets state to `Connected`
   - Removes closed subscriptions
   - Re-subscribes all active subscriptions (with adjusted `max = max - delivered`)
   - Re-subscribes the multiplexer (if active)
4. On failure with `MaxReconnects` reached, the handler loop exits

### Default Reconnect Delay

Exponential backoff capped at 4 seconds:

```rust
fn reconnect_delay_callback_default(attempts: usize) -> Duration {
    if attempts <= 1 {
        Duration::from_millis(0)
    } else {
        let exp: u32 = (attempts - 1).try_into().unwrap_or(u32::MAX);
        cmp::min(Duration::from_millis(2_u64.saturating_pow(exp)), Duration::from_secs(4))
    }
}
```

| Attempt | Delay |
|---------|-------|
| 1 | 0ms |
| 2 | 0ms |
| 3 | 2ms |
| 4 | 8ms |
| 5 | 32ms |
| 6 | 128ms |
| 7 | 512ms |
| 8 | 2048ms |
| 9+ | 4000ms (cap) |