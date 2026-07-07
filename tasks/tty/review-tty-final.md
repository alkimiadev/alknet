---
id: tty/review-tty-final
name: Final review of alknet-tty + alknet-tty-local for merge readiness
status: pending
depends_on: [tty/integration-test]
scope: broad
risk: low
impact: project
level: review
---

## Description

Final review of the `alknet-tty` and `alknet-tty-local` crates for merge
readiness. This is the last quality checkpoint before the tty work is
considered complete. It validates the two crates as a unit: the core crate's
wire format + adapter + trait, the local backend's PTY/pipe implementations,
the feature-gate seam, and the end-to-end integration tests.

### Review Checklist

1. **Cross-crate seam**:
   - `alknet-tty` `local` feature gate activates `dep:alknet-tty-local`
   - `alknet_tty::local::LocalTtyBackend` re-export works with `--features local`
   - `cargo tree -p alknet-tty` (default) does NOT include `portable_pty`
   - `cargo tree -p alknet-tty --features local` includes `portable_pty` via `alknet-tty-local`
   - `LocalTtyBackend` implements `alknet_tty::TtyBackend` (the trait compiles against the impl)
   - Assembly-layer pattern documented

2. **Wire format invariants (ADR-052)**:
   - Two-carriage model: JSON negotiation frame, then raw chunks
   - Fixed channel set (0-3), no extension escape hatch
   - `MAX_CHUNK_LEN` = 16 MiB shared between negotiation and raw chunks
   - Framing disambiguation: error frame first byte `0x00`, raw chunk first byte 1-3
   - Self-contained framing (no alknet-call dependency, ADR-057)

3. **Exit-chunk ordering (ADR-055)**:
   - Exit chunk is the last chunk before stream close
   - Adapter waits for stdout/stderr pumps AND exit_code resolve before exit chunk
   - Exit error → `{"type":"exit","code":-1}`
   - Integration test confirms exit-chunk-is-last with a real backend

4. **Cancel-cleanup (ADR-056)**:
   - `LocalExitFuture` (PTY) has kill-on-Drop guard (`ChildKiller::kill(SIGHUP)`)
   - `PipeExitFuture` (pipe) has kill-on-Drop guard (`Child::start_kill()`)
   - Both disarm on resolve (no-op Drop on happy path)
   - Neither is a bare `oneshot::Receiver` / bare `Child::wait()`
   - Integration tests confirm no orphaned processes on connection drop (PTY + pipe)

5. **REQ-TTY-01 (backends need not be natively async)**:
   - PTY mode uses three std threads (reader, writer, waiter) + tokio mpsc/oneshot
   - Trait's adapter-facing types (`AsyncWrite`, `Stream`, `BoxFuture`, `TtyControl`) are the contract
   - Documented as a supported implementation strategy

