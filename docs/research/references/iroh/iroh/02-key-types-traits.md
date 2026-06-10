# Iroh: Key Types and Traits

## Core Identity Types (`iroh-base`)

### `SecretKey`
Ed25519 signing key (32 bytes). Used for:
- TLS authentication (RFC 7250 Raw Public Key)
- Signing pkarr packets for address discovery
- Generating the corresponding `PublicKey`/`EndpointId`

```rust
// Generation
let secret_key = SecretKey::generate();

// From bytes
let secret_key = SecretKey::from_bytes(&[0u8; 32]);

// Access public key
let public_key: PublicKey = secret_key.public();
```

### `PublicKey` / `EndpointId`
`EndpointId` is a type alias for `PublicKey`. Both are 32-byte Ed25519 compressed points.

```rust
pub type EndpointId = PublicKey;

impl PublicKey {
    pub const LENGTH: usize = 32;
    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self, KeyParsingError>;
    pub fn as_bytes(&self) -> &[u8; 32];
    pub fn verify(&self, message: &[u8], signature: &Signature) -> Result<(), SignatureError>;
    pub fn fmt_short(&self) -> impl Display;  // First 5 bytes hex
}
```

Serialization: Human-readable → base32 z-base-32 encoding; Binary → 32 raw bytes.

### `Signature`
Ed25519 signature (64 bytes). Used in pkarr for signing endpoint discovery records.

### `KeyParsingError`
Error type for key parsing failures.

## Addressing Types (`iroh-base`)

### `EndpointAddr`
The primary addressing type — combines identity with network paths:

```rust
pub struct EndpointAddr {
    pub id: EndpointId,
    pub addrs: BTreeSet<TransportAddr>,
}

impl EndpointAddr {
    pub fn new(id: PublicKey) -> Self;
    pub fn from_parts(id: PublicKey, addrs: impl IntoIterator<Item = TransportAddr>) -> Self;
    pub fn with_relay_url(self, relay_url: RelayUrl) -> Self;
    pub fn with_ip_addr(self, addr: SocketAddr) -> Self;
    pub fn is_empty(&self) -> bool;
    pub fn ip_addrs(&self) -> impl Iterator<Item = &SocketAddr>;
    pub fn relay_urls(&self) -> impl Iterator<Item = &RelayUrl>;
}
```

Can be constructed from just an `EndpointId` (relies on Address Lookup), or with explicit paths:
```rust
// From just EndpointId — needs Address Lookup
let addr = EndpointAddr::new(endpoint_id);

// With relay URL
let addr = EndpointAddr::new(endpoint_id).with_relay_url(relay_url);

// With both
let addr = EndpointAddr::from_parts(endpoint_id, [
    TransportAddr::Relay(relay_url),
    TransportAddr::Ip(socket_addr),
]);
```

### `TransportAddr`
```rust
pub enum TransportAddr {
    Relay(RelayUrl),
    Ip(SocketAddr),
    Custom(CustomAddr),
}
```

### `CustomAddr`
Opaque custom transport address (for `unstable-custom-transports` feature):
```rust
pub struct CustomAddr {
    id: u32,
    addr: Vec<u8>,
}
```

### `RelayUrl`
Arc-wrapped `Url` identifying a relay server. Cheaply clonable. Encourages fully-qualified DNS names (trailing dot).

```rust
let url: RelayUrl = "https://use1-1.relay.n0.iroh-canary.iroh.link.".parse()?;
```

## Endpoint Trait (`iroh`)

### `Endpoint`
The central type — created via `Builder`, used for all connection operations:

```rust
impl Endpoint {
    // Construction
    pub fn builder(preset: impl Preset) -> Builder;
    pub async fn bind(preset: impl Preset) -> Result<Self, BindError>;

    // Connection
    pub async fn connect(&self, addr: impl Into<EndpointAddr>, alpn: &[u8]) -> Result<Connection, ConnectError>;
    pub async fn connect_with_opts(&self, addr: impl Into<EndpointAddr>, alpn: &[u8], opts: ConnectOptions) -> Result<Connecting, ConnectWithOptsError>;
    pub fn accept(&self) -> Accept<'_>;

    // Identity
    pub fn id(&self) -> EndpointId;
    pub fn secret_key(&self) -> &SecretKey;
    pub fn addr(&self) -> EndpointAddr;
    pub fn watch_addr(&self) -> impl Watcher<Value = EndpointAddr>;

    // Lifecycle
    pub async fn close(&self);
    pub fn is_closed(&self) -> bool;
    pub fn closed(&self) -> EndpointClosed;
    pub async fn online(&self);  // Wait for relay connection

    // Configuration changes
    pub fn set_alpns(&self, alpns: Vec<Vec<u8>>);
    pub async fn insert_relay(&self, relay: RelayUrl, config: Arc<RelayConfig>) -> Option<Arc<RelayConfig>>;
    pub async fn remove_relay(&self, relay: &RelayUrl) -> Option<Arc<RelayConfig>>;
    pub async fn add_external_addr(&self, addr: SocketAddr);
    pub async fn remove_external_addr(&self, addr: &SocketAddr) -> bool;
    pub fn set_user_data_for_address_lookup(&self, user_data: Option<UserData>);
    pub async fn network_change(&self);

    // Observers
    pub fn home_relay_status(&self) -> impl Watcher<Value = Vec<RelayStatus>>;
    pub fn net_report(&self) -> impl Watcher<Value = Option<NetReport>>;
    pub fn remote_info(&self, id: EndpointId) -> Option<RemoteInfo>;
    pub fn metrics(&self) -> &EndpointMetrics;
    pub fn bound_sockets(&self) -> Vec<SocketAddr>;
    pub fn dns_resolver(&self) -> Result<&DnsResolver, EndpointError>;
    pub fn tls_config(&self) -> &rustls::ClientConfig;
    pub fn address_lookup(&self) -> Result<&AddressLookupServices, EndpointError>;
}
```

### `Builder`
Fluent builder for `Endpoint`:

```rust
let ep = Endpoint::builder(presets::N0)
    .secret_key(secret_key)                        // Identity
    .alpns(vec![b"my-alpn".to_vec()])              // Accepted protocols
    .relay_mode(RelayMode::Default)                // Relay configuration
    .address_lookup(PkarrPublisher::n0_dns())      // Address discovery
    .address_lookup(DnsAddressLookup::n0_dns())    // DNS resolution
    .addr_filter(AddrFilter::relay_only())         // Filter published addresses
    .user_data_for_address_lookup(user_data)       // Custom discovery data
    .transport_config(QuicTransportConfig::default()) // QUIC tuning
    .dns_resolver(dns_resolver)                     // Custom DNS resolver
    .proxy_url(proxy_url)                          // HTTP proxy
    .ca_roots_config(CaRootsConfig::default())     // TLS CA roots
    .keylog(true)                                  // SSLKEYLOGFILE debug
    .max_tls_tickets(256)                          // 0-RTT ticket cache
    .hooks(my_hook)                                // Connection hooks
    .portmapper_config(PortmapperConfig::Enabled)  // UPnP/NAT-PMP
    .external_addr(addr)                           // Advertised external addr
    .bind_addr("0.0.0.0:0")?                       // Bind specific socket
    .bind()                                         // Build & bind
    .await?;
```

### `RelayMode`
```rust
pub enum RelayMode {
    Disabled,                       // No relay
    Default,                        // n0 production relays
    Staging,                        // n0 staging relays
    Custom(RelayMap),               // Custom relay configuration
}
```

## Protocol Handler (`iroh::protocol`)

### `ProtocolHandler`
Trait for handling incoming connections by ALPN:

```rust
pub trait ProtocolHandler: Send + Sync + Debug + 'static {
    // Optional: intercept at Accepting stage (supports 0-RTT)
    fn on_accepting(&self, accepting: Accepting) -> impl Future<Output = Result<Connection, AcceptError>> + Send;

    // Required: handle the established connection
    fn accept(&self, connection: Connection) -> impl Future<Output = Result<(), AcceptError>> + Send;

    // Optional: called on graceful shutdown
    fn shutdown(&self) -> impl Future<Output = ()> + Send;
}
```

### `Router`
Spawns an accept loop that dispatches incoming connections to registered handlers:

```rust
let router = Router::builder(endpoint)
    .accept(b"/my-alpn", Arc::new(MyHandler))
    .incoming_filter(|incoming| {
        if !incoming.remote_addr_validated() {
            IncomingFilterOutcome::Retry
        } else {
            IncomingFilterOutcome::Accept
        }
    })
    .spawn();

// Later...
router.shutdown().await?;
```

### `IncomingFilterOutcome`
```rust
pub enum IncomingFilterOutcome {
    Accept,   // Allow the connection
    Retry,    // Send QUIC retry (address validation)
    Reject,   // Refuse with CONNECTION_REFUSED
    Ignore,   // Drop silently (remote times out)
}
```

### `AccessLimit`
Wrapper that limits connections to allowed `EndpointId`s:

```rust
let handler = AccessLimit::new(MyHandler, |endpoint_id| allowed_set.contains(&endpoint_id));
```

