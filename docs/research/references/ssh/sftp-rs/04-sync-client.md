# sftp-rs: Synchronous Client (`sync.rs`)

## `SftpClient<C>`

A synchronous SFTP client parameterized over any `Read + Write` channel:

```rust
pub struct SftpClient<C> {
    channel: Mutex<C>,
    last_request_id: std::sync::atomic::AtomicU32,
    version: u32,
    extensions: Vec<(String, String)>,
}
```

The `Mutex<C>` ensures the channel is accessed by one thread at a time — each `process()` call writes a request, then reads the response, atomically. The `AtomicU32` for request IDs allows the client to be shared across threads via `&self` (all methods take `&self`, not `&mut self`).

## Construction

```rust
impl<C: Read + Write> SftpClient<C> {
    pub fn new(mut channel: C) -> std::io::Result<Self>
}
```

The constructor performs the SFTP handshake inline:
1. Writes `SSH_FXP_INIT` with version 3
2. Flushes the channel
3. Reads the response, expecting `SSH_FXP_VERSION`
4. Parses version and extensions
5. Returns the client if version == 3

### Platform-Specific Construction

```rust
impl SftpClient<std::fs::File> {
    #[cfg(unix)]
    pub fn from_fd(fd: i32) -> std::io::Result<Self>

    #[cfg(windows)]
    pub fn from_handle(handle: RawHandle) -> std::io::Result<Self>
}
```

These wrap an OS file descriptor or handle into a `std::fs::File`, then delegate to `new()`. This is the typical way to connect to an SSH subprocess that has the SFTP subsystem active on its stdin/stdout.

### ssh2 Integration

```rust
#[cfg(feature = "ssh2")]
impl TryFrom<ssh2::Channel> for SftpClient<ssh2::Channel> {
    type Error = std::io::Error;
    fn try_from(mut channel: ssh2::Channel) -> std::result::Result<Self, Self::Error>
}
```

Requests the `sftp` subsystem on the libssh2 channel, then delegates to `SftpClient::new()`.

## Request-Response Cycle: `process()`

The core internal method that drives all operations:

```rust
fn process(&self, cmd: u8, body: &[u8]) -> std::io::Result<(u8, Vec<u8>)>
```

1. Allocate a new `request_id` via `AtomicU32::fetch_add(1, SeqCst)`
2. Prepend the request-id to the body: `with_request_id(request_id, body)`
3. Lock the channel mutex
4. Write the packet: `write_raw_packet(channel, cmd, &body_with_id)`
5. Read the response: `read_raw_packet(channel)`
6. Strip the request-id from the response: `split_request_id(&buf)`
7. Assert the response request-id matches the sent one
8. Return `(response_cmd, payload)`

**Important limitation**: Because `process()` sends then immediately reads, the sync client cannot pipeline requests. Each request blocks until its response arrives. Multiple concurrent operations require separate connections.

## Public API

All methods take `&self` and return `Result<T>`:

### Directory Operations

```rust
pub fn mkdir(&self, path: &str, attr: &Attributes) -> Result<()>
pub fn rmdir(&self, path: &str) -> Result<()>
pub fn opendir(&self, path: &str) -> Result<Directory>
pub fn readdir(&self, dir: &Directory) -> Result<Vec<(String, String, Attributes)>>
pub fn closedir(&self, dir: &Directory) -> Result<()>
```

`readdir()` returns `(filename, longname, attributes)` triples. The `longname` is a human-readable string (like `ls -l` output) provided by the server in v3. Callers should loop on `readdir()` until it returns `Error::Eof` to exhaust all directory entries.

### File Operations

```rust
pub fn open(&self, path: &str, options: OpenOptions, attr: &Attributes) -> Result<File>
pub fn pread(&self, file: &File, offset: u64, length: u32) -> Result<Vec<u8>>
pub fn pwrite(&self, file: &File, offset: u64, data: &[u8]) -> Result<()>
pub fn fclose(&self, file: &File) -> Result<()>
```

File I/O is positional (`pread`/`pwrite`) — there is no implicit cursor. The caller tracks the offset.

### Attribute Operations

