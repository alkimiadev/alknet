# russh-sftp: Wire Protocol and Codec

## SFTP v3 Wire Format

The SFTP protocol (draft-ietf-secsh-filexfer-02) transmits packets over the SSH channel as:

```
┌────────────┬──────────┬─────────────────┐
│  length    │   type   │    payload      │
│  (u32 BE) │  (u8)    │  (variable)     │
│  4 bytes   │  1 byte  │  length-1 bytes│
└────────────┴──────────┴─────────────────┘
```

- `length` includes the type byte but not itself
- All multi-byte integers are **big-endian** (network byte order)
- Strings are encoded as `u32 length + UTF-8 bytes`
- Byte arrays are encoded as `u32 length + raw bytes`

### Packet Type Constants

Defined in `protocol/mod.rs`:

| Constant | Value | Direction | Description |
|----------|-------|-----------|-------------|
| `SSH_FXP_INIT` | 1 | C→S | Client initialization |
| `SSH_FXP_VERSION` | 2 | S→C | Server version response |
| `SSH_FXP_OPEN` | 3 | C→S | Open a file |
| `SSH_FXP_CLOSE` | 4 | C→S | Close a handle |
| `SSH_FXP_READ` | 5 | C→S | Read from a handle |
| `SSH_FXP_WRITE` | 6 | C→S | Write to a handle |
| `SSH_FXP_LSTAT` | 7 | C→S | Stat a path (no follow) |
| `SSH_FXP_FSTAT` | 8 | C→S | Stat an open handle |
| `SSH_FXP_SETSTAT` | 9 | C→S | Set file attributes by path |
| `SSH_FXP_FSETSTAT` | 10 | C→S | Set file attributes by handle |
| `SSH_FXP_OPENDIR` | 11 | C→S | Open a directory |
| `SSH_FXP_READDIR` | 12 | C→S | Read directory entries |
| `SSH_FXP_REMOVE` | 13 | C→S | Remove a file |
| `SSH_FXP_MKDIR` | 14 | C→S | Create a directory |
| `SSH_FXP_RMDIR` | 15 | C→S | Remove a directory |
| `SSH_FXP_REALPATH` | 16 | C→S | Canonicalize a path |
| `SSH_FXP_STAT` | 17 | C→S | Stat a path (follow symlinks) |
| `SSH_FXP_RENAME` | 18 | C→S | Rename a file |
| `SSH_FXP_READLINK` | 19 | C→S | Read a symbolic link |
| `SSH_FXP_SYMLINK` | 20 | C→S | Create a symbolic link |
| `SSH_FXP_STATUS` | 101 | S→C / C→S | Status response |
| `SSH_FXP_HANDLE` | 102 | S→C | Handle response |
| `SSH_FXP_DATA` | 103 | S→C | Data response |
| `SSH_FXP_NAME` | 104 | S→C | Name list response |
| `SSH_FXP_ATTRS` | 105 | S→C | File attributes response |
| `SSH_FXP_EXTENDED` | 200 | C→S | Extended request |
| `SSH_FXP_EXTENDED_REPLY` | 201 | S→C | Extended reply |

## Packet Reading

Wire I/O is handled by `utils::read_packet()`:

```rust
pub(crate) async fn read_packet<S: AsyncRead + Unpin>(
    stream: &mut S,
    max_length: u32,
) -> Result<Bytes, Error> {
    let length = stream.read_u32().await?;
    if length > max_length {
        return Err(Error::BadMessage("packet length limit exceeded".to_owned()));
    }
    let mut buf = vec![0; length as usize];
    stream.read_exact(&mut buf).await?;
    Ok(Bytes::from(buf))
}
```

The read packet buffer **includes the type byte** as the first byte, followed by the payload. This design means the caller can distinguish packet types before full deserialization.

## Packet Enum and Dispatch

All packets are unified into a single `Packet` enum:

```rust
pub enum Packet {
    Init(Init),           Version(Version),     Open(Open),
    Close(Close),         Read(Read),           Write(Write),
    Lstat(Lstat),         Fstat(Fstat),         SetStat(SetStat),
    FSetStat(FSetStat),   OpenDir(OpenDir),     ReadDir(ReadDir),
    Remove(Remove),       MkDir(MkDir),         RmDir(RmDir),
    RealPath(RealPath),   Stat(Stat),           Rename(Rename),
    ReadLink(ReadLink),   Symlink(Symlink),     Status(Status),
    Handle(Handle),       Data(Data),           Name(Name),
    Attrs(Attrs),         Extended(Extended),    ExtendedReply(ExtendedReply),
}
```

### Deserialization (`TryFrom<&mut Bytes> for Packet`)

Reads the type byte first, then delegates to the custom serde deserializer:

```rust
fn try_from(bytes: &mut Bytes) -> Result<Self, Self::Error> {
    let r#type = bytes.try_get_u8()?;
    match r#type {
        SSH_FXP_INIT => Self::Init(de::from_bytes(bytes)?),
        SSH_FXP_OPEN => Self::Open(de::from_bytes(bytes)?),
        // ... all 26 variants
        _ => Err(Error::BadMessage("unknown type".to_owned())),
    }
}
```

### Serialization (`TryFrom<Packet> for Bytes`)

