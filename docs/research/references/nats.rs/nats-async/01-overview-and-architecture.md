# async-nats: Overview & Architecture

**Crate**: `async-nats`  
**Version**: 0.49.1  
**Repository**: https://github.com/nats-io/nats.rs  
**License**: Apache-2.0  
**Rust Edition**: 2021  
**MSRV**: 1.88.0  
**Async Runtime**: Tokio

## What is async-nats?

`async-nats` is the official async Rust client for the [NATS messaging system](https://nats.io). It provides a Tokio-based asynchronous interface to NATS server features including:

- **Core NATS** — publish/subscribe, request/reply, queue groups
- **JetStream** — persistent stream-based messaging with at-least-once and exactly-once semantics
- **Key-Value Store** — KV abstraction built on JetStream streams
- **Object Store** — large-object storage built on JetStream streams
- **Service API** — microservice request/reply pattern with built-in PING/INFO/STATS verbs

The crate is positioned as the **core client** in the NATS Rust ecosystem. A separate project, [Orbit](https://github.com/synadia-io/orbit.rs), provides higher-level opinionated abstractions on top.

```
   ┌──────────────────────────────────────────────────────┐
   │  Application code                                    │
   └──────────────┬───────────────────────────┬───────────┘
                  │                           │
                  ▼                           ▼
        ┌───────────────────┐       ┌───────────────────┐
        │ Orbit crates       │  uses │ async-nats (core) │
        │ (opinionated,      │──────▶│ (parity, stable,  │
        │  per-crate semver) │       │  protocol-level)  │
        └───────────────────┘       └─────────┬─────────┘
                                              │
                                              ▼
                                       ┌─────────────┐
                                       │ nats-server │
                                       └─────────────┘
```

## Feature Flags

Features are extensive and control which subsystems are compiled:

| Feature | Default | Description |
|---------|---------|-------------|
| `jetstream` | ✅ | JetStream API (streams, consumers, publish) |
| `kv` | ✅ | Key-Value store (depends on `jetstream`) |
| `object-store` | ✅ | Object store (depends on `jetstream` + `crypto`) |
| `service` | ✅ | Service API (microservice pattern) |
| `nkeys` | ✅ | NKey/JWT authentication |
| `nuid` | ✅ | NUID-based unique ID generation |
| `crypto` | ✅ | Cryptographic primitives (SHA-256 for object store) |
| `websockets` | ✅ | WebSocket transport (`ws://`/`wss://`) |
| `ring` | ✅ | Use `ring` as TLS crypto backend |
| `aws-lc-rs` | ❌ | Use `aws-lc-rs` as TLS crypto backend |
| `fips` | ❌ | FIPS 140-2 compliant via `aws-lc-rs` |
| `chrono` | ❌ | Use `chrono` instead of `time` for datetime types |
| `server_2_10` | ✅ | Server 2.10+ features |
| `server_2_11` | ✅ | Server 2.11+ features |
| `server_2_12` | ✅ | Server 2.12+ features |
| `server_2_14` | ✅ | Server 2.14+ features |
| `experimental` | ❌ | Experimental features |

## Source Structure

```
async-nats/src/
├── lib.rs               # Entry point: connect(), ServerInfo, Command, ClientOp, ServerOp,
│                          ConnectionHandler, Subscriber, Event, ServerAddr, ConnectInfo
├── client.rs            # Client struct, publish/subscribe/request/drain/flush APIs,
│                          Request builder, Statistics, trait definitions
├── connection.rs        # Framed connection: NATS protocol parser/serializer,
│                          read/write buffer management, WebSocket adapter
├── connector.rs         # Server pool, reconnection logic, TLS setup, DNS resolution,
│                          authentication handshake
├── options.rs           # ConnectOptions builder, auth methods, TLS config, callbacks
├── auth.rs              # Auth struct (username, password, token, JWT, nkey, signature)
├── auth_utils.rs        # Credentials file parsing (JWT + NKey seed)
├── message.rs           # Message (inbound), OutboundMessage (outbound)
├── header.rs            # HeaderMap, HeaderName, HeaderValue (NATS headers)
├── subject.rs           # Subject type, ToSubject trait, SubjectError
├── status.rs            # StatusCode enum (NATS status codes)
├── error.rs             # Generic Error<K> type used throughout
├── datetime.rs          # DateTime type (time or chrono backend)
├── id_generator.rs      # Unique ID generation (NUID or rand fallback)
├── tls.rs               # TLS configuration helper
├── crypto.rs            # SHA-256 for object store integrity
├── jetstream/
│   ├── mod.rs           # Module entry: new(), with_domain(), with_prefix()
│   ├── context.rs       # Context: JetStream API (streams, consumers, KV, OS, publish)
│   ├── stream.rs        # Stream handle, Config, Info, purge/delete/message ops
│   ├── consumer/
│   │   ├── mod.rs       # Consumer trait, Info, Config base
│   │   ├── pull.rs      # PullConsumer: batch fetch, sequence, messages stream
│   │   └── push.rs      # PushConsumer: Ordered push consumer with auto-recreate
│   ├── publish.rs       # PublishAck, PublishAckFuture, PublishMessage builder
│   ├── message.rs       # JetStream Message (with ack methods), AckKind
│   ├── response.rs      # Response<T> (Ok/Err) for JetStream API calls
│   ├── errors.rs        # ErrorCode, Error for JetStream
│   ├── account.rs       # Account info
│   ├── kv/
│   │   ├── mod.rs       # Store: put/get/delete/purge/watch/history/keys
│   │   └── bucket.rs    # Bucket Status
│   └── object_store/
│       └── mod.rs       # ObjectStore: put/get/delete/watch/list/seal, Object (AsyncRead)
└── service/
    ├── mod.rs           # Service, ServiceBuilder, Group, EndpointBuilder, Request
    └── endpoint.rs      # Endpoint stream, Stats, Info
```

## Architecture: Core Connection Model

The client uses a **single-connection, actor-model** design:

```
                    ┌──────────────────────────────────────┐
  Client (clone) ──▶│  mpsc::Sender<Command>              │
  (many handles)    │  (bounded channel)                  │
                    └────────────┬────────────────────────┘
                                 │
                                 ▼
                    ┌────────────────────────────────────────┐
                    │  ConnectionHandler (tokio::task)      │
                    │  - Receives Command from channel       │
                    │  - Converts to ClientOp               │
                    │  - Manages subscriptions map          │
                    │  - Manages multiplexer (request/reply)│
                    │  - Pings server on interval           │
                    │  - Handles reconnection                │
                    └────────────┬──────────────────────────┘
                                 │
                                 ▼
                    ┌────────────────────────────────────────┐
                    │  Connection (framed TCP/TLS/WS)       │
                    │  - Protocol parser (try_read_op)      │
                    │  - Write buffer (VecDeque<Bytes>)     │
                    │  - Vectored I/O support               │
                    │  - Read buffer (BytesMut)              │
                    └────────────┬──────────────────────────┘
                                 │
                                 ▼
                          nats-server
```

### Key Design Decisions

1. **Cloneable Client**: `Client` is `Clone` (via `mpsc::Sender` clone), enabling shared use across tasks
2. **Single TCP connection**: All traffic (Core NATS, JetStream API, etc.) multiplexes over one connection
3. **Background task**: `ConnectionHandler` runs as a spawned Tokio task, bridging the mpsc channel to the TCP stream
4. **Automatic reconnection**: On disconnect, `Connector` retries servers from the pool with exponential backoff
5. **Subscription rehydration**: On reconnect, all active subscriptions are re-subscribed with adjusted `max` counts
6. **Multiplexer for request/reply**: A single wildcard subscription (`_INBOX.<id>.*`) multiplexes all pending request/reply correlations

## Dependencies (Key)

| Crate | Purpose |
|-------|---------|
| `tokio` | Async runtime, TCP, time, sync, io-util |
| `bytes` | Efficient byte buffer (`Bytes`, `BytesMut`) |
| `tokio-rustls` | TLS via rustls |
| `rustls-native-certs` | Load system root certificates |
| `serde` / `serde_json` | JSON serialization for JetStream API |
| `futures-util` | Stream trait, Sink trait, StreamExt |
| `tracing` | Structured logging |
| `thiserror` | Error derive macros |
| `memchr` | Fast substring search for protocol parsing |
| `portable-atomic` | Atomic types with portable-atomic fallback |
| `tokio-util` | `PollSender` for Sink implementation |
| `tokio-stream` | `ReceiverStream` adapter |