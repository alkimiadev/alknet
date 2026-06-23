//! Wire format: `EventEnvelope`, `ResponseEnvelope`, `CallError`, and
//! length-prefixed JSON framing.
//!
//! See `docs/architecture/crates/call/call-protocol.md` for the full
//! specification.

use std::io;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const EVENT_REQUESTED: &str = "call.requested";
pub const EVENT_RESPONDED: &str = "call.responded";
pub const EVENT_COMPLETED: &str = "call.completed";
pub const EVENT_ABORTED: &str = "call.aborted";
pub const EVENT_ERROR: &str = "call.error";

const LENGTH_PREFIX_BYTES: usize = 4;
const MAX_FRAME_SIZE: u32 = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventEnvelope {
    #[serde(rename = "type")]
    pub r#type: String,
    pub id: String,
    pub payload: Value,
}

impl EventEnvelope {
    pub fn new(event_type: impl Into<String>, id: impl Into<String>, payload: Value) -> Self {
        Self {
            r#type: event_type.into(),
            id: id.into(),
            payload,
        }
    }

    pub fn requested(id: impl Into<String>, payload: Value) -> Self {
        Self::new(EVENT_REQUESTED, id, payload)
    }

    pub fn responded(id: impl Into<String>, output: Value) -> Self {
        Self::new(EVENT_RESPONDED, id, serde_json::json!({ "output": output }))
    }

    pub fn completed(id: impl Into<String>) -> Self {
        Self::new(EVENT_COMPLETED, id, serde_json::json!({}))
    }

    pub fn aborted(id: impl Into<String>) -> Self {
        Self::new(EVENT_ABORTED, id, serde_json::json!({}))
    }

    pub fn error(id: impl Into<String>, error: &CallError) -> Self {
        let payload = serde_json::to_value(error).unwrap_or(Value::Null);
        Self::new(EVENT_ERROR, id, payload)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl CallError {
    pub fn new(code: impl Into<String>, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable,
            details: None,
        }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    pub fn not_found(op_name: &str) -> Self {
        Self::new(
            "NOT_FOUND",
            format!("operation not found: {op_name}"),
            false,
        )
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new("FORBIDDEN", message, false)
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new("INVALID_INPUT", message, false)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new("INTERNAL", message, false)
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new("TIMEOUT", message, true)
    }
}

impl Eq for CallError {}

#[derive(Debug, Clone, PartialEq)]
pub struct ResponseEnvelope {
    pub request_id: String,
    pub result: Result<Value, CallError>,
}

impl ResponseEnvelope {
    pub fn ok(request_id: impl Into<String>, output: Value) -> Self {
        Self {
            request_id: request_id.into(),
            result: Ok(output),
        }
    }

    pub fn error(request_id: impl Into<String>, error: CallError) -> Self {
        Self {
            request_id: request_id.into(),
            result: Err(error),
        }
    }

    pub fn not_found(request_id: impl Into<String>, op_name: &str) -> Self {
        Self::error(request_id, CallError::not_found(op_name))
    }

    pub fn forbidden(request_id: impl Into<String>, message: impl Into<String>) -> Self {
        Self::error(request_id, CallError::forbidden(message))
    }

    pub fn into_event(self) -> EventEnvelope {
        let id = self.request_id;
        match self.result {
            Ok(output) => EventEnvelope::responded(id, output),
            Err(ref err) => EventEnvelope::error(id, err),
        }
    }
}

impl From<ResponseEnvelope> for EventEnvelope {
    fn from(envelope: ResponseEnvelope) -> EventEnvelope {
        envelope.into_event()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("connection closed")]
    ConnectionClosed,
    #[error("invalid frame")]
    InvalidFrame,
}

pub struct FrameFramedReader<R: AsyncRead + Unpin> {
    reader: R,
    len_buf: [u8; LENGTH_PREFIX_BYTES],
}

impl<R: AsyncRead + Unpin> FrameFramedReader<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            len_buf: [0u8; LENGTH_PREFIX_BYTES],
        }
    }

    pub fn into_inner(self) -> R {
        self.reader
    }

    pub async fn read_frame(&mut self) -> Result<EventEnvelope, FrameError> {
        match self.reader.read_exact(&mut self.len_buf).await {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(FrameError::ConnectionClosed);
            }
            Err(e) => return Err(FrameError::Io(e)),
        }

        let length = u32::from_be_bytes(self.len_buf);
        if length == 0 {
            return Err(FrameError::InvalidFrame);
        }
        if length > MAX_FRAME_SIZE {
            return Err(FrameError::InvalidFrame);
        }

        let mut body = vec![0u8; length as usize];
        match self.reader.read_exact(&mut body).await {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(FrameError::ConnectionClosed);
            }
            Err(e) => return Err(FrameError::Io(e)),
        }

        let envelope: EventEnvelope = serde_json::from_slice(&body)?;
        Ok(envelope)
    }
}

