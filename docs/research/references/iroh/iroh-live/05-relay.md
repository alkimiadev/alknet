# iroh-live-relay: Browser Bridging

## Overview

The relay server bridges iroh P2P streams to browser clients via WebTransport. Browsers cannot speak iroh's QUIC protocol directly, so the relay accepts WebTransport connections and either serves locally-published broadcasts or pulls them from remote iroh publishers on demand.

**Architecture:**

```
iroh-live publish --(iroh P2P)--> iroh-live-relay <--(WebTransport)-- browser
browser --(WebTransport)--> iroh-live-relay --(iroh P2P)--> iroh-live subscribe
```

## Components

### `RelayConfig` (CLI Configuration)

```rust
pub struct RelayConfig {
    pub bind: SocketAddr,      // QUIC/WebTransport bind (default: [::]:4443)
    pub http_bind: SocketAddr,  // HTTP static files bind (default: same as bind)
}
```

Flattenable into a clap CLI via `#[command(flatten)]`.

### `run(config)` — Main Server Loop

The main entry point. Sets up:

1. **QUIC/WebTransport server** — Uses `moq-native::ServerConfig` with:
   - QUIC backend: `noq` (a custom QUIC implementation)
   - iroh endpoint integration
   - Self-signed TLS certificates (dev mode) for `localhost`
   - Max streams: `moq_relay::DEFAULT_MAX_STREAMS`

2. **iroh endpoint** — Binds an iroh endpoint for P2P connectivity, prints its ID

3. **moq-relay Cluster** — The broadcast routing engine. Manages broadcast lifecycle: when all subscribers disconnect, the broadcast is removed.

4. **HTTP server** — Axum router serving:
   - `GET /certificate.sha256` — TLS fingerprint for dev mode
   - `GET /` — Web viewer landing page
   - `GET /{path}` — Static file serving with CORS
   - Embedded via `include_dir!` from `web/dist/`

5. **Pull mode** — If iroh endpoint is available, creates a `PullState` for on-demand remote broadcast fetching

6. **Connection loop** — Accepts incoming connections, parses the URL path as a `LiveTicket`, and if valid, triggers a pull before running the connection

### `PullState` — On-Demand Remote Fetching

When a browser connects with a broadcast name that is a valid `LiveTicket`, the relay:

1. Checks if the broadcast already exists in the cluster (fast path)
2. If not, connects to the remote publisher via iroh-live's `Moq::connect()`
3. Subscribes to the remote broadcast
4. Publishes the consumer into the local cluster under the ticket string as the name
5. Spawns a keepalive task that holds the session until it closes

**Concurrency:** Duplicate concurrent pulls for the same ticket are deduplicated using a `HashMap<String, Arc<Notify>>`. Waiters block on the `Notify` until the first connector finishes.

```rust
pub(crate) struct PullState {
    live: iroh_live::Live,
    cluster: Cluster,
    connecting: Arc<Mutex<HashMap<String, Arc<Notify>>>>>,
}
```

### Web Viewer

The relay embeds a SolidJS + TypeScript web application compiled by Vite. It uses:
- `@moq/watch` — Web component for watching streams via WebCodecs
- `@moq/publish` — Web component for publishing from browser camera/mic
- WebTransport — For QUIC connectivity from the browser

Watch URLs: `https://relay:4443/?name=<BROADCAST_OR_TICKET>`

### Data Directory

The relay persists data to `$IROH_LIVE_RELAY_DATA` (or the platform default). This includes:
- iroh secret key (`iroh_secret_key`) — ensures endpoint ID stability across restarts
- TLS certificates

### TLS and Certificates

Currently **self-signed only**. ACME/Let's Encrypt is planned but not implemented. In dev mode, browsers need `--ignore-certificate-errors` or the relay's fingerprint (served at `/certificate.sha256`) for WebTransport to work.

## Error Handling

No authentication is implemented yet. The relay accepts all connections. MoQ supports token-based authentication which could be added.

## CLI Binary

```rust
// iroh-live-relay/src/main.rs
#[derive(Parser)]
struct Cli {
    #[command(flatten)]
    relay: RelayConfig,
}
```

Must call `rustls::crypto::aws_lc_rs::default_provider().install_default()` before `run()`.