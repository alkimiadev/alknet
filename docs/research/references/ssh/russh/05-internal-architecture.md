# Russh: Internal Architecture & Data Flow

This document covers the internal mechanics — session state machines, the event loop, packet handling, buffering, and the interaction between components.

## Session State

### `CommonSession<C>`

The shared session state used by both client and server:

```rust
pub(crate) struct CommonSession<C> {
    pub auth_user: String,
    pub remote_sshid: Vec<u8>,
    pub config: C,                    // Arc<client::Config> or Arc<server::Config>
    pub encrypted: Option<Encrypted>,
    pub auth_method: Option<auth::Method>,  // Client only
    pub auth_attempts: usize,
    pub packet_writer: PacketWriter,
    pub remote_to_local: Box<dyn OpeningKey + Send>,
    pub wants_reply: bool,
    pub disconnected: bool,
    pub buffer: Vec<u8>,              // Incoming packet scratch buffer
    pub strict_kex: bool,
    pub alive_timeouts: usize,
    pub received_data: bool,
}
```

### `Encrypted`

The state after encryption keys are established:

```rust
pub(crate) struct Encrypted {
    pub state: EncryptedState,
    pub exchange: Option<Exchange>,
    pub kex: KexAlgorithm,
    pub key: usize,
    pub client_mac: mac::Name,
    pub server_mac: mac::Name,
    pub session_id: CryptoVec,        // Constant across rekeys
    pub channels: HashMap<ChannelId, ChannelParams>,
    pub last_channel_id: Wrapping<u32>,
    pub write: Vec<u8>,               // Outgoing packet assembly buffer
    pub write_cursor: usize,          // Current position in write buffer
    pub last_rekey: Instant,
    pub server_compression: Compression,
    pub client_compression: Compression,
    pub decompress: Decompress,
    pub rekey_wanted: bool,
    pub received_extensions: Vec<String>,
    pub extension_info_awaiters: HashMap<String, Vec<oneshot::Sender<()>>>,
}
```

### `EncryptedState`

```rust
pub enum EncryptedState {
    WaitingAuthServiceRequest { sent: bool, accepted: bool },
    WaitingAuthRequest(auth::AuthRequest),
    InitCompression,
    Authenticated,
}
```

### `Exchange`

Protocol values collected during key exchange (all non-secret, visible on wire):

```rust
pub struct Exchange {
    pub client_id: Vec<u8>,
    pub server_id: Vec<u8>,
    pub client_kex_init: Vec<u8>,
    pub server_kex_init: Vec<u8>,
    pub client_ephemeral: Vec<u8>,
    pub server_ephemeral: Vec<u8>,
    pub gex: Option<(GexParams, DhGroup)>,
}
```

### `NewKeys`

Produced when key exchange completes, contains everything needed to activate encryption:

```rust
pub(crate) struct NewKeys {
    pub exchange: Exchange,
    pub names: negotiation::Names,
    pub kex: KexAlgorithm,
    pub key: usize,
    pub cipher: CipherPair,           // { local_to_remote, remote_to_local }
    pub session_id: CryptoVec,
}
```

### `ChannelParams`

Internal channel state (not exposed to users):

```rust
pub(crate) struct ChannelParams {
    pub recipient_channel: u32,       // Remote channel ID
    pub sender_channel: ChannelId,   // Local channel ID
    pub recipient_window_size: u32,
    pub sender_window_size: u32,
    pub recipient_maximum_packet_size: u32,
    pub sender_maximum_packet_size: u32,
    pub confirmed: bool,              // Whether server confirmed the channel
    pub wants_reply: bool,
    pub pending_data: VecDeque<(Bytes, Option<u32>, usize)>, // (data, ext_code, offset)
    pub pending_eof: bool,
    pub pending_close: bool,
}
```

---

## Client Event Loop

The client event loop is the core of `client::Session::run_inner()`:

