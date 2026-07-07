---
id: tty/review-tty
name: Review alknet-tty core crate for spec conformance before local backend begins
status: completed
depends_on: [tty/adapter]
scope: moderate
risk: low
impact: phase
level: review
---

## Description

Review the alknet-tty core crate implementation for spec conformance, pattern
consistency, and correctness before `alknet-tty-local` begins implementation
against the `TtyBackend` trait. This is the quality checkpoint at the end of
the core crate phase — the trait shape is a one-way door (ADR-053), and the
local backend builds against it, so any issues here propagate to the sibling
crate.

### Review Checklist

1. **Wire codec conformance** (tty-wire.md §"Phase 2"):
   - `ChunkReader`/`ChunkWriter` with 5-byte header (1 stream_type + 4 BE length)
   - `STREAM_STDIN`=0, `STREAM_STDOUT`=1, `STREAM_STDERR`=2, `STREAM_CONTROL`=3
   - `MAX_CHUNK_LEN` = 16 MiB; `ChunkTooLarge` on overflow
   - `InvalidStreamType` on stream_type > 3
   - `ConnectionClosed` on clean EOF (not `Io`)
   - Zero-length chunks are sentinels (codec doesn't special-case; adapter interprets)

2. **Control messages conformance** (tty-wire.md §"Control Channel"):
   - `ControlMessage` enum: `Resize`, `Signal`, `Eof`, `Exit`
   - `#[serde(tag = "type", rename_all = "snake_case")]`
   - `Exit.code` is `i32` (negative = signal-terminated)
   - `signal_from_name` for 9 names, `#[cfg(unix)]` gated
   - Unknown `type` values: `from_slice` returns error; adapter ignores (not a schema catch-all)

3. **Negotiation conformance** (tty-wire.md §"Phase 1", §"Constraints"):
   - `NegotiateRequest` with `carriage`, `backend`, `tty`, `cmd`, `cwd`, `env`, `backend_params`
   - `serde(flatten)` captures backend-specific fields into `backend_params`
   - `TerminalParamsWire` with `term`, `cols`, `rows`, `pixel_width`, `pixel_height`, `modes`
   - Length-prefixed framing (4-byte BE + JSON body), self-contained (no alknet-call dep, ADR-057)
   - `NegotiationReader` bounds-checks against `MAX_CHUNK_LEN`
   - Error response shape: `{"error":"...","field":"..."}`
   - Framing disambiguation: error frame first byte is `0x00` (under 16 MiB invariant)

4. **Backend trait conformance** (tty-backend.md, ADR-053):
   - `TtyBackend` trait: `allocate()` (async), `resource_id()` (default `None`)
   - `TtyError` `#[non_exhaustive]`: `AllocFailed`, `WaitFailed`, `Io`, `Backend`
   - `TtyParams`: `terminal`, `cmd`, `cwd`, `env`, `backend_params` (opaque `serde_json::Map`)
   - `TerminalParams`: `term`, `cols`, `rows`, `pixel_width`, `pixel_height`, `modes`
   - `TtyHandle`: `stdin` (Box<dyn AsyncWrite + Send + Unpin>), `stdout` (Pin<Box<dyn Stream<Item=Bytes> + Send>>), `stderr` (Option), `exit_code` (BoxFuture<Result<i32, TtyError>>), `control` (Option<TtyControlHandle>)
   - `TtyControl` trait: `resize`, `signal` (object-safe, NOT `Clone`)
   - `TtyControlHandle`: `#[derive(Clone)]`, `Arc<dyn TtyControl + Send + Sync>` newtype (OQ-43)
   - `From<NegotiateRequest> for TtyParams` (or constructor)
   - REQ-TTY-01 documented (backends need not be natively async)
   - ADR-056 kill-on-Drop contract documented on `exit_code` field

5. **Adapter conformance** (tty-adapter.md):
   - `TtyAdapter` struct with `backends` (`Arc<HashMap<String, Arc<dyn TtyBackend>>>`), `ownership` (Option)
   - `impl ProtocolHandler` with `alpn()` = `b"alknet/tty"`
   - `handle()` loops `accept_bi`, spawns `drive_session` per stream
   - Negotiation errors: `unknown_backend`, `malformed_negotiation`, `allocate_failed` as JSON in negotiation framing
   - Three-pump driver: stdout→client, client→backend (stdin + control), exit→exit-chunk
   - stderr pump concurrent when `stderr` is `Some`
   - **Exit-chunk-is-last** (ADR-055): stdout/stderr pumps complete AND exit_code resolves before exit chunk
   - Exit error → `{"type":"exit","code":-1}`
   - Unknown control `type` ignored; `Exit` from client ignored
   - Zero-length stdin chunk AND `eof` control both close backend stdin
   - Scope-gate (`tty:open`) at negotiation
   - Ownership check via `resource_id()` + `OwnershipProvider::owns()` when wired
   - Connection drop / stream reset drops `TtyHandle` (ADR-056 cancel-cleanup)
   - Client write-half close does NOT trigger cancel-cleanup

6. **Dependency constraints**:
   - No `portable_pty`, `bollard`, `russh` dependency (lean core)
   - No `alknet-call` dependency (ADR-057 — self-contained framing)
   - `alknet-core` is the only alknet dependency
   - `local` feature gate declared; `alknet-tty-local` not yet wired (later task)

7. **Pattern consistency**:
   - `thiserror` for error enums, `Result` propagation
   - `tracing` for structured logging (debug/warn)
   - `async-trait` for `TtyBackend` and `ProtocolHandler`
   - `tokio` runtime idioms (mpsc, oneshot, spawn)

8. **Test coverage**:
   - Wire codec round-trip + error cases
   - Control message round-trip + `signal_from_name`
   - Negotiation round-trip + `serde(flatten)` + error framing
   - `MockBackend` + `TtyControlHandle` delegation
   - Adapter: happy path, exit-chunk-is-last, stdin EOF, control dispatch, unknown control, all 3 negotiation errors, exit error, cancel cleanup, scope gate, ownership check

## Acceptance Criteria

- [ ] All wire codec types match tty-wire.md §"Phase 2"
- [ ] All control message types match tty-wire.md §"Control Channel"
- [ ] All negotiation types match tty-wire.md §"Phase 1" + §"Constraints"
- [ ] All backend trait types match tty-backend.md (ADR-053)
- [ ] Adapter matches tty-adapter.md (session lifecycle, exit ordering, access control)
- [ ] Exit-chunk-is-last invariant (ADR-055) enforced in the adapter, not the backend
- [ ] ADR-056 kill-on-Drop contract documented on `TtyHandle.exit_code`
- [ ] No `portable_pty`/`bollard`/`russh`/`alknet-call` dependency
- [ ] `local` feature gate declared, `alknet-tty-local` not yet wired
- [ ] `cargo fmt --check -p alknet-tty` passes
- [ ] `cargo clippy -p alknet-tty` passes with no warnings
- [ ] All tests pass
- [ ] `MockBackend` is reusable for the local backend's integration tests

## References

- docs/architecture/crates/tty/README.md
- docs/architecture/crates/tty/overview.md
- docs/architecture/crates/tty/tty-wire.md
- docs/architecture/crates/tty/tty-backend.md
- docs/architecture/crates/tty/tty-adapter.md
- docs/architecture/decisions/052-alknet-tty-wire-format-and-two-carriage.md
- docs/architecture/decisions/053-ttybackend-trait-and-ttyhandle.md
- docs/architecture/decisions/055-exit-code-on-control-chunk.md
- docs/architecture/decisions/056-backend-cleanup-on-session-cancel.md
- docs/architecture/decisions/057-alknet-tty-no-alknet-call-dep.md

## Notes

> This review verifies the core crate is spec-conformant before the local
> backend builds against the `TtyBackend` trait. The trait shape is one-way
> (ADR-053) — any issues here propagate to `alknet-tty-local` and the future
> docker/SSH backend crates. Pay special attention to the exit-chunk-is-last
> ordering (ADR-055) and the kill-on-Drop contract (ADR-056) — these are the
> subtle invariants the POC did not fully enforce. If deviations are found,
> document and fix before proceeding to the local backend tasks.

## Summary

> To be filled on completion