# russh-sftp: Client Implementation

## Client Architecture Overview

The client side provides two tiers of API:

1. **`RawSftpSession`** — Low-level request-response client that sends individual SFTP packets and awaits their responses. Suitable for custom or non-standard SFTP interactions.

2. **`SftpSession`** — High-level client modeled after `std::fs`. Provides ergonomic methods for file operations and creates `File` objects implementing `AsyncRead`/`AsyncWrite`/`AsyncSeek`.

Both operate on a stream type `S: AsyncRead + AsyncWrite + Unpin + Send + 'static`.

## Client Handler Trait

The raw client infrastructure uses an internal `Handler` trait to dispatch incoming response packets:

```rust
pub trait Handler: Sized {
    type Error: Into<Error>;

    fn version(&mut self, version: Version) -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn status(&mut self, status: Status) -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn handle(&mut self, handle: Handle) -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn data(&mut self, data: Data) -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn name(&mut self, name: Name) -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn attrs(&mut self, attrs: Attrs) -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn extended_reply(&mut self, reply: ExtendedReply) -> impl Future<Output = Result<(), Self::Error>> + Send;
}
```

With the `async-trait` feature enabled, this becomes `#[async_trait]`.

The `run()` function spawns two tasks:
- **Reader task**: reads packets from the stream, dispatches to the handler
- **Writer task**: sends serialized packets from an `mpsc::UnboundedSender<Bytes>`

```rust
pub fn run<S, H>(stream: S, handler: H) -> mpsc::UnboundedSender<Bytes>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    H: Handler + Send + 'static,
```

The returned sender is the write handle — callers push `Bytes` (serialized packets) into it.

## RawSftpSession

`RawSftpSession` is the core request-response client. It implements `Handler` internally via `SessionInner` to route response packets back to awaiting request futures.

### Construction

```rust
impl RawSftpSession {
    pub fn new<S>(stream: S) -> Self
    where S: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    pub fn new_with_config<S>(stream: S, cfg: Config) -> Self
    where S: AsyncRead + AsyncWrite + Unpin + Send + 'static;
}
```

### Internal Architecture

```
┌─────────────────────────────────────────────────────┐
│                  RawSftpSession                      │
│                                                      │
│  tx: UnboundedSender<Bytes>  ←── write to stream     │
│  requests: Arc<DashMap<Option<u32>, oneshot::Sender>>│
│  next_req_id: AtomicU32                              │
│  handles: AtomicU64                                  │
│  timeout: AtomicU64                                  │
│  limits: Limits                                      │
│                                                      │
│  ┌──────────────────────────────────────────────┐   │
│  │           SessionInner (Handler impl)          │   │
│  │                                                │   │
│  │  version() → stores version, replies to init  │   │
│  │  status() → replies by request ID             │   │
│  │  handle() → replies by request ID              │   │
│  │  data()   → replies by request ID              │   │
│  │  name()   → replies by request ID              │   │
│  │  attrs()  → replies by request ID              │   │
│  │  extended_reply() → replies by request ID      │   │
│  └──────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────┘
```

### Request-Response Mechanism

1. Caller invokes a method like `raw.open(filename, flags, attrs).await`
2. `RawSftpSession` generates a unique `id` via `AtomicU32::fetch_add`
3. Creates a `oneshot::channel` and inserts `(Some(id), sender)` into the `DashMap`
4. Serializes the packet to `Bytes` and sends it via the `tx` channel
5. Awaits the `oneshot::Receiver` with a timeout
6. When the reader task receives a response packet, `SessionInner::reply()` removes the entry from the `DashMap` and sends the `Packet` through the `oneshot` sender
7. The original request future receives the `Packet` and pattern-matches it

The `init()` method is special: it uses `id: None` (no request ID) and the version handler stores the negotiated version.

### Limits Tracking

```rust
pub struct Limits {
    pub packet_len: Option<u64>,
    pub read_len: Option<u64>,
    pub write_len: Option<u64>,
    pub open_handles: Option<u64>,
}
```

- Fetched from the server via the `limits@openssh.com` extension during `SftpSession::new_with_config()`
- Enforced client-side: `open()` and `opendir()` check `open_handles`, `read()` checks `read_len`, `write()` and `write_nowait()` check `write_len`, `send()` checks `packet_len`

### API Methods