```rust
async fn run_inner<H, R>(&mut self, stream_read, stream_write, handler, kex_done_signal)
    -> Result<RemoteDisconnectInfo, H::Error>
{
    // Initial setup
    self.flush()?;
    self.common.packet_writer.flush_into(stream_write).await?;

    // Set up timers
    let keepalive_timer = ...;
    let inactivity_timer = ...;
    let reading = start_reading(stream_read, buffer, opening_cipher);

    while !self.common.disconnected {
        tokio::select! {
            // 1. Incoming SSH packet
            r = &mut reading => {
                // Decrypt and decompress packet
                // Process DISCONNECT or pass to reply()
                // Restart reading
            }

            // 2. Keepalive timer
            () = &mut keepalive_timer => {
                // Send keepalive if authenticated
                // Track timeout count
            }

            // 3. Inactivity timer
            () = &mut inactivity_timer => {
                // Return InactivityTimeout error
            }

            // 4. Outgoing messages from Handle
            msg = self.receiver.recv(), if !self.kex.active() => {
                // Process message (auth, channel open, data, etc.)
                // Batch all pending outgoing messages
            }

            // 5. Inbound channel messages
            msg = self.inbound_channel_receiver.recv(), if !self.kex.active() => {
                // Process channel data/eof/close
                // Batch all pending messages
            }
        };

        // Flush all pending writes
        self.flush()?;
        self.common.packet_writer.flush_into(stream_write).await?;

        // Handle deferred compression after authentication
        if EncryptedState::InitCompression { ... } { 
            // Init client compression if deferred (zlib@openssh.com)
            // Transition to Authenticated
        }

        // Reset timers if data received or keepalive sent
    }
}
```

### Key Event Loop Behaviors

1. **Kex blocking**: When `self.kex.active()` is true, outgoing messages from `Handle` are NOT processed. This prevents sending data during key exchange.

2. **Batching**: After receiving one message from `receiver`, all pending messages are drained with `try_recv()` to batch writes.

3. **Keepalive management**: Keepalive timer resets when data is received. `alive_timeouts` tracks consecutive unanswered keepalives.

4. **Compression activation**: `zlib@openssh.com` compression is deferred until after authentication succeeds (handled by `InitCompression` state).

---

## Server Event Loop

Similar to the client, but the server accepts connections via `run_on_socket()` or `run_stream()`:

```rust
// Server::run_on_socket
loop {
    tokio::select! {
        _ = shutdown_rx.recv() => { /* Graceful shutdown */ },
        accept_result = socket.accept() => {
            // For each connection:
            // 1. Create a new Handler via Server::new_client()
            // 2. Call run_stream() in a spawned task
            // 3. Wait for session or shutdown
        },
        error = error_rx.recv() => {
            // Report session errors
        }
    }
}
```

### `run_stream`

```rust
pub async fn run_stream<H, R>(config, stream, handler) -> Result<RunningSession<H>, H::Error>
{
    // 1. Write server SSH ID
    // 2. Read client SSH ID
    // 3. Create Session with CommonSession state
    // 4. Begin initial rekey (sends KEXINIT)
    // 5. Spawn session.run() in a task
    // 6. Return RunningSession (implements Future)
}
```

### `RunningServer` and `RunningServerHandle`

The server returns a `RunningServer` that implements `Future` and a `RunningServerHandle` for graceful shutdown:

```rust
pub struct RunningServer<F: Future<Output = io::Result<()>> + Unpin + Send> {
    inner: F,
    shutdown_tx: broadcast::Sender<String>,
}

impl RunningServerHandle {
    pub fn shutdown(&self, reason: String) {
        let _ = self.shutdown_tx.send(reason);
    }
}
```

---

## Packet Handling Pipeline

### Reading Packets (`cipher::read`)

```
1. Read 4 bytes (or more for block ciphers) → encrypted packet length
2. Decrypt packet length
3. Parse length, check against MAXIMUM_PACKET_LEN (262159 bytes)
4. Read remaining bytes (length + tag_len - already_read)
5. Decrypt the ciphertext (including tag verification for AEAD)
6. Remove padding
7. Increment sequence number
```

### Writing Packets (`SealingKey::write`)

```
1. Compute padding length (block-aligned, min 4 bytes)
2. Write: [packet_length (4B)] [padding_length (1B)] [payload] [padding] [tag]
3. Encrypt the packet
4. Compute and append MAC/tag
5. Increment sequence number
6. Add payload bytes to rekey counter
```

### Packet Assembly (`PacketWriter`)

```rust
pub(crate) struct PacketWriter {
    cipher: Box<dyn SealingKey + Send>,
    compress: Compress,
    compress_buffer: Vec<u8>,
    write_buffer: SSHBuffer,
}
```

Methods:
- `packet_raw(buf)`: Compress and encrypt a raw packet
- `packet(f)`: Build a packet via closure, compress, encrypt, return the plaintext
- `flush_into(w)`: Write all buffered data to the async writer
- `set_cipher(c)`: Swap the cipher (for rekeying)
- `reset_seqn()`: Reset sequence number (for strict kex)

### `push_packet!` Macro

