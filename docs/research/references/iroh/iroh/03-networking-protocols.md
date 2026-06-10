# Iroh: Networking & Protocol Details

## Connection Establishment

### Overview
The connection process follows this sequence:

```
Caller                                    Callee
  |                                         |
  |--- connect(EndpointAddr, alpn) -------->|  (via relay first)
  |                                         |
  |<------ TLS Handshake (Raw Public Key) ->|
  |                                         |
  |<====== QUIC Connection Established ====|
  |                                         |
  |  (iroh attempts direct path migration)  |
  |                                         |
  |--- open_bi() / open_uni() ------------->|
  |<--- accept_bi() / accept_uni() ----------|
```

### Step-by-Step

1. **Resolve addressing** — `resolve_remote(EndpointAddr)` starts a `RemoteStateActor` for the peer. If no direct addresses or relay URL are provided, Address Lookup services are queried.

2. **Map addresses** — `EndpointId` is mapped to a synthetic IPv6 address for the QUIC layer (`EndpointIdMappedAddr`). Relay and custom transport addresses are similarly mapped.

3. **TLS connection** — Uses RFC 7250 Raw Public Keys. The server name is encoded as `<z32-encoded-pubkey>.iroh.invalid`. Both sides authenticate by `EndpointId`.

4. **ALPN negotiation** — The Application-Layer Protocol Negotiation determines which protocol handler receives the connection.

5. **Path migration** — Once a QUIC connection is established (initially via relay), iroh continuously searches for better paths. Direct IP paths are preferred when available.

## Transport Layer Architecture

### The `Socket` — Core Connectivity Engine

The `Socket` struct is the heart of iroh's networking. It manages:
- Multiple transport paths (IPv4, IPv6, relay, custom)
- Address discovery and NAT traversal
- Path migration between relay and direct connections

```
                    ┌──────────────┐
                    │   Endpoint   │  (Public API)
                    │   (Arc<EndpointInner>)  │
                    └──────┬───────┘
                           │
                    ┌──────▼───────┐
                    │    Socket     │  (Connectivity engine)
                    │  (Arc<Socket>)  │
                    └──────┬───────┘
                           │
              ┌────────────┼────────────┐
              │            │             │
        ┌─────▼─────┐ ┌───▼────┐ ┌──────▼──────┐
        │IpTransport│ │Relay   │ │CustomTransport│
        │(IPv4/v6)  │ │Transport│ │(unstable)    │
        └─────┬─────┘ └───┬────┘ └──────┬──────┘
              │           │             │
        ┌─────▼─────┐ ┌───▼────┐       │
        │  UdpSocket │ │WebSocket│       │
        │  (netwatch)│ │  Actor  │       │
        └────────────┘ └────────┘       │
```

### Transport Configuration

```rust
pub enum TransportConfig {
    Ip {
        config: IpConfig,        // IPv4 or IPv6 socket config
        is_user_defined: bool,
    },
    Relay {
        relay_map: RelayMap,     // Which relay servers to use
        is_user_defined: bool,
    },
    #[cfg(feature = "unstable-custom-transports")]
    Custom(Arc<dyn CustomTransport>),
}

pub enum IpConfig {
    V4 { ip_net: Ipv4Net, port: u16, is_required: bool, is_default: bool },
    V6 { ip_net: Ipv6Net, scope_id: u32, port: u16, is_required: bool, is_default: bool },
}
```

### Address Mapping

Iroh maps all transport addresses to IPv6 for the QUIC layer:

- **IPv4/IPv6 addresses** → used directly as QUIC path addresses
- **Relay addresses** → mapped to synthetic IPv6 addresses in a dedicated range
- **Custom addresses** → mapped to synthetic IPv6 addresses in another range

The `MappedAddrs` struct maintains these mappings:
```rust
pub(crate) struct MappedAddrs {
    pub(super) endpoint_addrs: AddrMap<EndpointId, EndpointIdMappedAddr>,
    pub(super) relay_addrs: AddrMap<(RelayUrl, EndpointId), RelayMappedAddr>,
    pub(super) custom_addrs: AddrMap<CustomAddr, CustomMappedAddr>,
}
```

### Transport Bias

Path selection uses a configurable bias system:

```rust
let endpoint = Endpoint::builder(presets::N0)
    .transport_bias(AddrKind::Custom(42), TransportBias::primary())
    .bind()
    .await?;
```

Default biases:
- IPv4 and IPv6 are **primary** (IPv6 gets small RTT advantage)
- Relay is **backup** (only used when no primary transport available)

