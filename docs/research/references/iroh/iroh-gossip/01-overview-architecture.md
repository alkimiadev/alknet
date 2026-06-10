# iroh-gossip: Overview & Architecture

## What Is iroh-gossip?

`iroh-gossip` is a Rust crate that implements an **epidemic broadcast tree** protocol for disseminating messages among a swarm of peers interested in a common **topic**. It is based on two academic papers:

- **HyParView** — A hybrid partial view membership protocol for reliable swarm management ([paper](https://asc.di.fct.unl.pt/~jleitao/pdf/dsn07-leitao.pdf))
- **PlumTree** — An epidemic broadcast tree protocol for efficient message dissemination ([paper](https://asc.di.fct.unl.pt/~jleitao/pdf/srds07-leitao.pdf))

The crate is designed as a protocol layer for the [iroh](https://docs.rs/iroh) networking library, but the core protocol logic is **IO-free** and can be used independently.

## High-Level Architecture

The crate is organized into two primary modules:

| Module | Purpose | IO-aware? |
|--------|---------|-----------|
| `proto` | Pure state-machine implementation of the gossip protocol | No — completely IO-free |
| `net` | Networking layer that runs the protocol over iroh connections | Yes — depends on `iroh` and tokio |

The `net` module is behind the `net` feature flag (enabled by default). An optional `rpc` feature adds remote procedure call support via the `irpc`/`noq` crates.

### Module Dependency Graph

```
┌──────────────┐
│     api      │  ← Public API (Gossip, GossipTopic, GossipSender, GossipReceiver)
└──────┬───────┘
       │
┌──────▼───────┐
│     net      │  ← Networking actor, connection loops, dialer
└──────┬───────┘
       │
┌──────▼───────┐
│    proto     │  ← Pure protocol state machines
│  ┌─────────┐ │
│  │hyparview│ │  ← Membership layer
│  ├─────────┤ │
│  │ plumtree│ │  ← Broadcast layer
│  ├─────────┤ │
│  │  topic  │ │  ← Per-topic coordinator
│  ├─────────┤ │
│  │  state  │ │  ← Multi-topic state manager
│  ├─────────┤ │
│  │  util   │ │  ← Shared data structures (IndexSet, TimeBoundCache, TimerMap)
│  └─────────┘ │
└──────────────┘
```

### Key Design Principles

1. **IO-free protocol core**: The `proto` module is a pure state machine. It takes `InEvent`s, produces `OutEvent`s, and has no knowledge of sockets, async runtimes, or network IO.

2. **Topic-based isolation**: Each topic (`TopicId` = 32-byte identifier) has completely independent state. Topics are separate swarms and broadcast scopes. Joining multiple topics increases connections and routing table size proportionally.

3. **Actor model for networking**: The `net` module runs a single async `Actor` that manages all topics, connections, and timers. It bridges between the protocol state machine and real network IO.

4. **Wire protocol**: Messages are serialized with `postcard` (a `no_std`-friendly serde format) and sent over QUIC streams via iroh connections. Each stream is prefixed with a `StreamHeader` containing the topic ID.

## Crate Features

| Feature | Default? | Description |
|---------|----------|-------------|
| `net` | Yes | Networking layer (requires `iroh`, `tokio`, etc.) |
| `rpc` | No | RPC support via `irpc`/`noq` for remote control |
| `metrics` | Yes | Prometheus-style metrics via `iroh-metrics` |
| `test-utils` | No | Test utilities (seeded RNG, etc.) |
| `simulator` | No | CLI simulator for testing |
| `examples` | No | Example binaries (chat, setup) |

## Cargo Dependencies (Key Ones)

- `iroh` / `iroh-base` — Networking primitives (Endpoint, EndpointId, PublicKey, etc.)
- `postcard` — Wire serialization (serde-based, `no_std` compatible)
- `blake3` — Message ID hashing
- `ed25519-dalek` — Cryptographic signatures
- `n0-future` / `n0-error` — Async utilities and error handling
- `irpc` / `noq` — RPC infrastructure (optional)
- `indexmap` — Order-preserving hash collections used in `IndexSet`