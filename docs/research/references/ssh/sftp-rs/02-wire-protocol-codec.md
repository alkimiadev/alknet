# sftp-rs: Wire Protocol Codec (`protocol.rs`)

The `protocol` module is the heart of the crate — a pure, I/O-free codec that encodes and decodes SFTP wire messages. Both the synchronous and asynchronous clients delegate all serialization and parsing to these functions.

## Packet Framing

Every SFTP message on the wire has this layout:

```
┌────────────┬──────────┬──────────────────┐
│  length    │   type   │      data         │
│  (4 bytes  │  (1 byte │  (length-1 bytes) │
│   BE u32)  │          │                  │
└────────────┴──────────┴──────────────────┘
```

For all numbered requests/responses (everything after INIT/VERSION), the `data` field begins with a 4-byte big-endian `request-id`:

```
data = [request_id (4 bytes BE u32)] [payload]
```

### Raw Packet I/O

```rust
// Sync I/O (used by SftpClient)
pub fn read_raw_packet<C: Read>(channel: &mut C) -> std::io::Result<(u8, Vec<u8>)>
pub fn write_raw_packet<C: Write>(channel: &mut C, kind: u8, buf: &[u8]) -> std::io::Result<()>

// Async I/O (used by AsyncSftpClient)
async fn read_packet_async<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<(u8, Vec<u8>)>
async fn write_packet_async<W: AsyncWrite + Unpin>(w: &mut W, kind: u8, body: &[u8]) -> std::io::Result<()>
```

Both return/accept `(kind: u8, body: Vec<u8>)` where `kind` is the SFTP message type byte and `body` is everything after it (including the request-id for numbered messages).

### Request-ID Helpers

```rust
// Prepend a 4-byte request-id to a request body
pub fn with_request_id(request_id: u32, body: &[u8]) -> Vec<u8>

// Strip the 4-byte request-id prefix from a response body
pub fn split_request_id(buf: &[u8]) -> std::io::Result<(u32, &[u8])>
```

## Message Type Constants

### Client → Server (Requests)

