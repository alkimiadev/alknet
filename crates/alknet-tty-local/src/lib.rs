//! alknet-tty-local: Local TTY backend for alknet-tty.
//!
//! `LocalTtyBackend` implements `alknet_tty::TtyBackend` via `portable_pty`
//! (PTY mode, terminal semantics) and `std::process::Command` (pipe mode,
//! the runner case). The blocking→async bridge for PTY mode uses three
//! dedicated std threads feeding tokio mpsc/oneshot channels (REQ-TTY-01).
//! Signal forwarding targets the foreground process group (REQ-TTY-02).

pub mod backend;
pub mod pipe;
pub mod pty;

pub use backend::LocalTtyBackend;
