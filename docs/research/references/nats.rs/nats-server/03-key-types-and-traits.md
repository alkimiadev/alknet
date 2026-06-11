# Key Types and Traits

This document covers the core data types in the `async-nats` crate that form the public API and internal plumbing.

## Public Types

### Client

**Location**: `client.rs`

`Client` is the primary user-facing type. It is a lightweight, cloneable handle to a NATS connection.

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

Key methods:
- `publish(subject, payload)` — fire-and-forget publish
- `publish_with_headers(subject, headers, payload)` — publish with NATS headers
- `publish_with_reply(subject, reply, payload)` — publish with reply subject
- `request(subject, payload)` — request-response (returns `Message`)
- `send_request(subject, request)` — request with `Request` builder
- `subscribe(subject)` — subscribe to a subject, returns `Subscriber`
- `queue_subscribe(subject, queue_group)` — subscribe as part of a queue group
- `flush()` — ensure all pending messages are written to the wire
- `drain()` — gracefully drain all subscriptions and close
- `force_reconnect()` — trigger immediate reconnection
- `new_inbox()` — generate a unique inbox subject for request-reply
- `server_info()` — get last received `ServerInfo`
- `max_payload()` — get server's maximum payload size
- `connection_state()` — get current connection `State`
- `statistics()` — get `Arc<Statistics>` for connection metrics
- `is_server_compatible(major, minor, patch)` — check server version compatibility
- `set_server_pool(addrs)` / `server_pool()` — manage server pool

`Client` also implements `Sink<OutboundMessage>` for backpressure-aware publishing.

### Subscriber

**Location**: `lib.rs`

A `Subscriber` receives messages from a single subscription. It implements `futures::Stream`.

```rust
#[derive(Debug)]
pub struct Subscriber {
    sid: u64,
    receiver: mpsc::Receiver<Message>,
    sender: mpsc::Sender<Command>,
}
```

Key methods:
- `unsubscribe()` — unsubscribe and close the stream
- `unsubscribe_after(max)` — auto-unsubscribe after N messages
- `drain()` — gracefully drain remaining messages then close

On `Drop`, `Subscriber` automatically sends an `Unsubscribe` command and closes the receiver channel.

### Message

**Location**: `message.rs`

Represents an inbound NATS message.

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

### OutboundMessage

**Location**: `message.rs`

Represents a message to be published. No status/description fields (those are inbound-only).

```rust
#[derive(Clone, Debug)]
pub struct OutboundMessage {
    pub subject: Subject,
    pub reply: Option<Subject>,
    pub payload: Bytes,
    pub headers: Option<HeaderMap>,
}
```

### Subject

**Location**: `subject.rs`

An immutable, validated UTF-8 string backed by `Bytes`. Used throughout the crate instead of raw `String` for subjects.

```rust
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Subject {
    bytes: Bytes,
}
```

Implements `Deref<Target = str>`, `From<&str>`, `From<String>`, `TryFrom<Bytes>`, `Serialize`, `Deserialize`.

Validation methods:
- `is_valid()` — checks NATS subject rules (no leading/trailing dots, no consecutive dots, no whitespace)
- `validated(s)` — construct with validation, returns `Result<Subject, SubjectError>`
- `from_static_validated(s)` — const-time validation for static strings (compile-time panic on invalid)

### ToSubject Trait

**Location**: `subject.rs`

```rust
pub trait ToSubject {
    fn to_subject(&self) -> Subject;
}
```

Implemented for `Subject`, `&'static str`, `String`. All methods accepting subjects are generic over `impl ToSubject`.

### HeaderMap

**Location**: `header.rs`

NATS message headers, modeled after the `http::header` crate.

```rust
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct HeaderMap {
    inner: HashMap<HeaderName, Vec<HeaderValue>>,
}
```

Supports multiple values per header name (like HTTP). Key methods:
- `insert(name, value)` — replace all values for a name
- `append(name, value)` — add a value to a name
- `get(name)` — get the first value
- `get_all(name)` — get all values as an iterator
- `len()` / `is_empty()` — number of header entries
- `to_bytes()` — serialize to NATS/1.0 wire format
- `wire_len()` — size in wire format (for payload size checks)

### StatusCode

**Location**: `status.rs`

NATS status codes (100-999), structurally similar to HTTP status codes.

```rust
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct StatusCode(NonZeroU16);
```

Constants:
| Constant | Code | Meaning |
|----------|------|---------|
| `IDLE_HEARTBEAT` | 100 | JetStream idle heartbeat |
| `OK` | 200 | Success |
| `NOT_FOUND` | 404 | Not found |
| `TIMEOUT` | 408 | Timeout |
| `REQUEST_TERMINATED` | 409 | Request terminated |
| `NO_RESPONDERS` | 503 | No responders |

### ServerInfo

**Location**: `lib.rs`

Deserialized from the server's `INFO` JSON message. Contains server capabilities, connection details, and cluster information.

### ConnectInfo

**Location**: `lib.rs`

Serialized into the client's `CONNECT` JSON message. Contains authentication credentials, client capabilities, and protocol preferences.

### ServerAddr

**Location**: `lib.rs`

A validated NATS server URL, supporting schemes `nats://`, `tls://`, `ws://`, `wss://`.

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServerAddr(Url);
```

Methods:
- `from_url(url)` — validate and create
- `tls_required()` — true for `tls://` scheme
- `is_websocket()` — true for `ws://` or `wss://`
- `host()` / `port()` / `scheme()` — URL component accessors
- `socket_addrs()` — async DNS resolution
- `username()` / `password()` — embedded credentials

### Auth