| Method | SFTP Packet | Response | Notes |
|--------|------------|----------|-------|
| `init()` | `Init` | `Version` | Handshake; version negotiation |
| `open(filename, flags, attrs)` | `Open` | `Handle` | Opens a file; increments handle count |
| `close(handle)` | `Close` | `Status` | Closes a handle; decrements handle count |
| `close_nowait(handle)` | `Close` | — | Fire-and-forget close (returns oneshot receiver) |
| `read(handle, offset, len)` | `Read` | `Data` | Reads data; respects `Limits.read_len` |
| `write(handle, offset, data)` | `Write` | `Status` | Writes data; respects `Limits.write_len` |
| `write_nowait(handle, offset, data)` | `Write` | — | Fire-and-forget write |
| `lstat(path)` | `Lstat` | `Attrs` | Stat without following symlinks |
| `fstat(handle)` | `Fstat` | `Attrs` | Stat an open handle |
| `setstat(path, attrs)` | `SetStat` | `Status` | Set file attributes by path |
| `fsetstat(handle, attrs)` | `FSetStat` | `Status` | Set file attributes by handle |
| `opendir(path)` | `OpenDir` | `Handle` | Opens a directory; increments handle count |
| `readdir(handle)` | `ReadDir` | `Name` | Reads directory entries |
| `remove(filename)` | `Remove` | `Status` | Deletes a file |
| `mkdir(path, attrs)` | `MkDir` | `Status` | Creates a directory |
| `rmdir(path)` | `RmDir` | `Status` | Removes a directory |
| `realpath(path)` | `RealPath` | `Name` | Canonicalizes a path |
| `stat(path)` | `Stat` | `Status` | Stat following symlinks |
| `rename(oldpath, newpath)` | `Rename` | `Status` | Renames a file |
| `readlink(path)` | `ReadLink` | `Name` | Reads symlink target |
| `symlink(path, target)` | `Symlink` | `Status` | Creates a symbolic link |
| `extended(request, data)` | `Extended` | `Packet` | Generic extension mechanism |
| `limits()` | `Extended` | `LimitsExtension` | `limits@openssh.com` |
| `hardlink(oldpath, newpath)` | `Extended` | `Status` | `hardlink@openssh.com` |
| `fsync(handle)` | `Extended` | `Status` | `fsync@openssh.com` |
| `statvfs(path)` | `Extended` | `Statvfs` | `statvfs@openssh.com` v2 |

### Response Classification

Two macros handle the common response patterns:

```rust
// For responses that return a specific packet type or error status
macro_rules! into_with_status {
    ($result:ident, $packet:ident) => {
        match $result {
            Packet::$packet(p) => Ok(p),
            Packet::Status(p) => Err(p.into()),
            _ => Err(Error::UnexpectedPacket),
        }
    };
}

// For responses that only return success/failure status
macro_rules! into_status {
    ($result:ident) => {
        match $result {
            Packet::Status(status) if status.status_code == StatusCode::Ok => Ok(status),
            Packet::Status(status) => Err(status.into()),
            _ => Err(Error::UnexpectedPacket),
        }
    };
}
```

### Drop Behavior

`RawSftpSession` implements `Drop` to call `close_session()`, which sends an empty `Bytes` to signal the writer task to shut down the stream.

## SftpSession (High-Level Client)

`SftpSession` wraps `RawSftpSession` in an `Arc` and provides `std::fs`-like methods:

```rust
pub struct SftpSession {
    session: Arc<RawSftpSession>,
    features: Features,
}
```

### Construction and Version Negotiation

```rust
impl SftpSession {
    pub async fn new<S>(stream: S) -> SftpResult<Self>;
    pub async fn new_with_config<S>(stream: S, cfg: Config) -> SftpResult<Self>;
}
```

Construction performs:
1. Creates `RawSftpSession` with config
2. Sends `SSH_FXP_INIT` via `session.init().await`
3. Checks for supported extensions in the version response
4. If `limits@openssh.com` extension is available, fetches and applies limits
5. Stores detected feature flags in `Features`

### Features Detection

```rust
pub(crate) struct Features {
    pub hardlink: bool,           // hardlink@openssh.com v1
    pub fsync: bool,              // fsync@openssh.com v1
    pub statvfs: bool,            // statvfs@openssh.com v2
    pub limits: Option<Limits>,   // from limits@openssh.com
    pub max_concurrent_writes: usize,
    pub max_packet_len: u32,
}
```

### High-Level API

| Method | Description |
|--------|-------------|
| `open(filename)` | Opens file read-only |
| `create(filename)` | Creates/truncates file write-only |
| `open_with_flags(filename, flags)` | Opens with specified `OpenFlags` |
| `open_with_flags_and_attributes(filename, flags, attrs)` | Opens with flags and initial attrs |
| `canonicalize(path)` | Resolves to absolute path via `realpath` |
| `create_dir(path)` | Creates directory |
| `read(path)` | Reads entire file to `Vec<u8>` |
| `write(path, data)` | Writes data to file |
| `try_exists(path)` | Checks existence (returns `Ok(false)` on `NoSuchFile`) |
| `read_dir(path)` | Returns `ReadDir` iterator over directory entries |
| `read_link(path)` | Reads symlink target |
| `remove_dir(path)` | Removes directory |
| `remove_file(filename)` | Removes file |
| `rename(oldpath, newpath)` | Renames file/directory |
| `symlink(path, target)` | Creates symbolic link |
| `metadata(path)` | Gets `FileAttributes` via `stat` |
| `set_metadata(path, metadata)` | Sets attributes via `setstat` |
| `symlink_metadata(path)` | Gets `FileAttributes` via `lstat` |
| `hardlink(oldpath, newpath)` | Creates hard link (returns `Ok(false)` if unsupported) |
| `fs_info(path)` | Gets filesystem stats via `statvfs` (returns `Ok(None)` if unsupported) |
| `set_timeout(secs)` | Sets request timeout |
| `close()` | Closes the SFTP session stream |

