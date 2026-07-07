//! Control messages carried in `stream_type 3` chunks (ADR-052).
//!
//! Control chunks carry a JSON payload tagged by `type`. The schema is the
//! POC's `ControlMessage` (`/workspace/alknet-tty-poc/src/control.rs`):
//!
//! ```json
//! {"type":"resize","cols":80,"rows":24,"pixel_width":0,"pixel_height":0}
//! {"type":"signal","name":"INT"}
//! {"type":"eof"}
//! {"type":"exit","code":0}
//! ```
//!
//! Control messages are rare (resize on window drag, signal on Ctrl-C), so
//! serialization cost is negligible versus data chunks. The JSON shape is
//! consistent with the call protocol's JSON-everything stance and easy to
//! extend: new types are additive on the `type` tag (ADR-052 §"Control
//! Channel").
//!
//! Unknown `type` values: `from_slice` returns a `serde_json::Error`. The
//! adapter (task `tty/adapter`) ignores that error per the wire spec's
//! "unknown types are ignored" policy — this keeps the enum exhaustive and
//! the policy an adapter-level concern, not a schema-level leak.

use serde::{Deserialize, Serialize};

/// A control message riding on `stream_type 3`.
///
/// Direction and mapping (per `tty-wire.md` §"Control Channel"):
///
/// | direction      | variant  | maps to                                            |
/// |----------------|----------|----------------------------------------------------|
/// | client→server  | `Resize` | SSH `window-change`, docker exec resize, `ioctl`   |
/// | client→server  | `Signal` | SSH `signal`, docker exec signal, `kill(-pgid, n)` |
/// | client→server  | `Eof`    | SSH channel EOF, docker stdin close, `ChildStdin`  |
/// | server→client  | `Exit`   | the completion signal (ADR-055)                    |
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlMessage {
    /// Terminal window resize (client→server).
    ///
    /// `pixel_width`/`pixel_height` default to 0 (most terminals don't
    /// report pixel dimensions; SSH's `pty_request` carries them for
    /// completeness).
    Resize {
        cols: u16,
        rows: u16,
        #[serde(default)]
        pixel_width: u16,
        #[serde(default)]
        pixel_height: u16,
    },
    /// Forward a signal to the child process group (client→server).
    ///
    /// `name` is an uppercase string from the supported set (see
    /// [`signal_from_name`]). Unknown names fall back to the backend's
    /// default kill in the adapter (tty-local.md REQ-TTY-02).
    Signal { name: String },
    /// Client stdin is done (client→server). The server closes the
    /// backend's stdin (`ChildStdin::drop` / PTY writer close) but keeps
    /// pumping stdout + the exit chunk. See `tty-wire.md` §"Stdin
    /// Closure".
    Eof,
    /// Process exit code (server→client). The exit chunk is the last
    /// control chunk before stream close (ADR-055). `code` is `i32`
    /// matching `std::process::ExitStatus::code()`; negative values are
    /// signal-terminated (e.g., `-9` for SIGKILL on Unix). `-1` is the
    /// adapter's best-effort "backend could not determine the exit code"
    /// sentinel (ADR-055 §4).
    Exit { code: i32 },
}

impl ControlMessage {
    /// Serialize to JSON bytes for the control chunk payload.
    pub fn to_json(&self) -> serde_json::Result<bytes::Bytes> {
        serde_json::to_vec(self).map(bytes::Bytes::from)
    }

    /// Deserialize from a control chunk payload (UTF-8 JSON).
    ///
    /// Returns `serde_json::Error` on unknown `type` tags; the adapter
    /// ignores that error per the wire spec's extensibility policy.
    pub fn from_slice(b: &[u8]) -> serde_json::Result<Self> {
        serde_json::from_slice(b)
    }
}