pub struct FrameFramedWriter<W: AsyncWrite + Unpin> {
    writer: W,
}

impl<W: AsyncWrite + Unpin> FrameFramedWriter<W> {
    pub fn new(writer: W) -> Self {
        Self { writer }
    }

    pub fn into_inner(self) -> W {
        self.writer
    }

    pub async fn write_frame(&mut self, envelope: &EventEnvelope) -> Result<(), FrameError> {
        let body = serde_json::to_vec(envelope)?;
        let len = body.len();
        if len > MAX_FRAME_SIZE as usize {
            return Err(FrameError::InvalidFrame);
        }
        let len_bytes = (len as u32).to_be_bytes();
        self.writer.write_all(&len_bytes).await?;
        self.writer.write_all(&body).await?;
        self.writer.flush().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{duplex, AsyncReadExt};

    fn sample_envelope() -> EventEnvelope {
        EventEnvelope::new(
            "call.requested",
            "req-1",
            serde_json::json!({
                "operationId": "/fs/readFile",
                "input": { "path": "/etc/hosts" }
            }),
        )
    }

    #[tokio::test]
    async fn round_trip_envelope() {
        let (client, server) = duplex(8 * 1024);
        let envelope = sample_envelope();

        let mut writer = FrameFramedWriter::new(client);
        writer.write_frame(&envelope).await.unwrap();
        drop(writer);

        let mut reader = FrameFramedReader::new(server);
        let read = reader.read_frame().await.unwrap();
        assert_eq!(read, envelope);
    }

    #[tokio::test]
    async fn round_trip_multiple_frames() {
        let (client, server) = duplex(8 * 1024);

        let envelopes = vec![
            EventEnvelope::responded("a", Value::String("hello".into())),
            EventEnvelope::completed("a"),
            EventEnvelope::aborted("b"),
        ];

        {
            let mut writer = FrameFramedWriter::new(client);
            for e in &envelopes {
                writer.write_frame(e).await.unwrap();
            }
        }

        let mut reader = FrameFramedReader::new(server);
        for expected in envelopes {
            let read = reader.read_frame().await.unwrap();
            assert_eq!(read, expected);
        }
    }

    #[tokio::test]
    async fn read_frame_on_closed_reader_returns_connection_closed() {
        let (_, server) = duplex(8 * 1024);
        let mut reader = FrameFramedReader::new(server);
        match reader.read_frame().await {
            Err(FrameError::ConnectionClosed) => {}
            other => panic!("expected ConnectionClosed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn truncated_body_returns_connection_closed() {
        let (mut client, server) = duplex(8 * 1024);
        let envelope = sample_envelope();
        let body = serde_json::to_vec(&envelope).unwrap();
        let len_bytes = (body.len() as u32).to_be_bytes();
        client.write_all(&len_bytes).await.unwrap();
        client.write_all(&body[..body.len() / 2]).await.unwrap();
        drop(client);

        let mut reader = FrameFramedReader::new(server);
        match reader.read_frame().await {
            Err(FrameError::ConnectionClosed) => {}
            other => panic!("expected ConnectionClosed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn zero_length_frame_is_invalid() {
        let (mut client, server) = duplex(8 * 1024);
        client.write_all(&[0u8, 0, 0, 0]).await.unwrap();
        drop(client);

        let mut reader = FrameFramedReader::new(server);
        match reader.read_frame().await {
            Err(FrameError::InvalidFrame) => {}
            other => panic!("expected InvalidFrame, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn oversized_frame_is_invalid() {
        let (mut client, server) = duplex(8 * 1024);
        let too_big = (MAX_FRAME_SIZE + 1u32).to_be_bytes();
        client.write_all(&too_big).await.unwrap();
        drop(client);

        let mut reader = FrameFramedReader::new(server);
        match reader.read_frame().await {
            Err(FrameError::InvalidFrame) => {}
            other => panic!("expected InvalidFrame, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn framing_handles_large_payload() {
        let (client, server) = duplex(1024 * 1024);
        let big = "x".repeat(64 * 1024);
        let envelope = EventEnvelope::responded("big", Value::String(big.clone()));

        let mut writer = FrameFramedWriter::new(client);
        writer.write_frame(&envelope).await.unwrap();
        drop(writer);

        let mut reader = FrameFramedReader::new(server);
        let read = reader.read_frame().await.unwrap();
        assert_eq!(read, envelope);
        match read.payload {
            Value::Object(map) => match map.get("output") {
                Some(Value::String(s)) => assert_eq!(s, &big),
                other => panic!("expected output string, got {other:?}"),
            },
            other => panic!("expected object payload, got {other:?}"),
        }
    }

    #[test]
    fn response_envelope_ok_produces_call_responded_event() {
        let response = ResponseEnvelope::ok("req-1", Value::String("hi".into()));
        let event: EventEnvelope = response.into();
        assert_eq!(event.r#type, EVENT_RESPONDED);
        assert_eq!(event.id, "req-1");
        let map = event.payload.as_object().expect("payload is object");
        assert_eq!(map.get("output"), Some(&Value::String("hi".into())));
    }

    #[test]
    fn response_envelope_error_produces_call_error_event() {
        let err = CallError::new("FILE_NOT_FOUND", "file not found: /etc/x", false)
            .with_details(serde_json::json!({ "path": "/etc/x" }));
        let response = ResponseEnvelope::error("req-2", err);
        let event: EventEnvelope = response.into();
        assert_eq!(event.r#type, EVENT_ERROR);
        assert_eq!(event.id, "req-2");
        assert_eq!(
            event.payload.get("code"),
            Some(&Value::String("FILE_NOT_FOUND".into()))
        );
        assert_eq!(
            event.payload.get("message"),
            Some(&Value::String("file not found: /etc/x".into()))
        );
        assert_eq!(event.payload.get("retryable"), Some(&Value::Bool(false)));
        assert_eq!(
            event.payload.get("details"),
            Some(&serde_json::json!({ "path": "/etc/x" }))
        );
    }

    #[test]
    fn response_envelope_not_found_helper() {
        let response = ResponseEnvelope::not_found("req-3", "fs/missing");
        assert_eq!(response.request_id, "req-3");
        match &response.result {
            Err(e) => {
                assert_eq!(e.code, "NOT_FOUND");
                assert!(!e.retryable);
                assert!(e.message.contains("fs/missing"));
            }
            other => panic!("expected Err, got {other:?}"),
        }
        let event: EventEnvelope = response.into();
        assert_eq!(event.r#type, EVENT_ERROR);
        assert_eq!(event.id, "req-3");
        assert_eq!(
            event.payload.get("code"),
            Some(&Value::String("NOT_FOUND".into()))
        );
    }

    #[test]
    fn response_envelope_forbidden_helper() {
        let response = ResponseEnvelope::forbidden("req-4", "authentication required");
        match &response.result {
            Err(e) => {
                assert_eq!(e.code, "FORBIDDEN");
                assert_eq!(e.message, "authentication required");
            }
            other => panic!("expected Err, got {other:?}"),
        }
        let event: EventEnvelope = response.into();
        assert_eq!(event.r#type, EVENT_ERROR);
        assert_eq!(event.id, "req-4");
    }

    #[test]
    fn event_envelope_completed_has_empty_payload() {
        let event = EventEnvelope::completed("sub-1");
        assert_eq!(event.r#type, EVENT_COMPLETED);
        assert_eq!(event.id, "sub-1");
        assert_eq!(event.payload, serde_json::json!({}));
    }

    #[test]
    fn event_envelope_aborted_has_empty_payload() {
        let event = EventEnvelope::aborted("req-9");
        assert_eq!(event.r#type, EVENT_ABORTED);
        assert_eq!(event.id, "req-9");
        assert_eq!(event.payload, serde_json::json!({}));
    }

    #[test]
    fn event_envelope_responded_wraps_output() {
        let event = EventEnvelope::responded("req-1", Value::Number(42.into()));
        assert_eq!(event.r#type, EVENT_RESPONDED);
        assert_eq!(event.payload.get("output"), Some(&Value::Number(42.into())));
    }

    #[test]
    fn event_envelope_serializes_type_field() {
        let event = sample_envelope();
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"call.requested\""));
        assert!(!json.contains("\"r#type\""));

        let parsed: EventEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, event);
    }

    #[test]
    fn call_error_skips_missing_details() {
        let err = CallError::new("INTERNAL", "boom", false);
        let json = serde_json::to_string(&err).unwrap();
        assert!(!json.contains("details"));
    }

    #[tokio::test]
    async fn read_after_eof_then_eof_returns_connection_closed() {
        let mut data = Vec::new();
        let envelope = EventEnvelope::responded("one", Value::Null);
        let body = serde_json::to_vec(&envelope).unwrap();
        data.extend_from_slice(&(body.len() as u32).to_be_bytes());
        data.extend_from_slice(&body);
        let cursor = std::io::Cursor::new(data);
        let mut reader = FrameFramedReader::new(cursor);
        let first = reader.read_frame().await.unwrap();
        assert_eq!(first, envelope);
        match reader.read_frame().await {
            Err(FrameError::ConnectionClosed) => {}
            other => panic!("expected ConnectionClosed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn writer_into_inner_recovers_stream() {
        let (client, server) = duplex(8 * 1024);
        let envelope = sample_envelope();
        let mut writer = FrameFramedWriter::new(client);
        writer.write_frame(&envelope).await.unwrap();
        let mut recovered = writer.into_inner();
        recovered.shutdown().await.unwrap();
        drop(recovered);

        let mut reader = FrameFramedReader::new(server);
        let read = reader.read_frame().await.unwrap();
        assert_eq!(read, envelope);
        let _ = reader.into_inner();
    }

    #[tokio::test]
    async fn reader_handles_partial_length_prefix() {
        let (mut client, server) = duplex(8 * 1024);
        client.write_all(&[0u8, 0]).await.unwrap();
        drop(client);
        let mut reader = FrameFramedReader::new(server);
        match reader.read_frame().await {
            Err(FrameError::ConnectionClosed) => {}
            other => panic!("expected ConnectionClosed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn reader_drains_remaining_after_read() {
        let mut data = Vec::new();
        let envelope = sample_envelope();
        let body = serde_json::to_vec(&envelope).unwrap();
        data.extend_from_slice(&(body.len() as u32).to_be_bytes());
        data.extend_from_slice(&body);
        data.extend_from_slice(&[9u8; 4]);
        let mut cursor = tokio::io::BufReader::new(std::io::Cursor::new(data));
        let mut reader = FrameFramedReader::new(&mut cursor);
        let read = reader.read_frame().await.unwrap();
        assert_eq!(read, envelope);
        let mut leftover = Vec::new();
        let _ = cursor.read_to_end(&mut leftover).await.unwrap();
        assert_eq!(leftover, vec![9u8; 4]);
    }
}