### `EndpointHooks`
Intercept connection establishment at two points:

```rust
pub trait EndpointHooks: Debug + Send + Sync {
    // Before outgoing connection starts
    fn before_connect<'a>(&'a self, remote_addr: &'a EndpointAddr, alpn: &'a [u8])
        -> BoxFuture<'a, BeforeConnectOutcome>;

    // After TLS handshake completes (on both sides)
    fn after_handshake<'a>(&'a self, info: &'a ConnectionInfo)
        -> BoxFuture<'a, AfterHandshakeOutcome>;
}
```

## Connection Types (`iroh::endpoint::connection`)

### `Connecting`
The state between initiating a connection and completing the handshake:

```rust
impl Connecting {
    pub async fn await?(self) -> Result<Connection, ConnectingError>;
    pub fn into_0rtt(self) -> Result<(OutgoingZeroRttConnection, Connection), Connecting>;
    pub fn alpn(&self) -> Result<Vec<u8>, ConnectingError>;
    pub fn remote_id(&self) -> Result<EndpointId, RemoteEndpointIdError>;
}
```

### `Connection`
Wraps a `noq::Connection` with iroh-specific metadata:

```rust
impl Connection {
    // Stream operations
    pub async fn open_bi(&self) -> Result<(SendStream, RecvStream), OpenBi>;
    pub async fn accept_bi(&self) -> Result<(SendStream, RecvStream), AcceptBi>;
    pub async fn open_uni(&self) -> Result<SendStream, OpenUni>;
    pub async fn accept_uni(&self) -> Result<RecvStream, AcceptUni>;

    // Datagrams
    pub fn send_datagram(&self, data: SendDatagram) -> Result<(), SendDatagramError>;
    pub async fn read_datagram(&self) -> Result<Bytes, ReadDatagram>;

    // Connection lifecycle
    pub fn close(&self, error_code: VarInt, reason: &[u8]);
    pub async fn closed(&self) -> ConnectionError;

    // Identity
    pub fn remote_id(&self) -> EndpointId;
    pub fn alpn(&self) -> Vec<u8>;

    // Path observation
    pub fn paths(&self) -> PathWatcher;

    // Keying material export
    pub fn export_keying_material(&self, output: &mut [u8], label: &[u8], context: Option<&[u8]>) -> Result<(), ExportKeyingMaterialError>;
}
```

### `Incoming`
Pre-accept incoming connection:

```rust
impl Incoming {
    pub fn accept(self) -> Result<Accepting, ConnectionError>;
    pub fn accept_with(self, server_config: Arc<ServerConfig>) -> Result<Accepting, ConnectionError>;
    pub fn refuse(self);
    pub fn retry(self) -> Result<(), RetryError>;
    pub fn ignore(self);
    pub fn remote_addr(&self) -> IncomingAddr;
    pub fn local_ip(&self) -> Option<IpAddr>;
    pub fn remote_addr_validated(&self) -> bool;
    pub fn decrypt(&self) -> Option<DecryptedInitial>;
}
```

### `IncomingAddr`
```rust
pub enum IncomingAddr {
    Ip(SocketAddr),
    Relay { url: RelayUrl, endpoint_id: EndpointId },
    Custom(CustomAddr),
}
```

## `RelayMap` and `RelayConfig` (`iroh-relay`)

### `RelayMap`
Thread-safe map of relay servers:

```rust
let map = RelayMap::from_iter([
    "https://relay1.example.org".parse()?,
    "https://relay2.example.org".parse()?,
]);
```

### `RelayConfig`
```rust
pub struct RelayConfig {
    pub url: RelayUrl,
    pub quic: Option<RelayQuicConfig>,  // QAD support
}

pub struct RelayQuicConfig {
    pub port: u16,  // Default: 3478
}
```

## `EndpointData` and `EndpointInfo` (`iroh-dns`)

### `EndpointData`
The data published about an endpoint:

```rust
pub struct EndpointData {
    addrs: Vec<TransportAddr>,
    user_data: Option<UserData>,
}
```

### `EndpointInfo`
Combines `EndpointId` with `EndpointData`:

```rust
pub struct EndpointInfo {
    pub endpoint_id: EndpointId,
    pub data: EndpointData,
}
```

### `UserData`
Application-defined string data published alongside addressing info:

```rust
pub struct UserData(String);  // Max 256 bytes
```

### `AddrFilter`
Controls which addresses are published to address lookup services:

```rust
let filter = AddrFilter::relay_only();    // Only relay URLs
let filter = AddrFilter::unfiltered();    // All addresses
let filter = AddrFilter::custom(|addrs| { /* custom logic */ });
```