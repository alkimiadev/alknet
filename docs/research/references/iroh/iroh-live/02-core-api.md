# iroh-live: Core API — Live, Call, Subscription, Ticket

## `Live` — Entry Point

The primary entry point for all iroh-live operations. Manages an iroh `Endpoint`, the MoQ transport (`Moq`), and optionally a `Gossip` instance for rooms.

### Construction

```rust
// Simple: from environment, accept incoming connections
let live = Live::from_env().await?.with_router().spawn();

// With gossip for rooms
let live = Live::from_env().await?.with_router().with_gossip().spawn();

// From an existing endpoint
let live = Live::builder(endpoint).with_router().with_gossip().spawn();

// Manual router mounting (when you have other protocols)
let router = live.register_protocols(Router::builder(endpoint));
let router = router.accept(other_protocol, other_handler);
let router = router.spawn();
```

### Key Methods

| Method | Description |
|--------|-------------|
| `publish(name, &LocalBroadcast)` | Register a broadcast for all connected peers |
| `subscribe(remote, name)` | Connect to a peer and subscribe to a broadcast → `Subscription` |
| `subscribe_media(remote, name, audio, config)` | Connect, subscribe, decode → `(MoqSession, MediaTracks)` |
| `join_room(ticket)` | Join a gossip-based multi-party room → `Room` |
| `endpoint()` | Access the underlying iroh `Endpoint` |
| `transport()` | Access the `Moq` transport for advanced operations |
| `gossip()` | Access the `Gossip` instance (if enabled) |
| `shutdown()` | Close all sessions, stop router, close endpoint |

### Builder Options

- **`with_router()`** — Spawns an internal `Router` so the endpoint accepts incoming MoQ sessions. Without this, only outbound connections work.
- **`with_gossip()`** — Creates a `Gossip` instance (required for rooms). Internally mounts on the Router if `with_router` is also set.
- **`gossip(gossip)`** — Use an externally-managed `Gossip` instance.

### Internal Architecture

`Live` holds:
- `endpoint: Endpoint` — iroh QUIC endpoint
- `moq: Moq` — Internal actor for session/broadcast management
- `gossip: Option<Gossip>` — For room coordination
- `router: Option<Router>` — For accepting incoming connections

The `from_env()` method reads `IROH_SECRET` for the secret key and generates one if not set. It uses the `N0` preset for relay and DNS discovery.

## `LiveTicket` — Connection Sharing

A serializable ticket that contains everything needed to connect to a publisher.

```rust
// Create a ticket
let ticket = LiveTicket::new(endpoint.addr(), "my-stream");

// Serialize to URI string (fits in QR codes)
let s = ticket.to_string();
// → "iroh-live:<base64url(postcard(EndpointAddr))>/my-stream"

// Deserialize
let parsed: LiveTicket = s.parse()?;

// With relay URLs for indirect connectivity
let ticket = LiveTicket::new(addr, "stream").with_relay_urls(vec![
    "https://relay.example.com".to_string(),
]);
```

**Format:** `iroh-live:<base64url(postcard(EndpointAddr))>/<name>`

Also supports legacy `name@base32` format for backward compatibility.

The ticket string is kept short enough for QR codes (< 2000 bytes). It uses postcard (binary) serialization with base64url encoding.

## `Call` — 1:1 Video Call

A convenience wrapper over MoQ primitives for bidirectional calls.

### Flow

1. One side creates a `LocalBroadcast` with video/audio configured
2. **Dialer:** `Call::dial(live, remote_addr, local_broadcast)` — connects, publishes "call" broadcast, subscribes to remote's "call" broadcast
3. **Acceptor:** `Call::accept(session, local_broadcast)` — accepts an incoming session, publishes and subscribes

The broadcast name is always `"call"` — this is hardcoded (`CALL_BROADCAST_NAME`).

```rust
// Dialer side
let local = LocalBroadcast::new();
local.video().set_source(camera, VideoCodec::H264, [VideoPreset::P720])?;
let call = Call::dial(&live, remote_addr, local).await?;

// Access remote media
let remote_broadcast = call.remote();
let video = remote_broadcast.video()?;

// Wait for call to end
let reason = call.closed().await;
```

### Key Properties

- `call.local()` → `&LocalBroadcast` (your media)
- `call.remote()` → `&RemoteBroadcast` (peer's media)
- `call.signals()` → `watch::Receiver<NetworkSignals>` (for adaptive bitrate)
- `call.close()` — closes with error code 0 and reason "call ended"
- `call.closed()` → waits for close, returns `DisconnectReason` (LocalClose, RemoteClose, TransportError)

Auto-wires stats recording and network signal production on the connection.

## `Subscription` — Subscribe Handle

Created by `Live::subscribe()`. Wraps the MoQ session, remote broadcast, and network signals into a single handle. The constructor auto-wires stats recording and signal production.

```rust
let sub = live.subscribe(remote_addr, "stream").await?;

// Access components
sub.session()     // &MoqSession
sub.broadcast()   // &RemoteBroadcast
sub.signals()     // &watch::Receiver<NetworkSignals>

// Convenience methods
let tracks = sub.media(&audio_backend, Default::default()).await?;
let tracks = sub.media_with_decoders::<DefaultDecoders>(&audio_backend, config).await?;

// Decompose
let (session, broadcast, signals) = sub.into_parts();
```

## `DisconnectReason`

```rust
pub enum DisconnectReason {
    LocalClose,
    RemoteClose,
    TransportError,
}
```

Derived from the QUIC connection's close reason. Used by `Call::closed()`.

## `util` Module

### `secret_key_from_env()`

Loads the iroh secret key from the `IROH_SECRET` environment variable. Generates a new key if not set, printing the hex-encoded key for reuse.

### `spawn_signal_producer(conn, shutdown)`

Spawns a background task that polls QUIC connection path stats every 200ms and produces `NetworkSignals` for adaptive rendition selection. Returns a `watch::Receiver<NetworkSignals>`.

Computes:
- **RTT** — from `selected_path.rtt()`
- **Loss rate** — delta-based (lost packets / (sent + lost) over the interval)
- **Available bandwidth** — estimated from congestion window: `cwnd * 8 / rtt`
- **Congestion events** — monotonically increasing counter

### `spawn_stats_recorder(conn, net_stats, shutdown)`

Records connection stats (RTT, loss rate, bandwidth, path type) into `NetStats` for debug overlay display. Runs every 200ms.