Converts each variant to bytes via `ser::to_bytes()`, prepends type byte, and wraps with the 4-byte length:

```rust
fn try_from(packet: Packet) -> Result<Self, Self::Error> {
    let (r#type, payload): (u8, Bytes) = match packet {
        Packet::Init(init) => (SSH_FXP_INIT, ser::to_bytes(&init)?),
        Packet::Open(open) => (SSH_FXP_OPEN, ser::to_bytes(&open)?),
        // ... all variants
    };
    let length = payload.len() as u32 + 1;
    let mut bytes = BytesMut::new();
    bytes.put_u32(length);
    bytes.put_u8(r#type);
    bytes.put_slice(&payload);
    Ok(bytes.freeze())
}
```

## Custom Serde Wire Codec

The crate implements a **custom serde `Serializer` and `Deserializer`** that directly maps Rust types to the SFTP binary format. This is NOT JSON, Bincode, or any standard serde format — it is a bespoke binary encoding matching the SFTP v3 wire specification.

### Serializer (`ser.rs`)

The `Serializer` writes directly into a `BytesMut` buffer:

| Rust Type | Wire Encoding |
|-----------|---------------|
| `u8` | 1 byte raw |
| `u32` | 4 bytes big-endian |
| `u64` | 8 bytes big-endian |
| `str` / `String` | `u32 length` + UTF-8 bytes |
| `bytes` | `u32 length` + raw bytes |
| `struct` | Fields concatenated in order (no field names) |
| `seq` | `u32 count` + elements |
| `map` | Key-value pairs (no length prefix) |
| `enum` | Variant index as `u32` + variant content |
| `None` | Nothing (zero bytes) |
| `Some(T)` | Serialized as `T` |
| `bool`, `i8`–`i64`, `u16`, `f32`/`f64`, `char` | **Not supported** — returns `BadMessage` error |

Key detail: `struct` serialization uses `serialize_struct` which delegates to `serialize_tuple` — fields are written in declaration order with **no field names or tags**. This matches SFTP's positional binary layout.

The `data_serialize` helper serializes `Vec<u8>` as a raw byte sequence **without** a length prefix (used for `Extended.data` and `ExtendedReply.data`).

### Deserializer (`de.rs`)

The `Deserializer` reads from a `&mut Bytes` buffer, consuming bytes as it goes:

| Wire Pattern | Rust Deserialize Target |
|--------------|------------------------|
| 1 byte | `u8` |
| 4 bytes BE | `u32` |
| 8 bytes BE | `u64` |
| `u32 len` + bytes | `String` / `str` |
| `u32 len` + bytes | `Vec<u8>` / byte buf |
| `u32 count` + elements | `Vec<T>` / seq |
| Positional fields | struct (tuple-like) |
| `u32 variant` + content | enum |
| Key-value pairs | `HashMap` |

The `data_deserialize` helper reads all remaining bytes into a `Vec<u8>` (no length prefix) — used for `Extended.data` and `ExtendedReply.data`.

### TryBuf Helper (`buf.rs`)

A small extension trait on `bytes::Buf`:

```rust
pub trait TryBuf: Buf {
    fn try_get_bytes(&mut self) -> Result<Vec<u8>, Error>;  // u32-length-prefixed
    fn try_get_string(&mut self) -> Result<String, Error>;   // u32-length-prefixed UTF-8
}
```

These are used internally by the deserializer for reading SFTP's length-prefixed byte and string fields.

## FileAttributes Serialization

`FileAttributes` has a custom `Serialize`/`Deserialize` implementation because the SFTP wire format uses a **flags bitmask** to indicate which optional fields are present. This is fundamentally different from serde's typical self-describing formats.

### Serialization Flow

1. Compute `FileAttr` flags bitmask based on which `Option` fields are `Some`:
   - `SIZE` (0x1) — `size` is present
   - `UIDGID` (0x2) — `uid`/`gid` are present
   - `PERMISSIONS` (0x4) — `permissions` is present
   - `ACMODTIME` (0x8) — `atime`/`mtime` are present
   - `EXTENDED` (0x80000000) — extended fields (not yet implemented)
2. Write flags as `u32`
3. Write fields conditionally based on flags

### Deserialization Flow

1. Read `u32` flags bitmask
2. Conditionally read fields based on which bits are set:
   - If `SIZE`: read `u64` for `size`
   - If `UIDGID`: read `u32` for `uid`, `u32` for `gid`
   - If `PERMISSIONS`: read `u32` for `permissions`
   - If `ACMODTIME`: read `u32` for `atime`, `u32` for `mtime`

This ensures that fields not flagged are left as `None` in the `FileAttributes` struct.

## Request ID Tracking

All request packets (except `Init`) carry a `u32 id` field used as a request identifier. The `RequestId` trait and macro provide uniform access:

```rust
pub(crate) trait RequestId: Sized {
    fn get_request_id(&self) -> u32;
}

macro_rules! impl_request_id {
    ($packet:ty) => {
        impl RequestId for $packet {
            fn get_request_id(&self) -> u32 { self.id }
        }
    };
}
```

This is used by the server to extract the request ID for constructing status responses on error, and by the client for demultiplexing responses.