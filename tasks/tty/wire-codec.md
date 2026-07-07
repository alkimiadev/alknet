---
id: tty/wire-codec
name: Implement raw chunk codec (ChunkReader/ChunkWriter, RawError, stream types)
status: pending
depends_on: [tty/crate-init]
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Implement the raw chunk codec in `src/wire.rs`. This is the Phase 2 "raw
carriage" format (ADR-052): after the single JSON negotiation frame, the bidi
stream switches to a chunk format for the life of the session.

The codec is a direct port of the POC's `/workspace/alknet-tty-poc/src/raw.rs`,
generalized into the crate's `wire` module. The POC code is the reference; this
task ports it verbatim with crate-appropriate doc comments and the `RawError`
type exposed publicly.

### Wire format

```text
[stream_type: u8][length: u32 be][payload bytes]
```

- `stream_type` (1 byte) — the channel:

  | stream_type | channel | direction | payload |
  |---|---|---|---|
  | 0 | data-in (stdin) | client→server | raw bytes |
  | 1 | data-out (stdout) | server→client | raw bytes |
  | 2 | data-err (stderr) | server→client | raw bytes |
  | 3 | control | bidirectional | JSON control message |

  `stream_type > 3` is a protocol error (`InvalidStreamType`). There is no
  extension escape hatch in the byte — a 5th channel is a wire-format change
  requiring a new ALPN (`alknet/tty/v2` per ADR-006).

- `length` (4 bytes, big-endian) — payload length in bytes. Max 16 MiB
  (`MAX_CHUNK_LEN = 16 * 1024 * 1024`). A chunk larger than 16 MiB is a
  protocol error (`ChunkTooLarge`).

- `payload` (`length` bytes) — raw bytes (data channels) or UTF-8 JSON (control).

### Types to implement

```rust
pub const STREAM_STDIN: u8 = 0;
pub const STREAM_STDOUT: u8 = 1;
pub const STREAM_STDERR: u8 = 2;
pub const STREAM_CONTROL: u8 = 3;

pub const CHUNK_HEADER_LEN: usize = 5; // 1 byte type + 4 bytes length
pub const MAX_CHUNK_LEN: u32 = 16 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum RawError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("connection closed")]
    ConnectionClosed,
    #[error("invalid chunk header: stream type {0}")]
    InvalidStreamType(u8),
    #[error("chunk too large: {0}")]
    ChunkTooLarge(u32),
}

#[derive(Debug, Clone)]
pub struct Chunk {
    pub stream_type: u8,
    pub bytes: bytes::Bytes,
}

impl Chunk {
    pub fn stdin(bytes: bytes::Bytes) -> Self;
    pub fn stdout(bytes: bytes::Bytes) -> Self;
    pub fn stderr(bytes: bytes::Bytes) -> Self;
    pub fn control(bytes: bytes::Bytes) -> Self;
}

pub struct ChunkReader<R: AsyncRead + Unpin> { /* ... */ }
impl<R: AsyncRead + Unpin> ChunkReader<R> {
    pub fn new(reader: R) -> Self;
    pub fn into_inner(self) -> R;
    pub async fn read_chunk(&mut self) -> Result<Chunk, RawError>;
}

pub struct ChunkWriter<W: AsyncWrite + Unpin> { /* ... */ }
impl<W: AsyncWrite + Unpin> ChunkWriter<W> {
    pub fn new(writer: W) -> Self;
    pub fn into_inner(self) -> W;
    pub async fn write_chunk(&mut self, chunk: &Chunk) -> Result<(), RawError>;
    pub async fn write_stdin(&mut self, bytes: &[u8]) -> Result<(), RawError>;
    pub async fn write_control_json(&mut self, json: &[u8]) -> Result<(), RawError>;
}
```

### Implementation notes

- `read_chunk` reads the 5-byte header, validates `stream_type <= 3` (else
  `InvalidStreamType`), validates `length <= MAX_CHUNK_LEN` (else
  `ChunkTooLarge`), reads `length` bytes. On `UnexpectedEof` reading the header
  or payload, return `ConnectionClosed` (not `Io`) — the stream ended cleanly.
- `write_chunk` writes the 5-byte header then the payload (if non-empty), then
  flushes. `write_stdin` and `write_control_json` are convenience helpers.
- Zero-length chunks are sentinels (see tty-wire.md §"Sentinels"): a zero-length
  stdin chunk is EOF from the client; a zero-length stdout chunk is "drained"
  from the server. The codec does not special-case these — they are just chunks
  with `length == 0`; the adapter interprets them.

### Tests

Port the POC's round-trip behavior: write a chunk, read it back, assert
equality. Test all four stream types. Test `InvalidStreamType` (stream_type 4)
and `ChunkTooLarge` (length > MAX_CHUNK_LEN). Test `ConnectionClosed` on
truncated header/payload. Use `tokio::io::duplex` as the transport stand-in.

## Acceptance Criteria

- [ ] `STREAM_STDIN`/`STREAM_STDOUT`/`STREAM_STDERR`/`STREAM_CONTROL` constants defined (0-3)
- [ ] `CHUNK_HEADER_LEN` (5) and `MAX_CHUNK_LEN` (16 MiB) constants defined
- [ ] `RawError` enum with `Io`, `ConnectionClosed`, `InvalidStreamType`, `ChunkTooLarge`
- [ ] `Chunk` struct with `stream_type`/`bytes` fields and `stdin`/`stdout`/`stderr`/`control` constructors
- [ ] `ChunkReader::read_chunk` validates stream_type and length, returns `ConnectionClosed` on clean EOF
- [ ] `ChunkWriter::write_chunk`/`write_stdin`/`write_control_json` write header + payload + flush
- [ ] Round-trip unit tests for all four stream types
- [ ] Unit test: `InvalidStreamType` on stream_type > 3
- [ ] Unit test: `ChunkTooLarge` on length > MAX_CHUNK_LEN
- [ ] Unit test: `ConnectionClosed` on truncated header and truncated payload
- [ ] `cargo test -p alknet-tty` succeeds
- [ ] `cargo clippy -p alknet-tty` succeeds with no warnings

## References

- docs/architecture/crates/tty/tty-wire.md — wire format spec (§"Phase 2: Raw Chunk Format", §"Sentinels")
- docs/architecture/decisions/052-alknet-tty-wire-format-and-two-carriage.md — ADR-052
- /workspace/alknet-tty-poc/src/raw.rs — the reference implementation to port

## Notes

> This is a near-verbatim port of the POC's `raw.rs`. The codec is
> transport-agnostic (works over any `AsyncRead`/`AsyncWrite`); the adapter
> task wires it to QUIC bidi streams. The `MAX_CHUNK_LEN` constant is shared
> with the negotiation module (the framing-disambiguation trick depends on
> error frames being under 16 MiB so the high byte of the length prefix is
> `0x00` — see tty-adapter.md §"Negotiation errors").

## Summary

> To be filled on completion