/// Map an uppercase signal name to a libc signal number.
///
/// Supports the common set a terminal front-end would forward: `HUP`,
/// `INT`, `QUIT`, `TERM`, `KILL`, `USR1`, `USR2`, `TSTP`, `CONT`
/// (Ctrl-C → `INT`, Ctrl-\ → `QUIT`, Ctrl-Z → `TSTP`). Unknown names
/// return `None`; the caller (the local backend) decides whether to
/// ignore or fall back to the backend's default kill
/// (`portable_pty`'s `ChildKiller::kill` sends SIGHUP — see
/// tty-local.md REQ-TTY-02).
///
/// Unix-only: the non-Unix path falls back to `ChildKiller::kill`
/// directly. The `#[cfg(unix)]` gate matches the POC.
#[cfg(unix)]
pub fn signal_from_name(name: &str) -> Option<i32> {
    use libc::*;
    match name {
        "HUP" => Some(SIGHUP),
        "INT" => Some(SIGINT),
        "QUIT" => Some(SIGQUIT),
        "TERM" => Some(SIGTERM),
        "KILL" => Some(SIGKILL),
        "USR1" => Some(SIGUSR1),
        "USR2" => Some(SIGUSR2),
        "TSTP" => Some(SIGTSTP),
        "CONT" => Some(SIGCONT),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_resize() {
        let msg = ControlMessage::Resize {
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
        };
        let bytes = msg.to_json().unwrap();
        let back = ControlMessage::from_slice(&bytes).unwrap();
        assert_eq!(msg, back);
    }

    #[test]
    fn round_trip_resize_with_pixels() {
        let msg = ControlMessage::Resize {
            cols: 120,
            rows: 40,
            pixel_width: 800,
            pixel_height: 600,
        };
        let bytes = msg.to_json().unwrap();
        let back = ControlMessage::from_slice(&bytes).unwrap();
        assert_eq!(msg, back);
    }

    #[test]
    fn round_trip_signal() {
        let msg = ControlMessage::Signal {
            name: "INT".to_string(),
        };
        let bytes = msg.to_json().unwrap();
        let back = ControlMessage::from_slice(&bytes).unwrap();
        assert_eq!(msg, back);
    }

    #[test]
    fn round_trip_eof() {
        let msg = ControlMessage::Eof;
        let bytes = msg.to_json().unwrap();
        let back = ControlMessage::from_slice(&bytes).unwrap();
        assert_eq!(msg, back);
    }

    #[test]
    fn round_trip_exit() {
        let msg = ControlMessage::Exit { code: 42 };
        let bytes = msg.to_json().unwrap();
        let back = ControlMessage::from_slice(&bytes).unwrap();
        assert_eq!(msg, back);
    }

    #[test]
    fn to_json_emits_snake_case_type_tag() {
        let resize = ControlMessage::Resize {
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
        };
        let json: serde_json::Value = serde_json::from_slice(&resize.to_json().unwrap()).unwrap();
        assert_eq!(json["type"], "resize");
        assert_eq!(json["cols"], 80);
        assert_eq!(json["rows"], 24);
        assert_eq!(json["pixel_width"], 0);
        assert_eq!(json["pixel_height"], 0);

        let signal = ControlMessage::Signal {
            name: "INT".to_string(),
        };
        let json: serde_json::Value = serde_json::from_slice(&signal.to_json().unwrap()).unwrap();
        assert_eq!(json["type"], "signal");
        assert_eq!(json["name"], "INT");

        let eof = ControlMessage::Eof;
        let json: serde_json::Value = serde_json::from_slice(&eof.to_json().unwrap()).unwrap();
        assert_eq!(json["type"], "eof");

        let exit = ControlMessage::Exit { code: 0 };
        let json: serde_json::Value = serde_json::from_slice(&exit.to_json().unwrap()).unwrap();
        assert_eq!(json["type"], "exit");
        assert_eq!(json["code"], 0);
    }

    #[test]
    fn resize_omits_pixel_defaults_on_deserialize() {
        let json = br#"{"type":"resize","cols":80,"rows":24}"#;
        let msg = ControlMessage::from_slice(json).unwrap();
        match msg {
            ControlMessage::Resize {
                cols,
                rows,
                pixel_width,
                pixel_height,
            } => {
                assert_eq!(cols, 80);
                assert_eq!(rows, 24);
                assert_eq!(pixel_width, 0);
                assert_eq!(pixel_height, 0);
            }
            _ => panic!("expected Resize"),
        }
    }

    #[test]
    fn from_slice_unknown_type_returns_error() {
        let json = br#"{"type":"unknown"}"#;
        assert!(ControlMessage::from_slice(json).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn signal_from_name_known() {
        use libc::*;
        assert_eq!(signal_from_name("HUP"), Some(SIGHUP));
        assert_eq!(signal_from_name("INT"), Some(SIGINT));
        assert_eq!(signal_from_name("QUIT"), Some(SIGQUIT));
        assert_eq!(signal_from_name("TERM"), Some(SIGTERM));
        assert_eq!(signal_from_name("KILL"), Some(SIGKILL));
        assert_eq!(signal_from_name("USR1"), Some(SIGUSR1));
        assert_eq!(signal_from_name("USR2"), Some(SIGUSR2));
        assert_eq!(signal_from_name("TSTP"), Some(SIGTSTP));
        assert_eq!(signal_from_name("CONT"), Some(SIGCONT));
    }

    #[cfg(unix)]
    #[test]
    fn signal_from_name_unknown() {
        assert_eq!(signal_from_name("NOPE"), None);
        assert_eq!(signal_from_name(""), None);
        assert_eq!(signal_from_name("int"), None);
    }
}
