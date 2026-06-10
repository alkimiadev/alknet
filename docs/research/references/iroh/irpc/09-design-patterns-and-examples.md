# irpc: Design Patterns and Usage Examples

## Pattern 1: Actor Model (Most Common)

The primary usage pattern is an actor that receives messages and processes them sequentially:

```rust
struct StorageActor {
    recv: tokio::sync::mpsc::Receiver<StorageMessage>,
    state: BTreeMap<String, String>,
}

impl StorageActor {
    pub fn spawn() -> StorageApi {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let actor = Self { recv: rx, state: BTreeMap::new() };
        tokio::task::spawn(actor.run());
        StorageApi { inner: Client::local(tx) }
    }

    async fn run(mut self) {
        while let Some(msg) = self.recv.recv().await {
            self.handle(msg).await;
        }
    }

    async fn handle(&mut self, msg: StorageMessage) {
        match msg {
            StorageMessage::Get(wc) => {
                let WithChannels { inner, tx, .. } = wc;
                tx.send(self.state.get(&inner.key).cloned()).await.ok();
            }
            StorageMessage::Set(wc) => {
                let WithChannels { inner, tx, .. } = wc;
                self.state.insert(inner.key, inner.value);
                tx.send(()).await.ok();
            }
        }
    }
}
```

**Key points:**
- The actor owns state and processes messages sequentially
- `Client::local(tx)` wraps the sender side of the mpsc channel
- `WithChannels` destructuring gives access to `inner` (the request data), `tx` (response channel), and `rx` (update channel)
- The `..` pattern ignores `rx` when it's `NoReceiver` and `span` (with `spans` feature)

## Pattern 2: Concurrent Task Per Request

For long-running or independent requests, spawn a task per message:

```rust
async fn run(mut self) {
    while let Ok(Some(msg)) = self.recv.recv().await {
        tokio::task::spawn(async move {
            if let Err(cause) = Self::handle(msg).await {
                eprintln!("Error: {cause}");
            }
        });
    }
}
```

This is useful for CPU-intensive or I/O-bound requests that shouldn't block other requests.

## Pattern 3: Local-Only Usage

irpc can be used without any RPC feature for pure in-process communication:

```rust
// Cargo.toml: default-features = false, features = ["derive"]
#[rpc_requests(message = StorageMessage, no_rpc, no_spans)]
#[derive(Serialize, Deserialize, Debug)]
enum StorageProtocol {
    #[rpc(tx=oneshot::Sender<Option<String>>)]
    Get(Get),
    #[rpc(tx=oneshot::Sender<()>)]
    Set(Set),
}
```

The `no_rpc` flag prevents `RemoteService` from being generated, and `no_spans` removes the tracing dependency. This leaves only the local channel mechanism, with minimal dependencies (serde, tokio, tokio-util).

## Pattern 4: API Type Wrapping Client

The recommended pattern is to wrap `Client<S>` in a higher-level API type:

```rust
struct StorageApi {
    inner: Client<StorageProtocol>,
}

impl StorageApi {
    // Local
    pub fn spawn() -> Self {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        tokio::task::spawn(StorageActor::new(rx).run());
        Self { inner: Client::local(tx) }
    }

    // Remote (noq)
    pub fn connect(endpoint: noq::Endpoint, addr: SocketAddr) -> Self {
        Self { inner: Client::noq(endpoint, addr) }
    }

    // Remote (iroh)
    pub fn connect_iroh(endpoint: iroh::Endpoint, addr: EndpointAddr) -> Self {
        Self { inner: irpc_iroh::client(endpoint, addr, ALPN) }
    }

    // Type-safe methods that work for both local and remote
    pub async fn get(&self, key: String) -> irpc::Result<Option<String>> {
        self.inner.rpc(Get { key }).await
    }

    pub async fn set(&self, key: String, value: String) -> irpc::Result<()> {
        self.inner.rpc(Set { key, value }).await
    }

    pub async fn list(&self) -> irpc::Result<mpsc::Receiver<String>> {
        self.inner.server_streaming(List, 16).await
    }
}
```

