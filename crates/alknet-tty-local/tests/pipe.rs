//! End-to-end integration tests for pipe mode (`terminal: None`) —
//! `LocalTtyBackend` + `TtyAdapter::drive_session` over a
//! `tokio::io::duplex` transport stand-in, running real commands.
//!
//! Covers scenarios 9–13 of `tasks/tty/integration-test.md`:
//!   9.  Happy path (echo): stdout "hello", stderr empty, exit 0
//!   10. Separate stderr: stdout "out", stderr "err", exit 0
//!   11. Signal (SIGTERM, Unix): exit signal-terminated
//!   12. Cancel cleanup: drop duplex → child killed
//!   13. Resize no-op: control chunk accepted, no error

mod common;

use std::sync::Arc;
use std::time::Duration;

use alknet_tty::wire::STREAM_STDOUT;
use alknet_tty_local::LocalTtyBackend;
use common::{negotiate_pipe_json, spawn_session};

/// 9. Happy path (echo): negotiate `{backend:"local", tty:null,
/// cmd:["echo","hello"]}`, read stdout chunks, assert "hello", exit 0.
/// Assert stderr is empty.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipe_happy_path_echo() {
    let backend = Arc::new(LocalTtyBackend::new());
    let (mut client, server) = spawn_session("local", backend);
    client
        .write_negotiation(negotiate_pipe_json("local", &["echo", "hello"]).as_str())
        .await;

    let (stdout, stderr, code) = client
        .read_until_exit()
        .await
        .expect("expected exit chunk before stream close");
    let out = String::from_utf8_lossy(&stdout);
    assert!(
        out.contains("hello"),
        "stdout should contain 'hello'; got: {out:?}"
    );
    assert!(stderr.is_empty(), "stderr should be empty; got: {stderr:?}");
    assert_eq!(code, 0, "echo should exit 0");
    client.assert_no_more_chunks().await;
    let _ = server.await;
}

/// 10. Separate stderr: negotiate `cmd:["sh","-c","echo out; echo err >&2"]`,
/// assert stdout stream receives "out", stderr stream receives "err"
/// (as stderr chunks, stream_type 2), exit 0.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipe_separate_stderr() {
    let backend = Arc::new(LocalTtyBackend::new());
    let (mut client, server) = spawn_session("local", backend);
    client
        .write_negotiation(
            negotiate_pipe_json("local", &["sh", "-c", "echo out; echo err >&2"]).as_str(),
        )
        .await;

    let (stdout, stderr, code) = client
        .read_until_exit()
        .await
        .expect("expected exit chunk before stream close");
    let out = String::from_utf8_lossy(&stdout);
    let err = String::from_utf8_lossy(&stderr);
    assert!(
        out.contains("out"),
        "stdout should contain 'out'; got: {out:?}"
    );
    assert!(
        err.contains("err"),
        "stderr should contain 'err'; got: {err:?}"
    );
    assert_eq!(code, 0, "sh should exit 0");
    let _ = server.await;
}

/// 11. Signal (SIGTERM, Unix): negotiate `cmd:["sleep","60"]`, send
/// `signal:"TERM"`, await exit, assert signal-terminated.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipe_signal_sigterm_kills_child() {
    let backend = Arc::new(LocalTtyBackend::new());
    let (mut client, server) = spawn_session("local", backend);
    client
        .write_negotiation(negotiate_pipe_json("local", &["sleep", "60"]).as_str())
        .await;

    tokio::time::sleep(Duration::from_millis(150)).await;

    client
        .write_control(br#"{"type":"signal","name":"TERM"}"#)
        .await;

    let (_out, _err, code) = client
        .read_until_exit_timeout(Duration::from_secs(5))
        .await
        .expect("expected exit chunk after SIGTERM");
    assert_ne!(
        code, 0,
        "child killed by SIGTERM should report non-zero exit; got {code}"
    );
    let _ = server.await;
}

/// 12. Cancel cleanup (ADR-056): negotiate `sleep 60`, drop the duplex
/// mid-session, assert the child is killed (no orphan). The child
/// writes its pid to a temp file so we can probe it after the drop.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipe_cancel_cleanup_kills_child_no_orphan() {
    let pid_file = std::env::temp_dir().join(format!(
        "alknet_pipe_cancel_pid_{}_{}.txt",
        std::process::id(),
        nanos_seed()
    ));
    let cmd = format!("echo $$ > '{}'; exec sleep 60", pid_file.display());
    let backend = Arc::new(LocalTtyBackend::new());
    let (mut client, server) = spawn_session("local", backend);
    client
        .write_negotiation(negotiate_pipe_json("local", &["sh", "-c", cmd.as_str()]).as_str())
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

/// 13. Resize no-op: negotiate `cmd:["cat"]`, send a `resize` control
/// chunk, assert no error (PipeControl::resize is a no-op). Close the
/// write half to signal end-of-input (the adapter's input pump drops
/// the `ChildStdin` on `ConnectionClosed`, which closes the pipe —
/// tokio's `ChildStdin::poll_shutdown` is a no-op on Unix). Await exit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipe_resize_noop() {
    let backend = Arc::new(LocalTtyBackend::new());
    let (mut client, server) = spawn_session("local", backend);
    client
        .write_negotiation(negotiate_pipe_json("local", &["cat"]).as_str())
        .await;

    tokio::time::sleep(Duration::from_millis(150)).await;

    client
        .write_control(br#"{"type":"resize","cols":120,"rows":40}"#)
        .await;

    client.write_control(br#"{"type":"eof"}"#).await;
    client.close_write_half().await;

    let (_out, _err, code) = client
        .read_until_exit_timeout(Duration::from_secs(5))
        .await
        .expect("expected exit chunk after resize + eof");
    assert_eq!(
        code, 0,
        "cat should exit 0 after no-op resize + write-half close"
    );

    let _ = server.await;
}

/// Sanity: an `echo` in pipe mode should produce at least one
/// stdout chunk with non-empty bytes (the adapter emits a
/// zero-length stdout sentinel after the backend stream ends).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipe_echo_emits_stdout_chunk_then_sentinel() {
    let backend = Arc::new(LocalTtyBackend::new());
    let (mut client, server) = spawn_session("local", backend);
    client
        .write_negotiation(negotiate_pipe_json("local", &["echo", "hi"]).as_str())
        .await;

    let mut saw_nonempty_stdout = false;
    while let Some((st, bytes)) = client.read_chunk_timeout(Duration::from_secs(5)).await {
        if st == STREAM_STDOUT && !bytes.is_empty() {
            saw_nonempty_stdout = true;
        }
        if st == alknet_tty::wire::STREAM_CTRL_OUT {
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            if v["type"] == "exit" {
                break;
            }
        }
    }
    assert!(
        saw_nonempty_stdout,
        "expected at least one non-empty stdout chunk"
    );
    let _ = server.await;
}

fn nanos_seed() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}
