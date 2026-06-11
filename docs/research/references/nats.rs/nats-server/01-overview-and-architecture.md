# nats.rs: Overview and Architecture

**Version**: async-nats 0.49.1, nats-server 0.1.0  
**Repository**: https://github.com/nats-io/nats.rs  
**License**: Apache-2.0  
**Rust Edition**: 2021  
**MSRV**: 1.88.0  
**Protocol**: NATS Client Protocol (INFO/CONNECT/PUB/SUB/UNSUB/PING/PONG)

## What It Is

The `nats.rs` repository contains the **official Rust client for NATS.io**, a high-performance messaging system. The active crate is **`async-nats`** — a fully async, Tokio-based NATS client. The deprecated `nats` crate (synchronous) receives security fixes only.

The `nats-server` crate is **not** an implementation of the NATS server. It is a **test harness** that spawns the Go-based `nats-server` binary for integration tests. The actual NATS server is a separate Go project at `github.com/nats-io/nats-server`.

Core design decisions:
- **Fully async** — all I/O is Tokio-based with async/await throughout
- **Cloneable Client handle** — `Client` is cheap to clone (Arc internals), all protocol work happens in a single `ConnectionHandler` task
- **Channel-based internal communication** — `Client` sends `Command` variants via `mpsc` channel to `ConnectionHandler`
- **Multiplexed request-reply** — one internal subscription handles all request-response patterns via inbox token routing
- **Automatic reconnection** — exponential backoff with configurable server pool rotation
- **Feature-gated subsystems** — JetStream, KV, Object Store, Service API, NKeys, WebSockets, and crypto backends are all optional

## Workspace Structure

```
nats.rs/
├── async-nats/              # Primary crate — async NATS client
│   ├── src/
│   │   ├── lib.rs           # Entry point: connect(), ServerOp, ClientOp, Command, ConnectionHandler, Subscriber
│   │   ├── client.rs        # Client handle: publish, subscribe, request, flush, drain
│   │   ├── connection.rs    # Low-level I/O: protocol parsing, read/write buffers
│   │   ├── connector.rs    # Connection establishment, reconnection, server pool
│   │   ├── options.rs      # ConnectOptions builder
│   │   ├── auth.rs         # Auth struct (credentials container)
│   │   ├── auth_utils.rs   # Credential file parsing (.creds files)
│   │   ├── error.rs        # Generic Error<Kind> type
│   │   ├── header.rs       # HeaderMap — NATS message headers
│   │   ├── subject.rs      # Subject type, ToSubject trait
│   │   ├── status.rs       # StatusCode (100-999 NATS protocol codes)
│   │   ├── message.rs      # Message and OutboundMessage types
│   │   ├── tls.rs          # TLS configuration helpers
│   │   ├── crypto.rs       # Crypto feature support
│   │   ├── id_generator.rs # NUID/rand-based unique ID generation
│   │   ├── datetime.rs     # DateTime helpers for JetStream/Service
│   │   ├── jetstream/      # JetStream API (feature-gated)
│   │   │   ├── mod.rs      # Module root, jetstream::new(), with_domain()
│   │   │   ├── context.rs  # JetStream Context — streams, publishing, consumers
│   │   │   ├── stream.rs   # Stream management, Config, Info, Consumer creation
│   │   │   ├── consumer/   # Pull, Push, Ordered consumers
│   │   │   ├── message.rs  # JetStream Message with ack methods
│   │   │   ├── publish.rs  # PublishAck
│   │   │   ├── response.rs # Response wrapper
│   │   │   ├── errors.rs   # JetStream error codes
│   │   │   ├── account.rs  # Account info
│   │   │   ├── kv/         # Key-Value store (feature: "kv")
│   │   │   └── object_store/ # Object store (feature: "object-store")
│   │   └── service/        # Service API (feature-gated)
│   │       ├── mod.rs      # Service, ServiceBuilder
│   │       ├── endpoint.rs # Endpoint handling
│   │       └── error.rs    # Service errors
│   ├── tests/              # Integration tests (require nats-server binary)
│   ├── examples/           # Runnable examples
│   └── benches/            # Criterion benchmarks
├── nats-server/            # Test harness — spawns Go nats-server for tests
│   ├── src/lib.rs          # Server struct, run_server(), run_cluster()
│   └── configs/            # Server config files for tests
│       └── jetstream.conf
└── nats/                   # DEPRECATED sync client — do not modify
```

## Architecture Diagram

