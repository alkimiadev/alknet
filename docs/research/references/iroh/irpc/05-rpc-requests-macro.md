# irpc: The rpc_requests Macro

The `#[rpc_requests]` attribute macro is the primary way to define an irpc protocol. It generates the boilerplate for channel typing, message wrapping, and service trait implementations.

## Basic Usage

```rust
use irpc::{channel::{mpsc, oneshot}, rpc_requests, Client, WithChannels};
use serde::{Deserialize, Serialize};

#[rpc_requests(message = ComputeMessage)]
#[derive(Debug, Serialize, Deserialize)]
enum ComputeProtocol {
    /// Unary RPC: one request, one response
    #[rpc(tx=oneshot::Sender<i64>)]
    #[wrap(Multiply)]
    Multiply(i64, i64),

    /// Bidirectional streaming
    #[rpc(tx=mpsc::Sender<i64>, rx=mpsc::Receiver<i64>)]
    #[wrap(Sum)]
    Sum,
}
```

This single macro invocation generates:

1. **Wrapper structs** (from `#[wrap]`): `Multiply` and `Sum` struct types
2. **`Channels<ComputeProtocol>` impls**: For each variant's inner type, specifying `Tx` and `Rx`
3. **`Service` impl**: `impl Service for ComputeProtocol { type Message = ComputeMessage; }`
4. **`RemoteService` impl** (rpc feature): Maps protocol variants + QUIC streams to messages
5. **`ComputeMessage` enum**: Wraps each request in `WithChannels`
6. **`From` conversions**: Between inner types, `ComputeProtocol`, and `ComputeMessage`

## Macro Arguments

### Top-level (on the enum)

| Argument | Required | Description |
|---|---|---|
| `message = Name` | Recommended | Name of the generated message enum. Also generates `Service` and `RemoteService` impls. |
| `alias = "Suffix"` | Optional | Generates type aliases like `MultiplyMsg = WithChannels<Multiply, ComputeProtocol>` |
| `rpc_feature = "feat"` | Optional | Feature-gates the `RemoteService` impl with `#[cfg(feature = "feat")]` |
| `no_rpc` | Optional | Skips generating `RemoteService` impl entirely |
| `no_spans` | Optional | Skips span-related code (for use without the `spans` feature) |

### Per-variant

#### `#[rpc(tx=Type, rx=Type)]`

Specifies channel types for each request:
- `tx` — response channel type (server → client). Defaults to `NoSender`.
- `rx` — update channel type (client → server). Defaults to `NoReceiver`.

Valid types:
- `oneshot::Sender<T>` — single response
- `mpsc::Sender<T>` — streaming response
- `oneshot::Receiver<T>` — not valid as tx (use for rx pattern)
- `mpsc::Receiver<T>` — streaming updates (client → server)
- `NoSender` / `NoReceiver` — no channel in that direction

#### `#[wrap(TypeName, derive(Traits))]`

Generates a struct from the variant's fields:
- `TypeName` — name of the generated struct
- Optional visibility prefix (e.g., `pub(crate) TypeName`)
- `derive(...)` — additional derive macros beyond the default `Serialize, Deserialize, Debug`

If `#[wrap]` is not used, each variant must have exactly one unnamed field (a named type).

## Generated Code Walkthrough

Given this input:
```rust
#[rpc_requests(message = StoreMessage)]
#[derive(Debug, Serialize, Deserialize)]
enum StoreProtocol {
    #[rpc(tx=oneshot::Sender<String>)]
    #[wrap(GetRequest, derive(Clone))]
    Get(String),

    #[rpc(tx=oneshot::Sender<()>)]
    #[wrap(SetRequest)]
    Set { key: String, value: String },
}
```

The macro generates:

### 1. Wrapper Structs

```rust
#[derive(Debug, Serialize, Deserialize, Clone)]
pub GetRequest(pub String);

#[derive(Debug, Serialize, Deserialize)]
pub SetRequest { pub key: String, pub value: String }
```

The variants are rewritten to use these:
```rust
enum StoreProtocol {
    Get(GetRequest),
    Set(SetRequest),
}
```

### 2. Channels Implementations

```rust
impl Channels<StoreProtocol> for GetRequest {
    type Tx = oneshot::Sender<String>;
    type Rx = NoReceiver;
}

impl Channels<StoreProtocol> for SetRequest {
    type Tx = oneshot::Sender<()>;
    type Rx = NoReceiver;
}
```

### 3. Message Enum

```rust
#[doc = "Message enum for [`StoreProtocol`]"]
#[allow(missing_docs)]
#[derive(Debug)]
pub enum StoreMessage {
    Get(WithChannels<GetRequest, StoreProtocol>),
    Set(WithChannels<SetRequest, StoreProtocol>),
}
```

### 4. Service Implementation

```rust
impl Service for StoreProtocol {
    type Message = StoreMessage;
}
```

