# async-nats: JetStream

## Overview

JetStream is NATS' built-in persistence layer, providing stream-based messaging with at-least-once and exactly-once delivery semantics. The `async-nats` JetStream API is accessed through a `Context` object.

### Creating a Context

```rust
// Default context (prefix: $JS.API)
let jetstream = async_nats::jetstream::new(client);

// With domain (prefix: $JS.<domain>.API)
let jetstream = async_nats::jetstream::with_domain(client, "hub");

// With custom prefix
let jetstream = async_nats::jetstream::with_prefix(client, "JS.acc@hub.API");

// Builder with fine-grained control
let context = ContextBuilder::new()
    .timeout(Duration::from_secs(5))
    .api_prefix("MY.JS.API")
    .max_ack_inflight(1000)
    .backpressure_on_inflight(true)
    .ack_timeout(Duration::from_secs(30))
    .build(client);
```

## Context

```rust
#[derive(Debug, Clone)]
pub struct Context {
    pub(crate) client: Client,
    pub(crate) prefix: String,
    pub(crate) timeout: Duration,
    pub(crate) max_ack_semaphore: Arc<tokio::sync::Semaphore>,
    pub(crate) ack_sender: mpsc::Sender<(oneshot::Receiver<Message>, OwnedSemaphorePermit)>,
    pub(crate) backpressure_on_inflight: bool,
    pub(crate) semaphore_capacity: usize,
}
```

### Publish Backpressure

The context uses a semaphore to limit the number of pending publish acknowledgments:

- `max_ack_inflight(n)` — sets semaphore capacity (default 5000)
- `backpressure_on_inflight(true)` — `publish()` waits for a permit when limit is reached
- `backpressure_on_inflight(false)` — `publish()` returns `MaxAckPending` error immediately when limit is reached

A background **acker task** monitors pending acks with a timeout (`ack_timeout`, default 30s), releasing permits when acks arrive or time out.

### JetStream API Request Pattern

All JetStream API calls follow the same pattern:

1. Build a subject from the prefix: `format!("{}.STREAM.CREATE.<name>", self.prefix)`
2. Serialize the request payload as JSON
3. Send a request via `client.send_request()` with the API subject
4. Deserialize the response as `Response<T>` (which is `Ok(T)` or `Err(ErrorCode)`)

## Streams

### Stream Handle

```rust
pub struct Stream<I = Info> {
    context: Context,
    info: I,
    name: String,
}
```

`Stream<Info>` carries server-side info. `Stream<()>` is a lightweight handle that skips the INFO fetch. `Stream` (no generic) defaults to `Stream<Info>`.

### Stream Config

```rust
pub struct Config {
    pub name: String,
    pub description: Option<String>,
    pub subjects: Vec<String>,
    pub retention: RetentionPolicy,
    pub max_consumers: i64,
    pub max_messages: i64,
    pub max_messages_per_subject: i64,
    pub max_bytes: i64,
    pub max_age: Duration,
    pub max_messages_per_stream: i64,
    pub max_msg_size: i32,
    pub discard: DiscardPolicy,
    pub discard_new_per_subject: bool,
    pub storage: StorageType,
    pub num_replicas: usize,
    pub no_ack: bool,
    pub duplicate_window: Duration,
    pub placement: Option<Placement>,
    pub mirror: Option<Source>,
    pub sources: Option<Vec<Source>>,
    pub sealed: bool,
    pub allow_direct: bool,
    pub allow_rollup_hdrs: bool,
    // server_2_10 features:
    pub compression: Option<Compression>,
    pub first_sequence: Option<u64>,
    pub subject_transform: Option<SubjectTransform>,
    pub republish: Option<Republish>,
    pub metadata: Option<HashMap<String, String>>,
}
```

### Stream Operations

