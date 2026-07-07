//! Raw chunk codec for the `alknet/tty` bidi stream (ADR-052, Phase 2
//! "raw carriage").
//!
//! Wire format:
//! ```text
//!   [stream_type: u8][length: u32 be][payload bytes]
//! ```
//!
//! `stream_type`:
//!   - 0 = stdin  (client→server, raw bytes)
//!   - 1 = stdout (server→client, raw bytes)
//!   - 2 = stderr (server→client, raw bytes)
//!   - 3 = control (bidirectional, JSON control message — see [`crate::control`])
//!
//! Zero-length data chunks are sentinels: a zero-length stdin chunk is EOF
//! from the client; a zero-length stdout chunk is "drained" from the
//! server. Control chunks are never zero-length (the JSON payload is at
//! least `{}`). The codec does not special-case sentinels — they are just
//! chunks with `length == 0`; the adapter interprets them. See
//! `docs/architecture/crates/tty/tty-wire.md` §"Sentinels".

use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// stdin channel (client→server, raw bytes).
pub const STREAM_STDIN: u8 = 0;
/// stdout channel (server→client, raw bytes).
pub const STREAM_STDOUT: u8 = 1;
/// stderr channel (server→client, raw bytes).
pub const STREAM_STDERR: u8 = 2;
/// control channel (bidirectional, JSON control message).
pub const STREAM_CONTROL: u8 = 3;

/// Chunk header length in bytes: 1 byte `stream_type` + 4 bytes `length`.
pub const CHUNK_HEADER_LEN: usize = 5;
/// Maximum payload length. A larger chunk is a `ChunkTooLarge` protocol
/// error. Shared with the negotiation module so error frames (which reuse
/// the 4-byte length-prefix framing) stay under 16 MiB — this keeps the
/// high byte of the length prefix `0x00`, which is what makes the
/// framing-disambiguation trick sound (see ADR-052 §5).
pub const MAX_CHUNK_LEN: u32 = 16 * 1024 * 1024;

/// Errors from the raw chunk codec.
///
/// `ConnectionClosed` is returned (rather than `Io`) when `read_chunk`
/// hits a clean `UnexpectedEof` reading either the header or the payload —
/// the peer closed the stream cleanly rather than failing the transport.
#[derive(Debug, thiserror::Error)]
pub enum RawError {
    /// Underlying transport I/O error (not a clean EOF).
    #[error("io: {0}")]
    Io(#[from] io::Error),
    /// The peer closed the stream cleanly (unexpected EOF on header or payload).
    #[error("connection closed")]
    ConnectionClosed,
    /// The chunk header's `stream_type` byte was > 3.
    #[error("invalid chunk header: stream type {0}")]
    InvalidStreamType(u8),
    /// The chunk payload length exceeded `MAX_CHUNK_LEN`.
    #[error("chunk too large: {0}")]
    ChunkTooLarge(u32),
}

/// A single raw chunk on the wire: a `stream_type` byte's channel and the
/// payload bytes.
///
/// Construct with [`Chunk::stdin`], [`Chunk::stdout`], [`Chunk::stderr`],
/// or [`Chunk::control`] for the four fixed channels.
#[derive(Debug, Clone)]
pub struct Chunk {
    /// The channel: one of [`STREAM_STDIN`], [`STREAM_STDOUT`],
    /// [`STREAM_STDERR`], [`STREAM_CONTROL`].
    pub stream_type: u8,
    /// The payload bytes (raw for data channels, UTF-8 JSON for control).
    pub bytes: bytes::Bytes,
}

impl Chunk {
    /// A stdin chunk (stream_type 0).
    pub fn stdin(bytes: bytes::Bytes) -> Self {
        Self {
            stream_type: STREAM_STDIN,
            bytes,
        }
    }

    /// A stdout chunk (stream_type 1).
    pub fn stdout(bytes: bytes::Bytes) -> Self {
        Self {
            stream_type: STREAM_STDOUT,
            bytes,
        }
    }

    /// A stderr chunk (stream_type 2).
    pub fn stderr(bytes: bytes::Bytes) -> Self {
        Self {
            stream_type: STREAM_STDERR,
            bytes,
        }
    }

