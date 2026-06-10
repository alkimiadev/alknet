# sftp-rs: Key Types

## `Error` Enum

The universal error type for all SFTP operations, covering both I/O failures and SFTP status codes:

```rust
#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Utf8(std::str::Utf8Error),
    Other(u32, String, String),          // (status_code, message, lang_tag)
    Eof(String, String),                // End of file/directory
    NoSuchFile(String, String),
    PermissionDenied(String, String),
    Failure(String, String),
    BadMessage(String, String),
    NoConnection(String, String),
    ConnectionLost(String, String),
    OpUnsupported(String, String),
    InvalidHandle(String, String),
    NoSuchPath(String, String),
    FileAlreadyExists(String, String),
    WriteProtect(String, String),
    NoMedia(String, String),
    NoSpaceOnFilesystem(String, String),
    QuotaExceeded(String, String),
    UnknownPrincipal(String, String),
    LockConflict(String, String),
    DirNotEmpty(String, String),
    NotADirectory(String, String),
    InvalidFilename(String, String),
    LinkLoop(String, String),
    CannotDelete(String, String),
    InvalidParameter(String, String),
    FileIsADirectory(String, String),
    ByteRangeLockConflict(String, String),
    ByteRangeLockRefused(String, String),
    DeletePending(String, String),
    FileCorrupt(String, String),
    OwnerInvalid(String, String),
    GroupInvalid(String, String),
    NoMatchingByteRangeLock(String, String),
}
```

The `String` pairs in each variant are `(error_message, language_tag)` as returned by the server. The `Error` type implements `From<std::io::Error>` and `From<std::str::Utf8Error>`, and there is a `From<Error> for std::io::Error` conversion that maps SFTP error codes to appropriate `std::io::ErrorKind` values:

| Error Variant | io::ErrorKind |
|---------------|---------------|
| `Eof` | `UnexpectedEof` |
| `NoSuchFile` | `NotFound` |
| `NoSuchPath` | `NotFound` |
| `PermissionDenied` | `PermissionDenied` |
| `WriteProtect` | `PermissionDenied` |
| `QuotaExceeded` | `PermissionDenied` |
| `LockConflict` | `PermissionDenied` |
| `NoConnection` | `NotConnected` |
| `ConnectionLost` | `ConnectionReset` |
| `InvalidHandle` | `InvalidInput` |
| `FileAlreadyExists` | `AlreadyExists` |
| `InvalidFilename` | `InvalidInput` |
| All others | formatted via `Error::other()` |

```rust
pub type Result<R> = std::result::Result<R, Error>;
```

## `Attributes`

Represents SFTP file attributes — a flag-driven, extensible structure where only fields present in the `valid_attribute_flags` mask are serialized:

```rust
#[derive(Debug, PartialEq, Eq, Clone, Default)]
pub struct Attributes {
    pub size: Option<u64>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub allocation_size: Option<u64>,
    pub owner: Option<String>,
    pub group: Option<String>,
    pub permissions: Option<u32>,
    pub access_time: Option<(u64, Option<u32>)>,  // (seconds, nanoseconds)
    pub create_time: Option<(u64, Option<u32>)>,
    pub modify_time: Option<(u64, Option<u32>)>,
    pub ctime: Option<(u64, Option<u32>)>,
    pub acl: Option<Vec<u8>>,
    pub attrib_bits: Option<u32>,
    pub attrib_bits_valid: Option<u32>,
    pub text_hint: Option<TextHint>,
    pub mime_type: Option<String>,
    pub link_count: Option<u32>,
    pub untranslated_name: Option<Vec<u8>>,
    pub extended: Option<Vec<(String, String)>>,
}
```

### Serialization

The `serialize()` method builds the wire format by writing a 4-byte `valid_attribute_flags` placeholder, then appending fields conditionally, then back-patching the flags:

```
┌──────────────────────────┐
│  valid_attribute_flags   │  4 bytes, BE u32
│  (back-patched at end)   │
├──────────────────────────┤
│  size (if flag set)      │  8 bytes, BE u64
│  uid (if flag set)       │  4 bytes, BE u32
│  gid (if flag set)       │  4 bytes, BE u32
│  allocation_size         │  8 bytes, BE u64
│  owner (length-prefixed) │
│  group (length-prefixed) │
│  permissions             │  4 bytes, BE u32
│  access_time             │  8 bytes + opt 4 bytes ns
│  create_time             │  8 bytes + opt 4 bytes ns
│  modify_time             │  8 bytes + opt 4 bytes ns
│  ctime                   │  8 bytes + opt 4 bytes ns
│  acl (length-prefixed)   │
│  attrib_bits             │  4 bytes
│  attrib_bits_valid       │  4 bytes
│  text_hint               │  1 byte
│  mime_type (len-prefixed)│
│  link_count              │  4 bytes
│  untranslated_name       │  length-prefixed bytes
│  extended                │  count + key/value pairs
└──────────────────────────┘
```

