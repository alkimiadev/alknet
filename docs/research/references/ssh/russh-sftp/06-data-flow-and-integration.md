# russh-sftp: Data Flow and Integration

## Client Data Flow: File Read

```
Application code
    │
    ▼
SftpSession::open("file.txt")
    │
    ▼
RawSftpSession::open("file.txt", OpenFlags::READ, FileAttributes::empty())
    │
    ├── 1. Generate request ID: AtomicU32::fetch_add(1)
    ├── 2. Check Limits.open_handles
    ├── 3. Create oneshot channel, insert (Some(id), tx) into DashMap
    ├── 4. Serialize Open packet → Bytes
    ├── 5. Send Bytes via mpsc::UnboundedSender
    └── 6. Await oneshot::Receiver with timeout
         │
         ▼
    Writer Task (tokio::spawn)               Reader Task (tokio::spawn)
    ┌──────────────────────┐               ┌──────────────────────┐
    │  rx.recv() → Bytes   │               │  read_packet(stream)  │
    │  → write_all(stream) │               │  → Packet::try_from() │
    │  → flush()           │               │  → SessionInner::reply │
    └──────────────────────┘               │    → DashMap.remove   │
                                           │    → oneshot::tx.send │
                                           └──────────┬───────────┘
                                                      │
                                          ┌───────────┘
                                          ▼
                                    oneshot::rx.recv()
                                          │
                                          ▼
                                Packet::Handle(Handle { id, handle: "..." })
                                          │
                                          ▼
                                Return handle string
    │
    ▼
File::new(session, handle, features)
    │
    ▼ (user calls AsyncRead::poll_read)
File::poll_read()
    │
    ├── Compute max_read_len (from Limits or max_packet_len - 9)
    ├── Compute offset (self.pos) and len (min of remaining buffer, max_read_len)
    ├── Create async future: session.read(handle, offset, len)
    │       └── RawSftpSession::read() → request/response cycle as above
    ├── Poll future; on Ready:
    │       ├── Err(Status::Eof) → Ok(None) → return Poll::Ready(Ok(()))
    │       ├── Ok(data) → buf.put_slice(&data), advance pos
    │       └── Other errors → return Poll::Ready(Err(...))
    └── On Pending: return Poll::Pending
```

## Client Data Flow: File Write (Pipelined)

```
Application code
    │
    ▼
file.write_all(&data).await  (AsyncWrite::poll_write)
    │
    ├── 1. If write_acks.len() >= max_concurrent_writes:
    │       Poll oldest pending ACK; if not ready, return Pending
    │
    ├── 2. Compute write chunk: min(data.len(), max_write_len)
    │       max_write_len = Limits.write_len or (max_packet_len - 21 - handle_len)
    │
    ├── 3. Call session.write_nowait(handle, offset, data)
    │       └── RawSftpSession::write_nowait()
    │           ├── Generate request ID
    │           ├── Check Limits.write_len
    │           ├── Create oneshot channel, insert into DashMap
    │           ├── Send packet via mpsc channel
    │           └── Return oneshot::Receiver (does NOT await)
    │
    ├── 4. Store oneshot::Receiver in write_acks VecDeque
    ├── 5. Advance pos by chunk length
    └── 6. Return Poll::Ready(Ok(chunk_len))
    
    Later: file.flush().await  (AsyncWrite::poll_flush)
    │
    ├── 1. Drain all pending write_acks (poll each one)
    └── 2. If fsync supported: call session.fsync(handle)
            └── RawSftpSession::fsync() → full request/response cycle

    Later: file.shutdown().await  (AsyncWrite::poll_shutdown)
    │
    ├── 1. Drain all pending write_acks
    └── 2. Call session.close(handle) → full request/response cycle
        └── Marks self.closed = true
```

## Server Data Flow

```
Client SSH connection
    │
    ▼ (subsystem request for "sftp")
russh server::Handler::subsystem_request()
    │
    ├── session.channel_success(channel_id)
    └── russh_sftp::server::run(channel.into_stream(), sftp_handler).await
         │
         ▼
    ┌──────────────────────────────────────────────┐
    │  tokio::spawn loop:                           │
    │                                               │
    │  loop {                                       │
    │      1. read_packet(stream, max_len)          │
    │      2. Packet::try_from(bytes)               │
    │         └── On error: Packet::error(0, BadMsg)│
    │      3. process_request(packet, handler)      │
    │         └── Match packet variant               │
    │         └── Call handler method with fields    │
    │         └── Ok → convert to Packet::from()    │
    │         └── Err → StatusReply → Packet::Status │
    │      4. Bytes::try_from(response)             │
    │      5. stream.write_all(&packet)             │
    │      6. stream.flush()                        │
    │  }                                            │
    │  // Break on UnexpectedEof                    │
    └──────────────────────────────────────────────┘
```

