# sftp-rs: Quick Reference

## Crate Info

| Field | Value |
|-------|-------|
| Name | `sftp` |
| Version | 0.3.0 |
| License | Apache-2.0 |
| Edition | 2021 |
| Repository | https://github.com/jelmer/sftp-rs |
| Author | Jelmer Vernooij |

## Feature Flags

| Feature | Default | Requires | Provides |
|---------|---------|----------|----------|
| `default` | ✅ | `bin` | CLI binary |
| `bin` | ✅ (via default) | `rustyline`, `shell-words` | `sftp` binary |
| `ssh2` | ❌ | `ssh2` crate | `TryFrom<ssh2::Channel>` |
| `async` | ❌ | `tokio` | `AsyncSftpClient` |
| `russh` | ❌ | `russh`, `async`, `tokio` | russh transport glue |

## Module Map

| Module | Feature Gate | Contents |
|--------|-------------|----------|
| `protocol` | always | Wire codec: types, builders, parsers |
| `sync` | always | `SftpClient<C>` |
| `r#async` | `async` | `AsyncSftpClient<W>` |
| `russh` | `russh` | `from_channel()`, `from_subsystem_channel()` |

## Re-exports (`lib.rs`)

```rust
// Always available
pub use protocol::{Attributes, Directory, Error, File, Kind, OpenOptions, Result, TextHint,
    /* all SSH_FILEXFER_ATTR_* constants */};
pub use sync::SftpClient;

// With "async" feature
pub use r#async::AsyncSftpClient;
```

## Client Construction

```rust
// Sync: from any Read+Write
let client = SftpClient::new(channel)?;             // channel: impl Read + Write
let client = SftpClient::from_fd(fd)?;              // Unix: from raw fd
let client = SftpClient::from_handle(handle)?;      // Windows: from raw handle
let client = SftpClient::try_from(ssh2_channel)?;    // ssh2 feature

// Async: from split halves
let client = AsyncSftpClient::new(reader, writer).await?;  // impl AsyncRead + AsyncWrite

// russh: from channel
let client = sftp::russh::from_channel(channel).await?;           // requests sftp subsystem
let client = sftp::russh::from_subsystem_channel(channel).await?; // subsystem already requested
```

## Operations Cheat Sheet

### File Operations

| Operation | Sync | Async | Request | Response |
|-----------|------|-------|---------|----------|
| Open | `open(path, opts, attrs)` | `open(path, opts, attrs).await` | OPEN | HANDLE → File |
| Read | `pread(&file, offset, len)` | `pread(&file, offset, len).await` | READ | DATA → Vec\<u8\> |
| Write | `pwrite(&file, offset, data)` | `pwrite(&file, offset, data).await` | WRITE | STATUS |
| Close | `fclose(&file)` | `fclose(&file).await` | CLOSE | STATUS |
| Line seek | `flineseek(&file, lineno)` | `flineseek(&file, lineno).await` | EXTENDED "text-seek" | STATUS/REPLY |

### Directory Operations

| Operation | Sync | Async | Request | Response |
|-----------|------|-------|---------|----------|
| Open dir | `opendir(path)` | `opendir(path).await` | OPENDIR | HANDLE → Directory |
| Read dir | `readdir(&dir)` | `readdir(&dir).await` | READDIR | NAME → Vec\<(name, long, attrs)\> |
| Close dir | `closedir(&dir)` | `closedir(&dir).await` | CLOSE | STATUS |
| Make dir | `mkdir(path, attrs)` | `mkdir(path, attrs).await` | MKDIR | STATUS |
| Remove dir | `rmdir(path)` | `rmdir(path).await` | RMDIR | STATUS |

### Attribute Operations

