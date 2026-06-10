# russh-sftp: Server Implementation

## Server Architecture

The server side of russh-sftp follows a **handler trait pattern**: users implement the `server::Handler` trait, and the framework calls their methods in response to incoming SFTP requests. Each request is processed and a response packet is sent back on the same stream.

The server processes requests **sequentially** — one at a time, in order. There is no concurrent request handling within a single SFTP session.

## Server Handler Trait

```rust
pub trait Handler: Sized {
    type Error: Into<StatusReply> + Send;

    fn unimplemented(&self) -> Self::Error;

    // --- Lifecycle ---
    fn init(&mut self, version: u32, extensions: HashMap<String, String>)
        -> impl Future<Output = Result<Version, Self::Error>> + Send;

    // --- File operations ---
    fn open(&mut self, id: u32, filename: String, pflags: OpenFlags, attrs: FileAttributes)
        -> impl Future<Output = Result<Handle, Self::Error>> + Send;
    fn close(&mut self, id: u32, handle: String)
        -> impl Future<Output = Result<Status, Self::Error>> + Send;
    fn read(&mut self, id: u32, handle: String, offset: u64, len: u32)
        -> impl Future<Output = Result<Data, Self::Error>> + Send;
    fn write(&mut self, id: u32, handle: String, offset: u64, data: Vec<u8>)
        -> impl Future<Output = Result<Status, Self::Error>> + Send;

    // --- Metadata ---
    fn lstat(&mut self, id: u32, path: String)
        -> impl Future<Output = Result<Attrs, Self::Error>> + Send;
    fn fstat(&mut self, id: u32, handle: String)
        -> impl Future<Output = Result<Attrs, Self::Error>> + Send;
    fn setstat(&mut self, id: u32, path: String, attrs: FileAttributes)
        -> impl Future<Output = Result<Status, Self::Error>> + Send;
    fn fsetstat(&mut self, id: u32, handle: String, attrs: FileAttributes)
        -> impl Future<Output = Result<Status, Self::Error>> + Send;

    // --- Directory operations ---
    fn opendir(&mut self, id: u32, path: String)
        -> impl Future<Output = Result<Handle, Self::Error>> + Send;
    fn readdir(&mut self, id: u32, handle: String)
        -> impl Future<Output = Result<Name, Self::Error>> + Send;

    // --- Filesystem operations ---
    fn remove(&mut self, id: u32, filename: String)
        -> impl Future<Output = Result<Status, Self::Error>> + Send;
    fn mkdir(&mut self, id: u32, path: String, attrs: FileAttributes)
        -> impl Future<Output = Result<Status, Self::Error>> + Send;
    fn rmdir(&mut self, id: u32, path: String)
        -> impl Future<Output = Result<Status, Self::Error>> + Send;
    fn realpath(&mut self, id: u32, path: String)
        -> impl Future<Output = Result<Name, Self::Error>> + Send;
    fn stat(&mut self, id: u32, path: String)
        -> impl Future<Output = Result<Attrs, Self::Error>> + Send;
    fn rename(&mut self, id: u32, oldpath: String, newpath: String)
        -> impl Future<Output = Result<Status, Self::Error>> + Send;
    fn readlink(&mut self, id: u32, path: String)
        -> impl Future<Output = Result<Name, Self::Error>> + Send;
    fn symlink(&mut self, id: u32, linkpath: String, targetpath: String)
        -> impl Future<Output = Result<Status, Self::Error>> + Send;

    // --- Extensions ---
    fn extended(&mut self, id: u32, request: String, data: Vec<u8>)
        -> impl Future<Output = Result<Packet, Self::Error>> + Send;
}
```

Every method has a default implementation that calls `self.unimplemented()` and returns it as `Err`, making it safe to only implement the methods you need.

### Handler Error Type

The associated `Error` type must implement `Into<StatusReply>`. When a handler method returns `Err(e)`, the framework converts the error to a `StatusReply` and sends an `SSH_FXP_STATUS` packet with the appropriate `StatusCode`, error message, and language tag.

### StatusReply

```rust
pub struct StatusReply {
    pub status_code: StatusCode,
    pub error_message: Option<String>,
    pub language_tag: Option<String>,
}
```

Convenience constructors:
```rust
impl StatusReply {
    pub fn new(status_code: StatusCode) -> Self;
    pub fn with_message(self, message: impl Into<String>) -> Self;
    pub fn with_language_tag(self, tag: impl Into<String>) -> Self;
}

impl StatusCode {
    pub fn with_message(self, message: impl Into<String>) -> StatusReply;
}
```

Example:
```rust
// Using StatusCode directly — minimal allocation
Err(StatusCode::NoSuchFile)

// With custom message
Err(StatusCode::PermissionDenied.with_message("access denied"))

// Full control
Err(StatusReply::new(StatusCode::Failure)
    .with_message("disk full")
    .with_language_tag("en-US"))
```

All of these produce `StatusReply`, which then maps to:
```
SSH_FXP_STATUS { id, status_code, error_message, language_tag }
```