## File (Async I/O)

`client::fs::File` implements `AsyncRead`, `AsyncWrite`, and `AsyncSeek` for remote file I/O:

```rust
pub struct File {
    session: Arc<RawSftpSession>,
    handle: String,
    state: FileState,
    pos: u64,
    closed: bool,
    features: Features,
}

struct FileState {
    f_read: Option<Pin<Box<dyn Future<Output = io::Result<Option<Vec<u8>>>>>>>,
    f_seek: Option<Pin<Box<dyn Future<Output = io::Result<u64>>>>>,
    f_flush: Option<Pin<Box<dyn Future<Output = io::Result<()>>>>>,
    f_shutdown: Option<Pin<Box<dyn Future<Output = io::Result<()>>>>>,
    write_acks: VecDeque<oneshot::Receiver<SftpResult<Packet>>>,
}
```

### AsyncRead Implementation

- Uses a state-machine pattern: `f_read` stores the in-progress read future
- Calculates read length as `min(remaining_buffer, max_read_len)` where `max_read_len` comes from `Limits.read_len` or `max_packet_len - 9` (read overhead)
- On `StatusCode::Eof`, returns `Ok(None)` → signals clean EOF to the caller
- Advances `pos` by the data length on successful read

### AsyncWrite Implementation

- Implements **pipelined writes** with configurable concurrency (`max_concurrent_writes`)
- Uses `write_nowait()` to fire off write requests without awaiting each ACK
- Stores `oneshot::Receiver`s in `write_acks: VecDeque`
- When `write_acks.len() >= max_concurrent_writes`, polls the oldest pending ACK before accepting new writes
- Write chunk size: `min(data.len(), max_write_len)` where `max_write_len` comes from `Limits.write_len` or `max_packet_len - 21 - handle_length`

Overhead constants:
```rust
const READ_OVERHEAD_LENGTH: u32 = 9;   // type(1) + id(4) + data_len(4)
const WRITE_OVERHEAD_LENGTH: u32 = 21;  // type(1) + id(4) + handle_len(4) + offset(8) + data_len(4)
```

### AsyncSeek Implementation

- `SeekFrom::Start(pos)` — direct position set
- `SeekFrom::Current(delta)` — arithmetic on current `pos`
- `SeekFrom::End(delta)` — requires an `fstat()` round-trip to get file size, then computes position

### Flush and Shutdown

- `poll_flush()`: Drains all pending write ACKs, then optionally calls `fsync` if the server supports it
- `poll_shutdown()`: Drains pending ACKs, then sends `close()` on the handle and marks `closed = true`

### Drop Behavior

If `closed` is not yet true, `File::drop()` sends a `close_nowait()` (fire-and-forget) to avoid blocking in a destructor.

### File Metadata Methods

```rust
impl File {
    pub async fn metadata(&self) -> SftpResult<Metadata>;    // fstat
    pub async fn set_metadata(&self, metadata: Metadata) -> SftpResult<()>; // fsetstat
    pub async fn sync_all(&self) -> SftpResult<()>;          // fsync (no-op if unsupported)
}
```

## ReadDir and DirEntry

```rust
pub struct ReadDir {
    parent: Arc<str>,
    entries: VecDeque<(String, Metadata)>,
}

pub struct DirEntry {
    parent: Arc<str>,
    file: String,
    metadata: Metadata,
}
```

- `ReadDir` implements `Iterator<Item = DirEntry>`
- Automatically filters out `.` and `..` entries
- `DirEntry::path()` constructs full paths using POSIX-style `/` separator
- `DirEntry::file_name()`, `file_type()`, `metadata()` provide accessors

## Runtime Abstraction (`client/runtime.rs`)

The client runtime abstracts over native tokio and WASM environments:

- **Native** (`not(target_arch = "wasm32")`): Uses `tokio::spawn` and `tokio::time::timeout`
- **WASM** (`target_arch = "wasm32"`): Uses `wasm_bindgen_futures::spawn_local` and `gloo_timers::future::TimeoutFuture`

The `spawn()` function returns a `JoinHandle<T>` that wraps a `oneshot::Receiver`, providing a unified API for both platforms.

## Timeout Behavior

Each request in `RawSftpSession` has a configurable timeout (default 10 seconds, set via `Config::request_timeout_secs`):

```rust
async fn request(&self, id: Option<u32>, packet: Packet) -> SftpResult<Packet> {
    let rx = self.send(id, packet)?;
    let timeout = self.timeout.load(Ordering::Relaxed);
    match runtime::timeout(Duration::from_secs(timeout), rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(Error::UnexpectedBehavior("sender dropped".into())),
        Err(error) => {
            self.requests.remove(&id);
            Err(error)  // Error::Timeout
        }
    }
}
```

On timeout, the pending request entry is cleaned up from the `DashMap`.