| Operation | Sync | Async | Request | Response |
|-----------|------|-------|---------|----------|
| Stat (follow) | `stat(path, flags)` | `stat(path, flags).await` | STAT | ATTRS |
| Lstat (no follow) | `lstat(path, flags)` | `lstat(path, flags).await` | LSTAT | ATTRS |
| Fstat (by handle) | `fstat(&file, flags)` | `fstat(&file, flags).await` | FSTAT | ATTRS |
| Set stat (path) | `setstat(path, attrs)` | `setstat(path, attrs).await` | SETSTAT | STATUS |
| Set stat (handle) | `fsetstat(&file, attrs)` | `fsetstat(&file, attrs).await` | FSETSTAT | STATUS |

### Path Operations

| Operation | Sync | Async | Request | Response |
|-----------|------|-------|---------|----------|
| Canonicalize | `realpath(path, ctrl, compose)` | `realpath(path, ctrl, compose).await` | REALPATH | NAME → String |
| Read symlink | `readlink(path)` | `readlink(path).await` | READLINK | NAME → String |
| Remove file | `remove(path)` | `remove(path).await` | REMOVE | STATUS |
| Rename | `rename(old, new, flags)` | `rename(old, new, flags).await` | RENAME | STATUS |
| Symlink | `symlink(path, target)` | `symlink(path, target).await` | SYMLINK | STATUS |
| Hard link | `hardlink(path, target)` | `hardlink(path, target).await` | LINK | STATUS |
| Link (generic) | `link(path, target, sym)` | `link(path, target, sym).await` | LINK | STATUS |

### Lock Operations

| Operation | Sync | Async | Request | Response |
|-----------|------|-------|---------|----------|
| Block | `block(&file, off, len, mask)` | `block(&file, off, len, mask).await` | BLOCK | STATUS |
| Unblock | `unblock(&file, off, len)` | `unblock(&file, off, len).await` | UNBLOCK | STATUS |

### Extension Operations

| Operation | Sync | Async | Request | Response |
|-----------|------|-------|---------|----------|
| Extended | `extended(req, data)` | `extended(req, data).await` | EXTENDED | REPLY → Option\<Vec\<u8\>\> |

## OpenOptions Builder

```rust
OpenOptions::new()
    .read(true)      // SFTP_FLAG_READ     = 0x01
    .write(true)     // SFTP_FLAG_WRITE    = 0x02
    .append(true)    // SFTP_FLAG_APPEND   = 0x04
    .create(true)    // SFTP_FLAG_CREAT    = 0x08
    .truncate(true)  // SFTP_FLAG_TRUNC    = 0x10
    .excl(true)      // SFTP_FLAG_EXCL     = 0x20
```

## Error Variants (Most Common)

| Error | When |
|-------|------|
| `Eof(msg, lang)` | End of file read, or end of directory listing |
| `NoSuchFile(msg, lang)` | File/path does not exist |
| `PermissionDenied(msg, lang)` | Insufficient permissions |
| `Failure(msg, lang)` | Generic failure |
| `DirNotEmpty(msg, lang)` | rmdir on non-empty directory |
| `NotADirectory(msg, lang)` | Expected directory, got file |
| `FileAlreadyExists(msg, lang)` | Exclusive create on existing file |
| `Io(err)` | Local I/O error or unexpected protocol message |
| `Other(code, msg, lang)` | Unrecognized SFTP status code |

## Testing Approach

Both sync and async clients have extensive test suites using **stub servers**:

- **Sync**: `spawn_stub()` on TCP sockets — a background thread that handles INIT/VERSION then routes requests through a handler closure
- **Async**: `with_stub()` using `tokio::io::duplex()` — a spawned async task that runs a router against a handler closure

Both approaches allow per-test programmable server behavior: the handler receives `(cmd, body_without_req_id)` and returns `(response_cmd, response_body_without_req_id)`.

The protocol module has unit tests for:
- `Attributes` round-trip serialization (empty, all fields, individual field groups)
- Builder byte layout verification (exact byte-level assertions)
- Request-ID wrap/unwrap
- Status parsing (OK, EOF, typed errors)
- Expect functions (correct type, status-as-error, unexpected-type rejection)