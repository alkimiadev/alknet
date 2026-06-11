# JetStream Internals

This document covers the JetStream subsystem — how it provides stream-based messaging with persistence, consumer management, and higher-level APIs like KV and Object Store.

## JetStream Context

**Location**: `jetstream/context.rs`

The `Context` is the entry point to the JetStream API. It wraps a `Client` and provides stream management, publishing, and consumer operations.

```rust
#[derive(Debug, Clone)]
pub struct Context {
    pub(crate) client: Client,
    pub(crate) prefix: String,                       // API subject prefix (default: "$JS.API")
    pub(crate) timeout: Duration,                     // Default request timeout
    pub(crate) max_ack_semaphore: Arc<Semaphore>,      // Limits in-flight ack waits
    pub(crate) ack_sender: mpsc::Sender<(oneshot::Receiver<Message>, OwnedSemaphorePermit)>,
    pub(crate) backpressure_on_inflight: bool,
}
```

### Context Creation

```rust
// Default context (prefix = "$JS.API")
let jetstream = async_nats::jetstream::new(client);

// With domain (prefix = "$JS.hub.API")
let jetstream = async_nats::jetstream::with_domain(client, "hub");

// With custom prefix
let jetstream = async_nats::jetstream::with_prefix(client, "JS.acc@hub.API");

// Builder pattern for more options
let jetstream = async_nats::jetstream::Context::builder(client)
    .domain("hub")
    .prefix("$JS.API")
    .timeout(Duration::from_secs(30))
    .max_ack_pending(256)
    .backpressure_on_inflight(true)
    .build();
```

### JetStream API Subject Convention

All JetStream API calls are request-response messages sent to subjects following the pattern:

```
$JS.API.<operation>.<stream-name>[.<consumer-name>]
```

Examples:
- `$JS.API.STREAM.CREATE.events` — create stream "events"
- `$JS.API.STREAM.INFO.events` — get stream info
- `$JS.API.CONSUMER.DURABLE.CREATE.events.myconsumer` — create durable consumer
- `$JS.API.CONSUMER.MSG.NEXT.events.myconsumer` — pull next message

With a domain, the prefix changes to `$JS.<domain>.API`.

## Stream Management

**Location**: `jetstream/stream.rs`

### Stream Config

```rust
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub name: String,
    pub subjects: Vec<String>,         // Subject filter
    pub retention: RetentionPolicy,     // Limits, Interest, WorkQueue
    pub max_consumers: i32,
    pub max_messages: i64,             // Per-stream message limit
    pub max_messages_per_subject: i64,
    pub max_bytes: i64,                // Per-stream byte limit
    pub max_age: Duration,             // Message TTL
    pub max_message_size: Option<i32>, // Max individual message size
    pub storage: StorageType,          // File or Memory
    pub num_replicas: usize,
    pub no_ack: bool,                  // Don't require ack
    pub discard: DiscardPolicy,       // Old or New
    pub duplicate_window: Duration,
    pub allow_rollup_hdrs: bool,
    pub allow_direct: bool,
    pub mirror: Option<External>,
    pub sources: Vec<External>,
    pub sealed: bool,
    pub compression: Option<Compression>,  // server_2_10+
    pub first_sequence: Option<u64>,        // server_2_11+
    pub subject_transform: Option<SubjectTransform>,  // server_2_12+
    pub metadata: Option<HashMap<String, String>>,    // server_2_10+
    pub placement: Option<Placement>,
    pub republish: Option<RePublish>,
}
```

### Stream Operations

Via `Context`:

| Method | API Subject | Description |
|--------|------------|-------------|
| `create_stream(config)` | `STREAM.CREATE.<name>` | Create a new stream |
| `get_stream(name)` | `STREAM.INFO.<name>` | Get existing stream |
| `get_or_create_stream(config)` | `STREAM.INFO` → `STREAM.CREATE` | Get or create |
| `delete_stream(name)` | `STREAM.DELETE.<name>` | Delete a stream |
| `update_stream(name, config)` | `STREAM.UPDATE.<name>` | Update stream config |
| `purge_stream(name)` | `STREAM.PURGE.<name>` | Purge all messages |
| `streams()` | `STREAM.LIST` | List all streams (paged iterator) |
| `stream_names()` | `STREAM.NAMES` | List stream names (paged iterator) |
| `account_info()` | `ACCOUNT.INFO` | Get account info |

