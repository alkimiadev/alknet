//! Test harness: client-side wire protocol helpers + `drive_session`
//! stand-in over a `tokio::io::duplex` pair.
//!
//! Mirrors the `MockBackend` test harness in
//! `crates/alknet-tty/src/adapter.rs`, but drives a real
//! `LocalTtyBackend` instead of a mock. The test acts as the client:
//! writes the negotiation frame, sends stdin/control chunks, reads
//! stdout/stderr/control chunks, asserts the exit chunk and stream
//! close (ADR-052, ADR-055, ADR-056).
//!
//! `allow(dead_code)` is needed because each test binary uses a
//! different subset of the helpers; clippy would otherwise flag the
//! unused ones (the helpers are part of the shared client-side wire
//! protocol toolkit the task requires).

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;

use alknet_core::auth::Identity;
use alknet_tty::adapter::drive_session;
use alknet_tty::backend::TtyBackend;
use alknet_tty::wire::{
    ChunkReader, STREAM_CTRL_IN, STREAM_CTRL_OUT, STREAM_STDERR, STREAM_STDOUT,
};
use bytes::Bytes;
use tokio::io::duplex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The scope the test identity carries (matches `TtyAdapter::TTY_OPEN_SCOPE`).
pub const TTY_OPEN_SCOPE: &str = "tty:open";

/// A client-side negotiator: writes the JSON negotiation frame and
/// provides helpers for sending stdin chunks and control messages,
/// and for reading chunks/error frames the server writes back.
pub struct ClientSide {
    pub write: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    pub read: tokio::io::ReadHalf<tokio::io::DuplexStream>,
}

impl ClientSide {
    /// Write a length-prefixed JSON negotiation frame (the Phase 1
    /// carriage). The `body` should already be a complete JSON
    /// `NegotiateRequest` object.
    pub async fn write_negotiation(&mut self, body: &str) {
        let len = body.len() as u32;
        self.write.write_all(&len.to_be_bytes()).await.unwrap();
        self.write.write_all(body.as_bytes()).await.unwrap();
        self.write.flush().await.unwrap();
    }

    /// Write a length-prefixed raw bytes negotiation frame (for
    /// malformed-JSON tests).
    pub async fn write_negotiation_bytes(&mut self, body: &[u8]) {
        let len = body.len() as u32;
        self.write.write_all(&len.to_be_bytes()).await.unwrap();
        self.write.write_all(body).await.unwrap();
        self.write.flush().await.unwrap();
    }

    /// Write a raw chunk: `[stream_type: u8][len: u32 be][payload]`.
    pub async fn write_chunk(&mut self, stream_type: u8, payload: &[u8]) {
        let mut header = [0u8; 5];
        header[0] = stream_type;
        let len = payload.len() as u32;
        header[1..].copy_from_slice(&len.to_be_bytes());
        self.write.write_all(&header).await.unwrap();
        if !payload.is_empty() {
            self.write.write_all(payload).await.unwrap();
        }
        self.write.flush().await.unwrap();
    }

    /// Write a client→server control chunk (`STREAM_CTRL_IN`, stream_type
    /// 3) carrying a serialized `ControlMessage` JSON payload (`Resize`,
    /// `Signal`, or `Eof`).
    pub async fn write_control(&mut self, json: &[u8]) {
        self.write_chunk(STREAM_CTRL_IN, json).await;
    }

    /// Read one raw chunk from the server. Returns the `stream_type`
    /// and the payload bytes, or `None` when the server closed the
    /// stream cleanly (EOF).
    pub async fn try_read_chunk(&mut self) -> Option<(u8, Bytes)> {
        let mut reader = ChunkReader::new(&mut self.read);
        match reader.read_chunk().await {
            Ok(chunk) => Some((chunk.stream_type, chunk.bytes)),
            Err(_) => None,
        }
    }

    /// Read one chunk, panicking on read errors (used when the test
    /// expects a chunk, not a close).
    pub async fn read_chunk(&mut self) -> (u8, Bytes) {
        self.try_read_chunk()
            .await
            .expect("expected a chunk, got stream close")
    }

    /// Read one chunk with a timeout. Returns `None` on timeout or
    /// stream close.
    pub async fn read_chunk_timeout(
        &mut self,
        timeout: std::time::Duration,
    ) -> Option<(u8, Bytes)> {
        tokio::time::timeout(timeout, self.try_read_chunk())
            .await
            .unwrap_or_default()
    }

