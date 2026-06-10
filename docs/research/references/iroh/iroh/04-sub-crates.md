# Iroh: Sub-Crates

## `iroh-base`

**Purpose**: Fundamental types shared across all iroh crates.  
**Features**: `key` (default), `relay` (default)

### Key Types

| Type | Description |
|------|-------------|
| `SecretKey` | Ed25519 signing key (32 bytes). Generated randomly or from bytes. |
| `PublicKey` | Ed25519 public key (32 bytes). Verifies signatures. |
| `EndpointId` | Type alias for `PublicKey` — used as network identity. |
| `Signature` | Ed25519 signature (64 bytes). |
| `RelayUrl` | Arc-wrapped `Url` identifying a relay server. |
| `EndpointAddr` | Combines `EndpointId` + `BTreeSet<TransportAddr>`. Primary addressing type. |
| `TransportAddr` | Enum: `Relay(RelayUrl)`, `Ip(SocketAddr)`, `Custom(CustomAddr)`. |
| `CustomAddr` | Opaque address for custom transports (id + bytes). |
| `KeyParsingError` | Error type for key parsing. |
| `RelayUrlParseError` | Error type for URL parsing. |

### `EndpointAddr` Methods

```rust
impl EndpointAddr {
    pub fn new(id: PublicKey) -> Self;
    pub fn from_parts(id: PublicKey, addrs: impl IntoIterator<Item = TransportAddr>) -> Self;
    pub fn with_relay_url(self, relay_url: RelayUrl) -> Self;
    pub fn with_ip_addr(self, addr: SocketAddr) -> Self;
    pub fn with_addrs(self, addrs: impl IntoIterator<Item = TransportAddr>) -> Self;
    pub fn is_empty(&self) -> bool;
    pub fn ip_addrs(&self) -> impl Iterator<Item = &SocketAddr>;
    pub fn relay_urls(&self) -> impl Iterator<Item = &RelayUrl>;
}
```

### Serialization
- `PublicKey`/`EndpointId`: Human-readable → base32 z-base-32; Binary → 32 raw bytes
- `EndpointAddr`: Serialized as `{id, addrs}` with `TransportAddr` as tagged enum
- `RelayUrl`: Serialized as URL string

---

## `iroh-dns`

**Purpose**: DNS resolver and endpoint info serialization for address discovery.  
**Key Features**: pkarr signed packet creation/verification, DNS TXT record parsing, configurable DNS resolver.

### Modules

| Module | Description |
|--------|-------------|
| `dns` | `DnsResolver` — configurable async DNS resolver with IPv4/IPv6 staggered lookup |
| `endpoint_info` | `EndpointInfo`, `EndpointData`, `AddrFilter`, `UserData` — serialization/deserialization |
| `pkarr` | Pkarr signed packet creation and verification |
| `attrs` | Low-level TXT record attribute parsing |

### `DnsResolver`

```rust
impl DnsResolver {
    pub fn new() -> Self;
    pub fn with_nameserver(addr: SocketAddr) -> Self;
    pub fn with_nameservers(addrs: Vec<SocketAddr>) -> Self;

    // Lookup methods
    pub async fn lookup_ipv4(&self, host: String) -> Result<...>;
    pub async fn lookup_ipv6(&self, host: String) -> Result<...>;
    pub async fn lookup_ipv4_ipv6_staggered(&self, host: &str, timeout: Duration, delays: &[u64]) -> Result<...>;
    pub async fn lookup_txt(&self, host: String) -> Result<...>;
    pub async fn lookup_endpoint_by_id(&self, id: &EndpointId, origin: &str) -> Result<EndpointInfo>;

    // Cache management
    pub fn clear_cache(&self);
    pub fn reset_resolver(&self);
}
```

### `EndpointInfo` & `EndpointData`

```rust
pub struct EndpointInfo {
    pub endpoint_id: EndpointId,
    pub data: EndpointData,
}

pub struct EndpointData {
    addrs: Vec<TransportAddr>,
    user_data: Option<UserData>,
}

impl EndpointData {
    pub fn new(addrs: Vec<TransportAddr>) -> Self;
    pub fn from_iter(addrs: impl IntoIterator<Item = TransportAddr>) -> Self;
    pub fn with_user_data(mut self, user_data: UserData) -> Self;
    pub fn addrs(&self) -> impl Iterator<Item = &TransportAddr>;
    pub fn user_data(&self) -> Option<&UserData>;
    pub fn apply_filter(&self, filter: &AddrFilter) -> Cow<'_, EndpointData>;
}
```

### `AddrFilter`

Controls which addresses are published in address lookup:

```rust
pub enum AddrFilter {
    RelayOnly,       // Only relay URLs
    Unfiltered,      // All addresses
    Custom(fn(&[TransportAddr]) -> Vec<TransportAddr>),
}
```

### Pkarr Integration

```rust
// Creating signed packets
let info = EndpointInfo::new(secret_key.public())
    .with_relay_url(relay_url);
let packet = info.to_pkarr_signed_packet(&secret_key, 30)?; // 30 second TTL

// Verifying and extracting
let info = EndpointInfo::from_pkarr_signed_packet(&packet)?;
```

---

## `iroh-relay`

**Purpose**: Relay server and client implementation. Provides DERP-like relay protocol, QAD support, and relay server binary.

### Key Exports