Via `Stream`:

| Method | API Subject | Description |
|--------|------------|-------------|
| `info()` | `STREAM.INFO.<name>` | Refresh stream info |
| `purge()` | `STREAM.PURGE.<name>` | Purge messages |
| `delete()` | `STREAM.DELETE.<name>` | Delete this stream |
| `update(config)` | `STREAM.UPDATE.<name>` | Update config |
| `get_raw_message(seq)` | `STREAM.MSG.GET.<name>` | Get message by sequence (stored mode) |
| `get_last_message(subject)` | `STREAM.MSG.GET.<name>` | Get last message for subject (stored mode) |
| `direct_get_last(subject)` | `DIRECT.GET.<name>` | Direct get last (bypasses RAA) |
| `direct_get(seq)` | `DIRECT.GET.<name>` | Direct get by sequence |
| `delete_message(seq)` | `STREAM.MSG.DELETE.<name>` | Delete a specific message |
| `create_consumer(config)` | `CONSUMER.CREATE.<stream>` | Create consumer |
| `get_or_create_consumer(name, config)` | `CONSUMER.DURABLE.CREATE.<stream>.<name>` | Get or create durable |
| `get_consumer(name)` | `CONSUMER.INFO.<stream>.<name>` | Get existing consumer |

### Stream Info

```rust
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Info {
    pub config: Config,
    pub created: DateTime,
    pub state: State,           // Messages, bytes, first/last sequence, consumer count
    pub cluster: Option<ClusterInfo>,
    pub timestamp: DateTime,
    pub leader: Option<String>,
    pub subjects: Option<HashMap<String, u64>>,  // Subject → message count
}
```

### Paged List Operations

Stream and consumer listing uses a paged iterator pattern:

```rust
// streams() returns an iterator that automatically pages
let mut streams = jetstream.streams();
while let Some(stream) = streams.next().await {
    let stream = stream?;
    // process stream
}

// stream_names() similarly pages
let mut names = jetstream.stream_names();
while let Some(name) = names.next().await {
    println!("{}", name?);
}
```

The paged iterator sends an initial request with `offset: 0` and continues fetching pages until no more results are returned.

## Publishing

**Location**: `jetstream/context.rs`, `jetstream/publish.rs`

### Publish

```rust
// Basic publish (fire-and-forget)
jetstream.publish("events.data", "payload".into()).await?;

// Publish with custom message builder
jetstream.publish_message(
    jetstream::message::PublishMessage::build()
        .payload("data".into())
        .message_id("unique-id")           // Nats-Msg-Id header for dedup
        .expected_last_message_id("prev")  // Nats-Expected-Last-Msg-Id
        .expected_last_sequence(42)        // Nats-Expected-Last-Sequence
        .expected_last_subject_sequence("events", 10)  // Per-subject sequence
        .header("Custom", "Value")
).await?;
```

### PublishAck

When a message is published to a JetStream stream, the server responds with a `PublishAck`:

```rust
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct PublishAck {
    pub stream: String,
    pub sequence: u64,
    pub domain: Option<String>,
    pub duplicate: bool,
}
```

### PublishAckFuture

Publishing returns a `PublishAckFuture` that resolves to `PublishAck`. The future uses a semaphore (`max_ack_semaphore`) to limit in-flight ack waits and prevent backpressure issues.

When `backpressure_on_inflight` is enabled, the publish operation blocks if there are too many pending acks, preventing the command channel from filling up with unbounded publish operations.

### Idempotent Publishing

Headers for exactly-once semantics:

| Header | Purpose |
|--------|---------|
| `Nats-Msg-Id` | Message ID for deduplication within the stream's duplicate window |
| `Nats-Expected-Last-Msg-Id` | Expected last message ID (conditional publish) |
| `Nats-Expected-Last-Sequence` | Expected last sequence number |
| `Nats-Expected-Last-Subject-Sequence` | Expected last sequence for a specific subject |

## Consumers

**Location**: `jetstream/consumer/`

### Consumer Types

| Type | Description |
|------|-------------|
| `PullConsumer` | Client pulls messages on demand |
| `PushConsumer` | Server pushes messages to a delivery subject |
| `OrderedConsumer` | Push consumer with automatic re-creation on failure |