| Constant | Value | Description |
|----------|-------|-------------|
| `SSH_FXP_INIT` | 1 | Protocol version negotiation (unnumbered) |
| `SSH_FXP_OPEN` | 3 | Open a file |
| `SSH_FXP_CLOSE` | 4 | Close a handle |
| `SSH_FXP_READ` | 5 | Read from a file |
| `SSH_FXP_WRITE` | 6 | Write to a file |
| `SSH_FXP_LSTAT` | 7 | Get file attributes (don't follow symlinks) |
| `SSH_FXP_FSTAT` | 8 | Get file attributes by handle |
| `SSH_FXP_SETSTAT` | 9 | Set file attributes by path |
| `SSH_FXP_FSETSTAT` | 10 | Set file attributes by handle |
| `SSH_FXP_OPENDIR` | 11 | Open a directory for listing |
| `SSH_FXP_READDIR` | 12 | Read directory entries |
| `SSH_FXP_REMOVE` | 13 | Remove a file |
| `SSH_FXP_MKDIR` | 14 | Create a directory |
| `SSH_FXP_RMDIR` | 15 | Remove a directory |
| `SSH_FXP_REALPATH` | 16 | Canonicalize a path |
| `SSH_FXP_STAT` | 17 | Get file attributes (follow symlinks) |
| `SSH_FXP_RENAME` | 18 | Rename a file/directory |
| `SSH_FXP_READLINK` | 19 | Read the target of a symlink |
| `SSH_FXP_SYMLINK` | 20 | Create a symbolic link |
| `SSH_FXP_LINK` | 21 | Create a hard link |
| `SSH_FXP_BLOCK` | 22 | Byte-range lock |
| `SSH_FXP_UNBLOCK` | 23 | Byte-range unlock |
| `SSH_FXP_EXTENDED` | 200 | Vendor-specific extension request |

### Server → Client (Responses)

| Constant | Value | Description |
|----------|-------|-------------|
| `SSH_FXP_VERSION` | 2 | Version reply (unnumbered) |
| `SSH_FXP_STATUS` | 101 | Status response (success or error) |
| `SSH_FXP_HANDLE` | 102 | Returns a file/directory handle |
| `SSH_FXP_DATA` | 103 | Returns file data |
| `SSH_FXP_NAME` | 104 | Returns filename entries |
| `SSH_FXP_ATTRS` | 105 | Returns file attributes |
| `SSH_FXP_EXTENDED_REPLY` | 201 | Extension response data |

## Status Codes

| Constant | Value | Error Variant |
|----------|-------|---------------|
| `SSH_FX_OK` | 0 | `Ok(())` |
| `SSH_FX_EOF` | 1 | `Eof` |
| `SSH_FX_NO_SUCH_FILE` | 2 | `NoSuchFile` |
| `SSH_FX_PERMISSION_DENIED` | 3 | `PermissionDenied` |
| `SSH_FX_FAILURE` | 4 | `Failure` |
| `SSH_FX_BAD_MESSAGE` | 5 | `BadMessage` |
| `SSH_FX_NO_CONNECTION` | 6 | `NoConnection` |
| `SSH_FX_CONNECTION_LOST` | 7 | `ConnectionLost` |
| `SSH_FX_OP_UNSUPPORTED` | 8 | `OpUnsupported` |
| `SSH_FX_INVALID_HANDLE` | 9 | `InvalidHandle` |
| `SSH_FX_NO_SUCH_PATH` | 10 | `NoSuchPath` |
| `SSH_FX_FILE_ALREADY_EXISTS` | 11 | `FileAlreadyExists` |
| `SSH_FX_WRITE_PROTECT` | 12 | `WriteProtect` |
| `SSH_FX_NO_MEDIA` | 13 | `NoMedia` |
| `SSH_FX_NO_SPACE_ON_FILESYSTEM` | 14 | `NoSpaceOnFilesystem` |
| `SSH_FX_QUOTA_EXCEEDED` | 15 | `QuotaExceeded` |
| `SSH_FX_UNKNOWN_PRINCIPAL` | 16 | `UnknownPrincipal` |
| `SSH_FX_LOCK_CONFLICT` | 17 | `LockConflict` |
| `SSH_FX_DIR_NOT_EMPTY` | 18 | `DirNotEmpty` |
| `SSH_FX_NOT_A_DIRECTORY` | 19 | `NotADirectory` |
| `SSH_FX_INVALID_FILENAME` | 20 | `InvalidFilename` |
| `SSH_FX_LINK_LOOP` | 21 | `LinkLoop` |
| `SSH_FX_CANNOT_DELETE` | 22 | `CannotDelete` |
| `SSH_FX_INVALID_PARAMETER` | 23 | `InvalidParameter` |
| `SSH_FX_FILE_IS_A_DIRECTORY` | 24 | `FileIsADirectory` |
| `SSH_FX_BYTE_RANGE_LOCK_CONFLICT` | 25 | `ByteRangeLockConflict` |
| `SSH_FX_BYTE_RANGE_LOCK_REFUSED` | 26 | `ByteRangeLockRefused` |
| `SSH_FX_DELETE_PENDING` | 27 | `DeletePending` |
| `SSH_FX_FILE_CORRUPT` | 28 | `FileCorrupt` |
| `SSH_FX_OWNER_INVALID` | 29 | `OwnerInvalid` |
| `SSH_FX_GROUP_INVALID` | 30 | `GroupInvalid` |
| `SSH_FX_NO_MATCHING_BYTE_RANGE_LOCK` | 31 | `NoMatchingByteRangeLock` |

Any unrecognized status code maps to `Error::Other(status, message, lang_tag)`.

## Request Body Builders

Each builder produces the `data` portion (without type byte or request-id) for a specific SFTP request:

### Handshake

```rust
// INIT: version 3 (no request-id)
pub fn build_init() -> Vec<u8>

// VERSION: parse server response
pub fn parse_version(body: &[u8]) -> std::io::Result<(u32, Vec<(String, String)>)>
```

### Single-Field Bodies

```rust
// Path-only requests: LSTAT, STAT, OPENDIR, REMOVE, MKDIR, RMDIR, READLINK
pub fn build_path_only(path: &str) -> Vec<u8>

// Handle-only requests: CLOSE, READDIR
pub fn build_handle_only(handle: &[u8]) -> Vec<u8>
```

### Composite Bodies

```rust
// OPEN: path + flags + attributes
pub fn build_open(path: &str, options: u32, attr: &Attributes) -> std::io::Result<Vec<u8>>

// READ: handle + offset + length
pub fn build_pread(handle: &[u8], offset: u64, length: u32) -> Vec<u8>

// WRITE: handle + offset + data
pub fn build_pwrite(handle: &[u8], offset: u64, data: &[u8]) -> Vec<u8>

// RENAME: oldpath + newpath + flags
pub fn build_rename(oldpath: &str, newpath: &str, flags: Option<u32>) -> Vec<u8>
// Default flags: OVERWRITE | ATOMIC | NATIVE = 0x07

// SYMLINK: path + target
pub fn build_two_paths(a: &str, b: &str) -> Vec<u8>

// LINK: path + target + symlink_flag
pub fn build_link(path: &str, target: &str, symlink: bool) -> Vec<u8>

// Path + attributes: SETSTAT, MKDIR
pub fn build_path_and_attrs(path: &str, attr: &Attributes) -> std::io::Result<Vec<u8>>

// Handle + attributes: FSETSTAT
pub fn build_handle_and_attrs(handle: &[u8], attr: &Attributes) -> std::io::Result<Vec<u8>>

// Path + flags: STAT, LSTAT
pub fn build_path_and_flags(path: &str, flags: u32) -> Vec<u8>

// Handle + flags: FSTAT
pub fn build_handle_and_flags(handle: &[u8], flags: u32) -> Vec<u8>

// REALPATH: path + optional control byte + optional compose path
pub fn build_realpath(path: &str, control_byte: Option<u8>, compose: Option<&str>) -> Vec<u8>

// BLOCK: handle + offset + length + lockmask
pub fn build_block(handle: &[u8], offset: u64, length: u64, lockmask: u32) -> Vec<u8>

// UNBLOCK: handle + offset + length
pub fn build_unblock(handle: &[u8], offset: u64, length: u64) -> Vec<u8>

// EXTENDED: request name + data
pub fn build_extended(request: &str, data: &[u8]) -> Vec<u8>
```

### Wire Encoding Helpers

```rust
// Write a length-prefixed UTF-8 string (4-byte BE length + bytes)
fn put_str(buf: &mut Vec<u8>, s: &str)

// Write a length-prefixed byte string (4-byte BE length + bytes)
fn put_bytes(buf: &mut Vec<u8>, b: &[u8])

// Read a length-prefixed UTF-8 string from a cursor
fn read_string(reader: &mut Cursor<&[u8]>, what: &str) -> std::io::Result<String>
```

## Response Parsers

Each parser takes the raw `data` portion (after stripping type byte and request-id) and returns a `Result`:

```rust
// Parse SSH_FXP_STATUS body → Ok(()) or typed Error
pub fn parse_status(respdata: &[u8]) -> Result<()>

// Parse SSH_FXP_HANDLE body → raw handle bytes
pub fn parse_handle(respdata: &[u8]) -> Result<Vec<u8>>

// Parse SSH_FXP_DATA body → raw data bytes
pub fn parse_data(respdata: &[u8]) -> Result<Vec<u8>>

// Parse SSH_FXP_ATTRS body → Attributes
pub fn parse_attrs(respdata: &[u8]) -> Result<Attributes>

// Parse SSH_FXP_NAME body (for READLINK, REALPATH) → (name, attrs) pairs
pub fn parse_name(respdata: &[u8]) -> Result<Vec<(String, Attributes)>>

// Parse SSH_FXP_NAME body (for READDIR) → (name, longname, attrs) triples
pub fn parse_readdir(respdata: &[u8]) -> Result<Vec<(String, String, Attributes)>>
```

## Response Expectation Functions

These are the primary entry points used by the client implementations. They take `(cmd, data)` — the type byte and payload from the server — and dispatch to the correct parser, or convert an unexpected `SSH_FXP_STATUS` into the appropriate typed error:

```rust
// Expect SSH_FXP_STATUS (for operations that return nothing on success)
pub fn expect_status(cmd: u8, data: &[u8]) -> Result<()>

// Expect SSH_FXP_HANDLE (for OPEN, OPENDIR)
pub fn expect_handle(cmd: u8, data: &[u8]) -> Result<Vec<u8>>

// Expect SSH_FXP_ATTRS (for STAT, LSTAT, FSTAT)
pub fn expect_attrs(cmd: u8, data: &[u8]) -> Result<Attributes>

// Expect SSH_FXP_DATA (for READ)
pub fn expect_data(cmd: u8, data: &[u8]) -> Result<Vec<u8>>

// Expect SSH_FXP_NAME with name+attrs (for READLINK, REALPATH)
pub fn expect_name(cmd: u8, data: &[u8]) -> Result<Vec<(String, Attributes)>>

// Expect SSH_FXP_NAME with name+longname+attrs (for READDIR)
pub fn expect_readdir(cmd: u8, data: &[u8]) -> Result<Vec<(String, String, Attributes)>>

// Expect SSH_FXP_EXTENDED_REPLY or SSH_FXP_STATUS
pub fn expect_extended(cmd: u8, data: Vec<u8>) -> Result<Option<Vec<u8>>>
```

If the server returns a different message type than expected, these functions produce `Error::Io("Unexpected response: ...")`. If the server returns `SSH_FXP_STATUS` where a data-bearing response was expected (even `SSH_FX_OK`), it is treated as a protocol violation and converted to the appropriate typed error.

## String Encoding

All strings in SFTP are length-prefixed with a 4-byte big-endian length followed by raw UTF-8 bytes:

```
┌───────────────┬──────────────────┐
│  length (4B)  │  UTF-8 bytes     │
│  BE u32       │                  │
└───────────────┴──────────────────┘
```

Byte arrays (handles, ACLs) use the same length-prefix format but are not required to be valid UTF-8.