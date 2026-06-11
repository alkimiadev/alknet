# async-nats: Quick Reference

## Connection

```rust
// Basic connect
let client = async_nats::connect("demo.nats.io").await?;

// With options
let client = async_nats::ConnectOptions::new()
    .require_tls(true)
    .name("my-service")
    .ping_interval(Duration::from_secs(10))
    .request_timeout(Some(Duration::from_secs(5)))
    .connect("demo.nats.io")
    .await?;

// Multiple servers
let client = async_nats::connect(vec![
    "nats://server1:4222".parse()?,
    "nats://server2:4222".parse()?,
]).await?;

// Background connect
let client = async_nats::ConnectOptions::new()
    .retry_on_initial_connect()
    .connect("demo.nats.io")
    .await?;
```

## Core NATS: Publish

```rust
// Simple publish
client.publish("subject", "payload".into()).await?;

// With reply-to
client.publish_with_reply("subject", "reply-to", "payload".into()).await?;

// With headers
let mut headers = HeaderMap::new();
headers.insert("X-Custom", "value");
client.publish_with_headers("subject", headers, "payload".into()).await?;

// Full control
client.publish_with_reply_and_headers("subject", "reply-to", headers, "payload".into()).await?;

// Flush (ensure all published messages are sent)
client.flush().await?;
```

## Core NATS: Subscribe

```rust
use futures_util::StreamExt;

// Basic subscribe
let mut subscriber = client.subscribe("subject").await?;

// Queue group
let mut subscriber = client.queue_subscribe("subject", "group".into()).await?;

// Receive messages (Subscriber implements Stream)
while let Some(message) = subscriber.next().await {
    println!("subject: {}, payload: {:?}", message.subject, message.payload);
}

// Unsubscribe
subscriber.unsubscribe().await?;

// Unsubscribe after N messages
subscriber.unsubscribe_after(10).await?;

// Drain (wait for in-flight, then unsubscribe)
subscriber.drain().await?;
```

## Core NATS: Request/Reply

```rust
// Simple request (uses default timeout)
let response = client.request("subject", "data".into()).await?;

// With custom timeout and headers
let request = async_nats::Request::new()
    .payload("data".into())
    .timeout(Some(Duration::from_secs(5)))
    .headers(headers);
let response = client.send_request("subject", request).await?;

// Custom inbox (bypasses multiplexer)
let request = async_nats::Request::new()
    .payload("data".into())
    .inbox("custom-inbox".into());
let response = client.send_request("subject", request).await?;
```

## Message Structure

```rust
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

## JetStream

```rust
let jetstream = async_nats::jetstream::new(client);

// Publish (returns ack future)
let ack = jetstream.publish("events", "data".into()).await?;
let publish_ack = ack.await?;

// Stream management
let stream = jetstream.create_stream(stream::Config {
    name: "events".to_string(),
    subjects: vec!["events.>".to_string()],
    max_messages: 10_000,
    ..Default::default()
}).await?;

let stream = jetstream.get_stream("events").await?;
let stream = jetstream.get_or_create_stream(config).await?;
jetstream.delete_stream("events").await?;
jetstream.update_stream(config).await?;

// Consumer management
let consumer: PullConsumer = stream.create_consumer(pull::Config {
    durable_name: Some("my-consumer".to_string()),
    ..Default::default()
}).await?;

// Pull consumer: fetch messages
let mut messages = consumer.messages().await?;
while let Some(message) = messages.next().await {
    let message = message?;
    message.ack().await?;
}

// Push consumer (ordered)
let consumer = stream.create_consumer(push::OrderedConfig {
    deliver_subject: client.new_inbox(),
    filter_subject: "events.>".to_string(),
    ..Default::default()
}).await?;
let mut messages = consumer.messages().await?;
```

## Key-Value Store

```rust
let kv = jetstream.create_key_value(kv::Config {
    bucket: "my-bucket".to_string(),
    history: 10,
    ..Default::default()
}).await?;

// CRUD
let revision = kv.put("key", "value".into()).await?;
let revision = kv.create("key", "value".into()).await?;
let value: Option<Bytes> = kv.get("key").await?;
let entry: Option<Entry> = kv.entry("key").await?;
let revision = kv.update("key", "new-value".into(), revision).await?;
kv.delete("key").await?;
kv.purge("key").await?;