    /// Read a length-prefixed JSON error frame (Phase 1 framing, used
    /// for negotiation errors). The first byte of the 4-byte length
    /// prefix MUST be `0x00` (the framing-disambiguation invariant —
    /// ADR-052 §5); this method asserts that. Returns the parsed JSON.
    pub async fn read_error_frame(&mut self) -> serde_json::Value {
        let mut first = [0u8; 1];
        self.read.read_exact(&mut first).await.unwrap();
        assert_eq!(
            first[0], 0x00,
            "error frame length prefix high byte must be 0x00 (ADR-052 §5)"
        );
        let mut len_rest = [0u8; 3];
        self.read.read_exact(&mut len_rest).await.unwrap();
        let len = u32::from_be_bytes([first[0], len_rest[0], len_rest[1], len_rest[2]]) as usize;
        let mut body = vec![0u8; len];
        self.read.read_exact(&mut body).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    /// Read chunks until the exit control chunk is observed, then
    /// return the accumulated stdout bytes, stderr bytes, and the
    /// exit code. Returns `None` if the stream closes before an exit
    /// chunk arrives.
    pub async fn read_until_exit(&mut self) -> Option<(Vec<u8>, Vec<u8>, i32)> {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        loop {
            let (st, bytes) = self.try_read_chunk().await?;
            match st {
                STREAM_STDOUT => {
                    if !bytes.is_empty() {
                        stdout.extend_from_slice(&bytes);
                    }
                }
                STREAM_STDERR => {
                    if !bytes.is_empty() {
                        stderr.extend_from_slice(&bytes);
                    }
                }
                STREAM_CTRL_OUT => {
                    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                    if v["type"] == "exit" {
                        return Some((stdout, stderr, v["code"].as_i64().unwrap() as i32));
                    }
                }
                other => panic!("unexpected stream_type {other} from server"),
            }
        }
    }

    /// Same as [`read_until_exit`](Self::read_until_exit) but with a
    /// timeout on each individual chunk read.
    pub async fn read_until_exit_timeout(
        &mut self,
        per_chunk_timeout: std::time::Duration,
    ) -> Option<(Vec<u8>, Vec<u8>, i32)> {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        loop {
            let (st, bytes) = self.read_chunk_timeout(per_chunk_timeout).await?;
            match st {
                STREAM_STDOUT => {
                    if !bytes.is_empty() {
                        stdout.extend_from_slice(&bytes);
                    }
                }
                STREAM_STDERR => {
                    if !bytes.is_empty() {
                        stderr.extend_from_slice(&bytes);
                    }
                }
                STREAM_CTRL_OUT => {
                    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                    if v["type"] == "exit" {
                        return Some((stdout, stderr, v["code"].as_i64().unwrap() as i32));
                    }
                }
                other => panic!("unexpected stream_type {other} from server"),
            }
        }
    }

    /// Read chunks until the exit chunk is observed, then assert
    /// the stream closes cleanly and NO further chunks arrive
    /// (ADR-055 exit-chunk-is-last invariant). Returns the stdout,
    /// stderr, and exit code.
    pub async fn read_until_exit_and_close(&mut self) -> (Vec<u8>, Vec<u8>, i32) {
        let (stdout, stderr, code) = self
            .read_until_exit()
            .await
            .expect("expected exit chunk before stream close");
        self.assert_no_more_chunks().await;
        (stdout, stderr, code)
    }

    /// After the exit chunk, assert the server closes the stream
    /// without sending any further chunks. Panics if a chunk arrives.
    pub async fn assert_no_more_chunks(&mut self) {
        match self
            .read_chunk_timeout(std::time::Duration::from_millis(500))
            .await
        {
            None => {}
            Some((st, bytes)) => panic!(
                "chunk arrived after exit chunk: stream_type={st}, bytes={bytes:?} (ADR-055)"
            ),
        }
    }

    /// Close the client write half cleanly. The server's read half sees
    /// a clean EOF on its `ChunkReader`, which the adapter treats as a
    /// client-cancel of the input direction (it signals EOF to the
    /// backend's stdin and stops the input pump). For pipe-mode
    /// backends, this is the path that actually closes the child's
    /// stdin pipe (tokio's `ChildStdin::poll_shutdown` is a no-op on
    /// Unix; dropping the `ChildStdin` is what closes the pipe, and
    /// the input pump drops it on ConnectionClosed).
    pub async fn close_write_half(&mut self) {
        let _ = self.write.shutdown().await;
    }
}

/// Build a `ClientSide` + spawned `drive_session` task over a
/// `tokio::io::duplex` pair. The single `backend` is registered under
/// the given key. The test identity carries `tty:open`, so the
/// adapter's scope-gate passes.
pub fn spawn_session(
    backend_key: &str,
    backend: Arc<dyn TtyBackend>,
) -> (ClientSide, tokio::task::JoinHandle<()>) {
    let mut backends: HashMap<String, Arc<dyn TtyBackend>> = HashMap::new();
    backends.insert(backend_key.to_string(), backend);
    spawn_session_with_backends(backends)
}

/// Like [`spawn_session`] but accepts the full backend map (for
/// `unknown_backend` tests).
pub fn spawn_session_with_backends(
    backends: HashMap<String, Arc<dyn TtyBackend>>,
) -> (ClientSide, tokio::task::JoinHandle<()>) {
    let (client, server) = duplex(8 * 1024);
    let (client_read, client_write) = tokio::io::split(client);
    let client_side = ClientSide {
        write: client_write,
        read: client_read,
    };
    let backends = Arc::new(backends);
    let identity = Some(test_identity());
    let handle = tokio::spawn(async move {
        let (server_read, server_write) = tokio::io::split(server);
        drive_session(server_write, server_read, backends, None, identity).await;
    });
    (client_side, handle)
}

/// A test identity carrying the `tty:open` scope, so the adapter's
/// scope-gate at negotiation passes.
pub fn test_identity() -> Identity {
    Identity {
        id: "test-user".to_string(),
        scopes: vec![TTY_OPEN_SCOPE.to_string()],
        resources: HashMap::new(),
    }
}

/// Build a negotiation frame JSON string for a PTY-mode session.
pub fn negotiate_pty_json(backend: &str, cmd: &[&str]) -> String {
    let cmd_json: Vec<String> = cmd.iter().map(|c| format!("\"{}\"", c)).collect();
    format!(
        r#"{{"carriage":"raw","backend":"{backend}","tty":{{"cols":80,"rows":24,"pixel_width":0,"pixel_height":0}},"cmd":[{cmd}]}}"#,
        cmd = cmd_json.join(",")
    )
}

/// Build a negotiation frame JSON string for a pipe-mode session
/// (`tty: null`).
pub fn negotiate_pipe_json(backend: &str, cmd: &[&str]) -> String {
    let cmd_json: Vec<String> = cmd.iter().map(|c| format!("\"{}\"", c)).collect();
    format!(
        r#"{{"carriage":"raw","backend":"{backend}","tty":null,"cmd":[{cmd}]}}"#,
        cmd = cmd_json.join(",")
    )
}
