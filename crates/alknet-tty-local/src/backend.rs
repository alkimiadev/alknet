//! `LocalTtyBackend`: the `TtyBackend` implementation for local processes.
//!
//! `allocate()` branches on `TtyParams::terminal`: `Some(TerminalParams)`
//! selects PTY mode (this crate's `pty` module); `None` selects pipe mode
//! (the runner case, this crate's `pipe` module). One backend serves both
//! (ADR-054).

// TODO: implement

/// Local TTY backend: implements `alknet_tty::TtyBackend` for local processes.
///
/// `allocate()` branches on `TtyParams::terminal`: `Some(TerminalParams)`
/// selects PTY mode (this crate's `pty` module); `None` selects pipe mode
/// (the runner case, this crate's `pipe` module). One backend serves both
/// (ADR-054).
pub struct LocalTtyBackend;