### Consumer Config

```rust
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub name: Option<String>,
    pub durable_name: Option<String>,
    pub description: Option<String>,
    pub deliver_subject: Option<String>,    // Push consumers only
    pub ack_policy: AckPolicy,
    pub ack_wait: Duration,
    pub max_deliver: i64,
    pub max_ack_pending: i32,
    pub max_waiting: i32,                    // Pull consumers only
    pub filter_subject: Option<String>,
    pub replay_policy: ReplayPolicy,
    pub sample_frequency: Option<i8>,
    pub max_batch: i32,                      // Pull consumers
    pub max_expires: Duration,               // Pull consumers
    pub inactive_threshold: Duration,
    pub flow_control: bool,                  // Push consumers
    pub heartbeat: Option<Duration>,         // Push consumers
    pub backoff: Vec<Duration>,
    pub deliver_group: Option<String>,
    pub num_replicas: usize,
    pub mem_storage: bool,
    pub metadata: Option<HashMap<String, String>>,
    pub ack_markers: Option<Vec<String>>,    // server_2_12+
}
```

### Pull Consumer

**Location**: `jetstream/consumer/pull.rs`

Pull consumers require explicit requests for messages:

```rust
// Batch request
let mut messages = consumer.messages().await?.take(100);
while let Some(message) = messages.next().await {
    let message = message?;
    message.ack().await?;
}

// Sequence-based batch
let mut batches = consumer.sequence(50)?.take(10);
while let Some(mut batch) = batches.try_next().await? {
    while let Some(Ok(message)) = batch.next().await {
        message.ack().await?;
    }
}

// Single message fetch
let message = consumer.fetch().await?;
```

Pull requests are sent to: `$JS.API.CONSUMER.MSG.NEXT.<stream>.<consumer>`

The request payload is JSON:
```json
{"batch": 10, "expires": 5000, "no_wait": false}
```

### Push Consumer

**Location**: `jetstream/consumer/push.rs`

Push consumers receive messages automatically on a delivery subject. The client subscribes to the delivery subject and processes messages as they arrive.

Features:
- **Flow control** — server sends flow control messages, client responds to maintain delivery rate
- **Heartbeats** — idle heartbeats (status code 100) when no messages are available
- **Ordered consumers** — automatically recreated on delivery failures with correct sequence positioning

### Acknowledgment

**Location**: `jetstream/message.rs`

JetStream messages support multiple acknowledgment types:

```rust
pub enum AckKind {
    Ack,           // Ack (message processed)
    Nack,          // Nak (re-deliver)
    Progress,      // Progress (still working)
    Next,          // Next (ack + pull next)
    Term,          // Term (don't redeliver, remove from stream)
    All,           // Ack all messages up to this sequence
}
```

Methods on JetStream `Message`:
- `ack()` — simple acknowledgment
- `ack_with(kind)` — acknowledgment with specific type
- `double_ack()` — exactly-once ack (ACK + separate ack message)
- `nack()` — negative acknowledgment (request redelivery)
- `in_progress()` — progress indicator
- `term()` — terminate message (no redelivery)

## JetStream Message

**Location**: `jetstream/message.rs`

JetStream messages wrap core `Message` with metadata extracted from headers:

```rust
#[derive(Debug)]
pub struct Message {
    pub message: crate::Message,           // The underlying NATS message
    pub context: Context,                  // JetStream context for acking
    pub ack_pending: Arc<AtomicU64>,       // Pending ack counter
}

impl Message {
    pub fn info(&self) -> Result<Info, MessageInfoError>  // Parse message info from headers
    pub async fn ack(&self) -> Result<(), AckError>
    pub async fn ack_with(&self, kind: AckKind) -> Result<(), AckError>
    pub async fn double_ack(&self) -> Result<(), AckError>
    pub async fn nack(&self) -> Result<(), AckError>
    pub async fn in_progress(&self) -> Result<(), AckError>
    pub async fn term(&self) -> Result<(), AckError>
}
```

Message info is extracted from the `HMSG` headers:
- `Nats-Stream` — stream name
- `Nats-Consumer` — consumer name
- `Nats-Delivered` — delivery count
- `Nats-Sequence` — stream sequence
- `Nats-Time-Stamp` — timestamp
- `Nats-Subject` — original subject
- `Nats-Pending-Messages` / `Nats-Pending-Bytes` — pending counts

