//! `LocalTtyBackend`: the `TtyBackend` implementation for local processes.
//!
//! `allocate()` branches on `TtyParams::terminal`: `Some(TerminalParams)`
//! selects PTY mode (this crate's `pty` module); `None` selects pipe mode
//! (the runner case, this crate's `pipe` module). One backend serves both
//! (ADR-054).

use alknet_tty::backend::{TtyBackend, TtyError, TtyHandle, TtyParams};
use async_trait::async_trait;

/// Local TTY backend: implements `alknet_tty::TtyBackend` for local processes.
///
/// `allocate()` branches on `TtyParams::terminal`: `Some(TerminalParams)`
/// selects PTY mode (this crate's `pty` module); `None` selects pipe mode
/// (the runner case, this crate's `pipe` module). One backend serves both
/// (ADR-054).
///
/// Takes no constructor dependencies — unlike `DockerTtyBackend` (wraps a
/// `bollard::Docker` client) or `SshTtyBackend` (wraps an SSH session), the
/// `portable_pty` system is process-global. The assembly layer constructs one
/// `LocalTtyBackend` and registers it as `"local"`.
pub struct LocalTtyBackend;

impl LocalTtyBackend {
    /// Construct a new `LocalTtyBackend`. The backend is dependency-free;
    /// `portable_pty`'s native system is process-global.
    pub fn new() -> Self {
        Self
    }
}

impl Default for LocalTtyBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TtyBackend for LocalTtyBackend {
    /// Allocate a terminal/process session, branching on
    /// `params.terminal`.
    ///
    /// `Some(TerminalParams)` dispatches to [`pty::allocate_pty`] (PTY mode,
    /// real terminal semantics — resize, process-group signal forwarding,
    /// merged stdout/stderr). `None` dispatches to [`pipe::allocate_pipe`]
    /// (pipe mode, the runner case — separate stdout/stderr, no-op resize,
    /// pid-only signal).
    ///
    /// `backend_params` is ignored: the local backend has no
    /// backend-specific selector fields. An empty `cmd` returns
    /// [`TtyError::AllocFailed`] (the adapter already checks this at
    /// negotiation, but the backend fails gracefully if called directly).
    async fn allocate(&self, params: &TtyParams) -> Result<TtyHandle, TtyError> {
        if params.cmd.is_empty() {
            return Err(TtyError::AllocFailed {
                message: "cmd must be non-empty".to_string(),
            });
        }
        match &params.terminal {
            Some(terminal) => crate::pty::allocate_pty(
                terminal.clone(),
                params.cmd.clone(),
                params.cwd.clone(),
                params.env.clone(),
            ),
            None => crate::pipe::allocate_pipe(
                params.cmd.clone(),
                params.cwd.clone(),
                params.env.clone(),
            ),
        }
    }

    /// The local backend creates its own resource (a process), so there is
    /// no pre-existing resource for the ownership check. Returns `None`.
    fn resource_id(&self, _params: &TtyParams) -> Option<(&'static str, String)> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn term() -> alknet_tty::backend::TerminalParams {
        alknet_tty::backend::TerminalParams {
            term: None,
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
            modes: serde_json::Value::Null,
        }
    }

    fn params(
        terminal: Option<alknet_tty::backend::TerminalParams>,
        cmd: Vec<String>,
    ) -> TtyParams {
        TtyParams {
            terminal,
            cmd,
            cwd: None,
            env: HashMap::new(),
            backend_params: serde_json::Map::new(),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn pty_dispatch_yields_no_stderr() {
        let backend = LocalTtyBackend::new();
        let handle = backend
            .allocate(&params(
                Some(term()),
                vec!["echo".to_string(), "hi".to_string()],
            ))
            .await
            .expect("allocate");
        assert!(handle.stderr.is_none(), "PTY backends merge stdout/stderr");
        assert!(handle.control.is_some(), "PTY backends provide control");
        let _ = handle.exit_code.await;
    }

    #[tokio::test]
    async fn pipe_dispatch_yields_some_stderr() {
        let backend = LocalTtyBackend::new();
        let handle = backend
            .allocate(&params(None, vec!["echo".to_string(), "hi".to_string()]))
            .await
            .expect("allocate");
        assert!(
            handle.stderr.is_some(),
            "pipe backends have separate stderr"
        );
        assert!(handle.control.is_some(), "pipe backends provide control");
        let _ = handle.exit_code.await;
    }

    #[tokio::test]
    async fn empty_cmd_returns_alloc_failed() {
        let backend = LocalTtyBackend::new();
        let result = backend.allocate(&params(None, vec![])).await;
        assert!(
            matches!(result, Err(TtyError::AllocFailed { .. })),
            "expected AllocFailed for empty cmd"
        );
    }

    #[tokio::test]
    async fn empty_cmd_with_terminal_returns_alloc_failed() {
        let backend = LocalTtyBackend::new();
        let result = backend.allocate(&params(Some(term()), vec![])).await;
        assert!(
            matches!(result, Err(TtyError::AllocFailed { .. })),
            "expected AllocFailed for empty cmd even with terminal set"
        );
    }

    #[test]
    fn resource_id_returns_none() {
        let backend = LocalTtyBackend::new();
        let params = params(None, vec!["true".to_string()]);
        assert!(backend.resource_id(&params).is_none());
    }

    #[test]
    fn resource_id_returns_none_with_terminal() {
        let backend = LocalTtyBackend::new();
        let params = params(Some(term()), vec!["true".to_string()]);
        assert!(backend.resource_id(&params).is_none());
    }

    #[test]
    fn default_constructs() {
        let backend = LocalTtyBackend::default();
        let params = params(None, vec!["true".to_string()]);
        assert!(backend.resource_id(&params).is_none());
    }
}
