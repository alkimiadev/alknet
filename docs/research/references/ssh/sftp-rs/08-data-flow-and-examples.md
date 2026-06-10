# sftp-rs: Data Flow and Examples

## File Download Flow (Sync Client)

The most common SFTP operation — reading a file from the server:

```
Client                                          Server
  │                                                │
  │  SSH_FXP_OPEN [req_id=1] [path] [flags] [attrs] │
  │───────────────────────────────────────────────→│
  │                                                │
  │  SSH_FXP_HANDLE [req_id=1] [handle_bytes]      │
  │←───────────────────────────────────────────────│
  │                                                │
  │  SSH_FXP_READ [req_id=2] [handle] [offset=0] [len=32K]
  │───────────────────────────────────────────────→│
  │                                                │
  │  SSH_FXP_DATA [req_id=2] [data_bytes]          │
  │←───────────────────────────────────────────────│
  │                                                │
  │  SSH_FXP_READ [req_id=3] [handle] [offset=N] [len=32K]
  │───────────────────────────────────────────────→│
  │                                                │
  │  SSH_FXP_STATUS [req_id=3] [SSH_FX_EOF]       │
  │←───────────────────────────────────────────────│
  │                                                │
  │  SSH_FXP_CLOSE [req_id=4] [handle]            │
  │───────────────────────────────────────────────→│
  │                                                │
  │  SSH_FXP_STATUS [req_id=4] [SSH_FX_OK]        │
  │←───────────────────────────────────────────────│
```

### Sync Code Path

```rust
let file = client.open("/remote/file", OpenOptions::new().read(true), &Attributes::new())?;
let mut offset: u64 = 0;
const CHUNK: u32 = 32 * 1024;
loop {
    match client.pread(&file, offset, CHUNK) {
        Ok(data) if data.is_empty() => break,
        Ok(data) => { /* write data locally */ offset += data.len() as u64; }
        Err(Error::Eof(_, _)) => break,
        Err(e) => return Err(e),
    }
}
client.fclose(&file)?;
```

### Async Code Path

```rust
let file = client.open("/remote/file", OpenOptions::new().read(true), &Attributes::new()).await?;
let mut offset: u64 = 0;
const CHUNK: u32 = 32 * 1024;
loop {
    match client.pread(&file, offset, CHUNK).await {
        Ok(data) if data.is_empty() => break,
        Ok(data) => { /* write data locally */ offset += data.len() as u64; }
        Err(Error::Eof(_, _)) => break,
        Err(e) => return Err(e),
    }
}
client.fclose(&file).await?;
```

## File Upload Flow

```
Client                                          Server
  │                                                │
  │  SSH_FXP_OPEN [path] [WRITE|CREAT|TRUNC] [attrs]│
  │───────────────────────────────────────────────→│
  │                                                │
  │  SSH_FXP_HANDLE [handle]                      │
  │←───────────────────────────────────────────────│
  │                                                │
  │  SSH_FXP_WRITE [handle] [offset=0] [data]     │
  │───────────────────────────────────────────────→│
  │                                                │
  │  SSH_FXP_STATUS [SSH_FX_OK]                  │
  │←───────────────────────────────────────────────│
  │                                                │
  │  ... (repeat for each chunk) ...               │
  │                                                │
  │  SSH_FXP_CLOSE [handle]                       │
  │───────────────────────────────────────────────→│
  │                                                │
  │  SSH_FXP_STATUS [SSH_FX_OK]                  │
  │←───────────────────────────────────────────────│
```

## Directory Listing Flow

```
Client                                          Server
  │                                                │
  │  SSH_FXP_OPENDIR [path]                       │
  │───────────────────────────────────────────────→│
  │                                                │
  │  SSH_FXP_HANDLE [dir_handle]                  │
  │←───────────────────────────────────────────────│
  │                                                │
  │  SSH_FXP_READDIR [dir_handle]                 │
  │───────────────────────────────────────────────→│
  │                                                │
  │  SSH_FXP_NAME [count] [name+longname+attrs]   │
  │←───────────────────────────────────────────────│
  │                                                │
  │  SSH_FXP_READDIR [dir_handle]                 │
  │───────────────────────────────────────────────→│
  │                                                │
  │  SSH_FXP_STATUS [SSH_FX_EOF]                  │
  │←───────────────────────────────────────────────│
  │                                                │
  │  SSH_FXP_CLOSE [dir_handle]                   │
  │───────────────────────────────────────────────→│
  │                                                │
  │  SSH_FXP_STATUS [SSH_FX_OK]                  │
  │←───────────────────────────────────────────────│
```

## Async Pipelining Flow

The async client can send multiple requests before any responses arrive:

```
Task 1          Task 2          Task 3          Reader Task
  │               │               │                │
  │ process()    │ process()     │ process()      │
  │ req_id=1     │ req_id=2      │ req_id=3       │
  │ insert tx1   │ insert tx2    │ insert tx3     │
  │ write pkt    │ write pkt     │ write pkt      │
  │ await rx1    │ await rx2     │ await rx3      │
  │              │               │                │ read pkt
  │              │               │                │ req_id=2 → tx2
  │              │               │                │
  │              │ rx2 resolved  │                │
  │              │ returns ───────│                │
  │              │               │                │ read pkt
  │              │               │                │ req_id=1 → tx1
  │ rx1 resolved │               │                │
  │ returns ─────│               │                │
  │              │               │                │ read pkt
  │              │               │                │ req_id=3 → tx3
  │              │               │ rx3 resolved   │
  │              │               │ returns ───────│
```

