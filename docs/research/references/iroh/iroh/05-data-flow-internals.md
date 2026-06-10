# Iroh: Data Flow & Internal Architecture

## Data Flow: Connecting to a Remote Endpoint

```
Endpoint::connect(endpoint_addr, alpn)
         │
         ▼
resolve_remote(endpoint_addr)
         │
         ├─ If addr has direct IPs or relay URL → use those
         │
         └─ If addr is just EndpointId → query AddressLookupServices
              │
              ├─ PkarrPublisher/PkarrResolver (HTTP)
              ├─ DnsAddressLookup (DNS TXT)
              ├─ MemoryLookup (in-memory)
              └─ ...custom implementations
              │
              ▼
         Map EndpointId → MappedAddr for QUIC layer
              │
              ▼
         noq::Endpoint::connect(client_config, dest_addr, server_name)
              │
              ├─ TLS handshake with Raw Public Key authentication
              │   server_name = "<z32-encoded-endpoint-id>.iroh.invalid"
              │
              └─ QUIC connection established
                    │
                    ▼
              Connecting → Connection
                    │
                    ├─ Connection stays on relay path initially
                    │
                    └─ RemoteStateActor discovers direct paths
                         │
                         ├─ QAD-discovered addresses
                         ├─ Addresses from Address Lookup
                         ├─ Port mapper external addresses
                         │
                         └─ Path migration: relay → direct (if possible)
```

## Data Flow: Accepting Connections

```
Endpoint::accept() → Accept<'_>
         │
         ▼ (incoming QUIC packet arrives on any transport)
         │
    noq::Endpoint::accept()
         │
         ▼
    Incoming
         │
         ├─ incoming.remote_addr() → IncomingAddr (Ip/Relay/Custom)
         ├─ incoming.remote_addr_validated() → bool
         ├─ incoming.accept() → Accepting
         ├─ incoming.refuse()  → reject
         ├─ incoming.retry()   → QUIC retry (address validation)
         └─ incoming.ignore()  → drop silently
         │
    Accepting
         │
         ├─ accepting.alpn().await → alpn bytes
         ├─ accepting.into_0rtt() → (OutgoingZeroRtt, Connection)  [optional]
         └─ accepting.await → Connection
```

## Data Flow: Router Accept Loop

```
Router::spawn()
    │
    ├─ endpoint.set_alpns(registered_alpns)
    │
    └─ Loop:
         │
         ├─ endpoint.accept().await → Incoming
         │    │
         │    ├─ Apply incoming_filter (optional)
         │    │    ├─ Accept → continue
         │    │    ├─ Retry → incoming.retry()
         │    │    ├─ Reject → incoming.refuse()
         │    │    └─ Ignore → incoming.ignore()
         │    │
         │    ├─ incoming.accept() → Accepting
         │    ├─ accepting.alpn().await → determine ALPN
         │    │
         │    └─ protocols.get(alpn) → handler
         │         │
         │         ├─ handler.on_accepting(accepting).await
         │         └─ handler.accept(connection).await
         │
         └─ On shutdown:
              ├─ protocols.shutdown().await
              ├─ handler_cancel_token.cancel()
              └─ endpoint.close().await
```

## Actor Model: Per-Remote State

Each remote peer gets a `RemoteStateActor` that manages the connection state:

```
┌───────────────────────────────────────────────┐
│              RemoteStateActor                  │
│                                               │
│  ┌─────────────┐     ┌─────────────────┐     │
│  │ Address     │     │ Connection      │     │
│  │ Lookup      │     │ Tracker          │     │
│  │ Resolution  │     │                  │     │
│  └──────┬──────┘     └────────┬────────┘     │
│         │                      │               │
│         ▼                      ▼               │
│  ┌──────────────────────────────────┐          │
│  │        Path Selection           │          │
│  │  ┌────────┐ ┌────────┐        │          │
│  │  │ IPv4   │ │ IPv6   │        │          │
│  │  │primary │ │primary │        │          │
│  │  └────────┘ └────────┘        │          │
│  │  ┌────────┐ ┌────────┐        │          │
│  │  │ Relay  │ │Custom  │        │          │
│  │  │backup  │ │primary │        │          │
│  │  └────────┘ └────────┘        │          │
│  └──────────────────────────────────┘          │
│                                               │
│  ┌──────────────────────────────────┐          │
│  │        Mapped Addresses          │          │
│  │  EndpointId → MappedIPv6Addr     │          │
│  │  (RelayUrl, EndpointId) → Addr   │          │
│  │  CustomAddr → MappedIPv6Addr     │          │
│  └──────────────────────────────────┘          │
│                                               │
│  Messages:                                    │
│  ├─ ResolveRemote(EndpointAddr, reply)        │
│  ├─ AddConnection(EndpointId, WeakConn, reply)│
│  └─ RemoteInfo(reply)                          │
└───────────────────────────────────────────────┘
```

## Data Flow: Socket Actor

The `Actor` in `Socket` runs as a background task handling network changes:

```
┌────────────────────────────────────────────────────────────┐
│                      Socket Actor                           │
│                                                            │
│  ┌──────────────────┐  ┌─────────────────┐               │
│  │ Network Monitor   │  │ Direct Addr      │               │
│  │ (netwatch)        │  │ Update State     │               │
│  │                   │  │                   │               │
│  │ Detects:          │  │ Manages:          │               │
│  │ - Interface up/down│ │ - NetReport runs │               │
│  │ - Address changes  │  │ - Port mapper    │               │
│  │ - Route changes    │  │ - Direct addrs   │               │
│  └────────┬─────────┘  └────────┬──────────┘               │
│           │                      │                          │
│           ▼                      ▼                          │
│  ┌──────────────────────────────────────────────┐          │
│  │            Triggers                           │          │
│  │  - NetworkChange (major/minor)               │          │
│  │  - PeriodicReStun (every 30s-5min)           │          │
│  │  - PortmapUpdated                            │          │
│  │  - RelayMapChange                            │          │
│  │  - DirectAddrRefresh                         │          │
│  │  - ResolveRemote (from connect)              │          │
│  │  - AddConnection (from new QUIC conn)        │          │
│  └──────────────────────────────────────────────┘          │
│                                                            │
│  On address change:                                        │
│  ┌──────────────────────────────────────────────┐          │
│  │ 1. Run net_report to discover external addrs │          │
│  │ 2. Update direct_addrs watchable             │          │
│  │ 3. Publish new addresses to AddressLookup    │          │
│  │ 4. Notify noq of network changes             │          │
│  └──────────────────────────────────────────────┘          │
└────────────────────────────────────────────────────────────┘
```

## Shutdown Sequence

```
Endpoint::close()
    │
    ├─ Cancel at_close_start token
    │   (stops net_reports, address lookups)
    │
    ├─ Clear address_lookup services
    │
    ├─ noq_endpoint.close(0, b"")
    │   (refuses new connections, starts close for existing)
    │
    ├─ noq_endpoint.wait_idle().await
    │   (waits for close frames to be acknowledged)
    │
    ├─ Cancel at_endpoint_closed token
    │
    ├─ Wait for actor task (100ms timeout, then abort)
    │
    └─ runtime.shutdown().await
        (waits for all spawned tasks)
```

## WASM/Browser Differences

When compiled to `wasm32-unknown-unknown`:

| Feature | Native | WASM/Browser |
|---------|--------|-------------|
| IP transports | Yes (IPv4 + IPv6) | No (no socket access) |
| DNS resolution | `DnsAddressLookup` (system DNS) | `PkarrResolver` (HTTP) |
| Network monitoring | `netwatch` (interface changes) | Not available |
| Port mapping | UPnP/PCP/NAT-PMP | Not available |
| Net report | Full (QAD, HTTPS probes) | Limited |
| Runtime | Tokio | `wasm-bindgen-futures` |
| Timer | Tokio timer | `web::Timer` wrapping `sleep_until` |

## Thread Safety & Concurrency

- `Endpoint` is `Clone` (wraps `Arc<EndpointInner>`)
- `Socket` is `Arc<Socket>` — shared across all connections
- `RemoteMap` uses `ConcurrentReadMap` — lock-free reads for hot path
- `AddressLookupServices` uses `RwLock` — infrequent writes, frequent reads
- `DirectAddrs` uses `Watchable` — publishes changes to watchers
- `HomeRelayWatch` uses `n0_watcher::Direct` — efficient change notification

## Error Handling Patterns

Iroh uses the `n0_error::stack_error` macro for rich error chains:

```rust
#[stack_error(derive, add_meta, from_sources)]
pub enum ConnectError {
    #[error(transparent)]
    Connect { source: ConnectWithOptsError },
    #[error(transparent)]
    Connecting { source: ConnectingError },
    #[error(transparent)]
    Connection { source: ConnectionError },
}

// Usage:
// ConnectError::Connect { source: ConnectWithOptsError::SelfConnect }
// ConnectError::Connecting { source: ConnectingError::AuthenticationError { .. } }
```

## Key Constants & Timeouts

| Constant | Value | Purpose |
|----------|-------|---------|
| `HEARTBEAT_INTERVAL` | 5s | Keepalive PING interval |
| `PATH_MAX_IDLE_TIMEOUT` | 15s | Max idle before closing direct path |
| `RELAY_PATH_MAX_IDLE_TIMEOUT` | 30s | Max idle before closing relay path |
| `MAX_MULTIPATH_PATHS` | 12 | Max concurrent paths per connection |
| `DEFAULT_MAX_TLS_TICKETS` | 256 (8×32) | TLS session ticket cache size |
| `NET_REPORT_TIMEOUT` | 10s | Max time for net report |
| `FULL_REPORT_INTERVAL` | 5min | Time between full net reports |
| `DEFAULT_RELAY_QUIC_PORT` | 3478 | QAD port on relay servers |