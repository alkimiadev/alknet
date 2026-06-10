# iroh-docs: Network Protocol and Wire Format

## ALPN

The docs protocol uses ALPN `/iroh-sync/1` for QUIC connection identification.

```rust
pub const ALPN: &[u8] = b"/iroh-sync/1";
```

## Connection Flow

### Outgoing Sync (Alice — Initiator)

```rust
pub async fn connect_and_sync(
    endpoint: &Endpoint,
    sync: &SyncHandle,
    namespace: NamespaceId,
    peer: EndpointAddr,
    metrics: Option<&Metrics>,
) -> Result<SyncFinished, ConnectError>
```

1. Open a QUIC connection to the peer with ALPN `/iroh-sync/1`
2. Open a bidirectional QUIC stream
3. Run the Alice (initiator) protocol via `run_alice()`
4. Close the stream and return `SyncFinished`

### Incoming Sync (Bob — Responder)

```rust
pub async fn handle_connection<F, Fut>(
    sync: SyncHandle,
    connection: Connection,
    accept_cb: F,
    metrics: Option<&Metrics>,
) -> Result<SyncFinished, AcceptError>
```

1. Accept a bidirectional QUIC stream from the connection
2. Run the Bob (responder) protocol via `BobState::run()`
3. The `accept_cb` determines whether to accept or reject each namespace
4. Close the stream and return `SyncFinished`

## Wire Format

### Frame Codec

All messages are length-prefixed:

```
┌──────────────────────┬──────────────────────────────┐
│ u32 big-endian len   │ postcard-serialized Message   │
└──────────────────────┴──────────────────────────────┘
```

Maximum message size: 1 GiB.

### Message Types

```rust
enum Message {
    Init {
        namespace: NamespaceId,        // Which document to sync
        message: ProtocolMessage,       // Initial sync message (ranger::Message<SignedEntry>)
    },
    Sync(ProtocolMessage),              // Subsequent sync round-trip messages
    Abort { reason: AbortReason },     // Responder rejects the request
}
```

### Serialization

Messages use `postcard` (a compact `serde` format optimized for embedded/no-std use). The `SyncCodec` implements `tokio_util::codec::Encoder` and `Decoder` for async stream framing.

## Protocol Sequence

```
Alice (Initiator)                           Bob (Responder)
    │                                            │
    │──── Init { namespace, initial_msg } ───────▶│
    │                                            │
    │◀─── Sync(reply_msg) ────────────────────── │ (or Abort)
    │                                            │
    │──── Sync(next_msg) ──────────────────────▶│
    │                                            │
    │◀─── Sync(reply_msg) ────────────────────── │
    │                                            │
    │──── Sync(next_msg) ──────────────────────▶│
    │                                            │
    │         ... until convergence ...          │
    │                                            │
    │──── (stream closed) ─────────────────────▶│
    │                                            │
```

The protocol terminates when one side has no more messages to send (convergence reached). Each `Sync` message carries a `ProtocolMessage` which is a `ranger::Message<SignedEntry>` containing `MessagePart`s (either `RangeFingerprint` or `RangeItem`).

## SyncFinished Result

```rust
pub struct SyncFinished {
    pub namespace: NamespaceId,
    pub peer: PublicKey,
    pub outcome: SyncOutcome,    // heads_received, num_recv, num_sent
    pub timings: Timings,        // connect duration, process duration
}
```

## Error Types

### ConnectError

```rust
pub enum ConnectError {
    Connect { error: anyhow::Error },        // Connection failed
    RemoteAbort(AbortReason),                 // Remote rejected our request
    Sync { error: anyhow::Error },            // Sync protocol error
    Close { error: anyhow::Error },           // Stream close error
}
```

### AcceptError

```rust
pub enum AcceptError {
    Connect { error: anyhow::Error },         // Connection failed
    Open { peer: PublicKey, error },           // Failed to open replica
    Abort { peer, namespace, reason },         // We aborted
    Sync { peer, namespace, error },           // Sync protocol error
    Close { peer, namespace, error },           // Stream close error
}
```

## Gossip Integration

The `GossipState` manages iroh-gossip subscriptions per namespace:

```rust
pub struct GossipState {
    gossip: Gossip,
    sync: SyncHandle,
    to_live_actor: mpsc::Sender<ToLiveActor>,
    active: HashMap<NamespaceId, ActiveState>,
    active_tasks: JoinSet<(NamespaceId, Result<()>)>,
}
```

When a document starts syncing:
1. The engine joins a gossip topic for that namespace
2. `GossipState::join()` subscribes with bootstrap peers
3. A receive loop task is spawned to process incoming gossip messages
4. `Op` messages (Put, ContentReady, SyncReport) are deserialized and forwarded to `LiveActor`

When receiving an `Op::Put`:
```rust
// In the gossip receive loop:
let entry = SignedEntry::from_entry(...); // deserialize
sync.insert_remote(namespace, entry, from, content_status).await?;
```

When receiving an `Op::SyncReport`:
```rust
// Forward to LiveActor which checks has_news_for_us()
to_live_actor.send(ToLiveActor::IncomingSyncReport { from, report }).await?;
```

Broadcasting:
```rust
// When a local insert occurs:
gossip.broadcast(&namespace, postcard::to_stdvec(&Op::Put(entry))).await;

// When content becomes ready:
gossip.broadcast(&namespace, postcard::to_stdvec(&Op::ContentReady(hash))).await;
```

## Sync Report Compression

`SyncReport` encodes `AuthorHeads` with an optional size limit:

```rust
pub struct SyncReport {
    namespace: NamespaceId,
    heads: Vec<u8>,  // postcard-encoded AuthorHeads with size limit
}
```

The size limit ensures gossip messages stay small, dropping the oldest (least recent) author timestamps when necessary.