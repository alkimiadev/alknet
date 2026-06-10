# russh-sftp: Key Types

## Protocol Types (`protocol/`)

### Init and Version (Handshake)

```rust
// SSH_FXP_INIT — sent by client to begin
pub struct Init {
    pub version: u32,                    // Always 3
    pub extensions: HashMap<String, String>,
}

// SSH_FXP_VERSION — server response
pub struct Version {
    pub version: u32,
    pub extensions: HashMap<String, String>,
}
```

Both implement `Default` with `version: 3` and empty extensions. Extensions are negotiated during the handshake (e.g., `"limits@openssh.com" → "1"`).

### Open and OpenFlags

```rust
// SSH_FXP_OPEN — open or create a file
pub struct Open {
    pub id: u32,
    pub filename: String,
    pub pflags: OpenFlags,         // Bitflags for access mode
    pub attrs: FileAttributes,     // Initial attributes for new files
}

bitflags! {
    pub struct OpenFlags: u32 {
        const READ     = 0x00000001;
        const WRITE    = 0x00000002;
        const APPEND   = 0x00000004;
        const CREATE   = 0x00000008;
        const TRUNCATE = 0x00000010;
        const EXCLUDE  = 0x00000020;
    }
}
```

`OpenFlags` implements `From<OpenFlags> for std::fs::OpenOptions`, converting SFTP flags into Rust's `OpenOptions` for server implementations. Notable behavior: if both `CREATE` and `EXCLUDE` are set, it maps to `create_new(true)`; otherwise `CREATE` maps to `create(true)`.

### Handle

```rust
// SSH_FXP_HANDLE — server returns a handle string for open files/dirs
pub struct Handle {
    pub id: u32,
    pub handle: String,
}
```

Handles are opaque strings identifying open file or directory references. They are returned by `SSH_FXP_OPEN` and `SSH_FXP_OPENDIR` responses, and used in subsequent `READ`, `WRITE`, `FSTAT`, `FSETSTAT`, `READDIR`, `FSYNC`, and `CLOSE` operations.

### Data and Write

```rust
// SSH_FXP_DATA — file data response
pub struct Data {
    pub id: u32,
    pub data: Vec<u8>,  // serde_bytes — no length prefix in inner serialization
}

// SSH_FXP_WRITE — write data to file
pub struct Write {
    pub id: u32,
    pub handle: String,
    pub offset: u64,
    pub data: Vec<u8>,  // serde_bytes
}
```

Both use `serde_bytes` for the `data` field, which serializes as a length-prefixed byte array in the outer packet encoding.

### Name and File

```rust
// SSH_FXP_NAME — directory listing / path resolution response
pub struct Name {
    pub id: u32,
    pub files: Vec<File>,
}

// Represents a single file entry
pub struct File {
    pub filename: String,
    pub longname: String,      // `ls -l` style long name
    pub attrs: FileAttributes,
}
```

`File` provides constructors:
- `File::dummy(filename)` — Creates a file with empty longname and default attributes (for `realpath` responses per spec)
- `File::new(filename, attrs)` — Creates a file with auto-generated longname from attributes

The `longname()` method generates an `ls -l`-style string: `"{type}{permissions} 0 {user} {group} {size} {mtime} {filename}"`

### Attrs

```rust
// SSH_FXP_ATTRS — file attributes response
pub struct Attrs {
    pub id: u32,
    pub attrs: FileAttributes,
}
```

### Status and StatusCode

```rust
pub enum StatusCode {
    Ok              = 0,   // Successful completion
    Eof             = 1,   // End of file / no more directory entries
    NoSuchFile      = 2,   // File does not exist
    PermissionDenied = 3,  // Permission denied
    Failure         = 4,   // Generic failure
    BadMessage      = 5,   // Badly formatted packet
    NoConnection    = 6,   // Client-side only: no connection
    ConnectionLost  = 7,   // Client-side only: connection lost
    OpUnsupported   = 8,   // Operation not supported
}

pub struct Status {
    pub id: u32,
    pub status_code: StatusCode,
    pub error_message: String,
    pub language_tag: String,     // e.g., "en-US"
}
```

`StatusCode` derives `Error` (thiserror), providing human-readable `Display` output for each variant.

### FileAttributes

The core metadata type. See the wire codec doc for serialization details.

```rust
pub struct FileAttributes {
    pub size: Option<u64>,
    pub uid: Option<u32>,
    pub user: Option<String>,       // User name for longname display
    pub gid: Option<u32>,
    pub group: Option<String>,      // Group name for longname display
    pub permissions: Option<u32>,   // Unix permission + file type bits
    pub atime: Option<u32>,         // Access time (unix epoch)
    pub mtime: Option<u32>,         // Modification time (unix epoch)
}
```

Key methods:
- `is_dir()`, `is_regular()`, `is_symlink()`, `is_character()`, `is_block()`, `is_fifo()` — check `FileMode` bits
- `set_dir()`, `set_regular()`, etc. — set `FileMode` bits
- `file_type()` → `FileType` — simplified type classification
- `len()` → `u64` — file size (defaults to 0)
- `permissions()` → `FilePermissions` — simplified permission struct
- `accessed()` → `io::Result<SystemTime>` — convert atime
- `modified()` → `io::Result<SystemTime>` — convert mtime
- `empty()` — all fields `None`
- `From<&std::fs::Metadata>` — convert OS metadata (unix-specific for uid/gid/mode)

#### Supporting Bitflag Types

