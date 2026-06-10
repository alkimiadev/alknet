# Russh: Key Types and Traits

## Client API

### `client::Handler` Trait

The primary interface for implementing SSH clients. All methods have default implementations (mostly no-ops returning `Ok(())`), except `check_server_key` which defaults to rejecting all keys.

```rust
pub trait Handler: Sized + Send {
    type Error: From<crate::Error> + Send + core::fmt::Debug;

    // --- Must implement (security critical) ---
    fn check_server_key(
        &mut self,
        server_public_key: &ssh_key::PublicKey,
    ) -> impl Future<Output = Result<bool, Self::Error>> + Send;
    // Default: async { Ok(false) } — REJECTS ALL KEYS

    // --- Optional callbacks ---
    fn auth_banner(&mut self, banner: &str, session: &mut Session) -> ...;
    fn kex_done(&mut self, shared_secret: Option<&[u8]>, names: &Names, session: &mut Session) -> ...;
    fn channel_open_confirmation(&mut self, id: ChannelId, max_packet_size: u32, window_size: u32, session: &mut Session) -> ...;
    fn channel_success(&mut self, channel: ChannelId, session: &mut Session) -> ...;
    fn channel_failure(&mut self, channel: ChannelId, session: &mut Session) -> ...;
    fn channel_close(&mut self, channel: ChannelId, session: &mut Session) -> ...;
    fn channel_eof(&mut self, channel: ChannelId, session: &mut Session) -> ...;
    fn channel_open_failure(&mut self, channel: ChannelId, reason: ChannelOpenFailure, description: &str, language: &str, session: &mut Session) -> ...;
    fn data(&mut self, channel: ChannelId, data: &[u8], session: &mut Session) -> ...;
    fn extended_data(&mut self, channel: ChannelId, ext: u32, data: &[u8], session: &mut Session) -> ...;
    fn exit_status(&mut self, channel: ChannelId, exit_status: u32, session: &mut Session) -> ...;
    fn exit_signal(&mut self, channel: ChannelId, signal_name: Sig, core_dumped: bool, error_message: &str, lang_tag: &str, session: &mut Session) -> ...;
    fn window_adjusted(&mut self, channel: ChannelId, new_size: u32, session: &mut Session) -> ...;
    fn adjust_window(&mut self, channel: ChannelId, window: u32) -> u32;
    
    // Server-initiated channels (port forwarding, agent, X11, etc.)
    fn server_channel_open_forwarded_tcpip(&mut self, channel: Channel<Msg>, ...) -> ...;
    fn server_channel_open_forwarded_streamlocal(&mut self, channel: Channel<Msg>, ...) -> ...;
    fn server_channel_open_agent_forward(&mut self, channel: Channel<Msg>, ...) -> ...;
    fn server_channel_open_session(&mut self, channel: Channel<Msg>, ...) -> ...;
    fn server_channel_open_x11(&mut self, channel: Channel<Msg>, ...) -> ...;
    fn server_channel_open_direct_tcpip(&mut self, channel: Channel<Msg>, ...) -> ...;
    fn server_channel_open_direct_streamlocal(&mut self, channel: Channel<Msg>, ...) -> ...;
    
    // OpenSSH extensions
    fn openssh_ext_host_keys_announced(&mut self, keys: Vec<PublicKey>, session: &mut Session) -> ...;
    fn disconnected(&mut self, reason: DisconnectReason<Self::Error>) -> ...;
}
```

### `client::Config`

```rust
pub struct Config {
    pub client_id: SshId,                  // SSH version string, default: "SSH-2.0-russh_0.60.2"
    pub limits: Limits,                    // Rekey limits (1GB read/write, 1hr time)
    pub window_size: u32,                   // Initial channel window (default: 2097152 = 2MB)
    pub maximum_packet_size: u32,           // Max single packet (default: 32768)
    pub channel_buffer_size: usize,         // Channel message buffer (default: 100)
    pub preferred: Preferred,              // Algorithm preferences
    pub inactivity_timeout: Option<Duration>, // Connection timeout (default: None)
    pub keepalive_interval: Option<Duration>, // Keepalive frequency (default: None)
    pub keepalive_max: usize,              // Max missed keepalives (default: 3)
    pub anonymous: bool,                   // Skip authentication (default: false)
    pub gex: GexParams,                    // DH-GEX parameters (default: 3072-8192 bits)
    pub nodelay: bool,                      // TCP_NODELAY (default: false)
}
```

