//! Negotiation carriage: `NegotiateRequest`, `TerminalParamsWire`,
//! length-prefixed framing reader/writer, and the error response shape.
//!
//! Phase 1 of the `alknet/tty` wire protocol (ADR-052). The client opens a
//! bidi stream and writes a single length-prefixed JSON frame carrying the
//! terminal parameters, backend selector, command, and environment. After
//! this frame, the stream switches to raw chunks (task `tty/wire-codec`).
//!
//! The framing is self-contained in alknet-tty (ADR-057): a 4-byte
//! big-endian length prefix + UTF-8 JSON body. The format coincides with
//! alknet-call's `EventEnvelope` framing by convention, not by code reuse
//! — alknet-tty does not depend on alknet-call.
//!
//! # Framing disambiguation
//!
//! A server-side error response (JSON, length-prefixed) and a successful
//! allocation's first raw chunk both begin with bytes the client reads
//! before knowing which framing applies. The disambiguation is by the
//! first byte:
//!
//! - An error frame's 4-byte big-endian length prefix starts with `0x00`
//!   because error frames MUST be under 16 MiB ([`MAX_CHUNK_LEN`]) so the
//!   high byte is zero (a wire-format invariant, not an assumption).
//! - A raw chunk's first byte is a `stream_type` in `{1, 2, 3}` —
//!   `0` (stdin from server) is invalid, so `0x00` is unambiguous.
//!
//! See ADR-052 §5 and `tty-wire.md` §"Constraints".

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;

use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::wire::MAX_CHUNK_LEN;

/// The Phase 1 JSON negotiation frame payload (ADR-052).
///
/// The client writes a single length-prefixed frame containing this struct,
/// then switches to raw chunks. The adapter parses it, dispatches on
/// `backend`, and passes `backend_params` verbatim to the selected
/// backend's `allocate()`.
///
/// # Validation
///
/// This struct only parses. The adapter (task `tty/adapter`) validates:
/// - `carriage` MUST be `"raw"` (else `malformed_negotiation`).
/// - `cmd` MUST be non-empty (else `malformed_negotiation`).
/// - `backend` MUST be a registered backend key (else `unknown_backend`).
///
/// Backend-specific params validation is the backend's job (in `allocate()`).
///
/// # `serde(flatten)` for backend-specific fields
///
/// The negotiation frame's top-level JSON object carries both the shared
/// fields (`carriage`, `backend`, `tty`, `cmd`, `cwd`, `env`) and
/// backend-specific fields (e.g., `"container": "abc123"` for docker); the
/// latter land in `backend_params` via the `serde(flatten)` below. The
/// shared fields are consumed by name; whatever remains flows into the
/// `backend_params` map.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct NegotiateRequest {
    /// `"raw"` in v1; any other value → `malformed_negotiation` (checked by
    /// the adapter, not this parser).
    pub carriage: String,
    /// Backend selector key (`"local"`, `"docker"`, `"ssh"`).
    pub backend: String,
    /// `None` = pipe mode (no PTY — ADR-054). `Some` = allocate a PTY with
    /// these dimensions.
    #[serde(default)]
    pub tty: Option<TerminalParamsWire>,
    /// Command vector (argv[0] + args); non-empty (checked by the adapter).
    pub cmd: Vec<String>,
    /// Working directory (`None` = inherit/default).
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// Environment variables (empty = inherit).
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Backend-specific selector fields, opaque to alknet-tty. The adapter
    /// passes this map through verbatim; each backend deserializes its own
    /// strongly-typed params struct from it. See ADR-053 §"Backend params
    /// are opaque."
    ///
    /// Populated by `serde(flatten)`: any top-level key not matching a named
    /// field above lands here.
    #[serde(flatten)]
    pub backend_params: serde_json::Map<String, serde_json::Value>,
}

