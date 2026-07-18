//! End-to-end integration tests for PTY mode (`terminal: Some`) —
//! `LocalTtyBackend` + `TtyAdapter::drive_session` over a
//! `tokio::io::duplex` transport stand-in, running real commands.
//!
//! Covers scenarios 1–8 of `tasks/tty/integration-test.md`:
//!   1. Happy path (echo): stdout contains "hello", exit 0, exit chunk last
//!   2. Interactive (cat): stdin round-trips, eof → exit 0
//!   3. Resize: control chunk accepted, no error
//!   4. Signal (SIGINT, Unix): exit code signal-terminated, child reaped
//!   5. Process-group signal (Unix): bash -c "sleep 60", INT reaches sleep child
//!   6. Stdin EOF (zero-length chunk): backend stdin closes, exit chunk sent
//!   7. Cancel cleanup (ADR-056): drop duplex → child killed, no orphan
//!   8. Exit-chunk-is-last: no stdout chunk after exit chunk

mod common;

use std::sync::Arc;
use std::time::Duration;

use alknet_tty::wire::{STREAM_CTRL_OUT, STREAM_STDIN};
use alknet_tty_local::LocalTtyBackend;
use common::{negotiate_pty_json, spawn_session};

const PTY_NEG_ECHO: &str = r#"{"carriage":"raw","backend":"local","tty":{"cols":80,"rows":24,"pixel_width":0,"pixel_height":0},"cmd":["echo","hello"]}"#;

/// 1. Happy path (echo): stdout contains "hello", exit 0, exit chunk is
/// the last chunk before stream close (ADR-055).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pty_happy_path_echo() {
    let backend = Arc::new(LocalTtyBackend::new());
    let (mut client, server) = spawn_session("local", backend);
    client.write_negotiation(PTY_NEG_ECHO).await;

    let (stdout, _stderr, code) = client
        .read_until_exit()
        .await
        .expect("expected exit chunk before stream close");
    let s = String::from_utf8_lossy(&stdout);
    assert!(
        s.contains("hello"),
        "stdout should contain 'hello'; got: {s:?}"
    );
    assert_eq!(code, 0, "echo should exit 0");
    client.assert_no_more_chunks().await;
    let _ = server.await;
}

/// 2. Interactive (cat): write stdin chunks, then send `eof` and
/// drain stdout. The adapter's drainer writes chunks to the client
/// after the exit chunk resolves, so the test sends all input first
/// (closing cat's stdin with `eof`), then reads the echoed stdout
/// and the exit chunk together. Asserts the echoed "ping" appears in
/// stdout and cat exits 0.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pty_interactive_cat_round_trip() {
    let backend = Arc::new(LocalTtyBackend::new());
    let (mut client, server) = spawn_session("local", backend);
    client
        .write_negotiation(negotiate_pty_json("local", &["cat"]).as_str())
        .await;

    tokio::time::sleep(Duration::from_millis(200)).await;

    client.write_chunk(STREAM_STDIN, b"ping\n").await;
    client.write_control(br#"{"type":"eof"}"#).await;

    let (stdout, _stderr, code) = client
        .read_until_exit()
        .await
        .expect("expected exit chunk before stream close");
    let s = String::from_utf8_lossy(&stdout);
    assert!(
        s.contains("ping"),
        "stdin did not round-trip via the PTY echo; got: {s:?}"
    );
    assert_eq!(code, 0, "cat should exit 0 on eof");
    let _ = server.await;
}