| Type | Description |
|------|-------------|
| `RelayMap` | Thread-safe map of `RelayUrl → RelayConfig` |
| `RelayConfig` | Configuration for a single relay server |
| `RelayQuicConfig` | QUIC address discovery configuration |
| `KeyCache` | Cache for relay server public keys |
| `PingTracker` | Ping/pong tracking for relay connections |
| `MAX_PACKET_SIZE` | Maximum relay packet size (64KB - overhead) |

### Modules

| Module | Description |
|--------|-------------|
| `client` | HTTP client for relay server connections |
| `http` | HTTP-related relay functionality |
| `protos` | Protocol definitions (handshake, relay, streams) |
| `quic` | QUIC client for QAD probing |
| `server` | Full relay server implementation (`feature = "server"`) |
| `tls` | TLS configuration utilities |

### `RelayConfig`

```rust
pub struct RelayConfig {
    pub url: RelayUrl,
    pub quic: Option<RelayQuicConfig>,
}

impl RelayConfig {
    pub fn new(url: RelayUrl, quic: Option<RelayQuicConfig>) -> Self;
    pub fn from(url: RelayUrl) -> Self;  // No QAD
}
```

### `RelayMap`

```rust
impl RelayMap {
    pub fn empty() -> Self;
    pub fn from(relay: RelayConfig) -> Self;
    pub fn from_iter(iter: impl IntoIterator<Item = impl Into<RelayConfig>>) -> Self;
    pub fn try_from_iter(iter: impl IntoIterator<Item = &str>) -> Result<Self, RelayUrlParseError>;
    pub fn insert(&self, url: RelayUrl, config: Arc<RelayConfig>) -> Option<Arc<RelayConfig>>;
    pub fn remove(&self, url: &RelayUrl) -> Option<Arc<RelayConfig>>;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn urls<T: FromIterator<RelayUrl>>(&self) -> T;
    pub fn relays<T: FromIterator<Arc<RelayConfig>>>(&self) -> T;
}
```

### Relay Protocol (DERP-like)

The relay protocol is based on Tailscale's DERP protocol, adapted for iroh:

1. Client connects via HTTPS, upgrades to custom protocol
2. Authentication via raw public key (Ed25519)
3. Encrypted datagram forwarding by `EndpointId`
4. QAD probes via QUIC for address discovery
5. Ping/pong keepalive mechanism

### TLS Utilities

```rust
pub use iroh_relay::tls::{CaRootsConfig, default_provider};

// Skip certificate verification (testing only)
let config = CaRootsConfig::insecure_skip_verify();

// Use system trust roots
let config = CaRootsConfig::platform_verifier();

// Use specific roots
let config = CaRootsConfig::from_pem(pem_bytes);
```

---

## `iroh-dns-server`

**Purpose**: DNS server that resolves iroh `EndpointId`s to addressing information. Powers `dns.iroh.link`.

### Key Features
- Serves DNS TXT records for `_iroh.<z32-endpoint-id>.<origin>` queries
- Integrates with pkarr for signed record verification
- Supports production (`dns.iroh.link`) and staging (`staging-dns.iroh.link`) origins
- Includes benchmarking support

### Configuration Files
- `config.dev.toml` — Development configuration
- `config.prod.toml` — Production configuration

---

## Internal Modules in `iroh` Crate

### `socket` Module
The connectivity layer — manages the `Socket` struct that orchestrates:
- Multiple transport paths
- Network change detection
- Address discovery and publication
- Remote state actors (per-peer state machines)

**Key sub-modules**:

| Sub-module | Description |
|-----------|-------------|
| `transports/` | Transport implementations (IP, relay, custom) |
| `transports/ip.rs` | IPv4/IPv6 UDP transport |
| `transports/relay.rs` | Relay WebSocket transport |
| `transports/relay/actor.rs` | Relay connection management actor |
| `transports/custom.rs` | Unstable custom transport API |
| `remote_map.rs` | Per-peer `RemoteStateActor` management |
| `remote_map/remote_state.rs` | State machine for connecting to a peer |
| `mapped_addrs.rs` | Address mapping for QUIC layer |
| `concurrent_read_map.rs` | Lock-free concurrent map for remote actors |
| `metrics.rs` | Socket-level metrics |

### `net_report` Module
Network condition reporter:
- Discovers external IP addresses (QAD)
- Measures relay latencies
- Detects NAT types
- Detects captive portals
- Selects preferred relay

### `portmapper` Module
UPnP/PCP/NAT-PMP port mapping:
- Gateway discovery
- Port mapping procurement
- External address monitoring

### `address_lookup` Module
Pluggable address discovery:

| Sub-module | Description |
|-----------|-------------|
| `dns.rs` | `DnsAddressLookup` — resolves via DNS TXT records |
| `pkarr.rs` | `PkarrPublisher` — publishes via HTTP PUT to pkarr relay; `PkarrResolver` — resolves from pkarr relay |
| `memory.rs` | `MemoryLookup` — in-memory lookup for testing |

### `runtime` Module
Tokio-based async runtime wrapper for `noq`:
- Task spawning with cancellation support
- Timer management
- Graceful and abrupt shutdown
- WASM browser support (delegates to `wasm-bindgen-futures`)

### `defaults` Module
Default configuration values:
- Production relay servers (4 regions)
- Staging relay servers (2 regions)
- Timeout constants
- Environment variable for forcing staging (`IROH_FORCE_STAGING_RELAYS`)

### `metrics` Module
`EndpointMetrics` collection:
- Socket metrics (datagrams sent/received, data by transport type)
- Net report metrics (reports generated, full vs incremental)
- Port mapper metrics