6. **REQ-TTY-02 (signal forwarding to process group)**:
   - PTY mode: `kill(-pgid, sig)` with `kill(pid, sig)` fallback
   - `portable_pty` child is a session leader (`controlling_tty = true`)
   - Pipe mode: `kill(pid, sig)` only (documented limitation, no process group)
   - Unknown names fall back to default kill (SIGHUP for PTY, SIGKILL for pipe)
   - Integration test confirms process-group targeting (shell's child receives signal)

7. **Access control (ADR-050)**:
   - Scope-gate at negotiation (`tty:open` or deployment-configured)
   - Ownership check via `backend.resource_id()` + `OwnershipProvider::owns()` when wired
   - `LocalTtyBackend::resource_id()` returns `None` (creates its own resource)

8. **Dependency hygiene**:
   - `alknet-tty` depends on `alknet-core` only (no `alknet-call`, no backend deps)
   - `alknet-tty-local` depends on `alknet-tty` + `portable_pty` + `libc`
   - No `alknet-core` direct dep in `alknet-tty-local`
   - No `bollard`/`russh` (future backend crates)

9. **Pattern consistency**:
   - `thiserror` for error enums (`TtyError`, `RawError`, `NegotiationError`)
   - `async-trait` for `TtyBackend` and `ProtocolHandler`
   - `tracing` for structured logging
   - `TtyControlHandle::new(Arc::new(...))` wrapping (OQ-43)
   - `#[cfg(unix)]` gates on `libc::kill` paths
   - `#[non_exhaustive]` on `TtyError`

10. **Test coverage**:
    - Wire codec unit tests (round-trip, error cases)
    - Control message unit tests (round-trip, signal_from_name)
    - Negotiation unit tests (round-trip, serde(flatten), error framing, disambiguation)
    - Backend trait unit tests (MockBackend, TtyControlHandle delegation)
    - Adapter unit tests (MockBackend over duplex, all scenarios)
    - PTY mode integration tests (happy, interactive, resize, signal, process-group, cancel, exit-last)
    - Pipe mode integration tests (happy, stderr, signal, cancel, resize)
    - Negotiation error integration tests (unknown_backend, malformed, allocate_failed)
    - All tests pass with `--features local`

11. **Documentation**:
    - Crate-level doc comments on both crates
    - Module-level doc comments
    - ADR-056 kill-on-Drop contract documented on `TtyHandle.exit_code`
    - REQ-TTY-01 documented on `TtyBackend` trait
    - REQ-TTY-02 documented on `PtyControl::signal`
    - Pipe-mode signal limitation documented on `PipeControl::signal`
    - Assembly-layer pattern documented

## Acceptance Criteria

- [ ] Cross-crate seam: `local` feature re-export works; `portable_pty` gated correctly
- [ ] Wire format invariants (ADR-052) hold: two-carriage, fixed channels, disambiguation
- [ ] Exit-chunk-is-last (ADR-055) enforced in adapter, validated in integration tests
- [ ] Cancel-cleanup (ADR-056) guards present in both PTY and pipe exit futures; integration tests confirm no orphans
- [ ] REQ-TTY-01 satisfied: PTY mode uses three-thread bridge; trait contract documented
- [ ] REQ-TTY-02 satisfied: PTY process-group signal forwarding; integration test passes
- [ ] Access control (ADR-050): scope-gate + ownership check wired
- [ ] Dependency hygiene: alknet-tty lean (core only); alknet-tty-local has portable_pty/libc
- [ ] Pattern consistency: thiserror, async-trait, tracing, TtyControlHandle wrapping, cfg(unix)
- [ ] All unit + integration tests pass with `--features local`
- [ ] `cargo fmt --check` passes for both crates
- [ ] `cargo clippy` passes with no warnings for both crates (default + `--features local`)
- [ ] Documentation complete (crate, module, contract doc comments)

## References

- docs/architecture/crates/tty/README.md
- docs/architecture/crates/tty/overview.md
- docs/architecture/crates/tty/tty-wire.md
- docs/architecture/crates/tty/tty-backend.md
- docs/architecture/crates/tty/tty-adapter.md
- docs/architecture/crates/tty/tty-local.md
- docs/architecture/decisions/052-alknet-tty-wire-format-and-two-carriage.md
- docs/architecture/decisions/053-ttybackend-trait-and-ttyhandle.md
- docs/architecture/decisions/054-local-tty-backend-sibling-crate.md
- docs/architecture/decisions/055-exit-code-on-control-chunk.md
- docs/architecture/decisions/056-backend-cleanup-on-session-cancel.md
- docs/architecture/decisions/057-alknet-tty-no-alknet-call-dep.md

## Notes

> This is the final review before the tty work is considered complete. It
> validates the two crates as a unit — the core crate's invariants
> (exit-chunk-is-last, kill-on-Drop, framing disambiguation) must hold with
> the real local backend, not just the MockBackend. The cancel-cleanup
> integration tests are the most critical: an orphaned process on connection
> drop is the bug ADR-056 exists to prevent. The process-group signal test
> (REQ-TTY-02) is the other critical validation — Ctrl-C must reach a
> shell's children. If any invariant fails, document and fix before merge.

## Summary

> To be filled on completion