use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::call::envelope::EventEnvelope;

pub fn encode(envelope: &EventEnvelope) -> Vec<u8> {
    let json = serde_json::to_vec(envelope).expect("EventEnvelope serialization must not fail");
    let len = json.len() as u32;
    let mut frame = Vec::with_capacity(4 + json.len());
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(&json);
    frame
}

pub fn decode(data: &[u8]) -> Result<EventEnvelope, FrameDecodeError> {
    if data.len() < 4 {
        return Err(FrameDecodeError::TooShort {
            expected: 4,
            actual: data.len(),
        });
    }
    let len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
    if data.len() < 4 + len {
        return Err(FrameDecodeError::Incomplete {
            expected: 4 + len,
            actual: data.len(),
        });
    }
    let body = &data[4..4 + len];
    let envelope: EventEnvelope = serde_json::from_slice(body).map_err(FrameDecodeError::Json)?;
    Ok(envelope)
}

pub fn decode_with_remainder(data: &[u8]) -> Result<(EventEnvelope, usize), FrameDecodeError> {
    if data.len() < 4 {
        return Err(FrameDecodeError::TooShort {
            expected: 4,
            actual: data.len(),
        });
    }
    let len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
    let total = 4 + len;
    if data.len() < total {
        return Err(FrameDecodeError::Incomplete {
            expected: total,
            actual: data.len(),
        });
    }
    let body = &data[4..total];
    let envelope: EventEnvelope = serde_json::from_slice(body).map_err(FrameDecodeError::Json)?;
    Ok((envelope, total))
}

#[derive(Debug, thiserror::Error)]
pub enum FrameDecodeError {
    #[error("frame too short: expected at least {expected} bytes, got {actual}")]
    TooShort { expected: usize, actual: usize },
    #[error("incomplete frame: expected {expected} bytes, got {actual}")]
    Incomplete { expected: usize, actual: usize },
    #[error("JSON deserialization error: {0}")]
    Json(#[from] serde_json::Error),
}

pub struct FrameFramedReader<S> {
    stream: S,
    buf: Vec<u8>,
}

impl<S> FrameFramedReader<S>
where
    S: AsyncRead + Unpin,
{
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            buf: Vec::with_capacity(4096),
        }
    }

    pub async fn read_frame(&mut self) -> io::Result<Option<EventEnvelope>> {
        loop {
            if self.buf.len() >= 4 {
                let len = u32::from_be_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]])
                    as usize;
                let total = 4 + len;
                if self.buf.len() >= total {
                    let body = &self.buf[4..total];
                    match serde_json::from_slice(body) {
                        Ok(envelope) => {
                            self.buf.drain(..total);
                            return Ok(Some(envelope));
                        }
                        Err(e) => {
                            self.buf.drain(..total);
                            return Err(io::Error::new(io::ErrorKind::InvalidData, e));
                        }
                    }
                }
            }

            let mut tmp = [0u8; 4096];
            match self.stream.read(&mut tmp).await {
                Ok(0) => return Ok(None),
                Ok(n) => self.buf.extend_from_slice(&tmp[..n]),
                Err(e) => return Err(e),
            }
        }
    }
}

pub struct FrameFramedWriter<S> {
    stream: S,
}

impl<S> FrameFramedWriter<S>
where
    S: AsyncWrite + Unpin,
{
    pub fn new(stream: S) -> Self {
        Self { stream }
    }

    pub async fn write_frame(&mut self, envelope: &EventEnvelope) -> io::Result<()> {
        let frame = encode(envelope);
        self.stream.write_all(&frame).await?;
        self.stream.flush().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call::events;
    use serde_json::json;

    #[test]
    fn frame_encode_decode_round_trip() {
        let envelope = EventEnvelope::new(
            events::CALL_REQUESTED,
            "req-1",
            json!({"namespace": "auth", "operation": "verify"}),
        );
        let frame = encode(&envelope);
        let decoded = decode(&frame).unwrap();
        assert_eq!(decoded, envelope);
    }

    #[test]
    fn frame_encode_starts_with_length_prefix() {
        let envelope = EventEnvelope::new(events::CALL_REQUESTED, "req-1", json!({}));
        let frame = encode(&envelope);
        let json = serde_json::to_vec(&envelope).unwrap();
        let expected_len = json.len() as u32;
        let stored_len = u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]);
        assert_eq!(stored_len, expected_len);
        assert_eq!(frame.len(), 4 + json.len());
    }

    #[test]
    fn frame_decode_too_short() {
        let data = [0u8; 2];
        let result = decode(&data);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            err,
            FrameDecodeError::TooShort {
                expected: 4,
                actual: 2
            }
        ));
    }

    #[test]
    fn frame_decode_incomplete() {
        let len = 100u32;
        let mut data = Vec::new();
        data.extend_from_slice(&len.to_be_bytes());
        data.extend_from_slice(&[0u8; 10]);
        let result = decode(&data);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            err,
            FrameDecodeError::Incomplete {
                expected: 104,
                actual: 14
            }
        ));
    }

    #[test]
    fn frame_decode_invalid_json() {
        let json = b"not valid json";
        let mut data = Vec::new();
        data.extend_from_slice(&(json.len() as u32).to_be_bytes());
        data.extend_from_slice(json);
        let result = decode(&data);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), FrameDecodeError::Json(_)));
    }

    #[test]
    fn frame_decode_with_remainder() {
        let envelope = EventEnvelope::new(events::CALL_RESPONDED, "req-1", json!({"result": 42}));
        let frame = encode(&envelope);
        let mut extended = frame.clone();
        extended.extend_from_slice(&[0u8; 50]);
        let (decoded, consumed) = decode_with_remainder(&extended).unwrap();
        assert_eq!(decoded, envelope);
        assert_eq!(consumed, frame.len());
    }

    #[test]
    fn frame_encode_decode_empty_payload() {
        let envelope = EventEnvelope::new(events::CALL_COMPLETED, "req-1", json!(null));
        let frame = encode(&envelope);
        let decoded = decode(&frame).unwrap();
        assert_eq!(decoded, envelope);
    }

    #[test]
    fn frame_encode_decode_large_payload() {
        let large_data: Vec<i32> = (0..1000).collect();
        let envelope = EventEnvelope::new(events::CALL_RESPONDED, "req-big", json!(large_data));
        let frame = encode(&envelope);
        let decoded = decode(&frame).unwrap();
        assert_eq!(decoded, envelope);
    }

    #[test]
    fn frame_decode_with_remainder_too_short() {
        let data = [0u8; 1];
        let result = decode_with_remainder(&data);
        assert!(result.is_err());
    }
}