    /// A control chunk (stream_type 3).
    pub fn control(bytes: bytes::Bytes) -> Self {
        Self {
            stream_type: STREAM_CONTROL,
            bytes,
        }
    }
}

/// Reads raw chunks from an [`AsyncRead`] transport.
///
/// [`ChunkReader::read_chunk`] reads the 5-byte header, validates the
/// `stream_type` (≤ 3, else [`RawError::InvalidStreamType`]) and the
/// payload length (≤ [`MAX_CHUNK_LEN`], else [`RawError::ChunkTooLarge`]),
/// then reads the payload. On a clean `UnexpectedEof` reading either the
/// header or the payload, it returns [`RawError::ConnectionClosed`] — the
/// stream ended cleanly, not with a transport error.
pub struct ChunkReader<R: AsyncRead + Unpin> {
    reader: R,
    header: [u8; CHUNK_HEADER_LEN],
}

impl<R: AsyncRead + Unpin> ChunkReader<R> {
    /// Wrap an [`AsyncRead`] transport in a chunk reader.
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            header: [0u8; CHUNK_HEADER_LEN],
        }
    }

    /// Consume the reader and return the underlying transport.
    pub fn into_inner(self) -> R {
        self.reader
    }

    /// Read one chunk: header, validate, payload.
    pub async fn read_chunk(&mut self) -> Result<Chunk, RawError> {
        match self.reader.read_exact(&mut self.header).await {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(RawError::ConnectionClosed);
            }
            Err(e) => return Err(RawError::Io(e)),
        }

        let stream_type = self.header[0];
        if stream_type > 3 {
            return Err(RawError::InvalidStreamType(stream_type));
        }

        let length = u32::from_be_bytes([
            self.header[1],
            self.header[2],
            self.header[3],
            self.header[4],
        ]);
        if length > MAX_CHUNK_LEN {
            return Err(RawError::ChunkTooLarge(length));
        }

        let mut buf = vec![0u8; length as usize];
        if length > 0 {
            match self.reader.read_exact(&mut buf).await {
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    return Err(RawError::ConnectionClosed);
                }
                Err(e) => return Err(RawError::Io(e)),
            }
        }

        Ok(Chunk {
            stream_type,
            bytes: bytes::Bytes::from(buf),
        })
    }
}

/// Writes raw chunks to an [`AsyncWrite`] transport.
///
/// [`ChunkWriter::write_chunk`] writes the 5-byte header then the payload
/// (if non-empty), then flushes. [`ChunkWriter::write_stdin`] and
/// [`ChunkWriter::write_control_json`] are convenience helpers for the
/// two most common write paths.
pub struct ChunkWriter<W: AsyncWrite + Unpin> {
    writer: W,
}

impl<W: AsyncWrite + Unpin> ChunkWriter<W> {
    /// Wrap an [`AsyncWrite`] transport in a chunk writer.
    pub fn new(writer: W) -> Self {
        Self { writer }
    }

    /// Consume the writer and return the underlying transport.
    pub fn into_inner(self) -> W {
        self.writer
    }

    /// Write a chunk: header + payload (if non-empty) + flush.
    pub async fn write_chunk(&mut self, chunk: &Chunk) -> Result<(), RawError> {
        let mut header = [0u8; CHUNK_HEADER_LEN];
        header[0] = chunk.stream_type;
        let len = chunk.bytes.len() as u32;
        header[1..].copy_from_slice(&len.to_be_bytes());
        self.writer.write_all(&header).await?;
        if !chunk.bytes.is_empty() {
            self.writer.write_all(&chunk.bytes).await?;
        }
        self.writer.flush().await?;
        Ok(())
    }

    /// Write a stdin chunk (stream_type 0) directly from a byte slice.
    pub async fn write_stdin(&mut self, bytes: &[u8]) -> Result<(), RawError> {
        let mut header = [0u8; CHUNK_HEADER_LEN];
        header[0] = STREAM_STDIN;
        let len = bytes.len() as u32;
        header[1..].copy_from_slice(&len.to_be_bytes());
        self.writer.write_all(&header).await?;
        if !bytes.is_empty() {
            self.writer.write_all(bytes).await?;
        }
        self.writer.flush().await?;
        Ok(())
    }

