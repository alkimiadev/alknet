# Connection and Reconnection

This document covers how connections are established, TLS handling, the server pool, and the reconnection mechanism.

## Connector

**Location**: `connector.rs`

The `Connector` manages the server pool and handles connection establishment and reconnection.

```rust
pub(crate) struct Connector {
    servers: Vec<Server>,               // Server pool with per-server metadata
    options: ConnectorOptions,          // Connection configuration
    connect_stats: Arc<Statistics>,     // Shared statistics
    attempts: usize,                    // Global reconnection attempt counter
    events_tx: mpsc::Sender<Event>,     // Event channel
    state_tx: watch::Sender<State>,     // Connection state watcher
    max_payload: Arc<AtomicUsize>,      // Server's max payload
    last_info: ServerInfo,             // Last known server info
}
```

### Server Pool

Each server in the pool carries metadata:

```rust
#[derive(Debug, Clone)]
pub struct Server {
    pub addr: ServerAddr,
    pub failed_attempts: usize,    // Consecutive failed attempts
    pub did_connect: bool,         // Ever successfully connected?
    pub is_discovered: bool,       // Discovered via INFO, not user-configured
    pub last_error: Option<String>, // Last connection error
}
```

### ConnectorOptions

```rust
pub(crate) struct ConnectorOptions {
    pub tls_required: bool,
    pub certificates: Vec<PathBuf>,
    pub client_cert: Option<PathBuf>,
    pub client_key: Option<PathBuf>,
    pub tls_client_config: Option<rustls::ClientConfig>,
    pub tls_first: bool,
    pub auth: Auth,
    pub no_echo: bool,
    pub connection_timeout: Duration,            // Default: 5 seconds
    pub name: Option<String>,
    pub ignore_discovered_servers: bool,
    pub retain_servers_order: bool,
    pub read_buffer_capacity: u16,               // Default: 65535
    pub reconnect_delay_callback: Arc<dyn Fn(usize) -> Duration>,
    pub auth_callback: Option<CallbackArg1<Vec<u8>, Result<Auth, AuthError>>>,
    pub max_reconnects: Option<usize>,
    pub local_address: Option<SocketAddr>,
    pub reconnect_to_server_callback: Option<ReconnectToServerCallback>,
}
```

## Connection Establishment Flow

```
Connector::try_connect_to_server(addr)
    │
    ├── 1. DNS resolution
    │      server_addr.socket_addrs()
    │
    ├── 2. For each resolved address:
    │      │
    │      ├── 2a. Connect with timeout
    │      │   tokio::time::timeout(connection_timeout, try_connect_to(socket_addr, ...))
    │      │
    │      └── 2b. try_connect_to():
    │            │
    │            ├── Select transport:
    │            │   ├── "ws"  → WebSocket (tokio_websockets)
    │            │   ├── "wss" → WebSocket over TLS
    │            │   └── default → TCP (TcpStream)
    │            │
    │            ├── Optional: bind to local_address
    │            ├── Set TCP_NODELAY
    │            ├── Create Connection with read_buffer_capacity
    │            │
    │            ├── If tls_first: upgrade to TLS before INFO
    │            │
    │            ├── Read INFO from server
    │            │
    │            ├── If TLS required (by option, server, or URL scheme):
    │            │   upgrade to TLS (rustls)
    │            │
    │            ├── Discover servers from INFO.connect_urls
    │            │   (unless ignore_discovered_servers)
    │            │
    │            ├── Build ConnectInfo with auth:
    │            │   ├── username/password (from Auth or URL)
    │            │   ├── token (from Auth)
    │            │   ├── nkey + signed nonce (feature: nkeys)
    │            │   ├── JWT + signature callback (feature: nkeys)
    │            │   └── auth_callback (custom async callback)
    │            │
    │            ├── Send CONNECT + PING
    │            │
    │            └── Wait for response:
    │                ├── -ERR (authorization violation) → error
    │                ├── PONG or +OK → success
    │                └── EOF → error
    │
    └── 3. On success:
        ├── Reset attempt counter
        ├── Increment connects statistic
        ├── Emit Event::Connected
        ├── Update State::Connected
        ├── Store max_payload
        ├── Update per-server metadata (did_connect, failed_attempts)
        └── Return (ServerInfo, Connection)
```

## TLS Handling

The client supports three TLS modes:

### 1. Standard TLS (INFO → TLS)
Default behavior. The client receives the `INFO` message in plaintext, then upgrades to TLS if:
- `tls_required` option is set
- Server's `INFO.tls_required` is true
- URL scheme is `tls://`

### 2. TLS First (TLS → INFO)
When `ConnectOptions::tls_first()` is enabled, the client establishes TLS before reading INFO. This requires the server to have `handshake_first` enabled. Useful for environments where plaintext INFO is not acceptable.

### 3. WebSocket TLS
For `wss://` URLs, TLS is handled by the WebSocket library (`tokio-websockets`) directly, not by the client's TLS layer.

