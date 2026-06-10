# sftp-rs: Overview and Architecture

**Version**: 0.3.0  
**Repository**: https://github.com/jelmer/sftp-rs  
**License**: Apache-2.0  
**Rust Edition**: 2021

## What It Is

`sftp-rs` is a Rust crate implementing an **SFTP client** — the SSH File Transfer Protocol as defined in [draft-ietf-secsh-filexfer-02](https://datatracker.ietf.org/doc/html/draft-ietf-secsh-filexfer-02). It provides a pure wire-protocol codec plus both synchronous and asynchronous client implementations that layer on top of any transport providing `Read + Write` (sync) or `AsyncRead + AsyncWrite` (async) byte streams.

Core design decisions:
- **Transport-agnostic** — does not include an SSH implementation itself; operates on top of an already-established SSH channel or any byte stream
- **Protocol v3** — targets SFTP protocol version 3, following the published draft but deviating where other servers and clients ignore the RFC
- **Pure codec** — the `protocol` module contains zero I/O; it builds and parses raw bytes, shared by both sync and async clients
- **Two concurrency models** — a synchronous `SftpClient<C>` and an async `AsyncSftpClient<W>` with a background reader task for concurrent pipelining

## Source Layout

```
sftp-rs/src/
├── lib.rs          # Crate root, re-exports, feature gating
├── protocol.rs     # Pure wire-protocol codec: types, builders, parsers
├── sync.rs         # Synchronous SftpClient<C: Read + Write>
├── async.rs        # AsyncSftpClient<W: AsyncWrite + Unpin> with background reader
├── russh.rs        # russh transport glue (optional, feature-gated)
└── bin/
    └── sftp.rs     # CLI interactive sftp client binary
```

## Feature Flags

| Feature | Default | Dependencies | Description |
|---------|---------|-------------|-------------|
| `default` | ✅ | `bin` | Includes the CLI binary |
| `bin` | ✅ (via default) | `rustyline`, `shell-words` | Interactive CLI binary |
| `ssh2` | ❌ | `ssh2` | Integration with the `ssh2` crate (libssh2 bindings) |
| `async` | ❌ | `tokio` | Async client (`AsyncSftpClient`) |
| `russh` | ❌ | `russh`, `async`, `tokio` | russh transport integration |

The `russh` feature implies `async` (it requires `tokio` and the async client).

## Key Dependencies

| Dependency | Version | Purpose |
|------------|---------|---------|
| `byteorder` | 1 | Big-endian binary read/write for wire protocol |
| `russh` | 0.61 (optional) | Pure-Rust SSH implementation, provides channel + stream |
| `ssh2` | 0.9 (optional) | libssh2 bindings, provides `Channel` |
| `tokio` | 1 (optional) | Async runtime, `AsyncRead`/`AsyncWrite`, `io::split` |
| `rustyline` | 18 (optional) | Readline library for CLI binary |
| `shell-words` | 1 (optional) | Shell-style token parsing for CLI |

## Architecture Diagram

```
┌───────────────────────────────────────────────────────────────┐
│                        Application Layer                       │
│                                                                │
│  ┌─────────────────┐  ┌──────────────────┐  ┌──────────────┐ │
│  │   CLI Binary     │  │  SftpClient<C>   │  │ AsyncSftp-  │ │
│  │   (bin/sftp)     │  │   (sync.rs)      │  │ Client<W>   │ │
│  │                  │  │                  │  │ (async.rs)   │ │
│  │  SshChannel →    │  │  C: Read+Write   │  │  W: AsyncW   │ │
│  │  SftpClient       │  │                  │  │             │ │
│  └────────┬─────────┘  └────────┬─────────┘  └──────┬──────┘ │
│           │                      │                    │        │
│           └──────────────────────┴────────────────────┘        │
│                                  │                              │
│                    ┌─────────────┴─────────────┐               │
│                    │    protocol.rs (codec)     │               │
│                    │                            │               │
│                    │  • Error, Result           │               │
│                    │  • Attributes (serde)      │               │
│                    │  • OpenOptions              │               │
│                    │  • build_*() builders      │               │
│                    │  • parse_*() / expect_*()  │               │
│                    │  • read/write_raw_packet    │               │
│                    │  • with/split_request_id    │               │
│                    └────────────────────────────┘               │
│                                  │                              │
└──────────────────────────────────┼──────────────────────────────┘
                                   │
              ┌────────────────────┼────────────────────┐
              │                    │                     │
    ┌─────────┴──────┐  ┌─────────┴───────┐  ┌─────────┴──────┐
    │  ssh subprocess│  │  russh Channel  │  │  ssh2 Channel  │
    │  (stdin/stdout)│  │  (via Channel-  │  │  (libssh2)     │
    │                │  │   Stream)       │  │                │
    └────────────────┘  └────────────────┘  └────────────────┘
```

## Protocol Version Negotiation

Both clients perform the same handshake on construction:

1. **Client sends `SSH_FXP_INIT`** with version `3`
2. **Server responds `SSH_FXP_VERSION`** with its version and optional extensions
3. If server version ≠ 3, the constructor returns an error

```rust
// From protocol.rs
pub fn build_init() -> Vec<u8> {
    let mut buf = Vec::with_capacity(4);
    buf.write_u32::<BigEndian>(3).unwrap();  // version = 3
    buf
}
```

The handshake is the only unnumbered (no request-id) exchange. All subsequent requests include a 4-byte request-id used for demultiplexing responses.

## Re-exports from `lib.rs`

The crate root re-exports the most commonly used types:

```rust
pub use protocol::{
    Attributes, Directory, Error, File, Kind, OpenOptions, Result, TextHint,
    SSH_FILEXFER_ATTR_ACCESSTIME, SSH_FILEXFER_ATTR_ACL, /* ... all flag constants */
};
pub use sync::SftpClient;

#[cfg(feature = "async")]
pub use r#async::AsyncSftpClient;
```