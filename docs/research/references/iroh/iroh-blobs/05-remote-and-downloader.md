# iroh-blobs: Remote API and Downloader

## Remote API

The `Remote` type (`api::remote::Remote`) provides the client-side interface for interacting with remote iroh-blobs providers. It's a thin wrapper around `ApiClient` that exposes fetch, observe, and push operations.

```rust
let remote = store.remote();  // or Remote::from_sender(client)

// Get local info about what we already have
let local = remote.local(hash_and_format).await?;

// Compute what we need
let missing = local.missing();

// Execute a download
let stats = remote.execute_get(connection, request).await?;

// Or use the simpler fetch API
let progress = remote.fetch(connection, hash, format, store);
```

### LocalInfo

```rust
pub struct LocalInfo {
    pub size: Option<u64>,           // Total size if known
    pub present: ChunkRanges,        // Chunks we already have
    pub missing: ChunkRanges,        // Chunks we still need
    pub hash_and_format: HashAndFormat,
}
```

`LocalInfo` is computed by querying the local store's bitfield for a given hash and comparing it against what a full download would require.

### Fetch Process

The `fetch` method handles the complete lifecycle:

1. **Local check**: Query the store for what we already have
2. **Request computation**: If format is HashSeq, read the local HashSeq to compute precise missing ranges
3. **Connection**: Open a QUIC stream to the provider
4. **Transfer**: Use the get FSM to stream data into the store
5. **Verification**: BLAKE3 verification happens in-stream during the transfer

For HashSeq format:
- First fetch the root blob (the HashSeq)
- Parse it to get child hashes
- For each child, check local availability and compute missing ranges
- Fetch only what's missing

### Observe

```rust
// Subscribe to bitfield updates from a remote provider
let mut stream = remote.observe(connection, hash).stream().await?;
while let Some(bitfield) = stream.next().await {
    // Process availability updates
}
```

The observe protocol sends `ObserveItem` messages (size + available ranges) whenever new chunks become available on the provider. The initial message contains the full current state, subsequent messages contain deltas.

### Push

```rust
// Push local data to a remote provider
let progress = remote.push(connection, request, store);
```

Push uses the same FSM-style approach but in reverse — the local side reads from the store and writes BLAKE3-verified data to the QUIC stream.

## Downloader API

The `Downloader` (`api::downloader::Downloader`) coordinates downloads from multiple sources:

```rust
let downloader = Downloader::new(store, endpoint);

// Download from specific providers
let progress = downloader.download(DownloadRequest {
    request: FiniteRequest::Get(get_request),
    providers: vec![endpoint_id_1, endpoint_id_2],
    strategy: SplitStrategy::Split,
}).stream();
```

### SplitStrategy

```rust
pub enum SplitStrategy {
    Split,  // Split the request across multiple providers
    None,   // Use a single provider
}
```

When `SplitStrategy::Split` is used, the downloader:
1. Splits the `GetRequest` into per-child requests
2. Distributes children across available providers
3. Downloads in parallel from multiple sources
4. Stores each completed child into the local store

### DownloadRequest

```rust
pub struct DownloadRequest {
    pub request: FiniteRequest,   // What to download
    pub providers: Vec<EndpointId>, // Who to download from
    pub strategy: SplitStrategy,   // How to split work
}

pub enum FiniteRequest {
    Get(GetRequest),
    GetMany(GetManyRequest),
}
```

### Download Progress

```rust
pub enum DownloadProgressItem {
    TryProvider { id: EndpointId, request: Arc<GetRequest> },
    ProviderFailed { id: EndpointId, request: Arc<GetRequest> },
    PartComplete { request: Arc<GetRequest> },
    Progress(u64),
    DownloadError,
}
```

## Connection Pooling

The `util::connection_pool::ConnectionPool` manages reusable QUIC connections:

```rust
let pool = ConnectionPool::new(endpoint, ALPN, options);
let connection = pool.connect(endpoint_id).await?;
```

Options include connection timeout, idle timeout, and maximum connections per peer.

## Integration with iroh

### BlobsProtocol

```rust
// src/net_protocol.rs
pub struct BlobsProtocol {
    inner: Arc<BlobsInner>,  // (Store, EventSender)
}

impl ProtocolHandler for BlobsProtocol {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        crate::provider::handle_connection(conn, store, events).await;
        Ok(())
    }
    async fn shutdown(&self) { /* shutdown store */ }
}
```

Usage with iroh Router:

```rust
let endpoint = Endpoint::bind(presets::N0).await?;
let store = MemStore::new();  // or FsStore::load(path).await?
let blobs = BlobsProtocol::new(&store, None);
let router = Router::builder(endpoint)
    .accept(iroh_blobs::ALPN, blobs)
    .spawn();
```

### Creating a BlobTicket

```rust
let endpoint = Endpoint::bind(presets::N0).await?;
endpoint.online().await;
let addr = endpoint.addr();

let tag = store.add_slice(b"hello world").await?;
let ticket = BlobTicket::new(addr, tag.hash, tag.format);
println!("Share this: {ticket}");
```

### Fetching from a Ticket

```rust
// On the requester side
let ticket: BlobTicket = ticket_str.parse()?;
let (addr, hash, format) = ticket.into_parts();

let endpoint = Endpoint::bind(presets::N0).await?;
let conn = endpoint.connect(addr, iroh_blobs::ALPN).await?;

let request = match format {
    BlobFormat::Raw => GetRequest::blob(hash),
    BlobFormat::HashSeq => GetRequest::all(hash),
};

// Use the get FSM
let fsm = get::fsm::start(conn, request, RequestCounters::default());
let connected = fsm.next().await?;
// ... drive the FSM to completion
```