    /// Write a control chunk (stream_type 3) carrying a JSON payload.
    pub async fn write_control_json(&mut self, json: &[u8]) -> Result<(), RawError> {
        let mut header = [0u8; CHUNK_HEADER_LEN];
        header[0] = STREAM_CONTROL;
        let len = json.len() as u32;
        header[1..].copy_from_slice(&len.to_be_bytes());
        self.writer.write_all(&header).await?;
        self.writer.write_all(json).await?;
        self.writer.flush().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use bytes::Bytes;
    use tokio::io::{duplex, AsyncWriteExt};

    async fn round_trip(stream_type: u8, payload: &[u8]) {
        let (mut a, mut b) = duplex(8 * 1024);
        let mut writer = ChunkWriter::new(&mut a);
        let mut reader = ChunkReader::new(&mut b);

        let chunk = Chunk {
            stream_type,
            bytes: Bytes::copy_from_slice(payload),
        };
        writer.write_chunk(&chunk).await.unwrap();

        let read = reader.read_chunk().await.unwrap();
        assert_eq!(read.stream_type, stream_type);
        assert_eq!(read.bytes.as_ref(), payload);
    }

    #[tokio::test]
    async fn round_trip_stdin() {
        round_trip(STREAM_STDIN, b"hello stdin").await;
    }

    #[tokio::test]
    async fn round_trip_stdout() {
        round_trip(STREAM_STDOUT, b"hello stdout").await;
    }

    #[tokio::test]
    async fn round_trip_stderr() {
        round_trip(STREAM_STDERR, b"hello stderr").await;
    }

    #[tokio::test]
    async fn round_trip_control() {
        round_trip(STREAM_CONTROL, br#"{"type":"eof"}"#).await;
    }

    #[tokio::test]
    async fn round_trip_empty_payload() {
        round_trip(STREAM_STDIN, b"").await;
    }

    #[tokio::test]
    async fn round_trip_write_stdin_helper() {
        let (mut a, mut b) = duplex(8 * 1024);
        let mut writer = ChunkWriter::new(&mut a);
        let mut reader = ChunkReader::new(&mut b);

        writer.write_stdin(b"piped").await.unwrap();
        let read = reader.read_chunk().await.unwrap();
        assert_eq!(read.stream_type, STREAM_STDIN);
        assert_eq!(read.bytes.as_ref(), b"piped");
    }

    #[tokio::test]
    async fn round_trip_write_control_json_helper() {
        let (mut a, mut b) = duplex(8 * 1024);
        let mut writer = ChunkWriter::new(&mut a);
        let mut reader = ChunkReader::new(&mut b);

        let json = br#"{"type":"resize","cols":80,"rows":24}"#;
        writer.write_control_json(json).await.unwrap();
        let read = reader.read_chunk().await.unwrap();
        assert_eq!(read.stream_type, STREAM_CONTROL);
        assert_eq!(read.bytes.as_ref(), json);
    }

    #[tokio::test]
    async fn invalid_stream_type() {
        let (mut a, mut b) = duplex(8 * 1024);
        a.write_all(&[4u8, 0, 0, 0, 0]).await.unwrap();
        a.flush().await.unwrap();

        let mut reader = ChunkReader::new(&mut b);
        let err = reader.read_chunk().await.unwrap_err();
        assert!(matches!(err, RawError::InvalidStreamType(4)));
    }

    #[tokio::test]
    async fn chunk_too_large() {
        let (mut a, mut b) = duplex(8 * 1024);
        let over = MAX_CHUNK_LEN + 1;
        a.write_all(&[0u8]).await.unwrap();
        a.write_all(&over.to_be_bytes()).await.unwrap();
        a.flush().await.unwrap();

        let mut reader = ChunkReader::new(&mut b);
        let err = reader.read_chunk().await.unwrap_err();
        assert!(matches!(err, RawError::ChunkTooLarge(v) if v == over));
    }

    #[tokio::test]
    async fn connection_closed_truncated_header() {
        let (mut a, mut b) = duplex(8 * 1024);
        a.write_all(&[0u8, 0]).await.unwrap();
        a.flush().await.unwrap();
        a.shutdown().await.unwrap();

        let mut reader = ChunkReader::new(&mut b);
        let err = reader.read_chunk().await.unwrap_err();
        assert!(matches!(err, RawError::ConnectionClosed));
    }

    #[tokio::test]
    async fn connection_closed_truncated_payload() {
        let (mut a, mut b) = duplex(8 * 1024);
        a.write_all(&[0u8, 0, 0, 0, 8]).await.unwrap();
        a.write_all(b"short").await.unwrap();
        a.flush().await.unwrap();
        a.shutdown().await.unwrap();

        let mut reader = ChunkReader::new(&mut b);
        let err = reader.read_chunk().await.unwrap_err();
        assert!(matches!(err, RawError::ConnectionClosed));
    }

    #[tokio::test]
    async fn connection_closed_clean_close_no_bytes() {
        let (mut a, mut b) = duplex(8 * 1024);
        a.shutdown().await.unwrap();

        let mut reader = ChunkReader::new(&mut b);
        let err = reader.read_chunk().await.unwrap_err();
        assert!(matches!(err, RawError::ConnectionClosed));
    }
}