### 5. RemoteService Implementation (rpc feature)

```rust
impl RemoteService for StoreProtocol {
    fn with_remote_channels(
        self,
        rx: noq::RecvStream,
        tx: noq::SendStream,
    ) -> Self::Message {
        match self {
            StoreProtocol::Get(msg) => {
                StoreMessage::from(WithChannels::from((msg, tx, rx)))
            }
            StoreProtocol::Set(msg) => {
                StoreMessage::from(WithChannels::from((msg, tx, rx)))
            }
        }
    }
}
```

### 6. From Conversions

```rust
// Inner type → Protocol enum
impl From<GetRequest> for StoreProtocol { ... }
impl From<SetRequest> for StoreProtocol { ... }

// WithChannels → Message enum
impl From<WithChannels<GetRequest, StoreProtocol>> for StoreMessage { ... }
impl From<WithChannels<SetRequest, StoreProtocol>> for StoreMessage { ... }
```

### 7. parent_span Method (spans feature)

```rust
impl StoreMessage {
    pub fn parent_span(&self) -> tracing::Span {
        let span = match self {
            StoreMessage::Get(inner) => inner.parent_span_opt(),
            StoreMessage::Set(inner) => inner.parent_span_opt(),
        };
        span.cloned().unwrap_or_else(|| tracing::Span::current())
    }
}
```

## Interaction Pattern Mapping

The `#[rpc]` attribute maps directly to gRPC-like patterns:

| Pattern | `tx` type | `rx` type | Example |
|---|---|---|---|
| **Unary RPC** | `oneshot::Sender<R>` | `NoReceiver` | Get by key, return value |
| **Server streaming** | `mpsc::Sender<R>` | `NoReceiver` | List all items |
| **Client streaming** | `oneshot::Sender<R>` | `mpsc::Receiver<U>` | Upload items, get count |
| **Bidirectional** | `mpsc::Sender<R>` | `mpsc::Receiver<U>` | Chat, live updates |
| **Notify (fire & forget)** | `NoSender` | `NoReceiver` | Log event |

## Client Methods Generated by Patterns

The `Client<S>` methods correspond to channel types:

```rust
// Unary RPC: tx=oneshot::Sender<Res>, rx=NoReceiver
client.rpc(Get { key: "x" }).await  // → Result<Res>

// Server streaming: tx=mpsc::Sender<Res>, rx=NoReceiver
client.server_streaming(List, 16).await  // → Result<mpsc::Receiver<Res>>

// Client streaming: tx=oneshot::Sender<Res>, rx=mpsc::Receiver<Update>
client.client_streaming(SetMany, 4).await  // → Result<(mpsc::Sender<Update>, oneshot::Receiver<Res>)>

// Bidirectional: tx=mpsc::Sender<Res>, rx=mpsc::Receiver<Update>
client.bidi_streaming(Sum, 4, 4).await  // → Result<(mpsc::Sender<Update>, mpsc::Receiver<Res>)>

// Notify: tx=NoSender, rx=NoReceiver
client.notify(Log { msg: "hi" }).await  // → Result<()>
```

## Manual Protocol Definition (Without Macro)

You can define protocols manually instead of using the macro:

```rust
use irpc::{channel::{mpsc, none::NoReceiver, oneshot}, Channels, Service, WithChannels};
use serde::{Deserialize, Serialize};

// 1. Define request types
#[derive(Debug, Serialize, Deserialize)]
struct Get { key: String }

#[derive(Debug, Serialize, Deserialize)]
struct Set { key: String, value: String }

// 2. Implement Channels for each type
impl Channels<StorageProtocol> for Get {
    type Tx = oneshot::Sender<Option<String>>;
    type Rx = NoReceiver;
}

impl Channels<StorageProtocol> for Set {
    type Tx = oneshot::Sender<()>;
    type Rx = NoReceiver;
}

// 3. Define protocol enum
#[derive(derive_more::From, Serialize, Deserialize, Debug)]
enum StorageProtocol {
    Get(Get),
    Set(Set),
}

// 4. Define message enum
#[derive(derive_more::From)]
enum StorageMessage {
    Get(WithChannels<Get, StorageProtocol>),
    Set(WithChannels<Set, StorageProtocol>),
}

// 5. Implement Service
impl Service for StorageProtocol {
    type Message = StorageMessage;
}

// 6. Implement RemoteService (rpc feature)
impl RemoteService for StorageProtocol {
    fn with_remote_channels(self, rx: noq::RecvStream, tx: noq::SendStream) -> Self::Message {
        match self {
            StorageProtocol::Get(msg) => WithChannels::from((msg, tx, rx)).into(),
            StorageProtocol::Set(msg) => WithChannels::from((msg, tx, rx)).into(),
        }
    }
}
```

This manual approach gives full control but requires more boilerplate. The macro generates all of this automatically.