```rust
bitflags! {
    pub struct FileAttr: u32 {
        const SIZE        = 0x00000001;
        const UIDGID      = 0x00000002;
        const PERMISSIONS = 0x00000004;
        const ACMODTIME   = 0x00000008;
        const EXTENDED    = 0x80000000;
    }

    pub struct FileMode: u32 {
        const FIFO = 0x1000;  // Named pipe
        const CHR  = 0x2000;  // Character device
        const DIR  = 0x4000;  // Directory
        const NAM  = 0x5000;  // Named file (rare)
        const BLK  = 0x6000;  // Block device
        const REG  = 0x8000;  // Regular file
        const LNK  = 0xA000;  // Symbolic link
        const SOCK = 0xC000;  // Socket
    }

    pub struct FilePermissionFlags: u32 {
        const OTHER_READ  = 0o4;
        const OTHER_WRITE = 0o2;
        const OTHER_EXEC  = 0o1;
        const GROUP_READ  = 0o40;
        const GROUP_WRITE = 0o20;
        const GROUP_EXEC  = 0o10;
        const OWNER_READ  = 0o400;
        const OWNER_WRITE = 0o200;
        const OWNER_EXEC  = 0o100;
    }
}

pub enum FileType { Dir, File, Symlink, Other }

pub struct FilePermissions {
    pub other_exec: bool, pub other_read: bool, pub other_write: bool,
    pub group_exec: bool, pub group_read: bool, pub group_write: bool,
    pub owner_exec: bool, pub owner_read: bool, pub owner_write: bool,
}
```

### Extended and ExtendedReply

```rust
pub struct Extended {
    pub id: u32,
    pub request: String,        // Extension name, e.g., "limits@openssh.com"
    pub data: Vec<u8>,          // serde_bytes, no inner length prefix
}

pub struct ExtendedReply {
    pub id: u32,
    pub data: Vec<u8>,          // serde_bytes
}
```

## Other Protocol Packets

All follow the same pattern of `id: u32` plus operation-specific fields:

| Packet | Fields |
|--------|--------|
| `Close` | `id`, `handle` |
| `Read` | `id`, `handle`, `offset: u64`, `len: u32` |
| `Lstat` | `id`, `path` |
| `Fstat` | `id`, `handle` |
| `SetStat` | `id`, `path`, `attrs` |
| `FSetStat` | `id`, `handle`, `attrs` |
| `OpenDir` | `id`, `path` |
| `ReadDir` | `id`, `handle` |
| `Remove` | `id`, `filename` |
| `MkDir` | `id`, `path`, `attrs` |
| `RmDir` | `id`, `path` |
| `RealPath` | `id`, `path` |
| `Stat` | `id`, `path` |
| `Rename` | `id`, `oldpath`, `newpath` |
| `ReadLink` | `id`, `path` |
| `Symlink` | `id`, `linkpath`, `targetpath` |

## Extension Types (`extensions.rs`)

OpenSSH extension constants and structures:

```rust
pub const LIMITS: &str = "limits@openssh.com";
pub const HARDLINK: &str = "hardlink@openssh.com";
pub const FSYNC: &str = "fsync@openssh.com";
pub const STATVFS: &str = "statvfs@openssh.com";

// Server limits advertisement
pub struct LimitsExtension {
    pub max_packet_len: u64,
    pub max_read_len: u64,
    pub max_write_len: u64,
    pub max_open_handles: u64,
}

// Hardlink request data
pub struct HardlinkExtension {
    pub oldpath: String,
    pub newpath: String,
}

// Fsync request data
pub struct FsyncExtension {
    pub handle: String,
}

// Statvfs request data
pub struct StatvfsExtension {
    pub path: String,
}

// Statvfs response
pub struct Statvfs {
    pub block_size: u64,
    pub fragment_size: u64,
    pub blocks: u64,
    pub blocks_free: u64,
    pub blocks_avail: u64,
    pub inodes: u64,
    pub inodes_free: u64,
    pub inodes_avail: u64,
    pub fs_id: u64,
    pub flags: u64,
    pub name_max: u64,
}
```

## Error Types

### Top-level Error (`error.rs`)

```rust
pub enum Error {
    IO(String),                    // I/O errors
    UnexpectedEof,                 // Stream EOF
    BadMessage(String),            // Malformed packet
    Client(String),                // Wraps client::error::Error
    UnexpectedBehavior(String),    // Protocol violations
}
```

### Client Error (`client/error.rs`)

```rust
pub enum Error {
    Status(Status),               // Server returned error status
    IO(String),                    // I/O errors
    Timeout,                       // Request timed out
    Limited(String),               // Limits exceeded
    UnexpectedPacket,              // Wrong packet type received
    UnexpectedBehavior(String),    // Protocol violations
}
```

### StatusReply (Server-side error conversion, `server/reply.rs`)

```rust
pub struct StatusReply {
    pub status_code: StatusCode,
    pub error_message: Option<String>,
    pub language_tag: Option<String>,
}
```

Server `Handler` errors must implement `Into<StatusReply>`, which allows the framework to convert any handler error into a `SSH_FXP_STATUS` response. `StatusCode` has a `.with_message()` helper.

## Config Types

### Client Config (`client/mod.rs`)

```rust
pub struct Config {
    pub max_packet_len: u32,        // Default: 262144 (256 KiB)
    pub max_concurrent_writes: usize, // Default: 8
    pub request_timeout_secs: u64,   // Default: 10
}
```

### Server Config (`server/mod.rs`)

```rust
pub struct Config {
    pub max_client_packet_len: u32,  // Default: 262144 (256 KiB)
}
```