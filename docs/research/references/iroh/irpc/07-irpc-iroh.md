# irpc: irpc-iroh — Iroh Transport Integration

The `irpc-iroh` crate provides transport integration for iroh, enabling irpc to work with iroh's QUIC connections that use endpoint IDs (rather than socket addresses) for routing.

## Crate Overview

```toml
[package]
name = "irpc-iroh"
version = "0.13.0"
description = "Iroh transport for irpc"
```

Dependencies: `iroh`, `irpc`, `tokio`, `tracing`, `serde`, `postcard`, `n0-error`, `n0-future`

## Key Types

### IrohRemoteConnection

```rust
#[derive(Debug, Clone)]
pub struct IrohRemoteConnection(Connection);
```

Wraps an existing iroh `Connection`. Simplest way to use irpc with iroh — create a connection externally and wrap it.

```rust
impl RemoteConnection for IrohRemoteConnection {
    fn clone_boxed(&self) -> Box<dyn RemoteConnection> { ... }
    fn open_bi(&self) -> BoxFuture<Result<(SendStream, RecvStream), RequestError>> {
        // Delegates to connection.open_bi()
    }
    fn zero_rtt_accepted(&self) -> BoxFuture<bool> {
        // Always true — fully authenticated connection
    }
}
```

**Note:** This stops working when the underlying connection is closed. For automatic reconnection, use `IrohLazyRemoteConnection`.

### IrohZrttRemoteConnection

```rust
#[derive(Debug, Clone)]
pub struct IrohZrttRemoteConnection(OutgoingZeroRttConnection);
```

Wraps an iroh 0-RTT (Zero Round Trip Time) connection. This enables sending data before the full handshake completes for reduced latency on reconnections.

```rust
impl RemoteConnection for IrohZrttRemoteConnection {
    fn open_bi(&self) -> BoxFuture<Result<(SendStream, RecvStream), RequestError>> {
        // Delegates to the 0-RTT connection's open_bi()
    }
    fn zero_rtt_accepted(&self) -> BoxFuture<bool> {
        // Actually checks handshake_completed() to determine
        // if 0-RTT data was accepted
    }
}
```

The `zero_rtt_accepted()` method:
- Returns `true` if `ZeroRttStatus::Accepted` 
- Returns `false` if `ZeroRttStatus::Rejected` or on error
- This allows the `Client` to decide whether to re-send data

### IrohLazyRemoteConnection

```rust
#[derive(Debug, Clone)]
pub struct IrohLazyRemoteConnection(Arc<IrohRemoteConnectionInner>);

struct IrohRemoteConnectionInner {
    endpoint: iroh::Endpoint,
    addr: iroh::EndpointAddr,
    connection: tokio::sync::Mutex<Option<Connection>>,
    alpn: Vec<u8>,
}
```

The lazy connection caches the underlying iroh `Connection` and reconnects automatically:

1. On first `open_bi()`, establishes a connection via `endpoint.connect(addr, alpn)`
2. Caches the connection in a `Mutex<Option<Connection>>`
3. On subsequent `open_bi()`, tries to reuse the cached connection
4. If the cached connection fails, clears the cache and reconnects once

The `alpn` field is required because iroh connections need an ALPN protocol identifier.

### `client()` Function

```rust
pub fn client<S: irpc::Service>(
    endpoint: iroh::Endpoint,
    addr: impl Into<iroh::EndpointAddr>,
    alpn: impl AsRef<[u8]>,
) -> irpc::Client<S>
```

Convenience function to create a `Client<S>` using iroh. Creates an `IrohLazyRemoteConnection` and wraps it with `Client::boxed()`.

## Server-Side: IrohProtocol

### IrohProtocol

```rust
pub struct IrohProtocol<R> {
    handler: Handler<R>,
    request_id: AtomicU64,
}
```

Implements `iroh::protocol::ProtocolHandler`, allowing it to be registered with iroh's `Router`:

```rust
impl<R: DeserializeOwned + Send + 'static> ProtocolHandler for IrohProtocol<R> {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        // Handle the connection using irpc's handle_connection
        let handler = self.handler.clone();
        let fut = handle_connection(&connection, handler).map_err(AcceptError::from_err);
        fut.instrument(span).await
    }
}
```

**Usage:**
```rust
let protocol = IrohProtocol::with_sender(local_sender);
// or
let protocol = IrohProtocol::new(handler);

let router = Router::builder(endpoint)
    .accept(ALPN, protocol)
    .spawn();
```

### Iroh0RttProtocol

```rust
pub struct Iroh0RttProtocol<R> { ... }
```

Supports 0-RTT connections by implementing `ProtocolHandler::on_accepting()`:

```rust
impl<R: DeserializeOwned + Send + 'static> ProtocolHandler for Iroh0RttProtocol<R> {
    async fn on_accepting(&self, accepting: Accepting) -> Result<Connection, AcceptError> {
        let zrtt_conn = accepting.into_0rtt();
        // Handle 0-RTT data immediately
        handle_connection(&zrtt_conn, handler).await?;
        // Wait for handshake completion
        let conn = zrtt_conn.handshake_completed().await?;
        Ok(conn)
    }

    async fn accept(&self, _connection: Connection) -> Result<(), AcceptError> {
        // Noop — handled in on_accepting
        Ok(())
    }
}
```

**Warning:** 0-RTT data is replayable. Only use for idempotent operations. See <https://www.iroh.computer/blog/0rtt-api>.

### IncomingRemoteConnection Trait

```rust
pub trait IncomingRemoteConnection {
    fn accept_bi(&self) -> impl Future<Output = Result<(SendStream, RecvStream), ConnectionError>> + Send;
    fn close(&self, error_code: VarInt, reason: &[u8]);
    fn remote_id(&self) -> Result<EndpointId, RemoteEndpointIdError>;
}
```

Abstraction over `Connection` and `IncomingZeroRttConnection`, enabling `handle_connection` and `read_request` to work with both regular and 0-RTT connections.

Implemented for:
- `Connection` — regular iroh connection
- `IncomingZeroRttConnection` — 0-RTT connection

## handle_connection (iroh variant)

```rust
pub async fn handle_connection<R: DeserializeOwned + 'static>(
    connection: &impl IncomingRemoteConnection,
    handler: Handler<R>,
) -> io::Result<()>
```

Similar to the noq version but works with iroh's `IncomingRemoteConnection` trait. Records the remote endpoint ID in the tracing span.

## read_request and read_request_raw (iroh variants)

Same logic as the noq versions but using `IncomingRemoteConnection` instead of `noq::Connection`:

```rust
pub async fn read_request<S: RemoteService>(
    connection: &impl IncomingRemoteConnection,
) -> io::Result<Option<S::Message>>

pub async fn read_request_raw<R: DeserializeOwned + 'static>(
    connection: &impl IncomingRemoteConnection,
) -> io::Result<Option<(R, RecvStream, SendStream)>>
```

## listen (iroh variant)

```rust
pub async fn listen<R: DeserializeOwned + 'static>(endpoint: iroh::Endpoint, handler: Handler<R>)
```

Accepts connections from an iroh `Endpoint` and handles them with the provided handler. Uses `n0_future::task::JoinSet` for task management.

## Example Usage

### Server

```rust
use irpc::{rpc_requests, channel::oneshot, Client, WithChannels};
use irpc_iroh::IrohProtocol;
use iroh::{endpoint::presets, protocol::Router, Endpoint};

#[rpc_requests(message = FooMessage)]
#[derive(Debug, Serialize, Deserialize)]
enum FooProtocol {
    #[rpc(tx=oneshot::Sender<String>)]
    Get(String),
}

async fn server() -> Result<()> {
    let (tx, rx) = tokio::sync::mpsc::channel(16);
    tokio::task::spawn(actor(rx));
    let client = Client::<FooProtocol>::local(tx);

    let endpoint = Endpoint::bind(presets::N0).await?;
    let protocol = IrohProtocol::with_sender(client.as_local().unwrap());
    let router = Router::builder(endpoint).accept(ALPN, protocol).spawn();
    // ... keep running
}
```

### Client

```rust
async fn connect(endpoint_id: EndpointId) -> Result<Client<FooProtocol>> {
    let endpoint = Endpoint::bind(presets::N0).await?;
    let client = irpc_iroh::client(endpoint, endpoint_id, ALPN);
    Ok(client)
}

// Or with direct connection:
async fn connect_direct(endpoint: Endpoint, addr: EndpointAddr) -> Result<Client<FooProtocol>> {
    let conn = endpoint.connect(addr, ALPN).await?;
    Ok(Client::boxed(IrohRemoteConnection::new(conn)))
}
```

### 0-RTT Client

```rust
async fn connect_0rtt(endpoint: Endpoint, addr: EndpointAddr) -> Result<Client<EchoProtocol>> {
    let connecting = endpoint.connect_with_opts(addr, ALPN, Default::default()).await?;
    match connecting.into_0rtt() {
        Ok(conn) => Ok(Client::boxed(IrohZrttRemoteConnection::new(conn))),
        Err(connecting) => {
            let conn = connecting.await?;
            Ok(Client::boxed(IrohRemoteConnection::new(conn)))
        }
    }
}
```