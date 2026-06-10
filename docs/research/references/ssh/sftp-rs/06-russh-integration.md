# sftp-rs: russh Integration (`russh.rs`)

The `russh` module provides transport glue for connecting an `AsyncSftpClient` to a russh SSH session. It handles the SSH-level work of requesting the SFTP subsystem and converting the russh channel into a split read/write stream.

**Feature gate**: `russh` (implies `async` + `tokio`)

## Core Type Alias

```rust
pub type RusshSftpClient = AsyncSftpClient<WriteHalf<ChannelStream<Msg>>>;
```

The concrete client type when operating over a russh channel. The write half of a `ChannelStream` serves as the `W` type parameter for `AsyncSftpClient`.

## `from_channel()`

```rust
pub async fn from_channel(channel: Channel<Msg>) -> std::io::Result<RusshSftpClient>
```

The primary entry point. Takes an already-open russh session channel and:

1. **Requests the `sftp` subsystem** via `channel.request_subsystem(true, "sftp")`
   - The `true` parameter means "want reply" — the server must acknowledge the subsystem request
   - If the subsystem request fails, returns an IO error
2. **Delegates to `from_subsystem_channel()`** to wrap the channel into a client

The caller is responsible for establishing the SSH session (host-key verification, authentication, proxy jumps, etc.) and opening the channel, e.g.:

```rust
let session = /* established russh client session */;
let channel = session.channel_open_session().await?;
let sftp = sftp::russh::from_channel(channel).await?;
```

## `from_subsystem_channel()`

```rust
pub async fn from_subsystem_channel(channel: Channel<Msg>) -> std::io::Result<RusshSftpClient>
```

For use when the subsystem request has already been made (or the caller wants to manage it differently):

1. **Converts the channel to a stream**: `channel.into_stream()` → `ChannelStream<Msg>`
2. **Splits the stream**: `tokio::io::split(stream)` → `(read_half, write_half)`
3. **Constructs the client**: `AsyncSftpClient::new(read_half, write_half).await`
   - This performs the SFTP handshake (INIT/VERSION)

Use cases:
- Custom subsystem names (not "sftp")
- Passing environment variables before the subsystem request
- Managing the subsystem request lifecycle externally

## Data Flow: russh → AsyncSftpClient

```
┌──────────────────────────────────────────────────────────────┐
│                      Application Code                         │
│                                                               │
│   let sftp = from_channel(channel).await?;                    │
│   sftp.open("/path", opts, &attrs).await?                     │
│                                                               │
└───────────────┬──────────────────────────────────────────────┘
                │
                ▼
┌──────────────────────────────────────────────────────────────┐
│  AsyncSftpClient<WriteHalf<ChannelStream<Msg>>>              │
│                                                               │
│  ┌──────────────┐      ┌────────────────────────────────┐     │
│  │ writer       │      │ reader task                    │     │
│  │ (WriteHalf)  │      │ (ReadHalf)                     │     │
│  └──────┬───────┘      └────────────┬───────────────────┘     │
│         │                           │                         │
│         │  SFTP packets              │  SFTP packets          │
│         ▼                           ▼                         │
└─────────┼───────────────────────────┼─────────────────────────┘
          │                           │
          ▼                           ▼
┌──────────────────────────────────────────────────────────────┐
│  tokio::io::split(ChannelStream<Msg>)                        │
│                                                               │
│  ┌───────────────────────────────────────────────────────┐   │
│  │  ChannelStream<Msg>                                    │   │
│  │  (implements AsyncRead + AsyncWrite)                  │   │
│  └───────────────────────┬───────────────────────────────┘   │
│                          │                                    │
│  ┌───────────────────────┴───────────────────────────────┐   │
│  │  Channel<Msg>  (russh)                                │   │
│  │  • request_subsystem(true, "sftp")                    │   │
│  │  • into_stream() → ChannelStream                      │   │
│  └───────────────────────┬───────────────────────────────┘   │
│                          │                                    │
│  ┌───────────────────────┴───────────────────────────────┐   │
│  │  SSH Session (russh client)                           │   │
│  │  • Authentication, host key verification               │   │
│  │  • channel_open_session()                              │   │
│  └───────────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────────┘
```

## Key russh Types Used

| Type | Source | Purpose |
|------|--------|---------|
| `Channel<Msg>` | `russh` | An open SSH channel, used to request subsystems |
| `ChannelStream<Msg>` | `russh` | Adapter from `Channel` to `AsyncRead + AsyncWrite` |
| `Msg` | `russh::client` | Message type parameter for russh channels |
| `WriteHalf<ChannelStream<Msg>>` | `tokio::io` | Write half after splitting the stream |

## Error Handling

The `from_channel()` function converts russh errors to `std::io::Error`:

```rust
channel
    .request_subsystem(true, "sftp")
    .await
    .map_err(|e| std::io::Error::other(format!("sftp subsystem request failed: {:?}", e)))?;
```

This means the caller only needs to handle `std::io::Error`, not russh-specific error types.

## Responsibility Split

| Layer | Responsibility |
|-------|---------------|
| **Caller** | SSH session creation, host-key verification, user authentication, channel opening |
| **`from_channel()`** | Subsystem request, stream creation, SFTP handshake |
| **`from_subsystem_channel()`** | Stream creation, SFTP handshake (no subsystem request) |
| **`AsyncSftpClient`** | All SFTP protocol operations (open, read, write, etc.) |

The russh module is intentionally thin — it does the minimal work to bridge from a russh channel to an `AsyncSftpClient`, keeping all SFTP logic in the shared `async.rs` and `protocol.rs` modules.