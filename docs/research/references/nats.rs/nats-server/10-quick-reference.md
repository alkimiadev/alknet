# Quick Reference

## Crate Summary

| | |
|---|---|
| **Crate** | `async-nats` |
| **Version** | 0.49.1 |
| **Edition** | 2021 |
| **MSRV** | 1.88.0 |
| **License** | Apache-2.0 |
| **Runtime** | Tokio |
| **Protocol** | NATS Client Protocol v1 (Dynamic) |
| **TLS** | rustls (ring / aws-lc-rs / fips) |
| **WebSocket** | tokio-websockets (feature-gated) |

## Quick Start

```rust
use async_nats::connect;
use futures_util::StreamExt;

#[tokio::main]
async fn main() -> Result<(), async_nats::Error> {
    let client = connect("demo.nats.io").await?;
    
    // Publish
    client.publish("events.data", "hello".into()).await?;
    
    // Subscribe
    let mut sub = client.subscribe("events.>").await?;
    while let Some(msg) = sub.next().await {
        println!("{:?}", msg);
    }
    
    // Request-Response
    let response = client.request("service", "input".into()).await?;
    
    Ok(())
}
```

## Architecture at a Glance

```
Client (cloneable handle, mpsc::Sender<Command>)
  │
  ▼
ConnectionHandler (single Tokio task)
  ├── Subscriptions HashMap<u64, Subscription>
  ├── Multiplexer (request-reply, SID 0)
  ├── Flush Observers
  └── Ping/Pong health check
  │
  ▼
Connection (protocol I/O, read/write buffers)
  │
  ▼
Connector (server pool, reconnection)
  │
  ▼
NATS Server (Go binary, TCP/TLS/WebSocket)
```

## Key Types

| Type | Location | Purpose |
|------|----------|---------|
| `Client` | `client.rs` | Cloneable connection handle |
| `Subscriber` | `lib.rs` | Message stream (impl `futures::Stream`) |
| `Message` | `message.rs` | Inbound NATS message |
| `OutboundMessage` | `message.rs` | Outbound publish message |
| `Subject` | `subject.rs` | Validated subject string (backed by `Bytes`) |
| `HeaderMap` | `header.rs` | NATS message headers |
| `StatusCode` | `status.rs` | NATS protocol status codes |
| `ServerInfo` | `lib.rs` | Server INFO data |
| `ConnectInfo` | `lib.rs` | Client CONNECT data |
| `ServerAddr` | `lib.rs` | Validated server URL |
| `Auth` | `auth.rs` | Authentication credentials |
| `ConnectOptions` | `options.rs` | Connection configuration builder |
| `Event` | `lib.rs` | Connection lifecycle events |
| `State` | `connection.rs` | Connection state (Pending/Connected/Disconnected) |
| `Statistics` | `client.rs` | Atomic connection metrics |
| `Request` | `client.rs` | Request-response builder |

## JetStream Types

| Type | Location | Purpose |
|------|----------|---------|
| `jetstream::Context` | `jetstream/context.rs` | JetStream API entry point |
| `jetstream::stream::Stream` | `jetstream/stream.rs` | Stream management |
| `jetstream::stream::Config` | `jetstream/stream.rs` | Stream configuration |
| `jetstream::stream::Info` | `jetstream/stream.rs` | Stream info/state |
| `jetstream::consumer::PullConsumer` | `jetstream/consumer/pull.rs` | Pull-based consumer |
| `jetstream::consumer::PushConsumer` | `jetstream/consumer/push.rs` | Push-based consumer |
| `jetstream::consumer::Config` | `jetstream/consumer/mod.rs` | Consumer configuration |
| `jetstream::Message` | `jetstream/message.rs` | Message with ack methods |
| `jetstream::PublishAck` | `jetstream/publish.rs` | Publish acknowledgment |
| `jetstream::kv::Store` | `jetstream/kv/bucket.rs` | Key-Value store |
| `jetstream::object_store::ObjectStore` | `jetstream/object_store/mod.rs` | Object store |
| `jetstream::ErrorCode` | `jetstream/errors.rs` | JetStream error codes |

## Protocol Operations

### Client → Server (ClientOp)

| Op | Wire Format | Purpose |
|----|-----------|---------|
| `CONNECT` | `CONNECT {json}\r\n` | Authentication and capabilities |
| `PUB` | `PUB <subject> [reply] <len>\r\n<payload>\r\n` | Publish message |
| `HPUB` | `HPUB <subject> [reply] <hlen> <tlen>\r\n<hdrs><payload>\r\n` | Publish with headers |
| `SUB` | `SUB <subject> [queue] <sid>\r\n` | Subscribe |
| `UNSUB` | `UNSUB <sid> [max]\r\n` | Unsubscribe |
| `PING` | `PING\r\n` | Keepalive / health check |
| `PONG` | `PONG\r\n` | Response to server PING |

### Server → Client (ServerOp)

