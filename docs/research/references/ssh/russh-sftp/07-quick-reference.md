# russh-sftp: Quick Reference

## Crate Overview

| Property | Value |
|----------|-------|
| Version | 2.3.0 |
| License | Apache-2.0 |
| Protocol | SFTP v3 (draft-ietf-secsh-filexfer-02) |
| Min Rust | 2021 edition |
| WASM | Client works on wasm32; server not supported |

## Feature Flags

| Feature | Default | Description |
|---------|---------|-------------|
| `async-trait` | ❌ | Enables `#[async_trait]` on Handler traits |

## Key Dependencies

`tokio` (io-util, rt, sync, time, macros), `tokio-util`, `serde`, `serde_bytes`, `bitflags`, `bytes`, `dashmap`, `chrono`, `thiserror`, `log`

## Public Modules

| Module | Description |
|--------|-------------|
| `client` | Client-side: `RawSftpSession`, `SftpSession`, `Handler`, `Config`, `fs::File`, `fs::ReadDir`, `fs::DirEntry` |
| `server` | Server-side: `Handler`, `StatusReply`, `Config`, `run()`, `run_with_config()` |
| `protocol` | All SFTP packet types, `Packet` enum, `StatusCode`, `OpenFlags`, `FileAttributes`, `File`, etc. |
| `extensions` | OpenSSH extensions: `LimitsExtension`, `HardlinkExtension`, `FsyncExtension`, `StatvfsExtension`, `Statvfs` |
| `de` | `from_bytes()` — public deserialization function for extension data |
| `ser` | `to_bytes()` — public serialization function |

## Client Quick Start

```rust
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;

// Connect (using any AsyncRead+AsyncWrite stream)
let sftp = SftpSession::new(stream).await?;

// Or with config:
let sftp = SftpSession::new_with_config(stream, Config {
    max_packet_len: 262144,
    max_concurrent_writes: 8,
    request_timeout_secs: 30,
}).await?;

// File operations
let mut file = sftp.open("remote.txt").await?;           // read-only
let mut file = sftp.create("new.txt").await?;              // create+truncate+write
let mut file = sftp.open_with_flags("f", OpenFlags::READ | OpenFlags::WRITE).await?;

file.write_all(b"hello").await?;
file.flush().await?;            // drains write pipeline + optional fsync
file.rewind().await?;
let mut buf = Vec::new();
file.read_to_end(&mut buf).await?;
file.shutdown().await?;        // properly closes handle

// Directory operations
for entry in sftp.read_dir(".").await? {
    println!("{}: {:?}", entry.file_name(), entry.file_type());
}

// Other operations
sftp.canonicalize(".").await?;
sftp.metadata("file").await?;
sftp.symlink_metadata("link").await?;
sftp.create_dir("dir").await?;
sftp.remove_dir("dir").await?;
sftp.remove_file("file").await?;
sftp.rename("old", "new").await?;
sftp.symlink("target", "link").await?;
sftp.read_link("link").await?;
sftp.hardlink("src", "dst").await?;   // returns false if unsupported
sftp.fs_info("/").await?;             // returns Ok(None) if unsupported
sftp.close().await?;
```

## Server Quick Start

```rust
use russh_sftp::protocol::{File, Handle, Name, Status, StatusCode, Version};
use russh_sftp::server::{Handler, StatusReply};

struct MyHandler;

impl Handler for MyHandler {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(&mut self, version: u32, _ext: HashMap<String, String>)
        -> Result<Version, Self::Error>
    {
        Ok(Version::new())
    }

    // ... implement methods as needed
}

// In your SSH server handler:
async fn subsystem_request(&mut self, channel_id: ChannelId, name: &str, session: &mut Session)
    -> Result<(), Error>
{
    if name == "sftp" {
        let channel = self.get_channel(channel_id).await;
        session.channel_success(channel_id)?;
        russh_sftp::server::run(channel.into_stream(), MyHandler).await;
    }
    Ok(())
}
```

## RawSftpSession Quick Reference

```rust
use russh_sftp::client::RawSftpSession;

let session = RawSftpSession::new(stream);
// or: RawSftpSession::new_with_config(stream, config);

// Must call init first
let version = session.init().await?;

// Request-response methods
let handle = session.open("file", OpenFlags::READ, FileAttributes::empty()).await?;
let data = session.read(handle.handle, 0, 32768).await?;
session.write(handle.handle, 0, vec![1,2,3]).await?;
session.close(handle.handle).await?;
let attrs = session.stat("/path").await?;
let name = session.realpath(".").await?;
let dir_handle = session.opendir("/").await?;
let entries = session.readdir(dir_handle.handle).await?;
session.close(dir_handle.handle).await?;

// Extensions
let limits = session.limits().await?;
session.hardlink("/old", "/new").await?;
session.fsync(handle).await?;
let fs_info = session.statvfs("/").await?;
```

## StatusCode Reference

| Code | Constant | Meaning |
|------|----------|---------|
| 0 | `Ok` | Successful completion |
| 1 | `Eof` | End of file / no more directory entries |
| 2 | `NoSuchFile` | File does not exist |
| 3 | `PermissionDenied` | Insufficient permissions |
| 4 | `Failure` | Generic failure |
| 5 | `BadMessage` | Badly formatted packet |
| 6 | `NoConnection` | Client-side: no connection (never from server) |
| 7 | `ConnectionLost` | Client-side: connection lost (never from server) |
| 8 | `OpUnsupported` | Operation not supported |

## OpenFlags Reference

| Flag | Value | Description |
|------|-------|-------------|
| `READ` | 0x01 | Open for reading |
| `WRITE` | 0x02 | Open for writing |
| `APPEND` | 0x04 | Append to existing data |
| `CREATE` | 0x08 | Create if doesn't exist |
| `TRUNCATE` | 0x10 | Truncate to zero length |
| `EXCLUDE` | 0x20 | Fail if file exists (must be with CREATE) |

## FileAttr (Attribute Flags) Reference

| Flag | Value | Fields Present |
|------|-------|---------------|
| `SIZE` | 0x00000001 | `size: u64` |
| `UIDGID` | 0x00000002 | `uid: u32`, `gid: u32` |
| `PERMISSIONS` | 0x00000004 | `permissions: u32` |
| `ACMODTIME` | 0x00000008 | `atime: u32`, `mtime: u32` |
| `EXTENDED` | 0x80000000 | (not yet implemented) |

## FileMode (File Type) Reference

| Constant | Value | Type |
|----------|-------|------|
| `FIFO` | 0x1000 | Named pipe |
| `CHR` | 0x2000 | Character device |
| `DIR` | 0x4000 | Directory |
| `NAM` | 0x5000 | Named file |
| `BLK` | 0x6000 | Block device |
| `REG` | 0x8000 | Regular file |
| `LNK` | 0xA000 | Symbolic link |
| `SOCK` | 0xC000 | Socket |

## Packet Wire Format

All SFTP packets:

```
[u32 length] [u8 type] [payload...]
```

- `length` = size of type byte + payload (does not include the length field itself)
- Strings: `[u32 len] [utf8 bytes]`
- Byte arrays: `[u32 len] [raw bytes]`
- `FileAttributes`: `[u32 flags] [conditional fields based on flags]`

## Extension Names

| Extension | Name | Version |
|-----------|------|---------|
| Limits | `limits@openssh.com` | 1 |
| Hardlink | `hardlink@openssh.com` | 1 |
| Fsync | `fsync@openssh.com` | 1 |
| Statvfs | `statvfs@openssh.com` | 2 |