```
┌──────────────────────────────────────────────────────────┐
│                     Application Layer                      │
│                                                           │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐ │
│  │ JetStream│  │    KV    │  │  Object  │  │ Service  │  │
│  │ Context  │  │  Store   │  │  Store   │  │   API    │  │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘  └────┬─────┘ │
│       └──────────────┴─────────────┴─────────────┘       │
│                          │                                │
│                   ┌──────┴──────┐                         │
│                   │   Client    │  Cloneable handle       │
│                   │  (mpsc::Sender)                        │
│                   └──────┬──────┘                         │
│                          │ Command channel                │
└──────────────────────────┼────────────────────────────────┘
                           │
┌──────────────────────────┼────────────────────────────────┐
│                   ConnectionHandler                        │
│                   (single Tokio task)                       │
│                          │                                │
│  ┌───────────┐  ┌───────┴───────┐  ┌──────────────────┐ │
│  │Subscriptions│ │  Multiplexer  │  │  Flush Observers │ │
│  │  HashMap    │ │ (request-reply)│  │                  │ │
│  └──────┬──────┘ └───────┬───────┘  └──────────────────┘ │
│         └────────────────┼                                │
│                   ┌──────┴──────┐                         │
│                   │  Connector   │  Server pool, reconnect │
│                   └──────┬──────┘                         │
│                          │                                │
│                   ┌──────┴──────┐                         │
│                   │ Connection  │  Protocol I/O            │
│                   │ (read/write)│  ServerOp / ClientOp     │
│                   └──────┬──────┘                         │
└──────────────────────────┼────────────────────────────────┘
                           │
                    ┌──────┴──────┐
                    │ NATS Server │  (Go binary, TCP/TLS/WS)
                    └─────────────┘
```

## Key Concepts

### Subject
NATS uses subject strings for message addressing. A `Subject` is a validated, immutable, UTF-8 string backed by `Bytes`. Subjects use dot-delimited tokens (e.g., `events.data.sensor1`). Wildcards `*` (single token) and `>` (multi-token suffix) are supported for subscriptions.

### ClientOp / ServerOp
The NATS client-server protocol is text-based with binary payloads. The client sends `ClientOp` variants (CONNECT, PUB/HPUB, SUB, UNSUB, PING, PONG) and receives `ServerOp` variants (INFO, MSG/HMSG, +OK, -ERR, PING, PONG).

### Command
Internal command type sent from `Client` to `ConnectionHandler` via `mpsc` channel. Includes Publish, Request, Subscribe, Unsubscribe, Flush, Drain, Reconnect, SetServerPool, ServerPool.

### Multiplexer
A single internal subscription (SID 0) that routes all request-reply responses. When a `Request` is made, a unique inbox token is registered in the multiplexer's sender map, and the response is dispatched to the corresponding `oneshot::Sender`.

### ConnectionHandler
A single Tokio task that drives all protocol I/O. It processes server operations from `Connection`, handles client commands from the `mpsc` channel, manages subscriptions, maintains ping/pong health, and orchestrates reconnection.

## nats-server Test Harness

The `nats-server` crate provides utilities for launching real NATS server instances in tests:

- `run_server(cfg)` — starts a single server with optional config
- `run_cluster(cfg)` — starts a 3-node cluster
- `Server` struct — holds the child process, cleans up on drop
- `Server::restart()` — kills and restarts the server process
- `Server::client_url()` — reads the INFO from the server to get the client URL
- `set_lame_duck_mode(server)` — sends LDM signal to the server process

The test harness spawns the Go `nats-server` binary via `std::process::Command`, using dynamic ports for parallel test execution. It auto-discovers the client URL by connecting to the server's TCP port and parsing the `INFO` JSON. On `Drop`, it kills the child process and cleans up JetStream storage directories.

## Feature Flags

```toml
# Default: everything enabled
default = ["server_2_10", "server_2_11", "server_2_12", "server_2_14",
           "service", "ring", "jetstream", "nkeys", "crypto",
           "object-store", "kv", "websockets", "nuid"]

# Subsystems
jetstream       # JetStream API
kv              # Key-Value store (requires jetstream)
object-store    # Object store (requires jetstream + crypto)
service         # Service API

# Crypto backends (pick one)
ring            # Default crypto backend
aws-lc-rs       # Alternative backend
fips            # FIPS mode (requires aws-lc-rs)

# Auth
nkeys           # NKey authentication

# Other
nuid            # NUID-based ID generation (falls back to rand)
crypto          # Encryption support
websockets      # WebSocket transport
experimental    # Experimental features

# Server version markers (enable version-specific API fields)
server_2_10
server_2_11
server_2_12
server_2_14
```

## Dependencies (Key)

| Dependency | Purpose |
|-----------|---------|
| `tokio` | Async runtime (macros, rt, net, sync, time, io-util) |
| `bytes` | Zero-copy byte buffers for payloads |
| `tokio-rustls` | TLS via rustls |
| `rustls-native-certs` | Load native TLS root certificates |
| `serde` / `serde_json` | JSON serialization for protocol messages and JetStream API |
| `memchr` | Fast CRLF search for protocol parsing |
| `futures-util` | Stream trait, Sink trait, StreamExt |
| `tracing` | Structured logging |
| `thiserror` | Error type derivation |
| `url` | URL parsing for server addresses |
| `portable-atomic` | Portable atomic operations |

## References

- [NATS Protocol Specification](https://docs.nats.io/reference/reference-protocols/nats-protocol)
- [NATS JetStream Documentation](https://docs.nats.io/nats-concepts/jetstream)
- [async-nats on docs.rs](https://docs.rs/async-nats)