| Op | Wire Format | Purpose |
|----|-----------|---------|
| `INFO` | `INFO {json}\r\n` | Server capabilities, cluster info |
| `MSG` | `MSG <subj> <sid> [reply] <len>\r\n<payload>\r\n` | Deliver message |
| `HMSG` | `HMSG <subj> <sid> [reply] <hlen> <tlen>\r\n<hdrs><payload>\r\n` | Message with headers |
| `+OK` | `+OK\r\n` | Success (verbose mode) |
| `-ERR` | `-ERR <desc>\r\n` | Server error |
| `PING` | `PING\r\n` | Server health check |
| `PONG` | `PONG\r\n` | Ack client PING |

## Internal Commands (Command → ConnectionHandler)

| Command | Purpose |
|---------|---------|
| `Publish(OutboundMessage)` | Queue message for sending |
| `Request { subject, payload, respond, headers, sender }` | Request-response via multiplexer |
| `Subscribe { sid, subject, queue_group, sender }` | Create subscription |
| `Unsubscribe { sid, max }` | Remove subscription |
| `Flush { observer }` | Wait for write buffer flush |
| `Drain { sid }` | Gracefully drain (sub or whole client) |
| `Reconnect` | Force reconnection |
| `SetServerPool { servers, result }` | Replace server pool |
| `ServerPool { result }` | Query server pool |

## Feature Flags

| Feature | Default | Enables |
|---------|---------|---------|
| `jetstream` | ✓ | JetStream API (streams, consumers, publish) |
| `kv` | ✓ | Key-Value store (requires jetstream) |
| `object-store` | ✓ | Object store (requires jetstream + crypto) |
| `service` | ✓ | Service API |
| `nkeys` | ✓ | NKey/JWT authentication |
| `crypto` | ✓ | Encryption support |
| `websockets` | ✓ | WebSocket transport |
| `nuid` | ✓ | NUID ID generation |
| `ring` | ✓ | Ring crypto backend |
| `aws-lc-rs` | ✗ | Alternative crypto backend |
| `fips` | ✗ | FIPS mode (requires aws-lc-rs) |
| `chrono` | ✗ | Use chrono instead of time |
| `experimental` | ✗ | Experimental features |
| `server_2_10` | ✓ | Server 2.10+ API fields |
| `server_2_11` | ✓ | Server 2.11+ API fields |
| `server_2_12` | ✓ | Server 2.12+ API fields |
| `server_2_14` | ✓ | Server 2.14+ API fields |

## Connection Defaults

| Parameter | Default |
|-----------|---------|
| Connection timeout | 5 seconds |
| Ping interval | 60 seconds |
| Max pending pings | 2 |
| Request timeout | 10 seconds |
| Command channel capacity | 2048 |
| Subscription capacity | 65536 |
| Read buffer capacity | 65535 |
| Inbox prefix | `_INBOX` |
| Reconnect delay | Exponential (0ms → 4s cap) |
| Max reconnects | Unlimited |
| TLS required | Auto (server-dependent) |

## Error Hierarchy

```
ConnectError (ConnectErrorKind::ServerParse | Dns | Authentication | AuthorizationViolation | TimedOut | Tls | Io | MaxReconnects)
PublishError (PublishErrorKind::MaxPayloadExceeded | InvalidSubject | Send)
RequestError (RequestErrorKind::TimedOut | NoResponders | InvalidSubject | MaxPayloadExceeded | Other)
SubscribeError (SubscribeErrorKind::InvalidSubject | InvalidQueueName | Other)
FlushError (FlushErrorKind::SendError | FlushError)
```

## nats-server Test Harness

| Function | Description |
|----------|-------------|
| `run_server(cfg)` | Start single server with config |
| `run_basic_server()` | Start bare server |
| `run_cluster(cfg)` | Start 3-node cluster |
| `set_lame_duck_mode(s)` | Send LDM signal |

## JetStream API Subjects

| Operation | Subject Pattern |
|-----------|---------------|
| Create stream | `$JS.API.STREAM.CREATE.<name>` |
| Stream info | `$JS.API.STREAM.INFO.<name>` |
| Update stream | `$JS.API.STREAM.UPDATE.<name>` |
| Delete stream | `$JS.API.STREAM.DELETE.<name>` |
| Purge stream | `$JS.API.STREAM.PURGE.<name>` |
| List streams | `$JS.API.STREAM.LIST` |
| Create consumer | `$JS.API.CONSUMER.CREATE.<stream>` |
| Create durable | `$JS.API.CONSUMER.DURABLE.CREATE.<stream>.<name>` |
| Consumer info | `$JS.API.CONSUMER.INFO.<stream>.<name>` |
| Pull next | `$JS.API.CONSUMER.MSG.NEXT.<stream>.<name>` |
| Account info | `$JS.API.ACCOUNT.INFO` |
| Direct get | `$JS.API.DIRECT.GET.<name>` |