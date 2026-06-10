# sftp-rs: Asynchronous Client (`async.rs`)

## `AsyncSftpClient<W>`

An async SFTP client that supports **concurrent pipelined requests** over a single connection via a background reader task:

```rust
pub struct AsyncSftpClient<W> {
    writer: TokioMutex<W>,
    pending: Pending,
    last_request_id: AtomicU32,
    version: u32,
    extensions: Vec<(String, String)>,
    reader_task: TokioMutex<Option<tokio::task::JoinHandle<()>>>,
}
```

Where:
```rust
type Pending = Arc<StdMutex<HashMap<u32, oneshot::Sender<(u8, Vec<u8>)>>>>;
```

## Architecture: Background Reader + Oneshot Channels

Unlike the sync client (which does send-then-receive per request), the async client decouples writing from reading:

1. **Writer side**: Each call to `process()` writes a request packet (with a unique request-id) to the `writer`, protected by a `TokioMutex`
2. **Reader side**: A spawned tokio task (`run_reader`) continuously reads packets from the reader half, strips the request-id from each response, and routes it to the matching `oneshot::Sender` in the `pending` map
3. **Caller**: Awaits on the `oneshot::Receiver`, which resolves when the reader task delivers the matching response

This allows multiple requests to be in flight simultaneously — the client can send requests 1, 2, and 3, and the reader will route each response to the correct waiter regardless of arrival order.

```
┌─────────────────┐     write     ┌──────────────┐
│  calling task   │──────────────→│  writer (W)   │
│  (await rx)     │              └──────────────┘
└────────┬────────┘
         │ oneshot channel
         │ (tx inserted into pending map)
         │
┌────────┴────────┐     read      ┌──────────────┐
│  reader task    │←──────────────│  reader (R)   │
│  (run_reader)   │               └──────────────┘
│                 │
│  1. read packet │
│  2. split req_id│
│  3. lookup pending[req_id]
│  4. send via tx │
└─────────────────┘
```

## Construction

```rust
impl<W: AsyncWrite + Unpin + Send + 'static> AsyncSftpClient<W> {
    pub async fn new<R>(mut reader: R, mut writer: W) -> std::io::Result<Self>
    where
        R: AsyncRead + Unpin + Send + 'static,
}
```

The constructor:
1. Sends `SSH_FXP_INIT` with version 3
2. Reads the response, expects `SSH_FXP_VERSION`
3. Parses version and extensions
4. Spawns the background reader task (`run_reader`)
5. Returns the client

The reader and writer are provided as separate halves — typically obtained via `tokio::io::split()` on a duplex stream.

## Drop Implementation

```rust
impl<W> Drop for AsyncSftpClient<W> {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.reader_task.try_lock() {
            if let Some(handle) = guard.take() {
                handle.abort();
            }
        }
    }
}
```

When the client is dropped, the background reader task is aborted. This prevents the task from running after the client's channels are gone. The `try_lock()` avoids blocking in the drop handler.

## Request-Response Cycle: `process()`

```rust
async fn process(&self, cmd: u8, body: &[u8]) -> std::io::Result<(u8, Vec<u8>)>
```

1. Allocate `request_id` via `AtomicU32::fetch_add(1, SeqCst)`
2. Create a `oneshot::channel()`
3. Insert `tx` into `pending[request_id]`
4. Prepend request-id: `with_request_id(request_id, body)`
5. Lock `writer` and send the packet via `write_packet_async`
6. If the write fails, remove the pending entry and return the error
7. Await on `rx` — resolves with `(cmd, payload)` when the reader task delivers the response

## Background Reader: `run_reader()`

```rust
async fn run_reader<R: AsyncRead + Unpin>(mut reader: R, pending: Pending)
```

Runs in a loop:
1. Read a packet via `read_packet_async`
2. Split the request-id from the body
3. Look up `pending[request_id]` and remove it
4. Send `(cmd, payload)` via the oneshot channel
5. If the read fails (EOF, connection error), clear the entire pending map so all waiting tasks get a `RecvError` and return errors

## Async Packet I/O

```rust
async fn read_packet_async<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<(u8, Vec<u8>)>
async fn write_packet_async<W: AsyncWrite + Unpin>(w: &mut W, kind: u8, body: &[u8]) -> std::io::Result<()>
```