Constraints enforced by serialization:
- `uid` and `gid` must both be present or both absent (same `SSH_FILEXFER_ATTR_UIDGID` flag)
- `owner` and `group` share the `SSH_FILEXFER_ATTR_OWNERGROUP` flag
- `attrib_bits` and `attrib_bits_valid` share the `SSH_FILEXFER_ATTR_BITS` flag
- `SSH_FILEXFER_ATTR_SUBSECOND_TIMES` is a shared flag — if set, all time fields include a 4-byte nanosecond component; if not, none do

### Deserialization

`deserialize(reader: &mut Cursor<&[u8]>)` reads the flags first, then conditionally reads each field based on flag bits. Subsecond nanoseconds are read for all time fields when `SSH_FILEXFER_ATTR_SUBSECOND_TIMES` is set.

## Attribute Flag Constants

| Constant | Value | Field(s) |
|----------|-------|----------|
| `SSH_FILEXFER_ATTR_SIZE` | 0x00000001 | `size` |
| `SSH_FILEXFER_ATTR_UIDGID` | 0x00000002 | `uid`, `gid` |
| `SSH_FILEXFER_ATTR_PERMISSIONS` | 0x00000004 | `permissions` |
| `SSH_FILEXFER_ATTR_ACCESSTIME` | 0x00000008 | `access_time` |
| `SSH_FILEXFER_ATTR_CREATETIME` | 0x00000010 | `create_time` |
| `SSH_FILEXFER_ATTR_MODIFYTIME` | 0x00000020 | `modify_time` |
| `SSH_FILEXFER_ATTR_ACL` | 0x00000040 | `acl` |
| `SSH_FILEXFER_ATTR_OWNERGROUP` | 0x00000080 | `owner`, `group` |
| `SSH_FILEXFER_ATTR_SUBSECOND_TIMES` | 0x00000100 | nanoseconds for all times |
| `SSH_FILEXFER_ATTR_BITS` | 0x00000200 | `attrib_bits`, `attrib_bits_valid` |
| `SSH_FILEXFER_ATTR_ALLOCATION_SIZE` | 0x00000400 | `allocation_size` |
| `SSH_FILEXFER_ATTR_TEXT_HINT` | 0x00000800 | `text_hint` |
| `SSH_FILEXFER_ATTR_MIME_TYPE` | 0x00001000 | `mime_type` |
| `SSH_FILEXFER_ATTR_LINK_COUNT` | 0x00002000 | `link_count` |
| `SSH_FILEXFER_ATTR_UNTRANSLATED_NAME` | 0x00004000 | `untranslated_name` |
| `SSH_FILEXFER_ATTR_CTIME` | 0x00008000 | `ctime` |
| `SSH_FILEXFER_ATTR_EXTENDED` | 0x80000000 | `extended` |

## `Kind` — File Type

Represents the type of a filesystem entry, encoded as a `u8` in the attributes:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    Regular,       // 1
    Directory,     // 2
    Symlink,       // 3
    Special,       // 4
    #[default]
    Unknown,       // 5
    Socket,        // 6
    CharDevice,    // 7
    BlockDevice,   // 8
    Fifo,          // 9
}
```

Implements `From<Kind> for u8` and `From<u8> for Kind` (panics on unknown values).

## `TextHint`

Indicates whether a file is text or binary, and whether that classification is known or guessed:

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TextHint {
    KnownText,     // 0x00
    GuessedText,   // 0x01
    KnownBinary,   // 0x02
    GuessedBinary, // 0x03
}
```

## `OpenOptions`

Builder-style type for controlling file open behavior:

```rust
#[derive(Debug, PartialEq, Eq, Clone, Copy, Default)]
pub struct OpenOptions(u32);

impl OpenOptions {
    pub fn new() -> OpenOptions;
    pub fn read(mut self, read: bool) -> OpenOptions;
    pub fn write(mut self, write: bool) -> OpenOptions;
    pub fn append(mut self, append: bool) -> OpenOptions;
    pub fn create(mut self, create: bool) -> OpenOptions;
    pub fn truncate(mut self, truncate: bool) -> OpenOptions;
    pub fn excl(mut self, excl: bool) -> OpenOptions;
    pub fn mode(&mut self, mode: u32) -> &mut OpenOptions;
    pub fn get(&self) -> u32;
}
```

### Open Flag Constants