## SFTP Handshake Protocol

### Client-side handshake (in SftpSession::new)

```
Client                                    Server
  │                                         │
  │──── SSH_FXP_INIT {version:3} ─────────►│
  │                                         │
  │◄─── SSH_FXP_VERSION {version:3,         │
  │      extensions: {...}} ────────────────│
  │                                         │
  │  (now SftpSession checks extensions      │
  │   and optionally fetches limits)         │
  │                                         │
  │──── SSH_FXP_EXTENDED {                  │
  │       request: "limits@openssh.com",    │
  │       data: [] } ───────────────────────►│
  │                                         │
  │◄─── SSH_FXP_EXTENDED_REPLY {            │
  │       data: LimitsExtension } ──────────│
  │                                         │
  │  (SftpSession stores limits in          │
  │   RawSftpSession and Features)          │
```

### Server-side handshake (in Handler::init)

```
Server receives SSH_FXP_INIT
    │
    ├── process_request() calls handler.init(version, extensions)
    ├── Handler returns Ok(Version { version: 3, extensions })
    └── Version packet is serialized and sent back
```

## Error Handling Flows

### Client Error Chain

```
RawSftpSession method call
    │
    ├── Timeout → Error::Timeout
    │       (DashMap entry cleaned up)
    │
    ├── Unexpected packet type → Error::UnexpectedPacket
    │       (e.g., expected Handle, got Data)
    │
    ├── Server status error → Error::Status(Status)
    │       (StatusCode != Ok)
    │
    ├── Limit exceeded → Error::Limited(String)
    │       (packet/read/write/handle limits from server)
    │
    └── Channel closed → Error::UnexpectedBehavior("session closed")
```

### Server Error Chain

```
Handler method returns Err(e)
    │
    ├── e.into() → StatusReply { status_code, error_message, language_tag }
    │
    ├── Default fill:
    │   error_message: None → status_code.to_string()
    │   language_tag: None → "en-US"
    │
    └── Packet::Status { id, status_code, error_message, language_tag }
        sent back to client
```

## russh Integration Patterns

### Client Pattern

```rust
use russh::client;
use russh_sftp::client::SftpSession;

struct Client;

impl client::Handler for Client {
    type Error = anyhow::Error;
    
    async fn check_server_key(&mut self, key: &russh::keys::PublicKey)
        -> Result<bool, Self::Error> { Ok(true) }
}

async fn connect_sftp() -> anyhow::Result<SftpSession> {
    let config = russh::client::Config::default();
    let mut session = client::connect(Arc::new(config), ("host", 22), Client).await?;
    let auth_ok = session.authenticate_password("user", "pass").await?;
    if !auth_ok.success() { return Err(anyhow!("auth failed")); }
    
    let channel = session.channel_open_session().await?;
    channel.request_subsystem(true, "sftp").await?;
    
    // channel.into_stream() provides AsyncRead + AsyncWrite + Unpin + Send + 'static
    let sftp = SftpSession::new(channel.into_stream()).await?;
    Ok(sftp)
}
```

### Server Pattern

```rust
use russh::server;
use russh_sftp::server;

struct SshSession { /* ... */ }

impl server::Handler for SshSession {
    type Error = anyhow::Error;
    // ... auth methods ...
    
    async fn subsystem_request(&mut self, channel_id: ChannelId, name: &str, session: &mut server::Session)
        -> Result<(), Self::Error>
    {
        if name == "sftp" {
            let channel = self.get_channel(channel_id).await;
            let sftp_handler = MySftpHandler::new();
            session.channel_success(channel_id)?;
            server::run(channel.into_stream(), sftp_handler).await;
        } else {
            session.channel_failure(channel_id)?;
        }
        Ok(())
    }
}
```

### Non-russh Transport

Since russh-sftp only requires `AsyncRead + AsyncWrite + Unpin + Send + 'static`, any transport providing these traits works:

```rust
// Using a TCP stream directly (e.g., for testing without SSH)
let stream = TcpStream::connect("127.0.0.1:2222").await?;
let sftp = SftpSession::new(stream).await?;

// Using a Unix socket
let stream = UnixStream::connect("/tmp/sftp-sock").await?;
let sftp = SftpSession::new(stream).await?;

// Using tokio::io::duplex for in-process testing
let (client, server) = tokio::io::duplex(8192);
// Feed server side to server::run(), client side to SftpSession::new()
```