/// Terminal parameters carried in [`NegotiateRequest::tty`] (ADR-052).
///
/// Maps to SSH's `pty_request` parameters, to docker's
/// `CreateExecOptions { tty: true }`, and to `portable_pty::PtySystem::openpty`
/// for the local backend. The `modes` field is reserved (OQ-44 — default
/// terminal modes suffice for the current scope); backends MUST ignore its
/// content in v1.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct TerminalParamsWire {
    /// `TERM` environment value (e.g., `"xterm-256color"`); `None` =
    /// backend default.
    #[serde(default)]
    pub term: Option<String>,
    /// Terminal columns.
    pub cols: u16,
    /// Terminal rows.
    pub rows: u16,
    /// Pixel width (most terminals don't report this; defaults to 0).
    #[serde(default)]
    pub pixel_width: u16,
    /// Pixel height (most terminals don't report this; defaults to 0).
    #[serde(default)]
    pub pixel_height: u16,
    /// Reserved — OQ-44; backends MUST ignore the content in v1.
    #[serde(default)]
    pub modes: serde_json::Value,
}

/// Errors from the negotiation framing reader/writer.
///
/// `ConnectionClosed` is returned (rather than `Io`) when `read_frame` hits
/// a clean `UnexpectedEof` reading either the length prefix or the body —
/// the peer closed the stream cleanly rather than failing the transport.
#[derive(Debug, thiserror::Error)]
pub enum NegotiationError {
    /// Underlying transport I/O error (not a clean EOF).
    #[error("io: {0}")]
    Io(#[from] io::Error),
    /// The peer closed the stream cleanly (unexpected EOF on the length
    /// prefix or the body).
    #[error("connection closed")]
    ConnectionClosed,
    /// The frame length exceeded [`MAX_CHUNK_LEN`]. A malformed length
    /// prefix can't trigger an oversized allocation.
    #[error("frame too large: {0}")]
    FrameTooLarge(u32),
    /// JSON parse error on the negotiation frame or error response.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

/// Reads length-prefixed negotiation frames from an [`AsyncRead`] transport.
///
/// [`NegotiationReader::read_frame`] reads a 4-byte big-endian length
/// prefix, bounds-checks it against [`MAX_CHUNK_LEN`] (so a malformed
/// prefix can't trigger an oversized allocation), then reads the body. On a
/// clean `UnexpectedEof` it returns [`NegotiationError::ConnectionClosed`].
///
/// After reading the single negotiation frame, call
/// [`NegotiationReader::into_inner`] to reclaim the underlying stream for
/// raw-chunk reading (the reader buffers nothing past the frame boundary,
/// so the stream is clean for [`crate::wire::ChunkReader`]).
pub struct NegotiationReader<R: AsyncRead + Unpin> {
    reader: R,
    len_buf: [u8; 4],
}

impl<R: AsyncRead + Unpin> NegotiationReader<R> {
    /// Wrap an [`AsyncRead`] transport in a negotiation frame reader.
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            len_buf: [0u8; 4],
        }
    }

    /// Consume the reader and return the underlying transport. Use this
    /// after reading the negotiation frame to reclaim the stream for
    /// raw-chunk reading.
    pub fn into_inner(self) -> R {
        self.reader
    }

    /// Read one length-prefixed frame: 4-byte BE length, bounds-check,
    /// body.
    ///
    /// Returns the raw frame bytes (the caller deserializes JSON). On a
    /// clean `UnexpectedEof` reading either the length prefix or the body,
    /// returns [`NegotiationError::ConnectionClosed`]. On a length prefix
    /// exceeding [`MAX_CHUNK_LEN`], returns
    /// [`NegotiationError::FrameTooLarge`].
    pub async fn read_frame(&mut self) -> Result<Bytes, NegotiationError> {
        match self.reader.read_exact(&mut self.len_buf).await {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(NegotiationError::ConnectionClosed);
            }
            Err(e) => return Err(NegotiationError::Io(e)),
        }

        let length = u32::from_be_bytes(self.len_buf);
        if length > MAX_CHUNK_LEN {
            return Err(NegotiationError::FrameTooLarge(length));
        }

        let mut buf = vec![0u8; length as usize];
        if length > 0 {
            match self.reader.read_exact(&mut buf).await {
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    return Err(NegotiationError::ConnectionClosed);
                }
                Err(e) => return Err(NegotiationError::Io(e)),
            }
        }

        Ok(Bytes::from(buf))
    }
}

