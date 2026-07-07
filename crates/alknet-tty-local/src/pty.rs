//! PTY mode for `LocalTtyBackend`: `portable_pty`-backed terminal sessions.
//!
//! Implements the blocking→async bridge (REQ-TTY-01) via three dedicated
//! std threads (reader, writer, waiter) feeding tokio mpsc/oneshot channels,
//! `PtyControl` for resize/signal, REQ-TTY-02 process-group signal
//! forwarding (`libc::kill(-pgid, sig)`), and the ADR-056 kill guard on the
//! `exit_code` future.

// TODO: implement