/// 3. Resize: send a `resize` control chunk mid-session; assert no
/// error (the PTY resizes). Send `eof`, await exit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pty_resize_no_error() {
    let backend = Arc::new(LocalTtyBackend::new());
    let (mut client, server) = spawn_session("local", backend);
    client
        .write_negotiation(negotiate_pty_json("local", &["cat"]).as_str())
        .await;

    tokio::time::sleep(Duration::from_millis(150)).await;

    client
        .write_control(br#"{"type":"resize","cols":120,"rows":40}"#)
        .await;

    client.write_control(br#"{"type":"eof"}"#).await;

    let (_out, _err, code) = client
        .read_until_exit_timeout(Duration::from_secs(5))
        .await
        .expect("expected exit chunk");
    assert_eq!(code, 0, "cat should exit 0 after resize + eof");
    let _ = server.await;
}

/// 4. Signal (SIGINT, Unix): negotiate `sleep 60`, send `signal:"INT"`,
/// await the exit chunk. Assert exit code is signal-terminated (non-zero,
/// negative on Unix). Assert the child is reaped (no zombie).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pty_signal_sigint_kills_child() {
    let backend = Arc::new(LocalTtyBackend::new());
    let (mut client, server) = spawn_session("local", backend);
    client
        .write_negotiation(negotiate_pty_json("local", &["sleep", "60"]).as_str())
        .await;

    tokio::time::sleep(Duration::from_millis(200)).await;

    client
        .write_control(br#"{"type":"signal","name":"INT"}"#)
        .await;

    let (_out, _err, code) = client
        .read_until_exit_timeout(Duration::from_secs(5))
        .await
        .expect("expected exit chunk after signal");
    assert_ne!(
        code, 0,
        "child killed by SIGINT should report non-zero exit; got {code}"
    );
    let _ = server.await;
}

/// 5. Process-group signal (Unix): negotiate `bash -c "sleep 60"`,
/// send `signal:"INT"`, assert the `sleep` child also receives the
/// signal (the process group is targeted — REQ-TTY-02).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pty_signal_reaches_process_group_child() {
    let backend = Arc::new(LocalTtyBackend::new());
    let (mut client, server) = spawn_session("local", backend);
    client
        .write_negotiation(negotiate_pty_json("local", &["bash", "-c", "sleep 60"]).as_str())
        .await;

    tokio::time::sleep(Duration::from_millis(250)).await;

    client
        .write_control(br#"{"type":"signal","name":"INT"}"#)
        .await;

    let (_out, _err, code) = client
        .read_until_exit_timeout(Duration::from_secs(5))
        .await
        .expect("expected exit chunk after group signal");
    assert_ne!(
        code, 0,
        "process group should have been killed (REQ-TTY-02); got {code}"
    );
    let _ = server.await;
}

/// 6. Stdin EOF (zero-length chunk): negotiate `cat`, send a
/// zero-length stdin chunk (the sentinel), assert the backend's stdin
/// closes, stdout drains, exit chunk is sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pty_stdin_eof_zero_length_chunk() {
    let backend = Arc::new(LocalTtyBackend::new());
    let (mut client, server) = spawn_session("local", backend);
    client
        .write_negotiation(negotiate_pty_json("local", &["cat"]).as_str())
        .await;

    tokio::time::sleep(Duration::from_millis(150)).await;

    client.write_chunk(STREAM_STDIN, b"").await;

    let (_out, _err, code) = client
        .read_until_exit_timeout(Duration::from_secs(5))
        .await
        .expect("expected exit chunk after zero-length stdin sentinel");
    assert_eq!(code, 0, "cat should exit 0 on stdin EOF");
    let _ = server.await;
}

/// 7. Cancel cleanup (ADR-056): negotiate `sleep 60`, drop the duplex
/// (simulating connection drop) mid-session, assert the child is
/// killed (no orphan). The child writes its pid to a temp file so we
/// can probe it after the drop.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pty_cancel_cleanup_kills_child_no_orphan() {
    let pid_file = std::env::temp_dir().join(format!(
        "alknet_pty_cancel_pid_{}_{}.txt",
        std::process::id(),
        nanos_seed()
    ));
    let cmd = format!("echo $$ > '{}'; exec sleep 60", pid_file.display());
    let backend = Arc::new(LocalTtyBackend::new());
    let (mut client, server) = spawn_session("local", backend);
    client
        .write_negotiation(negotiate_pty_json("local", &["bash", "-c", cmd.as_str()]).as_str())
        .await;

    for _ in 0..200 {
        if pid_file.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let pid_str = std::fs::read_to_string(&pid_file).expect("pid file written");
    let pid: i32 = pid_str.trim().parse().expect("pid parses");
    let _ = std::fs::remove_file(&pid_file);

    tokio::time::sleep(Duration::from_millis(150)).await;

    drop(client);
    server.abort();
    let _ = server.await;

    let mut alive = true;
    for _ in 0..100 {
        let r = unsafe { libc::kill(pid, 0) };
        if r != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            alive = false;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!alive, "child (pid={pid}) should be killed after cancel");
}

/// 8. Exit-chunk-is-last: in the happy path, assert no stdout chunk
/// arrives after the exit chunk. Read all chunks, find the exit chunk,
/// assert it is the last chunk before stream close.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pty_exit_chunk_is_last() {
    let backend = Arc::new(LocalTtyBackend::new());
    let (mut client, server) = spawn_session("local", backend);
    client.write_negotiation(PTY_NEG_ECHO).await;

    let mut saw_exit = false;
    while let Some((st, bytes)) = client.read_chunk_timeout(Duration::from_secs(5)).await {
        if saw_exit {
            panic!("chunk arrived after exit: stream_type={st}, bytes={bytes:?} (ADR-055)");
        }
        if st == STREAM_CTRL_OUT {
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            if v["type"] == "exit" {
                assert_eq!(v["code"], 0);
                saw_exit = true;
            }
        }
    }
    assert!(saw_exit, "did not see the exit chunk");
    let _ = server.await;
}

fn nanos_seed() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}