### `client::Handle<H>`

The handle returned after connecting. Used to send commands, open channels, authenticate, etc.

```rust
impl<H: Handler> Handle<H> {
    // Authentication
    pub async fn authenticate_none(&mut self, user: U) -> Result<AuthResult, Error>;
    pub async fn authenticate_password(&mut self, user: U, password: P) -> Result<AuthResult, Error>;
    pub async fn authenticate_publickey(&mut self, user: U, key: PrivateKeyWithHashAlg) -> Result<AuthResult, Error>;
    pub async fn authenticate_openssh_cert(&mut self, user: U, key: Arc<PrivateKey>, cert: Certificate) -> Result<AuthResult, Error>;
    pub async fn authenticate_publickey_with<U, S: Signer>(&mut self, user: U, key: PublicKey, hash_alg: Option<HashAlg>, signer: &mut S) -> Result<AuthResult, S::Error>;
    pub async fn authenticate_keyboard_interactive_start<U, S>(&mut self, user: U, submethods: S) -> Result<KeyboardInteractiveAuthResponse, Error>;
    pub async fn authenticate_keyboard_interactive_respond(&mut self, responses: Vec<String>) -> Result<KeyboardInteractiveAuthResponse, Error>;
    
    // Channels
    pub async fn channel_open_session(&self) -> Result<Channel<Msg>, Error>;
    pub async fn channel_open_x11(&self, originator_address: A, originator_port: u32) -> Result<Channel<Msg>, Error>;
    pub async fn channel_open_direct_tcpip(&self, host_to_connect: A, port_to_connect: u32, originator_address: B, originator_port: u32) -> Result<Channel<Msg>, Error>;
    pub async fn channel_open_direct_streamlocal(&self, socket_path: S) -> Result<Channel<Msg>, Error>;
    
    // Port forwarding
    pub async fn tcpip_forward(&self, address: A, port: u32) -> Result<u32, Error>;
    pub async fn cancel_tcpip_forward(&self, address: A, port: u32) -> Result<(), Error>;
    pub async fn streamlocal_forward(&self, socket_path: A) -> Result<(), Error>;
    pub async fn cancel_streamlocal_forward(&self, socket_path: A) -> Result<(), Error>;
    
    // Connection management
    pub async fn disconnect(&self, reason: Disconnect, description: &str, language_tag: &str) -> Result<(), Error>;
    pub async fn rekey_soon(&self) -> Result<(), Error>;
    pub async fn send_keepalive(&self, want_reply: bool) -> Result<(), Error>;
    pub async fn send_ping(&self) -> Result<(), Error>;
    pub async fn no_more_sessions(&self, want_reply: bool) -> Result<(), Error>;
    pub async fn best_supported_rsa_hash(&self) -> Result<Option<Option<HashAlg>>, Error>;
    pub fn is_closed(&self) -> bool;
}
```

`Handle<H>` also implements `Future<Output = Result<(), H::Error>>`, so you can `.await` it to wait for the session to end.

### `client::connect` and `client::connect_stream`

```rust
// Connect via TCP
pub async fn connect<H: Handler + Send + 'static, A: ToSocketAddrs>(
    config: Arc<Config>, addrs: A, handler: H,
) -> Result<Handle<H>, H::Error>;

// Connect via any AsyncRead+AsyncWrite stream
pub async fn connect_stream<H, R>(
    config: Arc<Config>, stream: R, handler: H,
) -> Result<Handle<H>, H::Error>
where
    H: Handler + Send + 'static,
    R: AsyncRead + AsyncWrite + Unpin + Send + 'static;
```

---

## Server API

### `server::Server` Trait

Factory trait that creates a new `Handler` for each client connection:

```rust
pub trait Server {
    type Handler: Handler + Send + 'static;
    fn new_client(&mut self, peer_addr: Option<SocketAddr>) -> Self::Handler;
    fn handle_session_error(&mut self, _error: <Self::Handler as Handler>::Error) {}
    
    // Run on a pre-bound TcpListener
    fn run_on_socket(&mut self, config: Arc<Config>, socket: &TcpListener) -> RunningServer<...>;
    
    // Bind and run on an address
    fn run_on_address<A: ToSocketAddrs + Send>(&mut self, config: Arc<Config>, addrs: A) -> impl Future<...>;
}
```

### `server::Handler` Trait

Per-client handler, similar to `client::Handler` but with different callback signatures (receives `&mut Session` for sending responses):