### TLS Configuration
The client uses `rustls` via `tokio-rustls`. Configuration steps:
1. Load root certificates from system store (`rustls-native-certs`)
2. Optionally add custom root certificates from PEM files
3. Optionally configure client certificate and key for mTLS
4. Optionally pass a custom `rustls::ClientConfig`

Crypto backend is selectable via feature flags:
- `ring` (default)
- `aws-lc-rs`
- `fips` (requires aws-lc-rs)

## Reconnection

### Reconnection Trigger

Reconnection is triggered when:
1. I/O error during read or write (`ExitReason::Disconnected`)
2. Too many pending PINGs (no PONG received)
3. User calls `Client::force_reconnect()` (`ExitReason::ReconnectRequested`)

### Reconnection Flow

```
ConnectionHandler::handle_disconnect()
    │
    ├── Reset pending_pings to 0
    ├── Emit Event::Disconnected
    ├── Update State::Disconnected
    │
    └── handle_reconnect()
        │
        └── Connector::connect()
            │
            └── Loop: try_connect()
                │
                ├── If reconnect_to_server_callback is set:
                │   │   Call callback with (server_pool, server_info)
                │   │   If returns Some(ReconnectToServer):
                │   │     Validate server is in pool
                │   │     Use callback's delay or default backoff
                │   │     Try connecting to selected server
                │   └── If None or invalid: fall through to default
                │
                ├── Default selection:
                │   ├── Shuffle servers (unless retain_servers_order)
                │   ├── Sort by failed_attempts (ascending)
                │   └── Try each server in order
                │
                ├── For each server:
                │   ├── Increment attempts counter
                │   ├── Check max_reconnects limit
                │   ├── Apply reconnect delay (exponential backoff)
                │   └── try_connect_to_server(addr)
                │
                ├── On success:
                │   ├── Reset attempts to 0
                │   ├── Re-subscribe all active subscriptions
                │   │   (filter out closed subscription channels)
                │   ├── Re-subscribe multiplexer wildcard
                │   └── Return (ServerInfo, Connection)
                │
                └── On failure:
                    ├── Update per-server metadata (failed_attempts, last_error)
                    ├── Auth errors → propagate immediately
                    └── Other errors → continue to next server
```

### Exponential Backoff

Default reconnect delay function:

```rust
fn reconnect_delay_callback_default(attempts: usize) -> Duration {
    if attempts <= 1 {
        Duration::from_millis(0)
    } else {
        let exp: u32 = (attempts - 1).try_into().unwrap_or(u32::MAX);
        let max = Duration::from_secs(4);
        cmp::min(Duration::from_millis(2_u64.saturating_pow(exp)), max)
    }
}
```

| Attempt | Delay |
|---------|-------|
| 1 | 0ms |
| 2 | 0ms |
| 3 | 2ms |
| 4 | 4ms |
| 5 | 8ms |
| ... | ... |
| 13 | 4096ms |
| 14+ | 4000ms (capped) |

Custom delay functions can be provided via `ConnectOptions::reconnect_delay_callback()`.

### Server Pool Updates

The server pool is dynamic:

1. **Initial pool**: from `connect()` / `ConnectOptions::connect()` URL(s)
2. **Discovered servers**: added from `INFO.connect_urls` on each connection (unless `ignore_discovered_servers` is set)
3. **Runtime updates**: via `Client::set_server_pool()` — replaces the entire pool while preserving per-server state for servers that appear in both old and new pools
4. **Order**: servers are shuffled by default (random selection), unless `retain_servers_order` is set

### Max Reconnects

The `max_reconnects` option limits total reconnection attempts:
- `None` or `0` → unlimited (default)
- `Some(n)` → give up after `n` total attempts
- Counter is reset on successful connection and when `set_server_pool()` is called

## ConnectOptions Defaults

| Option | Default |
|--------|---------|
| `connection_timeout` | 5 seconds |
| `ping_interval` | 60 seconds |
| `sender_capacity` | 2048 |
| `subscription_capacity` | 65536 |
| `inbox_prefix` | `"_INBOX"` |
| `request_timeout` | 10 seconds |
| `retry_on_initial_connect` | false |
| `ignore_discovered_servers` | false |
| `retain_servers_order` | false |
| `read_buffer_capacity` | 65535 |
| `skip_subject_validation` | false |
| `no_echo` | false |
| `tls_required` | false |
| `tls_first` | false |
| `max_reconnects` | None (unlimited) |

## Background Connection

When `ConnectOptions::retry_on_initial_connect()` is enabled, the `connect()` function returns a `Client` immediately, before the connection is established. The connection is established in a background Tokio task. This means:
- `client.server_info()` returns `ServerInfo::default()` until connected
- `client.connection_state()` returns `State::Pending`
- Operations like `publish()` will queue in the command channel
- The `Client` becomes usable once the background task connects