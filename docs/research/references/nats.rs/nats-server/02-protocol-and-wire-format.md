# NATS Client Protocol and Wire Format

**Protocol**: NATS Client Protocol v1 (with dynamic reconfiguration)  
**Transport**: TCP (port 4222), TLS, WebSocket (ws/wss)

## Protocol Overview

The NATS client-server protocol is a simple, text-based protocol with binary payload support. All operations are terminated with `\r\n`. Messages carry their payload length, allowing efficient binary data transfer.

### Connection Lifecycle

```
Client                                         Server
  │                                               │
  │◄──────────── INFO {json} ────────────────────│  Server sends INFO first
  │                                               │
  │────────────── CONNECT {json} ────────────────►│  Client sends CONNECT
  │────────────── PING ──────────────────────────►│  Client sends PING
  │◄──────────── PONG ──────────────────────────  │  Server confirms connection
  │                                               │
  │──── SUB/UNSUB/PUB/HPUB ──────────────────────►│  Normal operation
  │◄─── MSG/HMSG/+OK/-ERR/PING ─────────────────│
  │                                               │
```

## Server Operations (ServerOp)

These are operations received from the server. The `Connection` module parses these from the read buffer.

### INFO

Sent by the server upon connection and asynchronously when cluster topology changes.

```
INFO {json}\r\n
```

JSON fields (see `ServerInfo` struct):

| Field | Type | Description |
|-------|------|-------------|
| `server_id` | String | Unique server identifier |
| `server_name` | String | Generated server name |
| `host` | String | Cluster host |
| `port` | u16 | Cluster port |
| `version` | String | Server version |
| `auth_required` | bool | Authentication required |
| `tls_required` | bool | TLS required |
| `max_payload` | usize | Maximum payload size |
| `proto` | i8 | Protocol version (0 or 1) |
| `client_id` | u64 | Server-assigned client ID |
| `go` | String | Go build version |
| `nonce` | String | Nonce for nkey auth |
| `connect_urls` | Vec<String> | Cluster server URLs |
| `client_ip` | String | Client IP as seen by server |
| `headers` | bool | Server supports headers |
| `ldm` | bool | Lame duck mode |
| `cluster` | Option<String> | Cluster name |
| `domain` | Option<String> | NATS domain |
| `jetstream` | bool | JetStream enabled |

### MSG

Delivers a message to a subscription (no headers):

```
MSG <subject> <sid> [reply-to] <#bytes>\r\n
<payload>\r\n
```

### HMSG

Delivers a message with headers:

```
HMSG <subject> <sid> [reply-to] <#header-bytes> <#total-bytes>\r\n
<NATS/1.0 [status] [description]>\r\n
<header-name>: <header-value>\r\n
\r\n
<payload>\r\n
```

Header format follows the NATS/1.0 header spec:
- First line: `NATS/1.0` optionally followed by status code and description
- Subsequent lines: `name: value` headers
- Empty line separates headers from payload
- Header values may span multiple lines (continuation lines start with whitespace)

### PING / PONG

```
PING\r\n    →  Client responds with PONG
PONG\r\n    →  Acknowledges client's PING
```

### +OK / -ERR

```
+OK\r\n                 →  Success acknowledgment (verbose mode)
-ERR <description>\r\n  →  Error from server
```

Common server errors:
- `authorization violation` → parsed as `ServerError::AuthorizationViolation`
- Other strings → `ServerError::Other(String)`

## Client Operations (ClientOp)

These are operations sent from the client to the server. The `Connection` module serializes these to the write buffer.

### CONNECT

Sent as the first client operation after receiving INFO. Contains authentication and capability information.

```
CONNECT {json}\r\n
```

JSON fields (see `ConnectInfo` struct):

| Field | Type | Description |
|-------|------|-------------|
| `verbose` | bool | Enable +OK acknowledgments (always false in this client) |
| `pedantic` | bool | Strict format checking (always false) |
| `jwt` | Option<String> | User JWT for auth |
| `nkey` | Option<String> | Public nkey for auth |
| `sig` | Option<String> | Signed nonce (Base64URL encoded) |
| `name` | Option<String> | Client name |
| `echo` | bool | Whether server should echo messages back |
| `lang` | String | Implementation language ("rust") |
| `version` | String | Client version |
| `protocol` | u8 | Protocol version (1 = dynamic) |
| `tls_required` | bool | TLS required |
| `user` | Option<String> | Username |
| `pass` | Option<String> | Password |
| `auth_token` | Option<String> | Auth token |
| `headers` | bool | Client supports headers (always true) |
| `no_responders` | bool | Client supports no-responders (always true) |

### PUB / HPUB

Publish a message:

```
PUB <subject> [reply-to] <#payload-bytes>\r\n
<payload>\r\n
```

Publish with headers:

```
HPUB <subject> [reply-to] <#header-bytes> <#total-bytes>\r\n
<NATS/1.0\r\n
<header-name>: <header-value>\r\n
\r\n
<payload>\r\n
```

### SUB

Subscribe to a subject:

```
SUB <subject> [queue-group] <sid>\r\n
```

### UNSUB

Unsubscribe from a subscription:

```
UNSUB <sid> [max]\r\n
```

The optional `max` parameter tells the server to auto-unsubscribe after receiving the specified number of messages.

### PING / PONG

```
PING\r\n    →  Health check / keepalive
PONG\r\n    →  Response to server PING
```

## Protocol Version

The `Protocol` enum has two variants:

| Value | Name | Description |
|-------|------|-------------|
| 0 | Original | Basic protocol |
| 1 | Dynamic | Supports async INFO for cluster topology changes, lame duck mode |

This client always sends `protocol: 1` (Dynamic), enabling:
- Asynchronous INFO messages with updated server lists
- Lame duck mode notifications
- Dynamic reconfiguration of cluster topology

## Wire Format Details

### Message Length Calculation

For plain `MSG`:
```
length = subject.len() + reply.map_or(0, |r| r.len()) + payload.len()
```

For `HMSG`:
```
length = subject.len() + reply.map_or(0, |r| r.len()) + header_len + payload.len()
```

Where `header_len` = serialized header bytes and `total_len` = `header_len + payload.len()`.

### Write Buffer Architecture

The `Connection` uses a two-tier write buffer:

1. **`flattened_writes`** (`BytesMut`) — for small writes (< 4096 bytes). Protocol headers, short commands, and small messages are flattened into this buffer for efficient sequential writing.

2. **`write_buf`** (`VecDeque<Bytes>`) — for large writes (>= 4096 bytes). Large payloads are appended as separate `Bytes` chunks. Supports vectored writes (`write_vectored`) when the underlying stream supports it, writing up to 64 chunks at once.

The soft limit for the total write buffer is 65,535 bytes (`SOFT_WRITE_BUF_LIMIT`). When exceeded, the `ConnectionHandler` stops processing new commands until the buffer drains.

### Read Buffer Architecture

The `Connection` uses a single `BytesMut` read buffer with configurable initial capacity (default 65,535 bytes). Protocol parsing uses `memchr::memmem::find` to locate CRLF delimiters efficiently. If a partial message is in the buffer, the parser returns `None` and waits for more data.

### Header Serialization

Headers are serialized in NATS/1.0 format:

```
NATS/1.0\r\n
Header-Name: Header-Value\r\n
Multi-Line-Header: value part 1\r\n
 continuation of value\r\n
Another-Header: another value\r\n
\r\n
```

The `HeaderMap::to_bytes()` method handles this serialization, using `httparse`-compatible line folding for multi-line values.

### Status Codes in Headers

NATS status codes are embedded in the `HMSG` header version line:

```
NATS/1.0 404 No Messages\r\n
NATS/1.0 408 Request Timeout\r\n
NATS/1.0 503 No Responders\r\n
```

Common codes used by the client:

| Code | Constant | Meaning |
|------|----------|---------|
| 100 | `IDLE_HEARTBEAT` | JetStream idle heartbeat |
| 200 | `OK` | Success |
| 404 | `NOT_FOUND` | Message/stream not found |
| 408 | `TIMEOUT` | Request timeout |
| 409 | `REQUEST_TERMINATED` | Request terminated |
| 503 | `NO_RESPONDERS` | No responders available |

## Protocol Parsing Implementation

The `Connection::try_read_op()` method handles all protocol parsing:

1. Search for `\r\n` delimiter using `memchr::memmem::find`
2. Match the operation prefix:
   - `+OK` → `ServerOp::Ok`
   - `PING` → `ServerOp::Ping`
   - `PONG` → `ServerOp::Pong`
   - `-ERR` → parse error description → `ServerOp::Error`
   - `INFO ` → parse JSON → `ServerOp::Info`
   - `MSG ` → parse subject/sid/reply/length, read payload → `ServerOp::Message`
   - `HMSG ` → parse headers + payload → `ServerOp::Message`
3. Unknown prefix → return `io::Error` with `InvalidInput`

For `MSG` and `HMSG`, if the complete payload isn't yet in the read buffer (checked via `len + payload_len + 4 > remaining`), the method returns `Ok(None)` and the buffer accumulates more data before retrying.

Non-UTF8 subjects in server messages are handled gracefully — the parser returns an `io::Error` rather than panicking, which is critical because the Go server does not enforce UTF-8 in subjects (regression fix for issue #1572).