## Relay Protocol

### Architecture

The relay system is based on a revised version of Tailscale's DERP (Designated Encrypted Relay for Packets) protocol.

```
Client A            Relay Server            Client B
   │                     │                      │
   │─── HTTP CONNECT ──>|                      │
   │<── 200 OK ─────────│                      │
   │                     │<─── HTTP CONNECT ────│
   │                     │──── 200 OK ────────>│
   │                     │                      │
   │─── Encrypted QUIC ─>│─── Encrypted QUIC ─>│
   │<── Encrypted QUIC ──│<── Encrypted QUIC ──│
```

### Relay Actor

The `RelayActor` manages the WebSocket connection to the relay:
- Connects to relay via HTTPS, upgrades to custom protocol
- Sends/receives encrypted datagrams on behalf of the local endpoint
- Manages reconnection on network changes or relay restarts
- Reports connection status via `HomeRelayWatch`

### Relay Data Flow
1. Outgoing packet → `RelayTransport::send()` → `RelayActor` → WebSocket → Relay server → WebSocket → remote `RelayActor` → remote `RelayTransport::recv()` → QUIC
2. The relay only sees encrypted QUIC packets — it cannot decode application data

### Home Relay Selection

The `net_report` module continuously probes relay servers and maintains latency statistics. The "home relay" is selected based on:
- Lowest recent latency (with hysteresis to avoid flapping)
- At most a 2/3 improvement threshold to switch from current relay

## Hole-Punching & NAT Traversal

### QUIC Address Discovery (QAD)