```rust
pub trait Handler: Sized {
    type Error: From<crate::Error> + Send;

    // Authentication callbacks
    fn auth_none(&mut self, user: &str) -> impl Future<Output = Result<Auth, Self::Error>> + Send;
    fn auth_password(&mut self, user: &str, password: &str) -> impl Future<Output = Result<Auth, Self::Error>> + Send;
    fn auth_publickey_offered(&mut self, user: &str, public_key: &PublicKey) -> impl Future<...> + Send;
    fn auth_publickey(&mut self, user: &str, public_key: &PublicKey) -> impl Future<...> + Send;
    fn auth_openssh_certificate(&mut self, user: &str, certificate: &Certificate) -> impl Future<...> + Send;
    fn auth_keyboard_interactive<'a>(&'a mut self, user: &str, submethods: &str, response: Option<Response<'a>>) -> impl Future<...> + Send;
    fn auth_succeeded(&mut self, session: &mut Session) -> impl Future<...> + Send;
    fn authentication_banner(&mut self) -> impl Future<Output = Result<Option<String>, Self::Error>> + Send;

    // Channel callbacks (return bool = whether to grant the channel)
    fn channel_open_session(&mut self, channel: Channel<Msg>, session: &mut Session) -> impl Future<Output = Result<bool, Self::Error>> + Send;
    fn channel_open_x11(&mut self, channel: Channel<Msg>, originator_address: &str, originator_port: u32, session: &mut Session) -> impl Future<...> + Send;
    fn channel_open_direct_tcpip(&mut self, channel: Channel<Msg>, host_to_connect: &str, port_to_connect: u32, originator_address: &str, originator_port: u32, session: &mut Session) -> impl Future<...> + Send;
    fn channel_open_forwarded_tcpip(&mut self, channel: Channel<Msg>, ...) -> impl Future<...> + Send;
    fn channel_open_direct_streamlocal(&mut self, channel: Channel<Msg>, socket_path: &str, session: &mut Session) -> impl Future<...> + Send;

    // Channel events
    fn data(&mut self, channel: ChannelId, data: &[u8], session: &mut Session) -> impl Future<...> + Send;
    fn extended_data(&mut self, channel: ChannelId, code: u32, data: &[u8], session: &mut Session) -> impl Future<...> + Send;
    fn channel_close(&mut self, channel: ChannelId, session: &mut Session) -> impl Future<...> + Send;
    fn channel_eof(&mut self, channel: ChannelId, session: &mut Session) -> impl Future<...> + Send;
    fn window_adjusted(&mut self, channel: ChannelId, new_size: u32, session: &mut Session) -> impl Future<...> + Send;
    fn adjust_window(&mut self, channel: ChannelId, current: u32) -> u32;

    // Channel requests (use session.channel_success/failure to respond)
    fn pty_request(&mut self, channel: ChannelId, term: &str, col_width: u32, row_height: u32, pix_width: u32, pix_height: u32, modes: &[(Pty, u32)], session: &mut Session) -> impl Future<...> + Send;
    fn x11_request(&mut self, channel: ChannelId, ...) -> impl Future<...> + Send;
    fn env_request(&mut self, channel: ChannelId, variable_name: &str, variable_value: &str, session: &mut Session) -> impl Future<...> + Send;
    fn shell_request(&mut self, channel: ChannelId, session: &mut Session) -> impl Future<...> + Send;
    fn exec_request(&mut self, channel: ChannelId, data: &[u8], session: &mut Session) -> impl Future<...> + Send;
    fn subsystem_request(&mut self, channel: ChannelId, name: &str, session: &mut Session) -> impl Future<...> + Send;
    fn window_change_request(&mut self, channel: ChannelId, ...) -> impl Future<...> + Send;
    fn agent_request(&mut self, channel: ChannelId, session: &mut Session) -> impl Future<...> + Send;
    fn signal(&mut self, channel: ChannelId, signal: Sig, session: &mut Session) -> impl Future<...> + Send;

    // Port forwarding
    fn tcpip_forward(&mut self, address: &str, port: &mut u32, session: &mut Session) -> impl Future<...> + Send;
    fn cancel_tcpip_forward(&mut self, address: &str, port: u32, session: &mut Session) -> impl Future<...> + Send;
    fn streamlocal_forward(&mut self, socket_path: &str, session: &mut Session) -> impl Future<...> + Send;
    fn cancel_streamlocal_forward(&mut self, socket_path: &str, session: &mut Session) -> impl Future<...> + Send;

    // DH-GEX group lookup
    fn lookup_dh_gex_group(&mut self, gex_params: &GexParams) -> impl Future<Output = Result<Option<DhGroup>, Self::Error>> + Send;
}
```

