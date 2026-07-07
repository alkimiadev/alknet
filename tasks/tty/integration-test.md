---
id: tty/integration-test
name: "End-to-end integration test: LocalTtyBackend + drive_session over real commands"
status: pending
depends_on: [tty/local-feature-reexport]
scope: broad
risk: medium
impact: phase
level: implementation
---

## Description

Write end-to-end integration tests that exercise the full stack:
`LocalTtyBackend` (PTY and pipe modes) + `TtyAdapter::drive_session` over a
`tokio::io::duplex` (transport stand-in for a QUIC bidi stream), running real
commands. These tests validate the two crates work together through the
`TtyBackend` trait seam and that the wire-format invariants (exit-chunk-is-last,
ADR-055; kill-on-Drop, ADR-056) hold with a real backend, not just the
`MockBackend` from `tty/adapter`.

These tests live in `alknet-tty`'s test suite (with `--features local` so the
local backend is available) OR in `alknet-tty-local`'s test suite (depending
on `alknet-tty` for the adapter). The natural home is `alknet-tty`'s
`tests/` directory with a `local` feature gate, since the adapter is in
`alknet-tty` and the test exercises the adapter + backend together. Use
`#[cfg(feature = "local")]` on the test module.

### Test scenarios

All scenarios use `tokio::io::duplex` as the bidi stream stand-in (the POC's
pattern). The test acts as the client: writes the negotiation frame, sends
stdin/control chunks, reads stdout/stderr/control chunks, asserts the
exit chunk and stream close.

#### PTY mode (`terminal: Some`)

1. **Happy path (echo)**: negotiate `{backend:"local", tty:{cols:80,rows:24}, cmd:["echo","hello"]}`,
   read stdout chunks until the exit chunk, assert stdout contains "hello",
   exit code is 0, exit chunk is the last chunk before stream close.
2. **Interactive (cat)**: negotiate `cmd:["cat"]`, write stdin chunks, read
   them back via stdout, send `eof` control chunk, await exit 0. Assert
   stdin round-trips through the PTY.
3. **Resize**: negotiate `cmd:["cat"]`, send a `resize` control chunk, assert
   no error (the PTY resizes). Send `eof`, await exit.
4. **Signal (SIGINT, Unix)**: negotiate `cmd:["sleep","60"]`, send a `signal`
   control chunk with `name:"INT"`, await the exit chunk. Assert exit code is
   signal-terminated (negative, e.g., -2 for SIGINT, or 130). Assert the
   child is reaped (no zombie).
5. **Process-group signal (Unix)**: negotiate `cmd:["bash","-c","sleep 60"]`,
   send `signal:"INT"`, assert the `sleep` child also receives the signal
   (the process group is targeted). This validates REQ-TTY-02 end-to-end.
6. **Stdin EOF (zero-length chunk)**: negotiate `cmd:["cat"]`, send a
   zero-length stdin chunk (the sentinel), assert the backend's stdin closes,
   stdout drains, exit chunk is sent.
7. **Cancel cleanup (ADR-056)**: negotiate `cmd:["sleep","60"]`, drop the
   duplex (simulating connection drop) mid-session, assert the child is
   killed (no orphan). Use a short delay then check the process is gone
   (e.g., via `kill(pid, 0)` returning ESRCH, or a `ps` check, or a
   `tokio::process` tracker).
8. **Exit-chunk-is-last**: in the happy path, assert no stdout chunk arrives
   after the exit chunk. Read all chunks, find the exit chunk, assert it is
   the last chunk before stream close.

#### Pipe mode (`terminal: None`)

9. **Happy path (echo)**: negotiate `{backend:"local", tty:null, cmd:["echo","hello"]}`,
   read stdout chunks, assert "hello", exit 0. Assert stderr is empty.
10. **Separate stderr**: negotiate `cmd:["sh","-c","echo out; echo err >&2"]`,
    assert stdout stream receives "out", stderr stream receives "err" (as
    stderr chunks, stream_type 2), exit 0.
11. **Signal (SIGTERM, Unix)**: negotiate `cmd:["sleep","60"]`, send
    `signal:"TERM"`, await exit, assert signal-terminated.
12. **Cancel cleanup (ADR-056)**: negotiate `cmd:["sleep","60"]`, drop the
    duplex mid-session, assert the child is killed (no orphan).
13. **Resize no-op**: negotiate `cmd:["cat"]`, send a `resize` control chunk,
    assert no error (PipeControl::resize is a no-op). Send `eof`, await exit.

#### Negotiation errors

