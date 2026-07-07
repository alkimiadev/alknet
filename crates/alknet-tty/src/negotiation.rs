//! Negotiation carriage: `NegotiateRequest`, `TerminalParamsWire`,
//! length-prefixed framing reader/writer, and the error response shape.
//!
//! NOTE: `NegotiateRequest` and `TerminalParamsWire` are minimal
//! placeholders defined here so `crate::backend::From<NegotiateRequest>
//! for TtyParams` compiles before the negotiation task (tty/negotiation)
//! lands. That task replaces this file with the full wire types,
//! length-prefixed framing reader/writer, and the error response shape.
//! The struct field set is dictated by `tty-wire.md` and is stable; the
//! negotiation task adds the framing machinery around these shapes.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::Value;

/// Minimal placeholder wire-frame shape (see module NOTE). Field set per
/// `tty-wire.md` §"Negotiation carriage."
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct NegotiateRequest {
    /// `"raw"` in v1; any other value → `malformed_negotiation`.
    pub carriage: String,
    /// Backend selector key (`"local"`, `"docker"`, `"ssh"`).
    pub backend: String,
    /// `None` = pipe mode (no PTY — ADR-054). `Some` = allocate a PTY.
    #[serde(default)]
    pub tty: Option<TerminalParamsWire>,
    /// Command vector (argv[0] + args); non-empty.
    pub cmd: Vec<String>,
    /// Working directory (`None` = inherit/default).
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// Environment variables (empty = inherit).
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Backend-specific selector fields, opaque to alknet-tty. The adapter
    /// passes this through verbatim; the backend deserializes its own
    /// strongly-typed params struct. See ADR-053 §"Backend params are
    /// opaque."
    #[serde(default)]
    pub backend_params: serde_json::Map<String, Value>,
}

/// Minimal placeholder wire terminal params (see module NOTE). Field set
/// per `tty-wire.md` §"Negotiation carriage." Maps to
/// `crate::backend::TerminalParams` via `From`.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct TerminalParamsWire {
    /// `TERM` value; `None` = backend default.
    #[serde(default)]
    pub term: Option<String>,
    #[serde(default)]
    pub cols: u16,
    #[serde(default)]
    pub rows: u16,
    #[serde(default)]
    pub pixel_width: u16,
    #[serde(default)]
    pub pixel_height: u16,
    /// Reserved — OQ-44; backends MUST ignore content in v1.
    #[serde(default)]
    pub modes: Value,
}
