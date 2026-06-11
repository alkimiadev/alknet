# async-nats: Key Types & Traits

## Core Types

### `Client`

The primary handle to a NATS connection. Cheaply cloneable (wraps `mpsc::Sender<Command>`).

```rust
#[derive(Clone, Debug)]
pub struct Client {
    info: tokio::sync::watch::Receiver<Option<ServerInfo>>,
    state: tokio::sync::watch::Receiver<State>,
    sender: mpsc::Sender<Command>,
    poll_sender: PollSender<Command>,
    next_subscription_id: Arc<AtomicU64>,
    subscription_capacity: usize,
    inbox_prefix: Arc<str>,
    request_timeout: Option<Duration>,
    max_payload: Arc<AtomicUsize>,
    connection_stats: Arc<Statistics>,
    skip_subject_validation: bool,
}
```

**Key methods**:
- `publish(subject, payload)` — fire-and-forget publish
- `publish_with_headers(subject, headers, payload)` — publish with NATS headers
- `publish_with_reply(subject, reply, payload)` — publish with reply-to subject
- `subscribe(subject)` → `Subscriber` — subscribe to a subject
- `queue_subscribe(subject, queue_group)` → `Subscriber` — queue group subscription
- `request(subject, payload)` → `Message` — request/reply with default timeout
- `send_request(subject, request)` → `Message` — request with custom `Request` builder
- `flush()` — wait until all buffered writes are flushed to the server
- `drain()` — drain all subscriptions, flush, then close
- `force_reconnect()` — force a reconnection (e.g., to re-trigger auth)
- `new_inbox()` — generate a unique inbox subject (`_INBOX.<id>`)
- `server_info()` → `ServerInfo` — last known server info
- `connection_state()` → `State` — `Pending`/`Connected`/`Disconnected`
- `statistics()` → `Arc<Statistics>` — connection statistics (bytes, messages, connects)
- `max_payload()` → `usize` — server's max payload size
- `set_server_pool(addrs)` — replace the server pool for reconnection
- `server_pool()` — snapshot of current server pool

### `Subscriber`

A `Stream` yielding `Message` values from a subscription.

```rust
#[derive(Debug)]
pub struct Subscriber {
    sid: u64,
    receiver: mpsc::Receiver<Message>,
    sender: mpsc::Sender<Command>,
}
```

Implements `futures_util::Stream<Item = Message>`. Methods:
- `unsubscribe()` — immediately unsubscribe
- `unsubscribe_after(n)` — unsubscribe after `n` total delivered messages
- `drain()` — unsubscribe after in-flight messages are delivered

**Drop behavior**: When a `Subscriber` is dropped, it spawns a task to send `Command::Unsubscribe` to the connection handler, ensuring the server is always notified.

### `Message`

An inbound NATS message:

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub subject: Subject,
    pub reply: Option<Subject>,
    pub payload: Bytes,
    pub headers: Option<HeaderMap>,
    pub status: Option<StatusCode>,
    pub description: Option<String>,
    pub length: usize,
}
```

### `OutboundMessage`

An outbound message for publishing (no status/description):

```rust
#[derive(Clone, Debug)]
pub struct OutboundMessage {
    pub subject: Subject,
    pub reply: Option<Subject>,
    pub payload: Bytes,
    pub headers: Option<HeaderMap>,
}
```

### `Request`

Builder for request/reply calls:

```rust
#[derive(Default)]
pub struct Request {
    pub payload: Option<Bytes>,
    pub headers: Option<HeaderMap>,
    pub timeout: Option<Option<Duration>>,
    pub inbox: Option<String>,
}
```

Builder methods: `payload()`, `headers()`, `timeout()`, `inbox()`. The `inbox` field, when set, bypasses the multiplexer and uses a dedicated subscription instead.

### `ServerInfo`

Server metadata received during connection handshake:

```rust
#[derive(Debug, Deserialize, Default, Clone, Eq, PartialEq)]
pub struct ServerInfo {
    pub server_id: String,
    pub server_name: String,
    pub host: String,
    pub port: u16,
    pub version: String,
    pub auth_required: bool,
    pub tls_required: bool,
    pub max_payload: usize,
    pub proto: i8,
    pub client_id: u64,
    pub go: String,
    pub nonce: String,
    pub connect_urls: Vec<String>,
    pub client_ip: String,
    pub headers: bool,
    pub lame_duck_mode: bool,
    pub cluster: Option<String>,
    pub domain: Option<String>,
    pub jetstream: bool,
}
```

### `ConnectInfo`

Client → server `CONNECT` message payload:

```rust
#[derive(Clone, Debug, Serialize)]
pub struct ConnectInfo {
    pub verbose: bool,
    pub pedantic: bool,
    pub user_jwt: Option<String>,
    pub nkey: Option<String>,
    pub signature: Option<String>,
    pub name: Option<String>,
    pub echo: bool,
    pub lang: String,
    pub version: String,
    pub protocol: Protocol,   // Original(0) or Dynamic(1)
    pub tls_required: bool,
    pub user: Option<String>,
    pub pass: Option<String>,
    pub auth_token: Option<String>,
    pub headers: bool,
    pub no_responders: bool,
}
```

The client always sets: `verbose=false`, `pedantic=false`, `lang="rust"`, `protocol=Dynamic`, `headers=true`, `no_responders=true`.

### `Statistics`

Atomic connection statistics (shared via `Arc`):

```rust
#[derive(Default, Debug)]
pub struct Statistics {
    pub in_bytes: AtomicU64,
    pub out_bytes: AtomicU64,
    pub in_messages: AtomicU64,
    pub out_messages: AtomicU64,
    pub connects: AtomicU64,
}
```

## Subject Types

### `Subject`

A validated NATS subject string (newtype over `String`):

```rust
// Usage:
let subject: Subject = "foo.bar.baz".into();
```

### `ToSubject` trait

Conversion trait for subjects:

```rust
pub trait ToSubject {
    fn to_subject(self) -> Result<Subject, SubjectError>;
}
```

Implemented for `&str`, `String`, `Subject` directly.

### `SubjectError`

```rust
pub enum SubjectError {
    InvalidFormat,
}
```

## Header Types

### `HeaderMap`

A multimap of header name → values:

```rust
pub struct HeaderMap {
    inner: VecMap<HeaderName, Vec<HeaderValue>>,
}
```

Methods: `insert()`, `append()`, `get()`, `len()`, `is_empty()`, `iter()`, `to_bytes()`.

### `HeaderName`

Case-insensitive header name. Created via `FromStr`:

```rust
let name: HeaderName = "Nats-Expected-Last-Subject-Sequence".parse()?;
```

### `HeaderValue`

Header value string. Created via `FromStr` or `From<u64>`:

```rust
let val: HeaderValue = "some value".parse()?;
let val: HeaderValue = HeaderValue::from(42u64);
```

## Server Address Types

### `ServerAddr`

Wraps a `url::Url` with NATS-specific validation. Supports schemes: `nats://`, `tls://`, `ws://`, `wss://`. Default port is `4222`.

