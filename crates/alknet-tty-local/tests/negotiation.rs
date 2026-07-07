//! Negotiation error tests — scenarios 14–18 of
//! `tasks/tty/integration-test.md`.
//!
//!   14. unknown_backend: error response, first byte `0x00`
//!   15. malformed_negotiation (bad JSON): error response
//!   16. malformed_negotiation (carriage != raw): error response
//!   17. malformed_negotiation (empty cmd): error response
//!   18. allocate_failed (nonexistent binary): error response
//!
//! The test identity carries the `tty:open` scope, so the
//! `unknown_backend` test reaches the backend lookup; the scope-gate
//! is exercised in the adapter's unit tests. These tests assert the
//! server sends a length-prefixed JSON error frame (Phase 1 framing,
//! ADR-052) and closes the stream without entering raw mode.

mod common;

use std::sync::Arc;

use alknet_tty_local::LocalTtyBackend;
use common::{negotiate_pipe_json, spawn_session};

/// 14. unknown_backend: negotiate `{backend:"kubernetes",...}`, assert
/// the error response `{"error":"unknown_backend","backend":"kubernetes"}`,
/// the first byte of the error frame is `0x00`, and the stream closes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unknown_backend_error() {
    let backend = Arc::new(LocalTtyBackend::new());
    let (mut client, server) = spawn_session("local", backend);
    client
        .write_negotiation(
            r#"{"carriage":"raw","backend":"kubernetes","tty":null,"cmd":["echo","hi"]}"#,
        )
        .await;

    let err = client.read_error_frame().await;
    assert_eq!(err["error"], "unknown_backend");
    assert_eq!(err["backend"], "kubernetes");

    client.assert_no_more_chunks().await;
    let _ = server.await;
}

/// 15. malformed_negotiation (bad JSON): write garbage bytes as the
/// negotiation frame, assert `malformed_negotiation` error response.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_negotiation_bad_json() {
    let backend = Arc::new(LocalTtyBackend::new());
    let (mut client, server) = spawn_session("local", backend);
    client.write_negotiation("not valid json").await;

    let err = client.read_error_frame().await;
    assert_eq!(err["error"], "malformed_negotiation");

    client.assert_no_more_chunks().await;
    let _ = server.await;
}

/// 16. malformed_negotiation (carriage != raw): negotiate
/// `{carriage:"json",...}`, assert `malformed_negotiation`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_negotiation_carriage_not_raw() {
    let backend = Arc::new(LocalTtyBackend::new());
    let (mut client, server) = spawn_session("local", backend);
    client
        .write_negotiation(
            r#"{"carriage":"json","backend":"local","tty":null,"cmd":["echo","hi"]}"#,
        )
        .await;

    let err = client.read_error_frame().await;
    assert_eq!(err["error"], "malformed_negotiation");

    client.assert_no_more_chunks().await;
    let _ = server.await;
}

/// 17. malformed_negotiation (empty cmd): negotiate `{cmd:[]}`, assert
/// `malformed_negotiation`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_negotiation_empty_cmd() {
    let backend = Arc::new(LocalTtyBackend::new());
    let (mut client, server) = spawn_session("local", backend);
    client
        .write_negotiation(r#"{"carriage":"raw","backend":"local","tty":null,"cmd":[]}"#)
        .await;

    let err = client.read_error_frame().await;
    assert_eq!(err["error"], "malformed_negotiation");

    client.assert_no_more_chunks().await;
    let _ = server.await;
}

/// 18. allocate_failed (nonexistent binary): negotiate with a
/// nonexistent command path, assert `allocate_failed` (the spawn
/// fails). The adapter sends the error response in negotiation
/// framing and closes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn allocate_failed_nonexistent_binary() {
    let backend = Arc::new(LocalTtyBackend::new());
    let (mut client, server) = spawn_session("local", backend);
    client
        .write_negotiation(
            negotiate_pipe_json("local", &["/nonexistent/binary/that/does/not/exist"]).as_str(),
        )
        .await;

    let err = client.read_error_frame().await;
    assert_eq!(err["error"], "allocate_failed");

    client.assert_no_more_chunks().await;
    let _ = server.await;
}
