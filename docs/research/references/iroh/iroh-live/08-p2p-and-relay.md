# iroh-live: P2P Connectivity and Relay Architecture

## Direct Connectivity

iroh connects peers directly when possible:

- **Same LAN:** Communicates over the local network without traffic leaving the subnet
- **Public IP / simple NAT:** iroh's hole-punching establishes a direct UDP path
- **Symmetric NAT / corporate firewalls / CGNAT:** Falls back to iroh relay network

The iroh endpoint exposes path statistics via `conn.paths()`, which returns a `Watcher<PathInfoList>`. Each `PathInfo` reports RTT, whether the path is selected, and the remote address. The selected path is the one actively carrying traffic; iroh may maintain multiple candidate paths and switch between them.

The transition between direct and relayed paths is transparent to the application. The media pipeline sees only changes in RTT and bandwidth, which adaptive rendition switching handles automatically.

## iroh-live-relay: Architecture

The relay serves two transport protocols simultaneously:

```
iroh P2P publisher ──(QUIC, moq-lite-03)──> iroh-live-relay <──(WebTransport/H3, noq)── browser
```

Both protocols feed into `moq-relay`'s shared `Origin`, which manages broadcast routing. A broadcast published via iroh is automatically available to WebTransport subscribers, and vice versa.

### Pull Model

The relay operates in **pull mode**: it connects to iroh publishers on demand when a browser client requests a broadcast. The broadcast name in the URL can be a `LiveTicket` URI. Multiple browser clients watching the same broadcast share a single upstream iroh connection.

Pull flow:
1. Browser connects via WebTransport, requests broadcast by name (or ticket)
2. Relay checks if broadcast already exists in local cluster → fast path
3. If not, relay uses iroh-live `Moq::connect()` to connect to the remote publisher
4. Subscribes to the broadcast via `session.subscribe(broadcast_name)`
5. Publishes the consumer into the local cluster under the ticket string as the name
6. Spawns a keepalive task holding the session until it closes
7. Browser receives the stream through the relay's WebTransport frontend

### Connection Deduplication

`PullState` uses a `HashMap<String, Arc<Notify>>` to prevent duplicate concurrent connections to the same remote. If a pull is already in progress for a given ticket, subsequent requests wait on the `Notify` and then check if the broadcast appeared in the cluster.

### QUIC Backend: noq

The relay uses `noq` as its QUIC backend (not quinn). This is configured via:

```rust
server_config.backend = Some(moq_native::QuicBackend::Noq);
```

### iroh Endpoint Integration

The relay also binds an iroh endpoint:

```rust
let mut iroh_config = moq_native::IrohEndpointConfig::default();
iroh_config.enabled = Some(true);
iroh_config.secret = Some(relay.iroh_secret_path_str());
let iroh = iroh_config.bind().await?;
```

This enables the relay to participate in the iroh P2P network directly.

## Ticket Format

`LiveTicket` serves as the connection mechanism for both P2P and relay scenarios:

- **P2P:** Subscriber uses the `EndpointAddr` (node ID + relay URLs) to connect directly
- **Relay:** The full ticket string becomes the broadcast name in the URL: `https://relay:4443/?name=iroh-live:...`

The ticket format: `iroh-live:<base64url(postcard(EndpointAddr))>/<broadcast_name>`

It also supports a legacy format: `<name>@<base32(postcard(EndpointAddr))>`

## Connection Access in iroh-moq

`MoqSession::conn()` returns a reference to the underlying iroh `Connection`. This is used by:

1. **Signal producer** — Polls path stats for `NetworkSignals`
2. **Stats recorder** — Records into `NetStats` for debug overlays
3. **Call::closed()** — Inspects QUIC close reason to determine `DisconnectReason`

The connection provides:
- `paths().get()` — List of active network paths with RTT, stats, relay status
- `close_reason()` — Why the connection closed (LocallyClosed, ApplicationClosed, ConnectionClosed, Reset)
- `remote_id()` — Remote peer's endpoint ID