```rust
let addr: ServerAddr = "demo.nats.io".parse()?;
let addr: ServerAddr = "nats://demo.nats.io:4222".parse()?;
let addr: ServerAddr = "tls://demo.nats.io".parse()?;
```

### `ToServerAddrs` trait

Flexible server address input (single URL, `Vec`, slice, etc.):

```rust
pub trait ToServerAddrs {
    type Iter: Iterator<Item = ServerAddr>;
    fn to_server_addrs(&self) -> io::Result<Self::Iter>;
}
```

### `Server`

Metadata about a server in the pool:

```rust
pub struct Server {
    pub addr: ServerAddr,
    pub failed_attempts: usize,
    pub did_connect: bool,
    pub is_discovered: bool,
    pub last_error: Option<String>,
}
```

## Event & State Types

### `Event`

Asynchronous notifications from the connection:

```rust
pub enum Event {
    Connected,
    Disconnected,
    LameDuckMode,
    Draining,
    Closed,
    SlowConsumer(u64),        // subscription sid
    ServerError(ServerError),
    ClientError(ClientError),
}
```

Received via `ConnectOptions::event_callback()`.

### `State`

Connection state observable via `watch::Receiver`:

```rust
pub enum State {
    Pending,
    Connected,
    Disconnected,
}
```

### `StatusCode`

NATS protocol status codes (e.g., `NO_RESPONDERS = 404`, `TIMEOUT = 408`).

## Error Types

All error types follow the pattern `Error<Kind>` from `crate::error`:

| Error Type | Kind | Used By |
|------------|------|---------|
| `ConnectError` | `ConnectErrorKind` | Connection establishment |
| `PublishError` | `PublishErrorKind` | Publish operations |
| `RequestError` | `RequestErrorKind` | Request/reply |
| `SubscribeError` | `SubscribeErrorKind` | Subscribe |
| `FlushError` | `FlushErrorKind` | Flush |
| `DrainError` | — | Drain |

### `ConnectErrorKind`

```rust
pub enum ConnectErrorKind {
    ServerParse,    // URL parsing failed
    Dns,           // DNS resolution failed
    Authentication, // Auth signing failed
    AuthorizationViolation, // Server rejected auth
    TimedOut,      // Connection handshake timeout
    Tls,           // TLS error
    Io,            // Other I/O error
    MaxReconnects,  // Exceeded max reconnect attempts
}
```

## Trait Definitions

The `client::traits` module defines abstract interfaces:

```rust
pub trait Publisher {
    fn publish_with_reply(&self, subject, reply, payload) -> Future<Output = Result<(), PublishError>>;
    fn publish_message(&self, msg: OutboundMessage) -> Future<Output = Result<(), PublishError>>;
}

pub trait Subscriber {
    fn subscribe(&self, subject) -> Future<Output = Result<crate::Subscriber, SubscribeError>>;
}

pub trait Requester {
    fn send_request(&self, subject, request: Request) -> Future<Output = Result<Message, RequestError>>;
}

pub trait TimeoutProvider {
    fn timeout(&self) -> Option<Duration>;
}
```

`Client` implements all of these. The JetStream `Context` also implements them via delegation.

## Authentication Types

### `Auth`

Container for all authentication methods:

```rust
pub struct Auth {
    pub jwt: Option<String>,
    pub nkey: Option<String>,
    pub signature_callback: Option<CallbackArg1<String, Result<String, AuthError>>>,
    pub signature: Option<Vec<u8>>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub token: Option<String>,
}
```

### `AuthError`

Simple string error for auth callback failures.

### `ReconnectToServer`

Returned by `reconnect_to_server_callback` to select a server and delay:

```rust
pub struct ReconnectToServer {
    pub addr: ServerAddr,
    pub delay: Option<Duration>,
}
```