/// Writes length-prefixed negotiation frames to an [`AsyncWrite`] transport.
///
/// [`NegotiationWriter::write_frame`] writes a 4-byte big-endian length
/// prefix followed by the body, then flushes.
///
/// For server-side error responses, the body MUST be under 16 MiB
/// ([`MAX_CHUNK_LEN`]) so the high byte of the length prefix is `0x00` —
/// this is the wire-format invariant that makes the framing-disambiguation
/// trick sound (see ADR-052 §5). [`Self::write_frame`] does not enforce
/// this; callers building error responses with [`error_response_bytes`] are
/// well within the limit by construction.
pub struct NegotiationWriter<W: AsyncWrite + Unpin> {
    writer: W,
}

impl<W: AsyncWrite + Unpin> NegotiationWriter<W> {
    /// Wrap an [`AsyncWrite`] transport in a negotiation frame writer.
    pub fn new(writer: W) -> Self {
        Self { writer }
    }

    /// Consume the writer and return the underlying transport.
    pub fn into_inner(self) -> W {
        self.writer
    }

    /// Write one length-prefixed frame: 4-byte BE length + body + flush.
    pub async fn write_frame(&mut self, body: &[u8]) -> Result<(), NegotiationError> {
        let len = body.len() as u32;
        self.writer.write_all(&len.to_be_bytes()).await?;
        if !body.is_empty() {
            self.writer.write_all(body).await?;
        }
        self.writer.flush().await?;
        Ok(())
    }
}

/// Serialize a negotiation error response to JSON bytes.
///
/// Produces `{"error":"<error>","<field>":"<value>",...}` — the
/// length-prefixed error frame the server sends when it cannot allocate
/// the session (unknown backend, malformed negotiation, allocate failed).
/// The caller writes the result via [`NegotiationWriter::write_frame`].
///
/// Error frames MUST be under 16 MiB ([`MAX_CHUNK_LEN`]) so the high byte
/// of the 4-byte length prefix is `0x00` (framing disambiguation — ADR-052
/// §5). Realistic error responses are tens of bytes; this invariant holds
/// by construction.
pub fn error_response_bytes(error: &str, fields: &[(&str, &str)]) -> serde_json::Result<Vec<u8>> {
    use serde_json::json;
    let mut map = serde_json::Map::new();
    map.insert("error".to_string(), json!(error));
    for (k, v) in fields {
        map.insert((*k).to_string(), json!(v));
    }
    serde_json::to_vec(&map)
}

#[cfg(test)]
mod tests {
    use super::*;

    use tokio::io::duplex;