### `server::Auth` Enum

```rust
pub enum Auth {
    Reject { proceed_with_methods: Option<MethodSet>, partial_success: bool },
    Accept,
    UnsupportedMethod,
    Partial { name: Cow<'static, str>, instructions: Cow<'static, str>, prompts: Cow<'static, [(Cow<'static, str>, bool)]> },
}
```

### `server::Config`

```rust
pub struct Config {
    pub server_id: SshId,                         // "SSH-2.0-russh_0.60.2"
    pub methods: auth::MethodSet,                   // All methods by default
    pub auth_rejection_time: Duration,             // Constant-time rejection (default: 1s)
    pub auth_rejection_time_initial: Option<Duration>, // For "none" probe (default: None)
    pub keys: Vec<PrivateKey>,                     // Server host keys
    pub limits: Limits,
    pub window_size: u32,                          // Default: 2097152
    pub maximum_packet_size: u32,                  // Default: 32768
    pub channel_buffer_size: usize,                // Default: 100
    pub event_buffer_size: usize,                  // Default: 10
    pub preferred: Preferred,
    pub max_auth_attempts: usize,                  // Default: 10
    pub inactivity_timeout: Option<Duration>,      // Default: 600s
    pub keepalive_interval: Option<Duration>,       // Default: None
    pub keepalive_max: usize,                      // Default: 3
    pub nodelay: bool,                              // Default: false
}
```

### `server::Handle`

Server-side handle for sending unsolicited messages to a client:

```rust
impl Handle {
    pub async fn data(&self, id: ChannelId, data: impl Into<Bytes>) -> Result<(), Bytes>;
    pub async fn extended_data(&self, id: ChannelId, ext: u32, data: impl Into<Bytes>) -> Result<(), Bytes>;
    pub async fn eof(&self, id: ChannelId) -> Result<(), ()>;
    pub async fn channel_success(&self, id: ChannelId) -> Result<(), ()>;
    pub async fn channel_failure(&self, id: ChannelId) -> Result<(), ()>;
    pub async fn close(&self, id: ChannelId) -> Result<(), ()>;
    pub async fn xon_xoff_request(&self, id: ChannelId, client_can_do: bool) -> Result<(), ()>;
    pub async fn exit_status_request(&self, id: ChannelId, exit_status: u32) -> Result<(), ()>;
    pub async fn forward_tcpip(&self, address: String, port: u32) -> Result<u32, ()>;
    pub async fn cancel_tcpip_forward(&self, address: String, port: u32) -> Result<(), ()>;
    // ... etc.
}
```

---

## Channel Types

### `Channel<Send>`

A bidirectional handle to an SSH channel. `Send` is the message type (`client::Msg` or `server::Msg`).

```rust
pub struct Channel<Send: From<(ChannelId, ChannelMsg)>> {
    pub read_half: ChannelReadHalf,
    pub write_half: ChannelWriteHalf<Send>,
}

impl<S: From<(ChannelId, ChannelMsg)> + Send + Sync + 'static> Channel<S> {
    pub fn id(&self) -> ChannelId;
    pub async fn writable_packet_size(&self) -> usize;
    pub fn split(self) -> (ChannelReadHalf, ChannelWriteHalf<S>);
    pub async fn wait(&mut self) -> Option<ChannelMsg>;
    
    // Client-side operations
    pub async fn request_pty(&self, ...) -> Result<(), Error>;
    pub async fn request_shell(&self, want_reply: bool) -> Result<(), Error>;
    pub async fn exec(&self, want_reply: bool, command: A) -> Result<(), Error>;
    pub async fn signal(&self, signal: Sig) -> Result<(), Error>;
    pub async fn request_subsystem(&self, want_reply: bool, name: A) -> Result<(), Error>;
    pub async fn request_x11(&self, ...) -> Result<(), Error>;
    pub async fn set_env(&self, ...) -> Result<(), Error>;
    pub async fn window_change(&self, ...) -> Result<(), Error>;
    pub async fn agent_forward(&self, want_reply: bool) -> Result<(), Error>;
    pub async fn data<R: AsyncRead + Unpin>(&self, data: R) -> Result<(), Error>;
    pub async fn extended_data<R: AsyncRead + Unpin>(&self, ext: u32, data: R) -> Result<(), Error>;
    pub async fn eof(&self) -> Result<(), Error>;
    pub async fn exit_status(&self, exit_status: u32) -> Result<(), Error>;
    pub async fn close(&self) -> Result<(), Error>;
    
    // Streaming
    pub fn into_stream(self) -> ChannelStream<S>;
    pub fn make_reader(&mut self) -> impl AsyncRead + '_;
    pub fn make_reader_ext(&mut self, ext: Option<u32>) -> impl AsyncRead + '_;
    pub fn make_writer(&self) -> impl AsyncWrite + 'static;
    pub fn make_writer_ext(&self, ext: Option<u32>) -> impl AsyncWrite + 'static;
}
```