| Method | Description |
|--------|-------------|
| `create_stream(config)` | Create a new stream |
| `get_stream(name)` | Get stream handle (with INFO) |
| `get_stream_no_info(name)` | Get lightweight handle (no server round-trip) |
| `get_or_create_stream(config)` | Get existing or create new |
| `delete_stream(name)` | Delete a stream |
| `update_stream(config)` | Update stream configuration |
| `create_or_update_stream(config)` | Update or create if not found |
| `stream_names()` | `Stream` of stream names (paginated) |
| `streams()` | `Stream` of stream info (paginated) |
| `stream_by_subject(subject)` | Find stream name containing subject |

### Stream Handle Methods

```rust
let stream: Stream = jetstream.get_stream("events").await?;

// Info
let info: Info = stream.info().await?;  // Fresh info from server
let info: &Info = stream.cached_info(); // Cached info from last fetch

// Message operations
stream.get_raw_message(seq).await?;           // Get raw message by sequence
stream.get_last_raw_message_by_subject(subj).await?;  // Get last message for subject
stream.direct_get(seq).await?;                // Direct get (if allow_direct)
stream.direct_get_last_for_subject(subj).await?;      // Direct last by subject
stream.delete_message(seq).await?;             // Delete a specific message
stream.purge().await?;                         // Purge all messages
stream.purge().filter(subj).await?;            // Purge messages for subject

// Consumers
stream.create_consumer(config).await?;        // Create consumer bound to stream
stream.get_consumer(name).await?;              // Get existing consumer
stream.delete_consumer(name).await?;            // Delete consumer
```

## Consumers

### Consumer Types

Two consumer types, each with distinct delivery models:

1. **Pull Consumer** (`pull::Config` / `PullConsumer`) — Client explicitly requests batches of messages
2. **Push Consumer** (`push::Config` / `PushConsumer`) — Server pushes messages to a deliver subject

### Pull Consumer

```rust
let consumer: PullConsumer = stream
    .get_or_create_consumer("my-consumer", pull::Config {
        durable_name: Some("my-consumer".to_string()),
        ..Default::default()
    })
    .await?;
```

**Key methods**:
- `consumer.batch(n).await?` — Fetch up to `n` messages (one-shot batch)
- `consumer.messages().await?` — Continuous `Stream` of messages
- `consumer.sequence(n).await?` — Continuous `Stream` of batches of `n` messages
- `consumer.fetch().max(n).expires(dur).await?` — Configurable fetch

Each message from a pull consumer is a `jetstream::Message` which has `ack()` methods.

### Push Consumer

Two push consumer variants:

1. **Standard** (`push::Config`) — messages delivered to a specific subject
2. **Ordered** (`push::OrderedConfig`) — auto-recreated on failure, with flow control

```rust
// Standard push
let consumer = stream.create_consumer(push::Config {
    deliver_subject: "deliver.subject".to_string(),
    durable_name: Some("push-consumer".to_string()),
    ..Default::default()
}).await?;

// Ordered push (no durable name, auto-recreates on failure)
let consumer = stream.create_consumer(push::OrderedConfig {
    deliver_subject: client.new_inbox(),
    filter_subject: "events.>".to_string(),
    ..Default::default()
}).await?;
```

### Consumer Config (Shared Fields)

```rust
pub struct Config {
    // Pull fields
    pub durable_name: Option<String>,
    pub name: Option<String>,
    
    // Push fields
    pub deliver_subject: Option<String>,
    pub deliver_group: Option<String>,
    pub deliver_policy: DeliverPolicy,
    pub opt_start_time: Option<DateTime>,
    pub opt_start_sequence: Option<u64>,
    pub ack_policy: AckPolicy,
    pub ack_wait: Duration,
    pub max_deliver: i64,
    pub backoff: Vec<Duration>,
    pub filter_subject: String,
    pub filter_subjects: Vec<String>,   // server_2_10+
    pub replay_policy: ReplayPolicy,
    pub rate_limit_bps: Option<u64>,
    pub max_waiting: i64,               // pull: max outstanding pull requests
    pub max_ack_pending: i64,
    pub flow_control: bool,
    pub idle_heartbeat: Duration,
    pub headers_only: bool,
    pub num_replicas: usize,
    pub mem_storage: bool,
    pub description: Option<String>,
    pub metadata: Option<HashMap<String, String>>,
    pub inactive_threshold: Option<Duration>,  // for ephemeral consumers
}
```