14. **unknown_backend**: negotiate `{backend:"kubernetes",...}`, assert the
    error response `{"error":"unknown_backend","backend":"kubernetes"}`,
    stream closes, no raw mode (first byte of the error frame is `0x00`).
15. **malformed_negotiation (bad JSON)**: write garbage bytes as the
    negotiation frame, assert `{"error":"malformed_negotiation",...}`.
16. **malformed_negotiation (carriage != raw)**: negotiate
    `{carriage:"json",...}`, assert `malformed_negotiation`.
17. **malformed_negotiation (empty cmd)**: negotiate `{cmd:[]}`, assert
    `malformed_negotiation`.
18. **allocate_failed**: (harder to trigger with the local backend — skip or
    use a non-existent binary) negotiate `cmd:["/nonexistent"]`, assert
    `allocate_failed` (the spawn fails).

### Test harness

Build a small test helper that wraps the client-side wire protocol:
- `write_negotiation(frame)` — serialize + length-prefix + write
- `write_chunk(stream_type, bytes)` — write a raw chunk
- `write_control(ControlMessage)` — serialize + write as stream_type 3
- `read_chunk()` — read a chunk, return `(stream_type, bytes)`
- `read_error_frame()` — read the length-prefixed JSON error (first byte `0x00`)
- `read_until_exit()` — read chunks until the exit control chunk, return
  (stdout_bytes, stderr_bytes, exit_code)

This helper can live in a `tests/common/` module or be reused from the
adapter's unit tests (the `MockBackend` tests in `tty/adapter` use a similar
pattern).

## Acceptance Criteria

- [ ] Integration test module in `alknet-tty/tests/` (or `alknet-tty-local/tests/`), `#[cfg(feature = "local")]` gated
- [ ] PTY happy path (echo): stdout contains "hello", exit 0, exit chunk last
- [ ] PTY interactive (cat): stdin round-trips, eof → exit 0
- [ ] PTY resize: control chunk accepted, no error
- [ ] PTY signal (SIGINT): exit code signal-terminated, child reaped (Unix)
- [ ] PTY process-group signal: shell's child receives the signal (REQ-TTY-02, Unix)
- [ ] PTY stdin EOF (zero-length chunk): backend stdin closes, exit chunk sent
- [ ] PTY cancel cleanup: drop duplex → child killed, no orphan (ADR-056)
- [ ] PTY exit-chunk-is-last: no stdout chunk after exit chunk
- [ ] Pipe happy path (echo): stdout "hello", stderr empty, exit 0
- [ ] Pipe separate stderr: stdout "out", stderr "err", exit 0
- [ ] Pipe signal (SIGTERM): exit signal-terminated (Unix)
- [ ] Pipe cancel cleanup: drop duplex → child killed, no orphan (ADR-056)
- [ ] Pipe resize no-op: control chunk accepted, no error
- [ ] unknown_backend error: error response, stream closes, first byte `0x00`
- [ ] malformed_negotiation (bad JSON, bad carriage, empty cmd): error response
- [ ] allocate_failed (nonexistent binary): error response
- [ ] Test helper for client-side wire protocol (negotiation, chunks, control, error frames)
- [ ] `cargo test -p alknet-tty --features local` succeeds
- [ ] `cargo clippy -p alknet-tty --features local` succeeds with no warnings

## References

- docs/architecture/crates/tty/tty-adapter.md — session lifecycle, negotiation errors
- docs/architecture/crates/tty/tty-local.md — PTY and pipe mode behavior
- docs/architecture/decisions/055-exit-code-on-control-chunk.md — ADR-055 (exit-chunk-is-last)
- docs/architecture/decisions/056-backend-cleanup-on-session-cancel.md — ADR-056 (cancel cleanup)
- /workspace/alknet-tty-poc/tests/integration.rs — the POC's integration tests (reference)
- /workspace/alknet-tty-poc/tests/signal.rs — the POC's SIGINT-forwarding test (REQ-TTY-02)

## Notes

> These are the tests that validate the two crates work together through the
> `TtyBackend` trait seam. The `MockBackend` tests in `tty/adapter` validate
> the adapter in isolation; these tests validate the adapter + a real backend.
> The cancel-cleanup tests (ADR-056) are the most important — they confirm no
> orphaned processes when the connection drops mid-session. The
> process-group signal test (REQ-TTY-02) confirms Ctrl-C reaches a shell's
> children, not just the shell. Unix-only tests (signal, process-group) use
> `#[cfg(unix)]`.

## Summary

> To be filled on completion