This encapsulates the protocol details and provides a clean, type-safe API. The same `StorageApi` works identically whether connected locally or remotely.

## Pattern 5: Server Setup

### With noq

```rust
fn serve(api: &StorageApi, endpoint: noq::Endpoint) -> Result<JoinHandle<()>> {
    let local = api.inner.as_local().context("cannot listen on remote service")?;
    let handler = StorageProtocol::remote_handler(local);
    Ok(tokio::task::spawn(irpc::rpc::listen(endpoint, handler)))
}
```

### With iroh

```rust
fn serve(api: &StorageApi, endpoint: iroh::Endpoint) -> Result<Router> {
    let local = api.inner.as_local().context("cannot listen on remote service")?;
    let protocol = IrohProtocol::with_sender(local);
    Ok(Router::builder(endpoint).accept(ALPN, protocol).spawn())
}
```

## Pattern 6: Low-Level Request Handling

For more control than the `Client` methods provide, use `request()` directly:

```rust
async fn custom_request(&self, msg: Get) -> anyhow::Result<oneshot::Receiver<Option<String>>> {
    match self.inner.request().await? {
        Request::Local(request) => {
            let (tx, rx) = oneshot::channel();
            request.send((msg, tx)).await?;
            Ok(rx)
        }
        Request::Remote(request) => {
            let (_tx, rx) = request.write(msg).await?;
            Ok(rx.into())
        }
    }
}
```

This allows custom channel creation logic, e.g., different buffer sizes for local vs remote.

## Pattern 7: Channel Filtering and Mapping

irpc channels support filtering and mapping, which work for both local and remote channels:

```rust
// Server-side: filter responses to only include values > 10
let filtered_tx = wc.tx.with_filter(|v: &i64| *v > 10);

// Server-side: transform responses
let mapped_tx = wc.tx.with_map(|v: i64| v * 2);

// Client-side: filter received updates
let filtered_rx = rx.filter(|update: &Update| update.is_relevant());
```

For remote channels, these create boxed wrappers. For local channels, they also create boxed wrappers. The overhead is negligible for remote (network latency dominates) but present for local.

## Pattern 8: Using the `wrap` Attribute

The `#[wrap]` attribute generates named structs from variant fields:

```rust
#[rpc_requests(message = StoreMessage)]
#[derive(Debug, Serialize, Deserialize)]
enum StoreProtocol {
    #[rpc(tx=oneshot::Sender<Option<String>>)]
    #[wrap(GetRequest, derive(Clone))]
    Get(String),          // Generates: pub struct GetRequest(pub String);

    #[rpc(tx=oneshot::Sender<()>)]
    #[wrap(SetRequest)]
    Set { key: String, value: String },  // Generates: pub struct SetRequest { pub key: String, pub value: String }
}
```

Benefits:
- Named request types can be imported and constructed by name
- Additional derives (e.g., `Clone`) can be added
- Custom visibility can be specified: `#[wrap(pub(crate) GetRequest)]`
- The generated struct inherits the enum's visibility by default

## Pattern 9: 0-RTT Connections

For reduced latency on reconnections with iroh:

```rust
// Client side
let result = client.rpc_0rtt(Get { key: "x".into() }).await?;

// Server side (iroh)
let protocol = Iroh0RttProtocol::with_sender(local_sender);
let router = Router::builder(endpoint).accept(ALPN, protocol).spawn();
```

**Important:** Only use 0-RTT for idempotent operations, as the data may be replayed by an attacker.

## Pattern 10: Shared State in Actor

For actors that need shared state accessible from multiple handlers:

```rust
struct Actor {
    recv: tokio::sync::mpsc::Receiver<Message>,
    state: Arc<Mutex<SharedState>>,
}
```

Or use the actor pattern with internal mutation:

```rust
struct Actor {
    recv: tokio::sync::mpsc::Receiver<Message>,
    db: HashMap<String, String>,  // owned state
}
```

Since the actor processes messages sequentially, no internal synchronization is needed.