### Deliver Policy

```rust
pub enum DeliverPolicy {
    All,                     // Deliver all messages
    Last,                    // Deliver last message only
    New,                     // Deliver only new messages
    ByStartSequence { start_sequence: u64 },
    ByStartTime { start_time: DateTime },
    LastPerSubject,          // Deliver last message per subject
}
```

### Ack Policy

```rust
pub enum AckPolicy {
    None,       // No acknowledgment needed
    All,        // Ack all messages up to this one
    Explicit,   // Ack each message individually
}
```

## JetStream Messages

### `jetstream::Message`

Wraps a core `Message` with JetStream-specific metadata:

```rust
pub struct Message {
    pub message: crate::Message,   // The underlying NATS message
    pub ack_subject: Subject,        // Subject for sending acks
    pub stream: String,             // Stream name
    pub consumer: String,           // Consumer name
    pub stream_sequence: u64,      // Sequence in stream
    pub consumer_sequence: u64,     // Sequence for this consumer
    pub delivered: u64,              // Delivery count
    pub pending: u64,               // Pending message count
    pub published: DateTime,        // Original publish time
}
```

### Ack Methods

```rust
// In-memory ack (non-persistent, fast)
message.ack().await?;

// Ack with specific type
message.ack_with(AckKind::Nak).await?;
message.ack_with(AckKind::Progress).await?;
message.ack_with(AckKind::Term).await?;
message.ack_with(AckKind::NakWithDelay(duration)).await?;
message.ack_with(AckKind::TermWithReason("reason")).await?;
```

### `AckKind`

```rust
pub enum AckKind {
    Ack,                           // +ACK — message processed
    Nak,                           // -NAK — re-deliver
    Progress,                      // PRI — still working
    Term,                          // +TERM — don't redeliver
    NakWithDelay(Duration),        // -NAK with re-delivery delay
    TermWithReason(String),        // +TERM with reason
}
```

## JetStream Publish

### `Context::publish()`

JetStream publish returns a `PublishAckFuture` — a future that resolves to a `PublishAck`:

```rust
let ack_future = jetstream.publish("events", "data".into()).await?;
let ack: PublishAck = ack_future.await?;  // Wait for server acknowledgment
```

### `PublishAck`

```rust
pub struct PublishAck {
    pub stream: String,
    pub sequence: u64,
    pub domain: String,
    pub duplicate: bool,
}
```

### `PublishMessage` Builder

```rust
let ack = jetstream.send_publish(
    "events",
    PublishMessage::build()
        .payload("data".into())
        .message_id("uuid-123")        // Deduplication ID
        .expected_stream("events")      // Fail if wrong stream
        .expected_last_msg_id("prev-id")
        .expected_last_sequence(42)
        .headers(headers),
).await?;
```

## Pagination

Stream and consumer listing uses pagination internally:

```rust
pub struct StreamNames {
    context: Context,
    offset: usize,
    page_request: Option<Request>,
    streams: Vec<String>,
    subject: Option<String>,
    done: bool,
}
```

Implements `futures_util::Stream<Item = Result<String, Error>>`, lazily fetching pages as needed.

## Error Handling

JetStream errors follow the `Response<T>` pattern:

```rust
pub enum Response<T> {
    Ok(T),
    Err { error: ErrorCode },
}
```

`ErrorCode` carries the server's error code and description. Most JetStream-specific errors map to typed error enums (e.g., `CreateStreamError`, `ConsumerError`, etc.).