These mirror the sync `read_raw_packet` / `write_raw_packet` but use `AsyncReadExt` / `AsyncWriteExt`. The write function builds the header inline:

```rust
let mut hdr = Vec::with_capacity(5);
hdr.extend_from_slice(&(body.len() as u32 + 1).to_be_bytes());  // length (includes type byte)
hdr.push(kind);                                                  // type
w.write_all(&hdr).await?;
w.write_all(body).await?;
w.flush().await?;
```

## Public API

The async client exposes the same operations as the sync client, but all methods are `async`:

```rust
// Directory operations
pub async fn mkdir(&self, path: &str, attr: &Attributes) -> Result<()>
pub async fn rmdir(&self, path: &str) -> Result<()>
pub async fn opendir(&self, path: &str) -> Result<Directory>
pub async fn readdir(&self, dir: &Directory) -> Result<Vec<(String, String, Attributes)>>
pub async fn closedir(&self, dir: &Directory) -> Result<()>

// File operations
pub async fn open(&self, path: &str, options: OpenOptions, attr: &Attributes) -> Result<File>
pub async fn pread(&self, file: &File, offset: u64, length: u32) -> Result<Vec<u8>>
pub async fn pwrite(&self, file: &File, offset: u64, data: &[u8]) -> Result<()>
pub async fn fclose(&self, file: &File) -> Result<()>

// Attribute operations
pub async fn stat(&self, path: &str, flags: Option<u32>) -> Result<Attributes>
pub async fn lstat(&self, path: &str, flags: Option<u32>) -> Result<Attributes>
pub async fn fstat(&self, file: &File, flags: Option<u32>) -> Result<Attributes>
pub async fn setstat(&self, path: &str, attr: &Attributes) -> Result<()>
pub async fn fsetstat(&self, file: &File, attr: &Attributes) -> Result<()>

// Path operations
pub async fn realpath(&self, path: &str, control_byte: Option<u8>, compose_path: Option<&str>) -> Result<String>
pub async fn readlink(&self, path: &str) -> Result<String>
pub async fn remove(&self, path: &str) -> Result<()>
pub async fn rename(&self, oldpath: &str, newpath: &str, flags: Option<u32>) -> Result<()>

// Link operations
pub async fn symlink(&self, path: &str, target: &str) -> Result<()>
pub async fn hardlink(&self, path: &str, target: &str) -> Result<()>
pub async fn link(&self, path: &str, target: &str, symlink: bool) -> Result<()>

// Lock operations
pub async fn block(&self, file: &File, offset: u64, length: u64, lockmask: u32) -> Result<()>
pub async fn unblock(&self, file: &File, offset: u64, length: u64) -> Result<()>

// Extended operations
pub async fn extended(&self, request: &str, data: &[u8]) -> Result<Option<Vec<u8>>>
pub async fn flineseek(&self, file: &File, lineno: u64) -> Result<()>

// Introspection
pub fn extensions(&self) -> &[(String, String)]
pub fn version(&self) -> u32
```

## Concurrency Benefits

Because the reader task decouples receiving from sending, multiple async operations can run concurrently:

```rust
// Three concurrent mkdir requests — all three are sent before any
// response arrives, and the reader task routes each response correctly
let (r1, r2, r3) = tokio::join!(
    client.mkdir("/a", &attrs),
    client.mkdir("/b", &attrs),
    client.rmdir("/c"),
);
```

The sync client cannot do this — each `process()` call blocks on its response before the next request can be sent.

## Error Propagation on Disconnect

When the reader task encounters a read error (connection closed), it:
1. Clears the entire `pending` map
2. All `oneshot::Receiver`s in waiting tasks receive `Err(RecvError)`
3. The `process()` method converts this to `std::io::Error("reader task closed before response arrived")`

This ensures that pending operations fail promptly rather than hanging indefinitely when the connection drops.

## Pending Map: `StdMutex` vs `TokioMutex`

The `pending` map uses `std::sync::Mutex` rather than `tokio::sync::Mutex` because:
- The critical section is tiny (insert/remove from a HashMap)
- The reader task and writer are on different async tasks but need shared access
- `StdMutex` avoids holding a lock across `.await` points (the oneshot `rx.await` is outside the lock)