    #[test]
    fn round_trip_negotiate_request_all_fields() {
        let json = serde_json::json!({
            "carriage": "raw",
            "backend": "local",
            "tty": {
                "term": "xterm-256color",
                "cols": 80,
                "rows": 24,
                "pixel_width": 0,
                "pixel_height": 0,
                "modes": {}
            },
            "cmd": ["/bin/bash", "-l"],
            "cwd": "/tmp",
            "env": {"FOO": "bar"},
            "container": "abc123"
        });
        let req: NegotiateRequest = serde_json::from_value(json).expect("parse");
        assert_eq!(req.carriage, "raw");
        assert_eq!(req.backend, "local");
        let tty = req.tty.expect("tty");
        assert_eq!(tty.term.as_deref(), Some("xterm-256color"));
        assert_eq!(tty.cols, 80);
        assert_eq!(tty.rows, 24);
        assert_eq!(tty.pixel_width, 0);
        assert_eq!(tty.pixel_height, 0);
        assert_eq!(tty.modes, serde_json::json!({}));
        assert_eq!(req.cmd, vec!["/bin/bash".to_string(), "-l".to_string()]);
        assert_eq!(req.cwd.as_deref(), Some(std::path::Path::new("/tmp")));
        assert_eq!(req.env.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(
            req.backend_params.get("container").and_then(|v| v.as_str()),
            Some("abc123"),
        );
    }

    #[test]
    fn serde_flatten_captures_backend_specific_fields_into_backend_params() {
        let json = serde_json::json!({
            "carriage": "raw",
            "backend": "docker",
            "cmd": ["bash"],
            "container": "abc123",
            "image": "ubuntu:22.04",
            "remove": true
        });
        let req: NegotiateRequest = serde_json::from_value(json).expect("parse");
        assert_eq!(req.backend_params.len(), 3);
        assert_eq!(
            req.backend_params.get("container").and_then(|v| v.as_str()),
            Some("abc123"),
        );
        assert_eq!(
            req.backend_params.get("image").and_then(|v| v.as_str()),
            Some("ubuntu:22.04"),
        );
        assert_eq!(
            req.backend_params.get("remove").and_then(|v| v.as_bool()),
            Some(true),
        );
    }

    #[test]
    fn defaults_applied_when_optional_fields_absent() {
        let json = serde_json::json!({
            "carriage": "raw",
            "backend": "local",
            "cmd": ["true"]
        });
        let req: NegotiateRequest = serde_json::from_value(json).expect("parse");
        assert!(req.tty.is_none());
        assert!(req.cwd.is_none());
        assert!(req.env.is_empty());
        assert!(req.backend_params.is_empty());
    }

    #[test]
    fn terminal_params_wire_defaults() {
        let json = serde_json::json!({"cols": 80, "rows": 24});
        let tty: TerminalParamsWire = serde_json::from_value(json).expect("parse");
        assert!(tty.term.is_none());
        assert_eq!(tty.pixel_width, 0);
        assert_eq!(tty.pixel_height, 0);
        assert!(tty.modes.is_null());
    }

    #[test]
    fn carriage_not_raw_still_parses_adapter_validates() {
        let json = serde_json::json!({
            "carriage": "json",
            "backend": "local",
            "cmd": ["bash"]
        });
        let req: NegotiateRequest = serde_json::from_value(json).expect("parse");
        assert_eq!(req.carriage, "json");
    }

    #[tokio::test]
    async fn round_trip_frame_reader_writer() {
        let (mut a, mut b) = duplex(8 * 1024);
        let body = br#"{"carriage":"raw","backend":"local","cmd":["bash"]}"#;
        let mut writer = NegotiationWriter::new(&mut a);
        let mut reader = NegotiationReader::new(&mut b);
        writer.write_frame(body).await.expect("write");
        let read = reader.read_frame().await.expect("read");
        assert_eq!(read.as_ref(), body);
    }

    #[tokio::test]
    async fn frame_too_large_on_length_exceeding_max_chunk_len() {
        let (mut a, mut b) = duplex(8 * 1024);
        let over = MAX_CHUNK_LEN + 1;
        a.write_all(&over.to_be_bytes()).await.expect("write len");
        a.flush().await.expect("flush");

        let mut reader = NegotiationReader::new(&mut b);
        let err = reader.read_frame().await.unwrap_err();
        assert!(matches!(err, NegotiationError::FrameTooLarge(v) if v == over));
    }

    #[tokio::test]
    async fn connection_closed_on_truncated_length_prefix() {
        let (mut a, mut b) = duplex(8 * 1024);
        a.write_all(&[0u8, 0]).await.expect("write partial");
        a.flush().await.expect("flush");
        a.shutdown().await.expect("shutdown");

        let mut reader = NegotiationReader::new(&mut b);
        let err = reader.read_frame().await.unwrap_err();
        assert!(matches!(err, NegotiationError::ConnectionClosed));
    }

    #[tokio::test]
    async fn connection_closed_on_truncated_body() {
        let (mut a, mut b) = duplex(8 * 1024);
        a.write_all(&16u32.to_be_bytes()).await.expect("write len");
        a.write_all(b"short").await.expect("write partial body");
        a.flush().await.expect("flush");
        a.shutdown().await.expect("shutdown");

        let mut reader = NegotiationReader::new(&mut b);
        let err = reader.read_frame().await.unwrap_err();
        assert!(matches!(err, NegotiationError::ConnectionClosed));
    }

    #[tokio::test]
    async fn connection_closed_clean_close_no_bytes() {
        let (mut a, mut b) = duplex(8 * 1024);
        a.shutdown().await.expect("shutdown");

        let mut reader = NegotiationReader::new(&mut b);
        let err = reader.read_frame().await.unwrap_err();
        assert!(matches!(err, NegotiationError::ConnectionClosed));
    }

    #[tokio::test]
    async fn into_inner_reader_reclaims_stream() {
        let (mut a, mut b) = duplex(8 * 1024);
        let body = br#"{"carriage":"raw","backend":"local","cmd":["bash"]}"#;
        let mut writer = NegotiationWriter::new(&mut a);
        let mut reader = NegotiationReader::new(&mut b);
        writer.write_frame(body).await.expect("write");
        let read = reader.read_frame().await.expect("read");
        assert_eq!(read.as_ref(), body);

        let mut reclaimed = reader.into_inner();
        let mut leftover = [0u8; 4];
        a.write_all(b"tail").await.expect("write leftover");
        a.flush().await.expect("flush");
        reclaimed
            .read_exact(&mut leftover)
            .await
            .expect("read leftover");
        assert_eq!(&leftover, b"tail");
    }

    #[tokio::test]
    async fn into_inner_writer_reclaims_stream() {
        let (mut a, mut b) = duplex(8 * 1024);
        let mut writer = NegotiationWriter::new(&mut a);
        writer.write_frame(b"x").await.expect("write");
        let mut reclaimed = writer.into_inner();
        reclaimed.write_all(b"raw").await.expect("write raw");
        reclaimed.flush().await.expect("flush");

        let mut len = [0u8; 4];
        b.read_exact(&mut len).await.expect("read len");
        assert_eq!(u32::from_be_bytes(len), 1);
        let mut body = [0u8; 1];
        b.read_exact(&mut body).await.expect("read body");
        assert_eq!(&body, b"x");
        let mut tail = [0u8; 3];
        b.read_exact(&mut tail).await.expect("read tail");
        assert_eq!(&tail, b"raw");
    }

    #[test]
    fn error_response_bytes_shape() {
        let bytes = error_response_bytes("unknown_backend", &[("backend", "kubernetes")])
            .expect("serialize");
        let v: serde_json::Value = serde_json::from_slice(&bytes).expect("parse");
        assert_eq!(v["error"], "unknown_backend");
        assert_eq!(v["backend"], "kubernetes");
    }

    #[test]
    fn error_response_bytes_no_extra_fields() {
        let bytes = error_response_bytes("malformed_negotiation", &[("message", "bad")])
            .expect("serialize");
        let v: serde_json::Value = serde_json::from_slice(&bytes).expect("parse");
        assert_eq!(v["error"], "malformed_negotiation");
        assert_eq!(v["message"], "bad");
        assert_eq!(v.as_object().map(|m| m.len()), Some(2));
    }

    #[tokio::test]
    async fn error_frame_first_byte_is_zero_for_framing_disambiguation() {
        let (mut a, mut b) = duplex(8 * 1024);
        let body =
            error_response_bytes("unknown_backend", &[("backend", "kubernetes")]).expect("ser");
        let mut writer = NegotiationWriter::new(&mut a);
        writer.write_frame(&body).await.expect("write");

        let mut first = [0u8; 1];
        b.read_exact(&mut first).await.expect("read first byte");
        assert_eq!(first[0], 0x00);

        let mut len_rest = [0u8; 3];
        b.read_exact(&mut len_rest).await.expect("read len rest");
        let len = u32::from_be_bytes([first[0], len_rest[0], len_rest[1], len_rest[2]]);
        let mut buf = vec![0u8; len as usize];
        b.read_exact(&mut buf).await.expect("read body");
        let v: serde_json::Value = serde_json::from_slice(&buf).expect("parse");
        assert_eq!(v["error"], "unknown_backend");
        assert_eq!(v["backend"], "kubernetes");
    }

    #[tokio::test]
    async fn write_frame_empty_body_writes_length_zero() {
        let (mut a, mut b) = duplex(8 * 1024);
        let mut writer = NegotiationWriter::new(&mut a);
        writer.write_frame(b"").await.expect("write");
        let mut reader = NegotiationReader::new(&mut b);
        let read = reader.read_frame().await.expect("read");
        assert!(read.is_empty());
    }
}
