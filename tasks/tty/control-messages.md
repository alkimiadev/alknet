---
id: tty/control-messages
name: Implement ControlMessage enum and signal_from_name helper
status: pending
depends_on: [tty/crate-init]
scope: single
risk: low
impact: isolated
level: implementation
---

## Description

Implement the control channel schema in `src/control.rs`. Control chunks
(stream_type 3) carry a JSON payload tagged by `type`. This is a direct port of
the POC's `/workspace/alknet-tty-poc/src/control.rs`.

### ControlMessage enum

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlMessage {
    Resize {
        cols: u16,
        rows: u16,
        #[serde(default)]
        pixel_width: u16,
        #[serde(default)]
        pixel_height: u16,
    },
    Signal { name: String },
    Eof,
    Exit { code: i32 },
}
```

| Direction | Message | Shape | Maps to |
|---|---|---|---|
| client→server | resize | `{"type":"resize","cols":80,"rows":24,...}` | SSH `window-change`, docker exec resize, `ioctl(TIOCSWINSZ)` |
| client→server | signal | `{"type":"signal","name":"INT"}` | SSH `signal`, docker exec signal, `kill(-pgid, sig)` (REQ-TTY-02) |
| client→server | eof | `{"type":"eof"}` | SSH channel EOF, docker stdin close, `ChildStdin::drop` |
| server→client | exit | `{"type":"exit","code":0}` | the terminal/completion signal (ADR-055) |

### Serialization helpers

```rust
impl ControlMessage {
    pub fn to_json(&self) -> serde_json::Result<bytes::Bytes>;
    pub fn from_slice(b: &[u8]) -> serde_json::Result<Self>;
}
```

### signal_from_name (Unix-only)

```rust
#[cfg(unix)]
pub fn signal_from_name(name: &str) -> Option<i32>;
```

Maps uppercase signal names to `libc` signal numbers: `HUP`, `INT`, `QUIT`,
`TERM`, `KILL`, `USR1`, `USR2`, `TSTP`, `CONT`. Unknown names return `None`;
the caller (the local backend) decides whether to ignore or fall back to the
backend's default kill. This helper lives in `alknet-tty` (not
`alknet-tty-local`) because the wire spec defines the supported name set
(tty-wire.md §"Control Channel") and the local backend consumes it.

On non-Unix targets, `signal_from_name` is absent (the local backend's
non-Unix signal path falls back to `ChildKiller::kill` directly). The
`#[cfg(unix)]` gate matches the POC.

### Extensibility

Unknown `type` values are **ignored** (not a protocol error) so that a newer
client sending a control message an older server doesn't recognize degrades
gracefully rather than tearing down the session. This is handled in the
adapter's control dispatch (task `tty/adapter`), but the `Deserialize` impl
must tolerate it — use `#[serde(other)]` on a catch-all variant OR handle the
`serde_json::Error` in the adapter as "ignore unknown." The POC handles it in
the adapter (logs a warning on parse error, continues). Match the POC's
approach: do NOT add a catch-all variant to the enum; let `from_slice` return
an error on unknown `type` and have the adapter ignore the error. This keeps
the enum exhaustive and the wire format's "unknown types are ignored" rule an
adapter-level policy, not a schema-level leak.

### Tests

- Round-trip: serialize each variant, deserialize, assert equality.
- `to_json` produces the expected `{"type":"resize",...}` shape (snake_case tag).
- `signal_from_name` returns the right numbers for all 9 names, `None` for
  unknown names (Unix only).
- `from_slice` on `{"type":"unknown"}` returns an error (the adapter will
  ignore it).

## Acceptance Criteria

- [ ] `ControlMessage` enum with `Resize`, `Signal`, `Eof`, `Exit` variants
- [ ] `#[serde(tag = "type", rename_all = "snake_case")]` attribute
- [ ] `Resize` has `cols`, `rows`, `pixel_width` (default 0), `pixel_height` (default 0)
- [ ] `Exit` has `code: i32`
- [ ] `to_json` and `from_slice` helpers implemented
- [ ] `signal_from_name` implemented for the 9 supported names, `#[cfg(unix)]` gated
- [ ] Round-trip unit tests for all 4 variants
- [ ] Unit test: `to_json` produces snake_case `type` tag
- [ ] Unit test: `signal_from_name` for all 9 names + unknown (Unix only)
- [ ] Unit test: `from_slice` on unknown `type` returns error
- [ ] `cargo test -p alknet-tty` succeeds
- [ ] `cargo clippy -p alknet-tty` succeeds with no warnings

## References

- docs/architecture/crates/tty/tty-wire.md — §"Control Channel (stream_type 3)", §"Stdin Closure"
- docs/architecture/decisions/055-exit-code-on-control-chunk.md — ADR-055 (Exit variant)
- /workspace/alknet-tty-poc/src/control.rs — the reference implementation to port

## Notes

> Near-verbatim port of the POC's `control.rs`. The `signal_from_name` helper
> is Unix-only; the local backend's non-Unix path uses `ChildKiller::kill`
> directly. The "unknown type ignored" policy is enforced in the adapter, not
> the schema — keep the enum exhaustive.

## Summary

> To be filled on completion