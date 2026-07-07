//! Pipe mode for `LocalTtyBackend`: `tokio::process`-backed runner sessions.
//!
//! Spawns the command with `Stdio::piped()` for stdin/stdout/stderr, exposes
//! tokio-native `AsyncRead`/`AsyncWrite` (no std-thread bridge needed),
//! `PipeControl` (no-op resize, `libc::kill(pid, sig)` signal), and the
//! ADR-056 kill guard on the `exit_code` future.

// TODO: implement