| Constant | Value | Description |
|----------|-------|-------------|
| `SFTP_FLAG_READ` | 0x01 | Read access |
| `SFTP_FLAG_WRITE` | 0x02 | Write access |
| `SFTP_FLAG_APPEND` | 0x04 | Append data |
| `SFTP_FLAG_CREAT` | 0x08 | Create if not exists |
| `SFTP_FLAG_TRUNC` | 0x10 | Truncate to zero length |
| `SFTP_FLAG_EXCL` | 0x20 | Fail if file exists (exclusive create) |

Usage example:
```rust
let opts = OpenOptions::new()
    .read(true)
    .write(true)
    .create(true)
    .truncate(true);
// opts.get() == 0x1B (READ | WRITE | CREAT | TRUNC)
```

## `File` and `Directory` — Handle Wrappers

Opaque wrappers around the raw handle bytes returned by the server:

```rust
#[derive(Debug, Clone)]
pub struct File(pub Vec<u8>);

#[derive(Debug, Clone)]
pub struct Directory(pub Vec<u8>);
```

These are newtype wrappers that distinguish file handles from directory handles at the type level. The inner `Vec<u8>` is the server-assigned handle value (obtained from `SSH_FXP_HANDLE` responses), used in subsequent operations like `pread`, `pwrite`, `fclose`, `readdir`, `closedir`, etc.

## Rename Flags

Used with `SSH_FXP_RENAME`:

| Constant | Value | Description |
|----------|-------|-------------|
| `SSH_FXF_RENAME_OVERWRITE` | 0x01 | Overwrite existing target |
| `SSH_FXF_RENAME_ATOMIC` | 0x02 | Atomic rename |
| `SSH_FXF_RENAME_NATIVE` | 0x04 | Use native OS rename semantics |

Default for `build_rename()` when `flags` is `None`: `OVERWRITE | ATOMIC | NATIVE` = 0x07.

## Attribute Bits Flags

Used in `Attributes::attrib_bits`:

| Constant | Value |
|----------|-------|
| `SSH_FILEXFER_ATTR_FLAGS_READONLY` | 0x00000001 |
| `SSH_FILEXFER_ATTR_FLAGS_SYSTEM` | 0x00000002 |
| `SSH_FILEXFER_ATTR_FLAGS_HIDDEN` | 0x00000004 |
| `SSH_FILEXFER_ATTR_FLAGS_CASE_INSENSITIVE` | 0x00000008 |
| `SSH_FILEXFER_ATTR_FLAGS_ARCHIVE` | 0x00000010 |
| `SSH_FILEXFER_ATTR_FLAGS_ENCRYPTED` | 0x00000020 |
| `SSH_FILEXFER_ATTR_FLAGS_COMPRESSED` | 0x00000040 |
| `SSH_FILEXFER_ATTR_FLAGS_SPARSE` | 0x00000080 |
| `SSH_FILEXFER_ATTR_FLAGS_APPEND_ONLY` | 0x00000100 |
| `SSH_FILEXFER_ATTR_FLAGS_IMMUTABLE` | 0x00000200 |
| `SSH_FILEXFER_ATTR_FLAGS_SYNC` | 0x00000400 |
| `SSH_FILEXFER_ATTR_FLAGS_TRANSLATION_ERR` | 0x00000800 |

## ACE/MISC Open Flags (v5+ extensions)

These are defined for completeness but the crate targets v3:

| Constant | Value |
|----------|-------|
| `SSH_FXF_ACCESS_DISPOSITION` | 0x00000007 |
| `SSH_FXF_CREATE_NEW` | 0x00000000 |
| `SSH_FXF_CREATE_TRUNCATE` | 0x00000001 |
| `SSH_FXF_OPEN_EXISTING` | 0x00000002 |
| `SSH_FXF_OPEN_OR_CREATE` | 0x00000003 |
| `SSH_FXF_TRUNCATE_EXISTING` | 0x00000004 |
| `SSH_FXF_APPEND_DATA` | 0x00000008 |
| `SSH_FXF_APPEND_DATA_ATOMIC` | 0x00000010 |
| `SSH_FXF_TEXT_MODE` | 0x00000020 |
| `SSH_FXF_BLOCK_READ` | 0x00000040 |
| `SSH_FXF_BLOCK_WRITE` | 0x00000080 |
| `SSH_FXF_BLOCK_DELETE` | 0x00000100 |
| `SSH_FXF_BLOCK_ADVISORY` | 0x00000200 |
| `SSH_FXF_NOFOLLOW` | 0x00000400 |
| `SSH_FXF_DELETE_ON_CLOSE` | 0x00000800 |
| `SSH_FXF_ACCESS_AUDIT_ALARM_INFO` | 0x00001000 |
| `SSH_FXF_ACCESS_BACKUP` | 0x00002000 |
| `SSH_FXF_BACKUP_STREAM` | 0x00004000 |
| `SSH_FXF_OVERRIDE_OWNER` | 0x00008000 |