// Watch
let mut watch = kv.watch("key").await?;
let mut watch_all = kv.watch_all().await?;

// History & Keys
let mut history = kv.history("key").await?;
let mut keys = kv.keys().await?;
```

## Object Store

```rust
let bucket = jetstream.create_object_store(object_store::Config {
    bucket: "files".to_string(),
    ..Default::default()
}).await?;

// Put (from any AsyncRead)
let info = bucket.put("file.txt", &mut file).await?;

// Get (returns AsyncRead)
let mut object = bucket.get("file.txt").await?;
let mut bytes = Vec::new();
object.read_to_end(&mut bytes).await?;

// Info, delete, list, watch
let info = bucket.info("file.txt").await?;
bucket.delete("file.txt").await?;
let mut list = bucket.list().await?;
let mut watch = bucket.watch().await?;
```

## Service API

```rust
use async_nats::service::ServiceExt;
use futures_util::StreamExt;

let mut service = client
    .service_builder()
    .description("product service")
    .start("products", "1.0.0")
    .await?;

let mut endpoint = service.endpoint("get").await?;

while let Some(request) = endpoint.next().await {
    request.respond(Ok("result".into())).await?;
}
```

## Client State & Events

```rust
// Check connection state
match client.connection_state() {
    State::Connected => {},
    State::Disconnected => {},
    State::Pending => {},
}

// Get server info
let info: ServerInfo = client.server_info();
println!("max_payload: {}", info.max_payload);
println!("jetstream: {}", info.jetstream);

// Get statistics
let stats = client.statistics();
println!("in_messages: {}", stats.in_messages.load(Ordering::Relaxed));

// Force reconnect
client.force_reconnect().await?;

// Server pool management
client.set_server_pool(["nats://s1:4222".parse()?, "nats://s2:4222".parse()?].as_slice()).await?;
let pool = client.server_pool().await?;

// Drain
client.drain().await?;
```

## Error Handling Patterns

```rust
// Connect errors
match async_nats::connect("server").await {
    Err(e) => match e.kind() {
        ConnectErrorKind::TimedOut => {},
        ConnectErrorKind::Authentication => {},
        ConnectErrorKind::AuthorizationViolation => {},
        _ => {},
    },
    Ok(client) => {},
}

// Publish errors
match client.publish("subject", "data".into()).await {
    Err(e) => match e.kind() {
        PublishErrorKind::MaxPayloadExceeded => {},
        PublishErrorKind::InvalidSubject => {},
        PublishErrorKind::Send => {},
        _ => {},
    },
    _ => {},
}

// Request errors
match client.request("subject", "data".into()).await {
    Err(e) => match e.kind() {
        RequestErrorKind::TimedOut => {},
        RequestErrorKind::NoResponders => {},
        RequestErrorKind::InvalidSubject => {},
        RequestErrorKind::MaxPayloadExceeded => {},
        _ => {},
    },
    Ok(message) => {},
}
```

## Feature Flag Quick Reference

| Feature | Enables | Default |
|---------|---------|---------|
| `jetstream` | JetStream streams, consumers, publish | ✅ |
| `kv` | Key-Value store (implies `jetstream`) | ✅ |
| `object-store` | Object store (implies `jetstream` + `crypto`) | ✅ |
| `service` | Service API | ✅ |
| `nkeys` | NKey/JWT authentication | ✅ |
| `nuid` | NUID-based ID generation | ✅ |
| `crypto` | SHA-256 (for object store) | ✅ |
| `websockets` | WebSocket transport | ✅ |
| `ring` | `ring` TLS crypto backend | ✅ |
| `aws-lc-rs` | `aws-lc-rs` TLS crypto backend | ❌ |
| `fips` | FIPS mode via `aws-lc-rs` | ❌ |
| `chrono` | `chrono` datetime instead of `time` | ❌ |
| `server_2_10` | Server 2.10+ features | ✅ |
| `server_2_11` | Server 2.11+ features | ✅ |
| `server_2_12` | Server 2.12+ features | ✅ |
| `server_2_14` | Server 2.14+ features | ✅ |