Used throughout for building packets:

```rust
macro_rules! push_packet {
    ( $buffer:expr, $x:expr ) => {{
        let i0 = $buffer.len();
        $buffer.extend(b"\0\0\0\0");  // Placeholder for length
        let x = $x;                    // Build the packet body
        let i1 = $buffer.len();
        BigEndian::write_u32(&mut buf[i0..], (i1 - i0 - 4) as u32);
        x
    }};
}
```

---

## Channel Data Flow

### Client-side Channel Creation

```
1. Handle::channel_open_session()
   → Creates ChannelRef with mpsc::channel
   → Sends Msg::ChannelOpenSession to session
   
2. Session::handle_msg(Msg::ChannelOpenSession)
   → Calls self.channel_open_session()
   → Sends CHANNEL_OPEN packet with channel type "session"
   → Stores ChannelRef in self.channels
   
3. Server responds with CHANNEL_OPEN_CONFIRMATION
   → Session updates ChannelParams (recipient_channel, window sizes)
   → Sends ChannelMsg::Open through ChannelRef's sender
   
4. wait_channel_confirmation() receives the Open message
   → Creates Channel { read_half, write_half }
   → Returns the Channel to the caller
```

### Data Transmission (Client → Server)

```
1. channel.data(reader)
   → Uses ChannelTx (AsyncWrite) that reads from the reader
   → Sends ChannelMsg::Data through the channel sender
   → Waits for window availability via WindowSizeRef
   
2. Session receives ChannelMsg::Data
   → Calls Encrypted::data(channel, bytes, is_rekeying)
   → If pending data exists or rekeying: queues in pending_data
   → Otherwise: writes CHANNEL_DATA packet immediately
   → If window exhausted: queues remaining data
   
3. PacketWriter encrypts and buffers the packet
4. Event loop flushes the buffer to the TCP stream
```

### Data Reception (Server → Client)

```
1. Encrypted packet arrives, decrypted, decompressed
2. Packet type = CHANNEL_DATA
3. Encrypted::adjust_window_size() checks if window needs adjustment
4. ChannelMsg::Data sent through ChannelRef's sender
5. Channel::wait() returns the ChannelMsg::Data
6. ChannelRx (AsyncRead) reads from the channel receiver
```

---

## Rekeying Flow

```
1. Trigger:
   - Bytes written/read exceed limits
   - Time since last rekey exceeds limit
   - Explicit: Handle::rekey_soon()
   - Server-initiated: receiving KEXINIT when idle

2. Session::begin_rekey():
   - Creates new ClientKex/ServerKex
   - Sends new KEXINIT
   - Sets kex state to InProgress

3. During rekey:
   - Outgoing messages from Handle are blocked (!kex.active())
   - Incoming non-kex packets are buffered in pending_reads
   - Kex state machine processes kex packets (KEXINIT, DH init/reply, NEWKEYS)

4. On completion:
   - Flush all pending channel data
   - Process buffered pending_reads
   - Call CommonSession::newkeys() to swap ciphers
   - Reset byte counters
   - Set kex state to Idle
   - Resume processing outgoing messages
```

---

## Sub-Crates

### `russh-cryptovec` (cryptovec/)

A `Vec<u8>` alternative that:
- Zeroes memory on drop and reallocation (via `memset` / `ExplicitZero`)
- Locks memory pages with `mlock` (Unix) / `VirtualLock` (Windows) to prevent swapping
- Uses `unsafe` for performance-critical operations (copying, initialization)
- Integrates with `ssh-encoding` for Encode support

Used for all sensitive data: session keys, shared secrets, MAC keys, exchange hashes.

### `russh-util` (russh-util/)

Runtime abstraction layer:
- `russh_util::runtime::spawn()` — spawns a task (tokio or wasm)
- `russh_util::runtime::JoinHandle` — task join handle
- `russh_util::time::Instant` — time source (tokio or chrono for WASM)
- WASM compatibility: uses `wasm-bindgen-futures` and `chrono` when target is `wasm32`

### `russh-config` (russh-config/)

SSH config file parser:
- Parses `~/.ssh/config` format
- Supports `Host` matching with `globset`
- Provides `Stream::tcp_connect()` and `Stream::proxy_command()` for establishing connections based on config

### `pageant` (pageant/)

Windows Pageant SSH agent transport:
- `wmmessage` feature: Classic Pageant protocol via Windows messages
- `namedpipes` feature: Modern Pageant protocol via named pipes (PuTTY ≥ 0.75)