**Location**: `auth.rs`

Container for authentication credentials.

```rust
#[derive(Clone, Default)]
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

### Request

**Location**: `client.rs`

Builder for customized request-response operations.

```rust
#[derive(Default)]
pub struct Request {
    pub payload: Option<Bytes>,
    pub headers: Option<HeaderMap>,
    pub timeout: Option<Option<Duration>>,
    pub inbox: Option<String>,
}
```

### Statistics

**Location**: `client.rs`

Atomic connection statistics shared between Client and ConnectionHandler.

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

### Event

**Location**: `lib.rs`

Events emitted by the client for connection lifecycle monitoring.

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Connected,
    Disconnected,
    LameDuckMode,
    Draining,
    Closed,
    SlowConsumer(u64),
    ServerError(ServerError),
    ClientError(ClientError),
}
```

## Internal Types

### Command

**Location**: `lib.rs`

Internal commands sent from `Client` to `ConnectionHandler` via `mpsc` channel.

```rust
pub(crate) enum Command {
    Publish(OutboundMessage),
    Request { subject, payload, respond, headers, sender: oneshot::Sender<Message> },
    Subscribe { sid, subject, queue_group, sender: mpsc::Sender<Message> },
    Unsubscribe { sid, max: Option<u64> },
    Flush { observer: oneshot::Sender<()> },
    Drain { sid: Option<u64> },
    Reconnect,
    SetServerPool { servers: Vec<ServerAddr>, result: oneshot::Sender<Result<(), String>> },
    ServerPool { result: oneshot::Sender<Vec<connector::Server>> },
}
```

### ClientOp / ServerOp

**Location**: `lib.rs`

Protocol-level operation types used by `Connection` for wire format parsing and serialization.

### Subscription (Internal)

**Location**: `lib.rs`

```rust
struct Subscription {
    subject: Subject,
    sender: mpsc::Sender<Message>,
    queue_group: Option<String>,
    delivered: u64,
    max: Option<u64>,
}
```

### Multiplexer (Internal)

**Location**: `lib.rs`

```rust
struct Multiplexer {
    subject: Subject,     // Wildcard subscription subject (e.g., "_INBOX.xxx.*")
    prefix: Subject,      // Prefix for routing (e.g., "_INBOX.xxx.")
    senders: HashMap<String, oneshot::Sender<Message>>,  // token → sender
}
```

### Connection State

**Location**: `connection.rs`

```rust
#[derive(Debug, Eq, PartialEq, Clone)]
pub enum State {
    Pending,
    Connected,
    Disconnected,
}
```

### Protocol

**Location**: `lib.rs`

```rust
#[derive(Serialize_repr, Deserialize_repr, PartialEq, Eq, Debug, Clone, Copy)]
#[repr(u8)]
pub enum Protocol {
    Original = 0,
    Dynamic = 1,
}
```

## Error Type Pattern

The crate uses a generic `Error<Kind>` type throughout. Every subsystem defines its own `ErrorKind` enum and a type alias:

```rust
// Define the kind enum
#[derive(Clone, Debug, PartialEq)]
pub enum PublishErrorKind {
    MaxPayloadExceeded,
    InvalidSubject,
    Send,
}

// Define the error type alias
pub type PublishError = Error<PublishErrorKind>;

// Construct errors
PublishError::new(PublishErrorKind::MaxPayloadExceeded)
PublishError::with_source(PublishErrorKind::Send, io_error)

// Match on errors
if err.kind() == PublishErrorKind::MaxPayloadExceeded { ... }
```

Error kinds in the crate:

| Error Type | Kind Enum | Context |
|-----------|-----------|---------|
| `ConnectError` | `ConnectErrorKind` | Initial connection failures |
| `PublishError` | `PublishErrorKind` | Publish validation failures |
| `RequestError` | `RequestErrorKind` | Request-response failures |
| `SubscribeError` | `SubscribeErrorKind` | Subscription failures |
| `FlushError` | `FlushErrorKind` | Flush failures |
| `ServerPoolError` | `ServerPoolErrorKind` | Server pool query failures |
| `SetServerPoolError` | `SetServerPoolErrorKind` | Server pool modification failures |

## Trait Implementations

### Client Trait Interfaces

The `Client` implements several traits defined in `client::traits`:

```rust
// Publisher trait — publish with optional reply subject
trait Publisher {
    fn publish_with_reply<S, R>(&self, subject: S, reply: R, payload: Bytes) -> impl Future<Output = Result<(), PublishError>>;
    fn publish_message(&self, msg: OutboundMessage) -> impl Future<Output = Result<(), PublishError>>;
}

// Subscriber trait — subscribe to a subject
trait Subscriber {
    fn subscribe<S>(&self, subject: S) -> impl Future<Output = Result<crate::Subscriber, SubscribeError>>;
}

// Requester trait — send request-response
trait Requester {
    fn send_request<S>(&self, subject: S, request: Request) -> impl Future<Output = Result<Message, RequestError>>;
}

// TimeoutProvider trait — access request timeout
trait TimeoutProvider {
    fn timeout(&self) -> Option<Duration>;
}
```

### ToServerAddrs Trait

**Location**: `lib.rs`

Converts various address types into server address iterators. Implemented for `ServerAddr`, `str`, `String`, `&[T]`, `Vec<T>`, `&[ServerAddr]`, and references.

### Sink<OutboundMessage>

`Client` implements `futures::Sink<OutboundMessage>` for backpressure-aware publishing through the `PollSender` adapter.

### Stream for Subscriber

`Subscriber` implements `futures::Stream` with `Item = Message`, delegating to the internal `mpsc::Receiver`.