```rust
pub fn stat(&self, path: &str, flags: Option<u32>) -> Result<Attributes>    // follows symlinks
pub fn lstat(&self, path: &str, flags: Option<u32>) -> Result<Attributes>   // doesn't follow
pub fn fstat(&self, file: &File, flags: Option<u32>) -> Result<Attributes>
pub fn setstat(&self, path: &str, attr: &Attributes) -> Result<()>
pub fn fsetstat(&self, file: &File, attr: &Attributes) -> Result<()>
```

### Path Operations

```rust
pub fn realpath(&self, path: &str, control_byte: Option<u8>, compose_path: Option<&str>) -> Result<String>
pub fn readlink(&self, path: &str) -> Result<String>
pub fn remove(&self, path: &str) -> Result<()>
pub fn rename(&self, oldpath: &str, newpath: &str, flags: Option<u32>) -> Result<()>
```

### Link Operations

```rust
pub fn symlink(&self, path: &str, target: &str) -> Result<()>
pub fn hardlink(&self, path: &str, target: &str) -> Result<()>
pub fn link(&self, path: &str, target: &str, symlink: bool) -> Result<()>
```

`symlink()` uses `SSH_FXP_SYMLINK` (v3). `link()` and `hardlink()` use `SSH_FXP_LINK` (v5+ extension).

### Lock Operations

```rust
pub fn block(&self, file: &File, offset: u64, length: u64, lockmask: u32) -> Result<()>
pub fn unblock(&self, file: &File, offset: u64, length: u64) -> Result<()>
```

### Extended Operations

```rust
pub fn extended(&self, request: &str, data: &[u8]) -> Result<Option<Vec<u8>>>
pub fn flineseek(&self, file: &File, lineno: u64) -> Result<()>
```

`flineseek()` sends the `text-seek` extended request with the handle and line number. `extended()` returns `Some(data)` if the server sends `SSH_FXP_EXTENDED_REPLY`, or `None` if it sends `SSH_FXP_STATUS` with `SSH_FX_OK`.

## Introspection

```rust
pub fn version(&self) -> u32
pub fn extensions(&self) -> &[(String, String)]
```

Returns the negotiated protocol version and any server-advertised extensions from the handshake.

## Request-Response Mapping

| Method | Request Type | Expected Response |
|--------|-------------|-------------------|
| `open` | `SSH_FXP_OPEN` | `SSH_FXP_HANDLE` |
| `opendir` | `SSH_FXP_OPENDIR` | `SSH_FXP_HANDLE` |
| `pread` | `SSH_FXP_READ` | `SSH_FXP_DATA` |
| `pwrite` | `SSH_FXP_WRITE` | `SSH_FXP_STATUS` |
| `fclose` | `SSH_FXP_CLOSE` | `SSH_FXP_STATUS` |
| `stat` | `SSH_FXP_STAT` | `SSH_FXP_ATTRS` |
| `lstat` | `SSH_FXP_LSTAT` | `SSH_FXP_ATTRS` |
| `fstat` | `SSH_FXP_FSTAT` | `SSH_FXP_ATTRS` |
| `setstat` | `SSH_FXP_SETSTAT` | `SSH_FXP_STATUS` |
| `fsetstat` | `SSH_FXP_FSETSTAT` | `SSH_FXP_STATUS` |
| `mkdir` | `SSH_FXP_MKDIR` | `SSH_FXP_STATUS` |
| `rmdir` | `SSH_FXP_RMDIR` | `SSH_FXP_STATUS` |
| `remove` | `SSH_FXP_REMOVE` | `SSH_FXP_STATUS` |
| `rename` | `SSH_FXP_RENAME` | `SSH_FXP_STATUS` |
| `readdir` | `SSH_FXP_READDIR` | `SSH_FXP_NAME` |
| `closedir` | `SSH_FXP_CLOSE` | `SSH_FXP_STATUS` |
| `realpath` | `SSH_FXP_REALPATH` | `SSH_FXP_NAME` |
| `readlink` | `SSH_FXP_READLINK` | `SSH_FXP_NAME` |
| `symlink` | `SSH_FXP_SYMLINK` | `SSH_FXP_STATUS` |
| `link` | `SSH_FXP_LINK` | `SSH_FXP_STATUS` |
| `block` | `SSH_FXP_BLOCK` | `SSH_FXP_STATUS` |
| `unblock` | `SSH_FXP_UNBLOCK` | `SSH_FXP_STATUS` |
| `extended` | `SSH_FXP_EXTENDED` | `SSH_FXP_EXTENDED_REPLY` or `SSH_FXP_STATUS` |