# irpc: Overview and Architecture

## What is irpc?

`irpc` is a **streaming RPC system** built for [iroh](https://docs.rs/iroh) and [noq](https://docs.rs/noq) (QUIC-based transports). It provides a framework for defining RPC protocols in Rust that work identically whether the communication is **in-process** (via tokio channels) or **cross-process/cross-network** (via QUIC streams).

**Key design goals:**

1. **Zero-overhead local use** — When used in-process, irpc should be as lightweight as raw tokio channels, replacing the common pattern of a giant `enum` over an `mpsc` channel with typed backchannels.
2. **Transparent local/remote abstraction** — The same protocol definition and client API works for both in-process and remote communication.
3. **Streaming-first** — Full support for unary RPC, server streaming, client streaming, and bidirectional streaming interaction patterns.
4. **QUIC-native** — Does not abstract over stream types; directly uses noq/iroh QUIC streams, enabling per-request stream tuning (priorities, etc.).

**Non-goals:**

- Cross-language interop (Rust-to-Rust only)
- Versioning (users must handle this themselves)
- Making remote calls look like local async function calls
- Runtime agnosticism (tokio only)

## Crate Structure

```
irpc/
├── src/lib.rs              # Core library: traits, channels, Client, RPC module
├── src/util.rs             # Varint utilities, noq endpoint setup helpers
├── src/tests.rs            # Channel filter/map tests
├── irpc-derive/            # Procedural macro crate (rpc_requests)
├── irpc-iroh/              # Iroh transport integration
├── examples/               # Working examples (storage, compute, derive, local)
└── tests/                  # Integration tests (channels, derive)
```

### Features

| Feature | Default | Purpose |
|---|---|---|
| `rpc` | ✅ | Enables remote RPC (noq transport, postcard serialization) |
| `derive` | ✅ | Enables the `#[rpc_requests]` macro |
| `spans` | ✅ | Preserves tracing spans across message passing |
| `stream` | ✅ | Enables `into_stream()` on mpsc receivers |
| `noq_endpoint_setup` | ✅ | Utilities to create noq endpoints (testing, localhost) |
| `varint-util` | ❌ | Varint read/write utilities without full RPC |

## High-Level Architecture

```
┌─────────────────────────────────────────────────────────┐
│                      Application                         │
│                                                         │
│  ┌──────────┐      ┌───────────┐      ┌───────────┐    │
│  │  Client   │─────│  Protocol  │─────│  Actor/    │    │
│  │<S>        │     │  Enum (S)  │     │  Handler   │    │
│  └────┬─────┘      └───────────┘      └─────┬─────┘    │
│       │                                     │          │
│  ┌────▼─────────────────────────────────────▼─────┐    │
│  │              WithChannels<I, S>                │    │
│  │  ┌────────┐  ┌────────┐  ┌────────┐  ┌─────┐ │    │
│  │  │ inner  │  │   tx   │  │   rx   │  │span │ │    │
│  │  │  (I)   │  │(Sender)│  │(Recv)  │  │     │ │    │
│  │  └────────┘  └────────┘  └────────┘  └─────┘ │    │
│  └────────────────────────────────────────────────┘    │
│                                                         │
│  ┌────────────────────┐   ┌─────────────────────────┐  │
│  │  Local Path        │   │  Remote Path (rpc feat) │  │
│  │  tokio::mpsc       │   │  noq QUIC streams       │  │
│  │  tokio::oneshot    │   │  postcard serialization  │  │
│  └────────────────────┘   └─────────────────────────┘  │
└─────────────────────────────────────────────────────────┘
```

### Core Flow

1. **Define a protocol** — An enum where each variant represents an RPC method, annotated with `#[rpc(tx=..., rx=...)]`.
2. **The `rpc_requests` macro** generates:
   - `Channels<S>` impl for each request type
   - A message enum wrapping each request in `WithChannels<I, S>`
   - `Service` and `RemoteService` trait implementations
   - `From` conversions between request types, protocol enum, and message enum
3. **Client sends messages** — `Client<S>` either sends over a local `mpsc` channel or serializes and sends over a QUIC stream.
4. **Actor/handler processes messages** — Matches on the message enum, extracts `WithChannels { inner, tx, rx, .. }`, and uses `tx`/`rx` to communicate back.

## Dependency Graph

```
irpc (core)
  ├── serde (always)
  ├── tokio (sync, macros)
  ├── tokio-util
  ├── n0-error
  ├── n0-future
  ├── postcard (rpc feature)
  ├── noq (rpc feature)
  ├── smallvec (rpc feature)
  ├── tracing (spans feature)
  └── irpc-derive (derive feature)

irpc-iroh
  ├── irpc
  ├── iroh
  ├── iroh-base
  ├── postcard
  └── n0-error, n0-future, tokio, tracing, serde
```

## License

Dual-licensed: Apache-2.0 OR MIT