## russh Integration Flow

```
┌──────────────────────────────────────────────────────────┐
│  Application Code                                        │
│                                                           │
│  // 1. Establish SSH session                              │
│  let session = /* russh client session setup */;         │
│                                                           │
│  // 2. Open a channel                                     │
│  let channel = session.channel_open_session().await?;   │
│                                                           │
│  // 3. Request SFTP subsystem + SFTP handshake            │
│  let sftp = sftp::russh::from_channel(channel).await?;  │
│                                                           │
│  // 4. Use SFTP                                           │
│  let file = sftp.open("/path", opts, &attrs).await?;    │
│  let data = sftp.pread(&file, 0, 4096).await?;          │
│  sftp.fclose(&file).await?;                              │
└──────────────────────────────────────────────────────────┘

Expanded step 3:
  from_channel(channel)
    │
    ├─ channel.request_subsystem(true, "sftp")
    │  └─ SSH_MSG_CHANNEL_REQUEST subsystem=sftp
    │     └─ SSH_MSG_CHANNEL_SUCCESS
    │
    ├─ channel.into_stream() → ChannelStream<Msg>
    │
    ├─ tokio::io::split(stream) → (read_half, write_half)
    │
    └─ AsyncSftpClient::new(read_half, write_half)
       ├─ write SSH_FXP_INIT version=3
       ├─ read SSH_FXP_VERSION
       ├─ parse version + extensions
       └─ spawn reader task
```

## Wire Format: `build_open` Example

Opening a file `/hello.txt` for read:

```rust
let opts = OpenOptions::new().read(true);
let body = build_open("/hello.txt", opts.get(), &Attributes::new())?;
// body layout:
// [0x00 0x00 0x00 0x0A]  path length = 10
// [0x2F 0x68 0x65 0x6C 0x6C 0x6F 0x2E 0x74 0x78 0x74]  "/hello.txt"
// [0x00 0x00 0x00 0x01]  flags = SFTP_FLAG_READ
// [0x00 0x00 0x00 0x00]  attrs (empty: valid_attribute_flags = 0)
```

Full packet on wire (with request-id = 42):
```
[0x00 0x00 0x00 0x19]  length = 25 (1 type + 4 req_id + 10 path + 4 flags + 4 flags + 4 attrs_len)
[0x03]                  type = SSH_FXP_OPEN
[0x00 0x00 0x00 0x2A]  request_id = 42
[0x00 0x00 0x00 0x0A]  path length = 10
"/hello.txt"            path
[0x00 0x00 0x00 0x01]  open flags = READ
[0x00 0x00 0x00 0x00]  attrs valid_flags = 0 (no attributes)
```

## Wire Format: `build_pread` Example

Reading 1024 bytes from offset 4096:

```rust
let body = build_pread(&handle, 4096, 1024);
// body layout:
// [0x00 0x00 0x00 0x04]  handle length = 4
// [0x48 0x41 0x4E 0x44]  handle = "HAND"
// [0x00 0x00 0x00 0x00 0x00 0x00 0x10 0x00]  offset = 4096
// [0x00 0x00 0x04 0x00]  length = 1024
```

## Wire Format: `SSH_FXP_STATUS` Response

Server returns `SSH_FX_NO_SUCH_FILE`:

```
[0x00 0x00 0x00 0x2A]  request_id = 42 (matching the request)
[0x00 0x00 0x00 0x02]  status = SSH_FX_NO_SUCH_FILE
[0x00 0x00 0x00 0x0F]  error message length = 15
"No such file"          error message
[0x00 0x00 0x00 0x02]  language tag length = 2
"en"                    language tag
```

The `parse_status()` function maps this to `Error::NoSuchFile("No such file".into(), "en".into())`.

## Error Handling Pattern

Both sync and async clients follow the same pattern for response handling:

```rust
// For operations that expect a handle (open, opendir):
let (cmd, data) = self.process(SSH_FXP_OPEN, &body).await?;
Ok(File(expect_handle(cmd, &data)?))
// If server sends SSH_FXP_STATUS instead of SSH_FXP_HANDLE,
// expect_handle() converts the status error to the appropriate Error variant.

// For operations that expect only status (mkdir, remove, close):
let (cmd, data) = self.process(SSH_FXP_MKDIR, &body).await?;
expect_status(cmd, &data)
// If server sends anything other than SSH_FXP_STATUS, returns unexpected response error.

// For operations that expect data (read):
let (cmd, data) = self.process(SSH_FXP_READ, &body).await?;
expect_data(cmd, &data)
// SSH_FXP_DATA → returns the bytes
// SSH_FXP_STATUS with SSH_FX_EOF → Error::Eof (used to signal end-of-file)
// SSH_FXP_STATUS with other codes → appropriate Error variant
```