### `ChannelMsg` Enum

All possible messages receivable on a channel:

```rust
pub enum ChannelMsg {
    Open { id: ChannelId, max_packet_size: u32, window_size: u32 },
    Data { data: Bytes },
    ExtendedData { data: Bytes, ext: u32 },
    Eof,
    Close,
    RequestPty { want_reply: bool, term: String, col_width: u32, row_height: u32, pix_width: u32, pix_height: u32, terminal_modes: Vec<(Pty, u32)> },
    RequestShell { want_reply: bool },
    Exec { want_reply: bool, command: Vec<u8> },
    Signal { signal: Sig },
    RequestSubsystem { want_reply: bool, name: String },
    RequestX11 { want_reply: bool, single_connection: bool, x11_authentication_protocol: String, x11_authentication_cookie: String, x11_screen_number: u32 },
    SetEnv { want_reply: bool, variable_name: String, variable_value: String },
    WindowChange { col_width: u32, row_height: u32, pix_width: u32, pix_height: u32 },
    AgentForward { want_reply: bool },
    XonXoff { client_can_do: bool },
    ExitStatus { exit_status: u32 },
    ExitSignal { signal_name: Sig, core_dumped: bool, error_message: String, lang_tag: String },
    WindowAdjusted { new_size: u32 },
    Success,
    Failure,
    OpenFailure(ChannelOpenFailure),
}
```

### `ChannelId`

A `u32` wrapper identifying a channel within a session:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct ChannelId(u32);
```

---

## Shared Types

### `Limits`

Rekey thresholds (following RFC 4253 Section 9):

```rust
pub struct Limits {
    pub rekey_write_limit: usize,   // Default: 1 << 30 (1 GB)
    pub rekey_read_limit: usize,    // Default: 1 << 30 (1 GB)
    pub rekey_time_limit: Duration, // Default: 3600s (1 hour)
}
```

### `Preferred`

Algorithm preference lists for negotiation:

```rust
pub struct Preferred {
    pub kex: Cow<'static, [kex::Name]>,
    pub key: Cow<'static, [Algorithm]>,
    pub cipher: Cow<'static, [cipher::Name]>,
    pub mac: Cow<'static, [mac::Name]>,
    pub compression: Cow<'static, [compression::Name]>,
}
```

Default order prioritizes modern algorithms:
- **KEX**: ML-KEM-768-X25519 → Curve25519 → DH-GEX-SHA256 → DH-G18/17/16/15/14
- **Key**: Ed25519 → ECDSA-P256/P384/P521 → RSA-SHA512/256
- **Cipher**: Chacha20-Poly1305 → AES-256-GCM → AES-256/192/128-CTR
- **MAC**: HMAC-SHA512-ETM → HMAC-SHA256-ETM → HMAC-SHA512/256

### `Disconnect` Enum

RFC 4253 Section 11.1 disconnect reason codes:

```rust
pub enum Disconnect {
    HostNotAllowedToConnect = 1,
    ProtocolError = 2,
    KeyExchangeFailed = 3,
    MACError = 5,
    CompressionError = 6,
    ServiceNotAvailable = 7,
    ProtocolVersionNotSupported = 8,
    HostKeyNotVerifiable = 9,
    ConnectionLost = 10,
    ByApplication = 11,
    TooManyConnections = 12,
    AuthCancelledByUser = 13,
    NoMoreAuthMethodsAvailable = 14,
    IllegalUserName = 15,
}
```

### `CryptoVec`

A vector that zeroes its memory on clears and reallocations, using `mlock` on Unix and `VirtualLock` on Windows. Used for all sensitive key material.

```rust
// From russh-cryptovec
pub struct CryptoVec { /* ... */ }
// Implements mlock/munlock for sensitive data
// Zeroes memory on drop/resize
```