## Key-Value Store

**Location**: `jetstream/kv/`

The KV store is a JetStream-based key-value API. Each bucket maps to a JetStream stream with specific configuration:

```rust
// Create a KV store
let kv = jetstream
    .create_key_value(async_nats::jetstream::kv::Config {
        bucket: "my_bucket".to_string(),
        history: 5,                      // Max history per key (1-64)
        ttl: Duration::from_secs(3600),  // Key TTL
        max_bytes: 1024 * 1024,          // Max bucket size
        storage: StorageType::File,
        replicas: 1,
        ..Default::default()
    })
    .await?;
```

Under the hood:
- Each key is stored as a message with subject `$KV.<bucket>.<key>`
- Keys support wildcard patterns (`$KV.bucket.prefix.*`)
- History is managed via stream `max_messages_per_subject`
- TTL is managed via stream `max_age`
- `put(key, value)` publishes to the key subject
- `get(key)` reads the last message for the key subject
- `delete(key)` publishes an internal delete marker
- `purge(key)` uses stream purge API
- `watch()` subscribes to key changes and returns a `Watch` stream
- `keys()` / `history(key)` list keys and history

## Object Store

**Location**: `jetstream/object_store/`

The Object Store provides large object storage built on JetStream. Objects are chunked and stored across multiple messages in a stream.

```rust
// Create an object store
let store = jetstream
    .create_object_store(async_nats::jetstream::object_store::Config {
        bucket: "my_objects".to_string(),
        ..Default::default()
    })
    .await?;

// Put an object
let info = store.put("file.txt", stream).await?;

// Get an object
let mut object_stream = store.get("file.txt").await?;
```

Under the hood:
- Objects are chunked into ~128KB messages
- Metadata (object info) is stored as the first "chunk 0" message
- Each chunk is a message with subject `$OBJ.<bucket>.<object-nuid>.C<chunk-number>`
- Metadata includes: name, description, headers, size, chunks, digest (SHA-256)
- `get()` returns a stream of chunks
- Links allow referencing one object from another (like symlinks)

## JetStream Error Codes

**Location**: `jetstream/errors.rs`

Standard JetStream error codes returned by the server:

| Code | Constant | Description |
|------|----------|-------------|
| 10001 | `NOT_FOUND` | Resource not found |
| 10002 | `STREAM_NOT_FOUND` | Stream not found |
| 10003 | `CONSUMER_NOT_FOUND` | Consumer not found |
| 10004 | `REQUEST_NOT_FOUND` | Request not found |
| 10005 | `STREAM_WRONG_LAST_SEQ` | Wrong last sequence |
| 10006 | `STREAM_NAME_EXISTS` | Stream already exists |
| 10007 | `CONSUMER_NAME_EXISTS` | Consumer already exists |
| 10008 | `INSUFFICIENT_RESOURCES` | Insufficient resources |
| 10009 | `NO_MESSAGE_FOUND` | No message found |
| 10013 | `CONSUMER_EXISTS` | Consumer already exists (duplicate) |
| 10014 | `STREAM_NOT_CONFIGURED` | Stream not configured |
| 10015 | `CLUSTER_NOT_ACTIVE` | Cluster not active |
| 10016 | `CLUSTER_NOT_LEADER` | Not the cluster leader |
| 10017 | `CLUSTER_NOT_ENOUGH_PEERS` | Not enough peers |
| 10018 | `CLUSTER_INCOMPLETE` | Cluster incomplete |
| 10019 | `CONSUMER_DELETED` | Consumer was deleted |
| 10020 | `CONSUMER_BAD_ACK` | Bad acknowledgment |
| 10021 | `CONSUMER_BAD_SUBJECT` | Bad consumer subject |
| 10022 | `CONSUMER_DELETED_DRIFT` | Consumer deleted due to drift |
| ... | ... | Additional codes |

## Account

**Location**: `jetstream/account.rs`

The `Account` struct provides information about the JetStream account:

```rust
pub struct Account {
    pub memory: i64,
    pub storage: i64,
    pub streams: i64,
    pub consumers: i64,
    pub limits: AccountLimits,
}
```