## Request Processing Pipeline

### The `into_wrap!` Macro

Each request is processed via a macro that extracts fields from the packet and calls the handler:

```rust
macro_rules! into_wrap {
    ($id:expr, $handler:expr, $var:ident; $($arg:ident),*) => {
        match $handler.$var($($var.$arg),*).await {
            Err(err) => {
                let StatusReply { status_code, error_message, language_tag } = err.into();
                Packet::Status(Status {
                    id: $id,
                    status_code,
                    error_message: error_message.unwrap_or_else(|| status_code.to_string()),
                    language_tag: language_tag.unwrap_or_else(|| "en-US".to_string()),
                })
            },
            Ok(packet) => packet.into(),
        }
    };
}
```

This macro:
1. Calls the handler method with the packet fields
2. On `Ok(packet)` — converts the result to a `Packet` via `.into()`
3. On `Err(err)` — converts error to `StatusReply`, then constructs a `Packet::Status` with defaults for missing fields

### The `process_request` Function

```rust
async fn process_request<H>(packet: Packet, handler: &mut H) -> Packet
where H: Handler + Send
{
    let id = packet.get_request_id();
    match packet {
        Packet::Init(init) => into_wrap!(id, handler, init; version, extensions),
        Packet::Open(open) => into_wrap!(id, handler, open; id, filename, pflags, attrs),
        Packet::Close(close) => into_wrap!(id, handler, close; id, handle),
        Packet::Read(read) => into_wrap!(id, handler, read; id, handle, offset, len),
        // ... all other variants
        Packet::Extended(extended) => into_wrap!(id, handler, extended; id, request, data),
        _ => Packet::error(0, StatusCode::BadMessage),
    }
}
```

### The `run` Functions

```rust
pub async fn run<S, H>(stream: S, handler: H)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    H: Handler + Send + 'static;

pub async fn run_with_config<S, H>(mut stream: S, mut handler: H, cfg: Config)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    H: Handler + Send + 'static;
```

The server loop:
1. Reads a packet from the stream (`read_packet`)
2. Deserializes to `Packet`
3. On deserialization error, sends `Packet::error(0, StatusCode::BadMessage)`
4. Calls `process_request()` to dispatch to the handler
5. Serializes the response and writes it back to the stream
6. Flushes the stream
7. Repeats until `UnexpectedEof` or cancellation

The server is spawned on `tokio::spawn` (or `wasm_bindgen_futures::spawn_local` on WASM) and runs independently.

### Response Types

Each handler method return type maps to a specific response packet:

| Handler Method | Success Return | Packet Type |
|---------------|----------------|-------------|
| `init` | `Version` | `SSH_FXP_VERSION` |
| `open` | `Handle` | `SSH_FXP_HANDLE` |
| `close` | `Status` | `SSH_FXP_STATUS` |
| `read` | `Data` | `SSH_FXP_DATA` |
| `write` | `Status` | `SSH_FXP_STATUS` |
| `lstat` / `stat` / `fstat` | `Attrs` | `SSH_FXP_ATTRS` |
| `setstat` / `fsetstat` | `Status` | `SSH_FXP_STATUS` |
| `opendir` | `Handle` | `SSH_FXP_HANDLE` |
| `readdir` | `Name` | `SSH_FXP_NAME` |
| `remove` / `mkdir` / `rmdir` / `rename` / `symlink` | `Status` | `SSH_FXP_STATUS` |
| `realpath` / `readlink` | `Name` | `SSH_FXP_NAME` |
| `extended` | `Packet` | Any response type |

## Server Example

A minimal SFTP server with russh integration:

```rust
#[derive(Default)]
struct SftpSession {
    version: Option<u32>,
}

impl russh_sftp::server::Handler for SftpSession {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(&mut self, version: u32, _ext: HashMap<String, String>)
        -> Result<Version, Self::Error>
    {
        self.version = Some(version);
        Ok(Version::new())
    }

    async fn close(&mut self, id: u32, _handle: String) -> Result<Status, Self::Error> {
        Ok(Status { id, status_code: StatusCode::Ok, error_message: "Ok".into(), language_tag: "en-US".into() })
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        Ok(Handle { id, handle: path })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        // Return EOF when done
        Err(StatusCode::Eof)
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        Ok(Name { id, files: vec![File::dummy("/")] })
    }
}

// In the russh Handler:
async fn subsystem_request(&mut self, channel_id: ChannelId, name: &str, session: &mut Session)
    -> Result<(), Self::Error>
{
    if name == "sftp" {
        let channel = self.get_channel(channel_id).await;
        let sftp = SftpSession::default();
        session.channel_success(channel_id)?;
        russh_sftp::server::run(channel.into_stream(), sftp).await;
    } else {
        session.channel_failure(channel_id)?;
    }
    Ok(())
}
```

## WASM Considerations

The `server` module is gated behind `#[cfg(not(target_arch = "wasm32"))]` and is not available on WASM targets. The `client` module is available on both native and WASM via the `runtime.rs` abstraction.