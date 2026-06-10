# russh-sftp: Overview and Architecture

**Version**: 2.3.0  
**Repository**: https://github.com/AspectUnk/russh-sftp  
**License**: Apache-2.0  
**Rust Edition**: 2021  
**Protocol**: SFTP v3 ([draft-ietf-secsh-filexfer-02](https://datatracker.ietf.org/doc/html/draft-ietf-secsh-filexfer-02))

## What It Is

`russh-sftp` is a Rust crate providing **both SFTP client and server** implementations. It operates on any transport that provides `AsyncRead + AsyncWrite` byte streams — not just russh. The crate targets SFTP protocol version 3, the most widely deployed version.

Core design decisions:
- **Transport-agnostic** — works with any `AsyncRead + AsyncWrite` stream, not tied to a specific SSH library
- **Both client and server** — provides handler traits and run functions for both sides
- **Two client tiers** — a low-level `RawSftpSession` (request–response) and a high-level `SftpSession` (std::fs-like API with `AsyncRead`/`AsyncWrite` file I/O)
- **Custom serde wire format** — implements its own `Serializer`/`Deserializer` over `bytes::BytesMut` for SFTP binary encoding, not using serde's typical self-describing formats
- **Concurrent writes** — the high-level `File` type supports pipelined writes with configurable concurrency
- **WASM support** — the server module is gated behind `not(target_arch = "wasm32")`; client runtime abstracts over tokio and wasm-bindgen-futures

## Source Layout

```
russh-sftp/src/
├── lib.rs              # Crate root: module declarations, macro imports
├── buf.rs              # TryBuf trait: try_get_bytes/try_get_string on Buf
├── de.rs               # Custom serde Deserializer (binary SFTP wire format)
├── ser.rs              # Custom serde Serializer (binary SFTP wire format)
├── error.rs            # Top-level Error enum
├── extensions.rs       # OpenSSH extension types: limits, hardlink, fsync, statvfs
├── utils.rs            # unix() time helper, read_packet() async wire reader
├── protocol/           # SFTP v3 message types and Packet enum
│   ├── mod.rs          # Packet enum, type constants, TryFrom<Bytes>/Into<Bytes>
│   ├── init.rs         # SSH_FXP_INIT
│   ├── version.rs      # SSH_FXP_VERSION
│   ├── open.rs         # SSH_FXP_OPEN + OpenFlags bitflags
│   ├── close.rs        # SSH_FXP_CLOSE
│   ├── read.rs         # SSH_FXP_READ
│   ├── write.rs        # SSH_FXP_WRITE
│   ├── lstat.rs        # SSH_FXP_LSTAT
│   ├── stat.rs         # SSH_FXP_STAT
│   ├── fstat.rs        # SSH_FXP_FSTAT
│   ├── setstat.rs      # SSH_FXP_SETSTAT
│   ├── fsetstat.rs     # SSH_FXP_FSETSTAT
│   ├── opendir.rs      # SSH_FXP_OPENDIR
│   ├── readdir.rs      # SSH_FXP_READDIR
│   ├── remove.rs       # SSH_FXP_REMOVE
│   ├── mkdir.rs        # SSH_FXP_MKDIR
│   ├── rmdir.rs        # SSH_FXP_RMDIR
│   ├── realpath.rs     # SSH_FXP_REALPATH
│   ├── rename.rs       # SSH_FXP_RENAME
│   ├── readlink.rs     # SSH_FXP_READLINK
│   ├── symlink.rs      # SSH_FXP_SYMLINK
│   ├── status.rs       # SSH_FXP_STATUS + StatusCode enum
│   ├── handle.rs       # SSH_FXP_HANDLE
│   ├── data.rs         # SSH_FXP_DATA
│   ├── name.rs         # SSH_FXP_NAME
│   ├── attrs.rs        # SSH_FXP_ATTRS
│   ├── extended.rs     # SSH_FXP_EXTENDED / SSH_FXP_EXTENDED_REPLY
│   ├── file.rs         # File struct (filename + longname + attrs)
│   └── file_attrs.rs   # FileAttributes, FileAttr flags, FileMode, FileType, FilePermissions
├── client/             # Client-side implementation
│   ├── mod.rs          # Config, run(), execute_handler()
│   ├── handler.rs      # Client Handler trait
│   ├── rawsession.rs   # RawSftpSession: request-response SFTP client
│   ├── session.rs      # SftpSession: high-level std::fs-like client
│   ├── error.rs        # Client-specific Error enum
│   ├── runtime.rs      # Runtime abstraction (tokio native vs WASM)
│   └── fs/
│       ├── mod.rs      # Re-exports: File, DirEntry, ReadDir, Metadata
│       ├── file.rs     # File: AsyncRead + AsyncWrite + AsyncSeek
│       └── dir.rs      # DirEntry, ReadDir iterator
└── server/             # Server-side implementation
    ├── mod.rs          # Config, run(), run_with_config(), process_request()
    ├── handler.rs     # Server Handler trait
    └── reply.rs        # StatusReply type for error responses
```

## Key Dependencies

| Dependency | Version | Purpose |
|------------|---------|---------|
| `tokio` | 1 | Async runtime: io-util, rt, sync, time, macros |
| `tokio-util` | 0.7.18 | Runtime utilities |
| `serde` | 1.0 | Derive macros for protocol types |
| `serde_bytes` | 0.11 | Efficient byte array serialization |
| `bitflags` | 2.11 | Bitflag types: OpenFlags, FileAttr, FileMode, FilePermissionFlags |
| `bytes` | 1.11 | BytesMut/Bytes for zero-copy wire I/O |
| `dashmap` | 6.1 | Concurrent HashMap for request/response tracking |
| `chrono` | 0.4 | DateTime for File::longname formatting |
| `thiserror` | 2.0 | Error derive macros |
| `log` | 0.4 | Logging facade |

Dev dependencies: `russh` 0.61.0 (for examples), `criterion` 0.8.2 (benchmarks).

## Feature Flags

| Feature | Default | Description |
|---------|---------|-------------|
| `async-trait` | ❌ | Enables `#[async_trait]` attribute on Handler traits |

## Architecture Diagram

```
┌──────────────────────────────────────────────────────────────────────┐
│                         Application Layer                            │
│                                                                      │
│  ┌───────────────────┐  ┌──────────────────────┐  ┌──────────────┐ │
│  │   SftpSession      │  │   RawSftpSession      │  │  server::    │ │
│  │   (high-level)     │  │   (low-level)          │  │  Handler     │ │
│  │                    │  │                        │  │  (user impl) │ │
│  │  • open/create     │  │  • init/open/close     │  │              │ │
│  │  • read/write      │  │  • read/write          │  │  init, open, │ │
│  │  • metadata        │  │  • stat/lstat/fstat    │  │  read, write │ │
│  │  • read_dir        │  │  • opendir/readdir     │  │  close, ...  │ │
│  │  • canonicalize    │  │  • mkdir/rmdir/remove  │  └──────┬───────┘ │
│  │  • hardlink/fsync  │  │  • symlink/readlink    │         │         │
│  │  • fs_info (statvfs)│  │  • extended            │         │         │
│  └────────┬──────────┘  └──────────┬────────────┘         │         │
│           │                         │                       │         │
│           │    File (AsyncIO)       │                       │         │
│           │  ┌─────────────────┐   │                       │         │
│           │  │ AsyncRead/Write  │   │                       │         │
│           │  │ AsyncSeek        │   │                       │         │
│           │  │ • pipelined writes│  │                       │         │
│           │  │ • handle tracking │  │                       │         │
│           │  └────────┬─────────┘   │                       │         │
│           └───────────┼─────────────┘                       │         │
│                       │                                     │         │
├───────────────────────┼─────────────────────────────────────┼─────────┤
│                       │    Protocol Layer                    │         │
│  ┌────────────────────┼─────────────────────────────────────┼───┐    │
│  │                   ▼                                     ▼   │    │
│  │   ┌─────────────────────────────────────────────────────┐  │    │
│  │   │                    Packet enum                       │  │    │
│  │   │  Init, Version, Open, Close, Read, Write,           │  │    │
│  │   │  Lstat, Fstat, SetStat, FSetStat, OpenDir,         │  │    │
│  │   │  ReadDir, Remove, MkDir, RmDir, RealPath,          │  │    │
│  │   │  Stat, Rename, ReadLink, Symlink, Status,           │  │    │
│  │   │  Handle, Data, Name, Attrs, Extended, ExtendedReply │  │    │
│  │   └─────────────────────────────────────────────────────┘  │    │
│  │                           │                                │    │
│  │              ┌────────────┴────────────┐                  │    │
│  │              │    ser.rs / de.rs        │                  │    │
│  │              │  Custom serde for binary │                  │    │
│  │              │  SFTP wire format         │                  │    │
│  │              └─────────────────────────┘                  │    │
│  └────────────────────────────────────────────────────────────┘    │
│                                  │                                  │
├──────────────────────────────────┼──────────────────────────────────┤
│                                  ▼                                  │
│                    ┌──────────────────────────┐                      │
│                    │  utils::read_packet()    │                      │
│                    │  buf::TryBuf             │                      │
│                    │  Wire I/O (length-prefixed)                    │
│                    └────────────┬─────────────┘                      │
│                                 │                                   │
├─────────────────────────────────┼───────────────────────────────────┤
│                                 ▼                                   │
│              ┌──────────────────────────────────────┐               │
│              │  Transport (AsyncRead + AsyncWrite)   │               │
│              │  e.g., russh Channel::into_stream()   │               │
│              └──────────────────────────────────────┘               │
└──────────────────────────────────────────────────────────────────────┘
```

## How russh Integration Works

The crate does **not** depend on `russh` at runtime — it only appears as a dev-dependency for examples. Integration is by the caller providing a stream:

```rust
// From examples/client.rs — typical russh integration
let channel = session.channel_open_session().await.unwrap();
channel.request_subsystem(true, "sftp").await.unwrap();
let sftp = SftpSession::new(channel.into_stream()).await.unwrap();
```

```rust
// From examples/server.rs — typical russh server integration
async fn subsystem_request(&mut self, channel_id: ChannelId, name: &str, session: &mut Session)
    -> Result<(), Self::Error>
{
    if name == "sftp" {
        let channel = self.get_channel(channel_id).await;
        let sftp = SftpSession::default();
        session.channel_success(channel_id)?;
        russh_sftp::server::run(channel.into_stream(), sftp).await;
    }
    Ok(())
}
```

The `into_stream()` method on russh's `Channel` produces a type implementing `AsyncRead + AsyncWrite + Unpin + Send + 'static`, which is exactly what `russh-sftp`'s `run()` and `SftpSession::new()` accept.

## Re-exports from `lib.rs`

```rust
pub mod client;
pub mod de;
pub mod extensions;
pub mod protocol;
pub mod ser;
#[cfg(not(target_arch = "wasm32"))]
pub mod server;

// Key re-exports:
pub use client::Handler;        // client::Handler
pub use client::RawSftpSession; // low-level client
pub use client::SftpSession;    // high-level client
pub use server::Handler;        // server::Handler
pub use server::StatusReply;    // server error reply type
```