Iroh uses QUIC Address Discovery (based on [draft-ietf-quic-address-discovery](https://datatracker.ietf.org/doc/draft-ietf-quic-address-discovery/)) to discover external IP addresses. The relay servers expose QAD endpoints.

The `net_report` module:
1. Establishes QUIC connections to relay servers
2. Uses `observed_external_addr()` to learn external addresses
3. Reports NAT type, mapping behavior, and preferred relay

### NAT Traversal Strategy

```
                    ┌──────────────────────────────┐
                    │        NAT Traversal          │
                    │                               │
                    │  1. Direct connection attempt  │
                    │     (simultaneous open)        │
                    │                               │
                    │  2. QAD-discovered addresses   │
                    │     (relay reports observed IP)│
                    │                               │
                    │  3. Port mapping (UPnP/PCP/NAT-PMP)│
                    │     (if supported by gateway)  │
                    │                               │
                    │  4. Relay fallback              │
                    │     (always available)          │
                    └──────────────────────────────┘
```

### Port Mapper

```rust
pub enum PortmapperConfig {
    Enabled {},    // Default: tries UPnP, PCP, NAT-PMP
    Disabled,      // No port mapping
}
```

When enabled, the port mapper:
- Discovers gateway devices
- Requests port mappings
- Provides external addresses to the endpoint
- Updates when mappings change

### Net Report

`NetReport` discovers network conditions:
- IPv4/IPv6 connectivity
- NAT mapping behavior (varies by destination or not)
- Captive portal detection
- Preferred relay selection
- External IP addresses (via QAD)

Key timeouts:
- `NET_REPORT_TIMEOUT` = 10 seconds
- `FULL_REPORT_INTERVAL` = 5 minutes
- `HEARTBEAT_INTERVAL` = 5 seconds (keepalive)
- `PATH_MAX_IDLE_TIMEOUT` = 15 seconds (direct)
- `RELAY_PATH_MAX_IDLE_TIMEOUT` = 30 seconds (relay)

## Address Lookup System

### Trait Definition

```rust
pub trait AddressLookup: Debug + Send + Sync + 'static {
    fn publish(&self, data: &EndpointData);
    fn resolve(&self, endpoint_id: EndpointId) -> Option<BoxStream<Result<Item, Error>>>;
}
```

### `AddressLookupServices`
A composite that runs multiple lookup services concurrently:

```rust
let services = AddressLookupServices::default();
services.set_addr_filter(AddrFilter::relay_only());
services.add(publisher);
services.add(resolver);
```

Resolution merges results from all services. Individual service errors don't block other services.

### Built-in Implementations

#### `PkarrPublisher`
Publishes endpoint info to a pkarr relay via HTTP PUT:
```rust
let publisher = PkarrPublisher::builder(pkarr_url)
    .addr_filter(AddrFilter::relay_only())  // Default: relay-only
    .build(secret_key, tls_config);
```

#### `PkarrResolver` (browser/WASM)
Resolves endpoint info from a pkarr relay via HTTP GET.

#### `DnsAddressLookup` (non-browser)
Resolves endpoint info via DNS TXT records:
```rust
// Default n0 DNS
let lookup = DnsAddressLookup::n0_dns();

// Custom DNS origin
let lookup = DnsAddressLookup::new(dns_resolver, origin);
```

#### `MemoryLookup`
In-memory address lookup for testing:
```rust
let lookup = MemoryLookup::new();
lookup.add_endpoint(endpoint_id, endpoint_data);
```

### DNS Record Format
```
_iroh.<z32-encoded-endpoint-id>.<origin-domain> TXT
```
Attributes:
- `relay=<url>` — Home relay URL
- `addr=<addr> <addr>` — Space-separated socket addresses
- `user_data=<base64-encoded-data>` — Application-specific data

## TLS Configuration

### `TlsConfig`
Manages TLS state shared across sessions:
```rust
struct TlsConfig {
    secret_key: SecretKey,
    cert_resolver: Arc<ResolveRawPublicKeyCert>,
    server_verifier: Arc<ServerCertificateVerifier>,
    client_verifier: Arc<ClientCertificateVerifier>,
    session_store: Arc<dyn ClientSessionStore>,
    crypto_provider: Arc<CryptoProvider>,
}
```

### Raw Public Key Certificate
Uses RFC 7250 — no X.509 certificates. The `ResolveRawPublicKeyCert` resolver creates TLS certificates on-the-fly from the Ed25519 public key.

### Verification Flow
- **Client verifies server**: The `ServerCertificateVerifier` checks that the server's `EndpointId` matches the expected `EndpointId` encoded in the TLS server name.
- **Server verifies client**: The `ClientCertificateVerifier` ensures the client presents a valid raw public key.

### Crypto Providers
Two built-in options via feature flags:
- `tls-ring` — uses `ring` crypto (default)
- `tls-aws-lc-rs` — uses AWS LC-RS crypto

Custom providers can be set via `Builder::crypto_provider()`.

## Multipath & Path Migration

Iroh supports QUIC multipath connections. Multiple paths can be active simultaneously:

```rust
// Watch path changes
let paths = connection.paths();
while let Some(infos) = paths.stream().next().await {
    for info in infos.iter() {
        if info.is_ip() { /* direct path */ }
        if info.is_relay() { /* relay path */ }
    }
}
```

Maximum multipath paths per connection: 12 (`MAX_MULTIPATH_PATHS`).

### Path Types
```rust
pub struct PathInfo {
    pub addr: TransportAddr,
    pub usage: TransportAddrUsage,
}

pub enum TransportAddrUsage {
    DefaultRoute,
    SubnetRoute,
    Backup,
}
```

## Connection Hooks

```rust
#[derive(Debug, Clone)]
struct MyHook;

impl EndpointHooks for MyHook {
    fn before_connect<'a>(
        &'a self,
        remote_addr: &'a EndpointAddr,
        alpn: &'a [u8],
    ) -> BoxFuture<'a, BeforeConnectOutcome> {
        Box::pin(async move {
            if is_allowed(remote_addr.id()) {
                BeforeConnectOutcome::Accept
            } else {
                BeforeConnectOutcome::Reject
            }
        })
    }

    fn after_handshake<'a>(
        &'a self,
        info: &'a ConnectionInfo,
    ) -> BoxFuture<'a, AfterHandshakeOutcome> {
        Box::pin(async move {
            AfterHandshakeOutcome::Accept
        })
    }
}
```

## Custom Transports (Unstable)

```rust
pub trait CustomTransport: Send + Sync + Debug + 'static {
    // Create an endpoint for this transport
    fn create_endpoint(&self, config: CustomEndpointConfig) -> Result<Arc<dyn CustomEndpoint>, CustomTransportError>;
}

pub trait CustomEndpoint: Send + Sync + Debug + 'static {
    fn send(&self, item: CustomSendItem) -> Result<(), CustomTransportError>;
    fn recv(&self) -> Result<CustomRecvItem, CustomTransportError>;
}

// Register:
let ep = Endpoint::builder(presets::N0)
    .add_custom_transport(Arc::new(MyTransport))
    .bind()
    .await?;
```

Transport IDs (from `TRANSPORTS.md`):

| ID | Transport | Address format |
|----|-----------|---------------|
| `0x00-0x1F` | Reserved | - |
| `0x20` | Test | Ed25519 public key (32 bytes) |
| `0x544F52` | Tor | Ed25519 public key (32 bytes) |
| `0x424C45` | BLE | Bluetooth MAC address (6 bytes) |